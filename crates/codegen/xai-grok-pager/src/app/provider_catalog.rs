//! Ordered catalog of selectable agent providers.
//!
//! Reads the `[external_providers]` table of the effective config and pairs
//! it with the native Grok agent so UI code (a provider picker overlay) has a
//! single ordered list with stable ids, display labels, account slots, and
//! per-entry command availability.
//!
//! Only the launch command is inspected. Provider `env` values are never read
//! or exposed here: they can carry account routing and must not reach the UI
//! or any log line.

use std::path::{Path, PathBuf};

/// Which agent a catalog entry launches.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelection {
    /// The built-in Grok agent — relaunch without `--provider`.
    Native,
    /// A configured external ACP provider — relaunch with `--provider <id>`.
    External(String),
}

impl ProviderSelection {
    /// The `--provider` value, or `None` for the native agent.
    pub fn provider_id(&self) -> Option<&str> {
        match self {
            ProviderSelection::Native => None,
            ProviderSelection::External(id) => Some(id.as_str()),
        }
    }

    /// Stable id usable as a list key (`"grok"` for the native agent).
    pub fn catalog_id(&self) -> &str {
        match self {
            ProviderSelection::Native => NATIVE_PROVIDER_ID,
            ProviderSelection::External(id) => id.as_str(),
        }
    }
}

/// Whether the entry's launch command can actually be executed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCommandStatus {
    /// The native agent is built in — nothing to resolve.
    BuiltIn,
    /// The command resolved to an executable file.
    Available,
    /// The command was not found on `PATH` (or at its absolute path).
    Missing,
}

impl ProviderCommandStatus {
    /// True when selecting this entry can be expected to start.
    pub fn is_selectable(&self) -> bool {
        matches!(
            self,
            ProviderCommandStatus::BuiltIn | ProviderCommandStatus::Available
        )
    }
}

/// One selectable row in the provider picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderEntry {
    /// What a switch to this entry launches.
    pub selection: ProviderSelection,
    /// Human-facing name (config `label`, else the id).
    pub label: String,
    /// Account slot for the entry (config `account`, else the id segment
    /// after the first `-`). Display text only, never a credential.
    pub account: Option<String>,
    /// Whether the launch command resolves to an executable.
    pub command_status: ProviderCommandStatus,
    /// Whether this entry is the provider the running process was started on.
    pub active: bool,
}

impl ProviderEntry {
    /// Row text for pickers and typed matching: `"label · account"`, or just
    /// the label when the entry has no account slot.
    pub fn row_label(&self) -> String {
        match self.account.as_deref() {
            Some(account) if !account.is_empty() => format!("{} · {}", self.label, account),
            _ => self.label.clone(),
        }
    }
}

/// Ordered provider list plus the active selection.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderCatalog {
    /// Native agent first, then configured providers in `order` then config
    /// declaration order.
    pub entries: Vec<ProviderEntry>,
}

/// Catalog id of the built-in agent (not a `--provider` value).
pub const NATIVE_PROVIDER_ID: &str = "grok";

impl ProviderCatalog {
    /// Build the catalog from the effective config root.
    ///
    /// `active_provider` is the `--provider` value this process started with
    /// (`None` = native Grok).
    pub fn load(raw_config: &toml::Value, active_provider: Option<&str>) -> Self {
        let mut entries = vec![ProviderEntry {
            selection: ProviderSelection::Native,
            label: "Grok".to_string(),
            account: None,
            command_status: ProviderCommandStatus::BuiltIn,
            active: active_provider.is_none(),
        }];

        let configured = raw_config
            .get("external_providers")
            .and_then(toml::Value::as_table);
        if let Some(table) = configured {
            let mut external: Vec<(Option<i64>, usize, ProviderEntry)> = Vec::new();
            for (position, (id, value)) in table.iter().enumerate() {
                let order = value.get("order").and_then(toml::Value::as_integer);
                let command = value
                    .get("command")
                    .and_then(toml::Value::as_str)
                    .map(str::to_string);
                let entry = ProviderEntry {
                    selection: ProviderSelection::External(id.clone()),
                    label: string_field(value, "label").unwrap_or_else(|| id.clone()),
                    account: string_field(value, "account").or_else(|| account_suffix(id)),
                    command_status: match command.as_deref() {
                        Some(command) if command_is_executable(command) => {
                            ProviderCommandStatus::Available
                        }
                        _ => ProviderCommandStatus::Missing,
                    },
                    active: active_provider == Some(id.as_str()),
                };
                external.push((order, position, entry));
            }
            // Explicit `order` first (ascending), then config declaration order.
            external.sort_by_key(|(order, position, _)| (order.is_none(), *order, *position));
            entries.extend(external.into_iter().map(|(_, _, entry)| entry));
        }

        Self { entries }
    }

    /// Look up an entry by catalog id (`"grok"` selects the native agent).
    pub fn find(&self, catalog_id: &str) -> Option<&ProviderEntry> {
        self.entries
            .iter()
            .find(|entry| entry.selection.catalog_id() == catalog_id)
    }

    /// Resolve typed `/provider` input: exact catalog id first, then a
    /// case-insensitive match on the id, the label, or the rendered
    /// "label · account" row text.
    pub fn resolve(&self, query: &str) -> Option<&ProviderEntry> {
        let query = query.trim();
        if query.is_empty() {
            return None;
        }
        if let Some(entry) = self.find(query) {
            return Some(entry);
        }
        self.entries.iter().find(|entry| {
            entry.selection.catalog_id().eq_ignore_ascii_case(query)
                || entry.label.eq_ignore_ascii_case(query)
                || entry.row_label().eq_ignore_ascii_case(query)
        })
    }
}

fn string_field(value: &toml::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(toml::Value::as_str)
        .map(str::to_string)
}

/// `claude-c1` → `Some("c1")`; `codex` → `None`.
fn account_suffix(id: &str) -> Option<String> {
    match id.split_once('-') {
        Some((family, account)) if !family.is_empty() && !account.is_empty() => {
            Some(account.to_string())
        }
        _ => None,
    }
}

/// Whether `command` resolves to an executable file, either at an explicit
/// path or on `PATH`. Never runs the command.
fn command_is_executable(command: &str) -> bool {
    let path = Path::new(command);
    if path.components().count() > 1 {
        return executable_file(path);
    }
    find_on_path(command).is_some()
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(name))
        .find(|candidate| executable_file(candidate))
}

#[cfg(unix)]
fn executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn executable_file(path: &Path) -> bool {
    path.is_file()
}
