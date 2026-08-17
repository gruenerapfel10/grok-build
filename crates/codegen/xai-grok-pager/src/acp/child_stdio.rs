//! ACP transport over a child process's standard streams.

use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::thread;

use agent_client_protocol as acp;
use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, simplex};
use tokio::process::{Child, Command};
use tokio_util::compat::{TokioAsyncReadCompatExt, TokioAsyncWriteCompatExt};
use tokio_util::sync::CancellationToken;
use xai_acp_lib::{
    AcpClientChannel, AcpGatewayReceiver, AcpGatewaySender, LineBufferedRead, acp_channels,
};

const MAX_BUF: usize = 8 * 1024 * 1024;

pub struct ChildProcessGuard(Arc<Mutex<Option<Child>>>);

impl Drop for ChildProcessGuard {
    fn drop(&mut self) {
        if let Ok(mut child) = self.0.lock()
            && let Some(child) = child.as_mut()
        {
            let _ = child.start_kill();
        }
    }
}

pub struct ChildStdioBridge {
    pub channel: AcpClientChannel,
    pub cancel: CancellationToken,
    pub child: ChildProcessGuard,
}

pub async fn spawn_child_acp(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    cwd: Option<&Path>,
    cancel: CancellationToken,
) -> Result<ChildStdioBridge> {
    let mut process = Command::new(command);
    process
        .args(args)
        .envs(env.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = cwd {
        process.current_dir(cwd);
    }

    let mut child = process
        .spawn()
        .with_context(|| format!("Failed to spawn external ACP agent '{command}'"))?;
    let stdin = child
        .stdin
        .take()
        .context("external ACP agent stdin was not piped")?;
    let stdout = child
        .stdout
        .take()
        .context("external ACP agent stdout was not piped")?;
    let stderr = child
        .stderr
        .take()
        .context("external ACP agent stderr was not piped")?;
    let child = Arc::new(Mutex::new(Some(child)));
    let child_guard = ChildProcessGuard(child.clone());
    let command_label = command.to_string();

    let (client_channel, agent_channel) = acp_channels();
    let (incoming_read, incoming_write) = simplex(MAX_BUF);
    let (outgoing_read, outgoing_write) = simplex(MAX_BUF);
    let bridge_cancel = cancel.clone();
    thread::Builder::new()
        .name("pager-child-acp-bridge".into())
        .spawn(move || -> Result<()> {
            let mut builder = tokio::runtime::Builder::new_current_thread();
            let runtime = xai_tty_utils::runtime::apply_blocking_pool(builder.enable_all()).build()?;
            let local = tokio::task::LocalSet::new();
            local.block_on(&runtime, async move {
                let cancel_reader = bridge_cancel.clone();
                let reader_task = tokio::task::spawn_local(async move {
                    let mut lines = BufReader::new(stdout).lines();
                    let mut incoming_write = incoming_write;
                    loop {
                        tokio::select! {
                            _ = cancel_reader.cancelled() => break,
                            line = lines.next_line() => match line {
                                Ok(Some(line)) => {
                                    if incoming_write.write_all(line.as_bytes()).await.is_err()
                                        || incoming_write.write_all(b"\n").await.is_err()
                                    { break; }
                                }
                                Ok(None) | Err(_) => { cancel_reader.cancel(); break; }
                            }
                        }
                    }
                });

                let cancel_writer = bridge_cancel.clone();
                let writer_task = tokio::task::spawn_local(async move {
                    let mut lines = BufReader::new(outgoing_read).lines();
                    let mut stdin = stdin;
                    loop {
                        tokio::select! {
                            _ = cancel_writer.cancelled() => break,
                            line = lines.next_line() => match line {
                                Ok(Some(line)) => {
                                    if stdin.write_all(line.as_bytes()).await.is_err()
                                        || stdin.write_all(b"\n").await.is_err()
                                        || stdin.flush().await.is_err()
                                    { cancel_writer.cancel(); break; }
                                }
                                Ok(None) | Err(_) => break,
                            }
                        }
                    }
                });

                let cancel_stderr = bridge_cancel.clone();
                let stderr_task = tokio::task::spawn_local(async move {
                    let mut lines = BufReader::new(stderr).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tracing::debug!(command = %command_label, stderr = %line, "external ACP agent stderr");
                        if cancel_stderr.is_cancelled() { break; }
                    }
                });

                let gateway_tx = AcpGatewaySender::new(agent_channel.tx).with_tracing(true);
                let incoming = LineBufferedRead::spawn_local(incoming_read.compat());
                let (connection, handle_io) = acp::ClientSideConnection::new(
                    gateway_tx,
                    outgoing_write.compat_write(),
                    incoming,
                    |future| { tokio::task::spawn_local(future); },
                );
                let gateway_rx = AcpGatewayReceiver::new(agent_channel.rx, connection).with_tracing(true);
                tokio::task::spawn_local(handle_io);
                tokio::task::spawn_local(gateway_rx.run());
                tokio::task::yield_now().await;

                bridge_cancel.cancelled().await;
                if let Ok(mut child) = child.lock()
                    && let Some(child) = child.as_mut()
                {
                    let _ = child.start_kill();
                }
                reader_task.abort();
                writer_task.abort();
                stderr_task.abort();
                Ok(())
            })
        })?;

    Ok(ChildStdioBridge {
        channel: client_channel,
        cancel,
        child: child_guard,
    })
}
