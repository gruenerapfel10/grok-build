# design notes

## provider picker (`/provider`, `/p`)

The provider and account switcher reuses the existing slash arg picker path (`ArgPicker` modal and the inline slash dropdown), same as `/model` and `/theme`. Rows come from `AppView::provider_catalog` via `SlashController::set_provider_catalog`; the command does not load config itself.

Missing launch commands render dimmed with a `not installed` description. `SlashCommand::arg_item_selectable` (overridden only on `/provider`) gates selection from `command_status`. Enter on the active provider is a no-op (`HandledNoOp`); available rows dispatch `Action::SwitchProvider { catalog_id }`.
