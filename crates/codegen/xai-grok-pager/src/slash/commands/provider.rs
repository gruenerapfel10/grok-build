//! `/provider` (alias `/p`) — switch agent provider / account.

use crate::app::actions::Action;
use crate::app::provider_catalog::{ProviderCatalog, ProviderEntry};
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

/// Switch the running pager to another configured agent provider.
pub struct ProviderCommand;

impl SlashCommand for ProviderCommand {
    fn name(&self) -> &str {
        "provider"
    }

    fn aliases(&self) -> &[&str] {
        &["p"]
    }

    fn description(&self) -> &str {
        "Switch agent provider"
    }

    fn session_scoped(&self) -> bool {
        false
    }

    fn offered_when_session_less(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/provider <id or label>"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn args_required(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("<provider>")
    }

    fn suggest_provider_args(
        &self,
        _ctx: &AppCtx,
        catalog: &ProviderCatalog,
        _args_query: &str,
    ) -> Option<Vec<ArgItem>> {
        if catalog.entries.is_empty() {
            return None;
        }
        Some(build_provider_items(catalog))
    }

    fn arg_item_selectable(&self, catalog: &ProviderCatalog, item: &ArgItem) -> bool {
        catalog
            .find(item.insert_text.trim())
            .map(|entry| entry.command_status.is_selectable())
            .unwrap_or(true)
    }

    /// The catalog lives in `AppView`, so the raw query travels with the
    /// action and the router resolves it against the live catalog.
    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        let trimmed = args.trim();
        if trimmed.is_empty() {
            return CommandResult::Error("Usage: /provider <id or label>".into());
        }
        CommandResult::Action(Action::SwitchProvider {
            query: trimmed.to_string(),
        })
    }
}

fn build_provider_items(catalog: &ProviderCatalog) -> Vec<ArgItem> {
    catalog
        .entries
        .iter()
        .map(|entry| {
            let catalog_id = entry.selection.catalog_id();
            let display = provider_row_display(entry);
            let description = if entry.command_status.is_selectable() {
                String::new()
            } else {
                "not installed".into()
            };
            let account = entry.account.as_deref().unwrap_or("");
            let match_text = format!("{catalog_id} {} {account}", entry.label);
            ArgItem {
                display,
                match_text,
                insert_text: catalog_id.to_string(),
                description,
            }
        })
        .collect()
}

/// Picker label. Active rows place `(current)` after the provider label so
/// truncation on long account text does not hide it.
fn provider_row_display(entry: &ProviderEntry) -> String {
    if entry.active {
        match entry.account.as_deref() {
            Some(account) if !account.is_empty() => {
                format!("{} (current) · {account}", entry.label)
            }
            _ => format!("{} (current)", entry.label),
        }
    } else {
        entry.row_label()
    }
}
