# Development Guide

## Prerequisites

- **Rust** stable toolchain, 2024 edition (see `rust-toolchain.toml`)
- **aivyx-core** checked out at `../aivyx-core` (10 foundation crates)
- **aivyx** monorepo checked out at `../aivyx` (task-engine crate)
- The workspace uses `[patch]` sections to resolve git deps to local paths for both repos

## Building

```sh
cargo build              # Debug
cargo build --release    # Release (binary at target/release/aivyx)
cargo check              # Type-check only (fast)

# With optional desktop interaction backends:
cargo build --features accessibility        # AT-SPI2 (atspi + zbus)
cargo build --features browser-automation   # CDP (tokio-tungstenite)
cargo build --features media-control        # D-Bus MPRIS (zbus)
cargo build --features desktop-full         # All interaction backends
```

## Running

```sh
# Launch TUI (auto-runs Genesis wizard on first launch)
cargo run

# Or explicitly run setup
cargo run -- init

# One-shot chat
cargo run -- chat "What's the weather?"

# With debug logging
RUST_LOG=debug cargo run

# Targeted logging
RUST_LOG=aivyx_loop=trace,aivyx_agent=debug cargo run

# Skip passphrase prompt
AIVYX_PASSPHRASE=mypass cargo run
```

## Project Layout

```
Cargo.toml                    Workspace root
├── [workspace.dependencies]  All dependency versions defined here
├── [patch]                   Redirects aivyx-core git deps to ../aivyx-core
│
crates/
├── aivyx-pa/                 Library + binary crate (13 source files)
│   ├── main.rs               CLI entry, passphrase handling, dispatch
│   ├── lib.rs                Library root (re-exports)
│   ├── agent.rs              Agent construction, tool registration, brain/self-model seeding
│   ├── config.rs             PA-specific config, system prompts, PA_SOUL_CORE, onboarding messages
│   ├── api.rs                HTTP API server (axum) — chat SSE, goals, audit, approvals, settings
│   ├── runtime.rs            Shared key derivation, loop context, loop config building
│   ├── sessions.rs           Conversation session persistence (encrypted)
│   ├── settings.rs           Settings snapshot, config writers (toggle, array, multiline string)
│   ├── schedule_tools.rs     Schedule CRUD agent tools (create, edit, delete [[schedules]])
│   ├── init.rs               Genesis wizard (10 steps with persona-aware defaults)
│   ├── persona_defaults.rs   Per-persona bundles (skills, goals, schedules, heartbeat, soul)
│   ├── oauth.rs              OAuth2 flow for MCP plugins (PKCE, CSRF, encrypted token storage)
│   ├── webhook.rs            Webhook server for inbound integrations
│   └── tests/agent_integration.rs  Agent integration tests
│
├── aivyx-tui/                TUI binary crate (ratatui + crossterm)
│   ├── main.rs               TUI entry, event loop, focus-based key handler, modal popup intercepts
│   ├── app.rs                App state, View/Focus enums, 3 popup state machines (Settings/Goal/Chat),
│   │                         data refresh, chat streaming with timeout, session management, export
│   ├── theme.rs              Stitch design system colors and styles (14 semantic styles)
│   ├── views/                10 view renderers
│   │   ├── home.rs           Dashboard with status cards + telemetry sidebar
│   │   ├── chat.rs           Chat with scroll, streaming, context bar, session/prompt/export popups
│   │   ├── activity.rs       Timeline + detail panel + filter tabs (All/Schedule/Heartbeat)
│   │   ├── goals.rs          Goal CRUD: create, edit, complete, abandon, detail panel with sub-goals
│   │   ├── approvals.rs      Approval queue with approve/deny
│   │   ├── missions.rs       Mission list with step progress
│   │   ├── audit.rs          Audit log + chain status + detail
│   │   ├── memory.rs         Memory browser with kind badges
│   │   ├── settings.rs       Fully interactive settings (9 cards, 5 popup types, next-fire times)
│   │   └── genesis.rs        First-launch 10-step setup wizard
│   └── widgets/              Reusable TUI widgets
│       ├── sidebar.rs        Navigation with badges and agent footer
│       ├── header.rs         Top bar with view name + streaming status
│       └── telemetry.rs      System telemetry sidebar
│
├── aivyx-actions/            Action library (45 modules)
│   ├── lib.rs                Action trait, ActionRegistry, shared resolve_url utility
│   ├── bridge.rs             Action→Tool adapter, 15 registration functions
│   ├── email.rs              IMAP/SMTP (TLS), fetch_email, reply threading (in_reply_to)
│   ├── files.rs              File operations (tokio::fs)
│   ├── shell.rs              Shell execution (tokio::process)
│   ├── web.rs                HTTP fetch + DuckDuckGo search (reqwest)
│   ├── reminders.rs          Time-based reminders (encrypted persistence)
│   ├── calendar.rs           CalDAV integration (today_agenda, events, conflicts)
│   ├── contacts.rs           CardDAV sync + local contact store (search, list)
│   ├── documents.rs          Document vault (index, search, read, list)
│   ├── finance.rs            Transactions, budgets, bills, receipt filing
│   ├── knowledge.rs          Knowledge base actions
│   ├── plugin.rs             Plugin system actions
│   ├── triage_tools.rs       User-facing triage tools (log viewer, rule manager)
│   ├── workflow/              Workflow system
│   │   ├── mod.rs            Workflow templates, triggers, conditions, CRUD + run tools
│   │   └── library.rs        Pre-built workflow template library
│   ├── undo.rs               Undo system (record, list, reverse destructive actions)
│   ├── retry.rs              Retry helper for transient network failures
│   ├── messaging/            Telegram + Matrix + Signal + SMS messaging integrations
│   │   ├── mod.rs            Shared config types (TelegramConfig, MatrixConfig, Message)
│   │   ├── telegram.rs       Telegram Bot API (send_telegram, read_telegram)
│   │   ├── matrix.rs         Matrix CS API (send_matrix, read_matrix)
│   │   ├── signal.rs         Signal messaging integration
│   │   └── sms.rs            SMS messaging integration
│   ├── desktop/              Desktop interaction (subprocess-based, zero deps)
│   │   ├── mod.rs            Config, denylist, display server detection, subprocess runner
│   │   ├── open.rs           App launching (xdg-open / direct spawn)
│   │   ├── clipboard.rs      Clipboard read/write (xclip / wl-copy)
│   │   ├── windows.rs        Window management (wmctrl / xdotool, X11 only)
│   │   ├── notify.rs         Desktop notifications (notify-send)
│   │   └── interaction/      Deep application interaction (multi-backend)
│   │       ├── mod.rs        InteractionConfig, UiBackend trait, BackendRouter (smart routing), InteractionContext
│   │       ├── atspi.rs      AT-SPI2 accessibility backend (feature: accessibility)
│   │       ├── cdp.rs        Chrome DevTools Protocol backend (feature: browser-automation)
│   │       ├── dbus.rs       D-Bus MPRIS2 media control (feature: media-control)
│   │       ├── ydotool.rs    Universal input injection fallback (subprocess)
│   │       ├── screenshot.rs Native window screenshot (grim/import subprocess)
│   │       ├── window_manage.rs Window management (minimize/maximize/resize/move via wmctrl/xdotool/hyprctl)
│   │       ├── system_ctl.rs System controls (volume, brightness, notifications, file manager)
│   │       ├── screen_ocr.rs  Screen region OCR via Tesseract subprocess
│   │       ├── desktop_info.rs Running apps listing + workspace management
│   │       ├── documents.rs   Document creation, editing, conversion, PDF reading
│   │       └── tools.rs      48 Action implementations (19 UI + 14 browser + 2 media + 5 system + 2 desktop + 6 document)
│   └── devtools/             Developer tool integrations
│       ├── mod.rs            Shared devtools types and utilities
│       ├── git.rs            Git operations
│       ├── pr.rs             Pull request management
│       ├── issues.rs         Issue tracking
│       └── ci.rs             CI/CD pipeline actions
│
└── aivyx-loop/               Background loop (8 modules)
    ├── lib.rs                AgentLoop, goal evaluation, InteractionSignals, MoodSignal, ResourceBudget, HeartbeatConfig
    ├── heartbeat.rs          LLM-driven autonomous reasoning, context fusion, 15 action types, milestone/achievement detection
    ├── pacing.rs             Notification throttling (quiet hours, rate limit, mood gating, engagement deferral)
    ├── trigger.rs            Workflow trigger engine (cron, email, goal, cooldown dedup)
    ├── priority.rs           Heuristic urgency scoring (0.0–1.0), priority ranking, shared Priority enum
    ├── triage.rs             Autonomous email triage (rules + LLM classification)
    ├── schedule.rs           Briefing time detection
    ├── briefing.rs           Morning briefing structures (uses priority::Priority)
    └── sources.rs            Reserved for future trait-based source implementations
```

## Adding a New Action

1. Create the action struct in a new file under `crates/aivyx-actions/src/`:

```rust
// crates/aivyx-actions/src/my_action.rs
use crate::Action;
use aivyx_core::Result;

pub struct MyAction;

#[async_trait::async_trait]
impl Action for MyAction {
    fn name(&self) -> &str { "my_action" }

    fn description(&self) -> &str {
        "One-line description for the LLM's tool list"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "param": { "type": "string", "description": "What this does" }
            },
            "required": ["param"]
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let param = input["param"].as_str().unwrap_or_default();
        // Do the thing
        Ok(serde_json::json!({ "status": "done" }))
    }
}
```

2. Add the module to `lib.rs`:
```rust
pub mod my_action;
```

3. Register it in `bridge.rs`:
```rust
pub fn register_default_actions(registry: &mut aivyx_core::ToolRegistry) {
    // ... existing registrations ...
    use crate::my_action::MyAction;
    registry.register(Box::new(ActionTool::new(Box::new(MyAction))));
}
```

If the action needs configuration (like email needs `EmailConfig`), create a separate registration function like `register_email_actions()`.

If the action needs capability gating:
```rust
registry.register(Box::new(
    ActionTool::new(Box::new(MyAction))
        .with_scope(CapabilityScope::Shell { allowed_commands: vec![] }),
));
```

## Adding a New Goal Action Type

To make the background loop recognize a new kind of goal:

1. Add a variant to `GoalAction` in `crates/aivyx-loop/src/lib.rs`:
```rust
enum GoalAction {
    CheckEmail { query: String },
    CheckWeb { url: String },
    CheckReminders,
    CheckCalendar { calendar_id: String },  // New
    NoAction,
}
```

2. Add keyword matching in `match_goal_to_action()`:
```rust
if desc.contains("calendar") || desc.contains("meeting") || desc.contains("schedule") {
    return GoalAction::CheckCalendar {
        calendar_id: "primary".into(),
    };
}
```

3. Add the execution handler in `evaluate_goals()`:
```rust
GoalAction::CheckCalendar { calendar_id } => {
    check_calendar_for_goal(ctx, tx, goal, &calendar_id).await;
}
```

4. Implement `check_calendar_for_goal()` following the pattern of `check_email_for_goal()`.

## Key Patterns

### MasterKey is !Clone

`MasterKey` intentionally does not implement `Clone`. When multiple consumers need derived keys, call `derive_brain_key(&master_key)` (or `derive_memory_key`) separately for each:

```rust
// Each brain tool needs its own derived key
registry.register(Box::new(BrainSetGoalTool::new(
    Arc::clone(&brain_store), derive_brain_key(&master_key),
)));
registry.register(Box::new(BrainListGoalsTool::new(
    Arc::clone(&brain_store), derive_brain_key(&master_key),
)));
```

HKDF is deterministic — all calls with the same master key and domain produce the same derived key.

### Derive Before Move

When `master_key` is moved into `build_agent()`, any keys needed afterward must be derived first:

```rust
let loop_brain_key = derive_brain_key(&master_key);  // Before move
let built = build_agent(..., master_key, ...);        // master_key moved here
// loop_brain_key is still usable
```

### MasterKey Bytes for Task Engine

The task engine needs to reconstruct a `MasterKey` on demand (each mission tool call opens a fresh `TaskEngine`). Since `MasterKey` is `!Clone`, `build_agent()` captures the raw bytes early:

```rust
let master_key_bytes = Zeroizing::new(master_key.expose_secret().to_vec());
```

The `MissionToolContext` stores these bytes and reconstructs via:

```rust
fn reconstruct_master_key(&self) -> Result<MasterKey> {
    let bytes: [u8; 32] = self.master_key_bytes.as_slice().try_into()?;
    Ok(MasterKey::from_bytes(bytes))
}
```

`Zeroizing<Vec<u8>>` ensures the key material is zeroed on drop.

### Graceful Degradation

Every optional subsystem uses the same pattern:

```rust
let subsystem = match wire_subsystem(...) {
    Ok(s) => Some(s),
    Err(e) => {
        tracing::warn!("Subsystem unavailable: {e}");
        None
    }
};
```

Never `unwrap()` or `expect()` on subsystem initialization. The assistant should always start.

### Security Hardening

Key security properties maintained across the codebase:

- **IMAP timeout**: All IMAP tag reads wrapped in `tokio::time::timeout(30s)` to prevent hangs
- **No unwrap on serialization**: All `serde_json::to_value()` calls use `?` with the `Serialization(#[from])` error variant
- **Secret redaction**: `TelegramConfig`, `MatrixConfig` implement manual `Debug` with `[REDACTED]` tokens; `CalendarConfig.password` and `ContactsConfig.password` use `#[serde(skip)]`
- **Passphrase minimum**: 8 characters (enforced in `init.rs`)
- **Body size limit**: Webhook server caps request bodies at 1 MB (`DefaultBodyLimit::max`)
- **Conversation cap**: Saved conversations are truncated to 500 messages
- **Triage log pruning**: Capped at 500 entries, calendar markers pruned after 7 days, trigger state after 24h
- **Prompt injection defense**: Email subjects and calendar data sanitized in briefing prompts (control chars stripped, backticks/angle brackets replaced, 200-char truncation)
- **OAuth CSRF protection**: OAuth callback validates a random `state` parameter to prevent cross-site request forgery
- **Briefing hour validation**: Values outside 0-23 are clamped with a warning

### Action vs Tool

- **Action** (aivyx-actions): knows how to do something in the real world. No UUID, no capability scope.
- **Tool** (aivyx-core): something the LLM can call. Has a UUID, capability scope, and is registered in the `ToolRegistry`.
- **ActionTool** (bridge.rs): wraps an Action as a Tool.

Keep new actions in `aivyx-actions` using only the `Action` trait. The bridge handles the rest.

### Notification Flow

Loop → TUI communication is via `mpsc::UnboundedReceiver<Notification>`:

```rust
// Loop emits
tx.send(Notification { kind: NotificationKind::Info, title: "...", ... });

// TUI drains
while let Ok(notif) = notification_rx.try_recv() {
    app.notifications.push(format_notification(&notif));
}
```

Never block on `notification_rx` — the TUI event loop must stay responsive (50ms poll).

## Debugging

### Common Issues

**"Memory system unavailable"** — The embedding provider couldn't be created. Check that `[embedding]` is configured in config.toml, or that your Ollama instance has an embedding model pulled.

**"Brain system unavailable"** — The BrainStore couldn't open. Check filesystem permissions on `~/.aivyx/brain/`.

**"Email password not set in keystore"** — Run `aivyx init` and configure email, or manually add `EMAIL_PASSWORD` to the encrypted store.

**"Wrong passphrase"** — The passphrase doesn't match what was used during `aivyx init`. There's no recovery — if you've forgotten it, delete `~/.aivyx/` and re-run `aivyx init`.

### Logging Targets

| Target | What It Shows |
|---|---|
| `aivyx_loop` | Goal evaluation, tick execution, email/web checks, triage orchestration |
| `aivyx_loop::heartbeat` | Heartbeat ticks, context fusion, priority scoring, 15-action dispatch |
| `aivyx_loop::pacing` | Notification throttling — quiet hours, rate limit, mood gating, engagement |
| `aivyx_loop::triage` | Email triage decisions, rule matching, LLM classification, cursor state |
| `aivyx_loop::priority` | Priority scoring details, item ranking |
| `aivyx_agent` | Agent turns, tool calls, LLM interactions |
| `aivyx_memory` | Memory store/recall operations |
| `aivyx_brain` | Goal CRUD, self-model updates |
| `aivyx_crypto` | Key derivation, encryption operations |
| `aivyx_llm` | Provider API calls, streaming |
| `aivyx_actions` | Action execution results |
| `aivyx_task_engine` | Mission planning, step execution, checkpoints |

**Note:** In TUI mode, all logs route to `~/.aivyx/pa.log` via `tracing-appender` to avoid corrupting the terminal UI. In CLI mode (`chat`, `status`), logs go to stderr as usual.

## Phase 5B Patterns

### Adding a Heartbeat Action

The heartbeat system follows a consistent pattern for new actions:

1. **Enum variant** — Add to `HeartbeatAction` in `heartbeat.rs` with `#[serde(rename = "...")]`
2. **Permission flag** — Add `can_*: bool` to `HeartbeatConfig` in `lib.rs` (default `false`)
3. **Prompt entry** — Add to `build_heartbeat_prompt()` gated by the `can_*` flag
4. **Dispatch handler** — Add match arm in `dispatch_actions()` with guard: `HeartbeatAction::Foo if config.can_foo =>`
5. **PA config** — Mirror the flag in `PaHeartbeatConfig` in `config.rs` (default `false`)
6. **Runtime wiring** — Map the flag in `build_loop_config()` in `runtime.rs`
7. **Settings** — Add to `SettingsSnapshot` in `settings.rs` and populate in `reload_settings_snapshot()`
8. **TUI** — Display in the Heartbeat card in `views/settings.rs`

Current actions (15): `reflect`, `update_goal`, `set_reminder`, `suggest`, `notify`, `learn_from_failure`, `extract_knowledge`, `no_action`, `prune_audit`, `backup`, `plan_review`, `strategy_review`, `track_mood`, `encourage`, `consolidate_memory`.

#### Notification Pacing

The `pacing` module (`crates/aivyx-loop/src/pacing.rs`) gates notification delivery at the Rust level before they reach the user. Rules (priority order):

1. `Urgent` always sends
2. During quiet hours: only Urgent sends
3. Hourly rate limit: defer non-urgent when exceeded
4. Frustrated mood: only Urgent + ActionTaken
5. Active engagement: defer Info when user message rate is high

Pacing is a hard programmatic gate; mood-based tone adaptation is a soft LLM-level signal via heartbeat context injection.

### Adding a TUI View

1. Add variant to `View` enum in `crates/aivyx-tui/src/app.rs`
2. Update `View::ALL` array, `label()`, `icon()`, `group()` methods
3. Add state fields to `App` struct
4. Create view renderer in `crates/aivyx-tui/src/views/new_view.rs`
5. Register the view in `views/mod.rs`
6. Add `content_up`/`content_down` handlers in `main.rs` key handler
7. Wire data refresh in `App::refresh_data()`

### Genesis Wizard (init.rs)

The 10-step interactive setup generates a complete agent config (~100-150 lines):

| Step | Collects | Config Section |
|---|---|---|
| 1. Passphrase | Master key encryption | `master_key.json` |
| 2. Provider + Model | LLM backend; auto-detects Ollama models via `/api/tags` | `[provider]` |
| 3. Name + Persona | Agent identity; shows persona preview from `PersonaBundle` | `[agent]` |
| 4. Soul | Personality narrative (accept/edit/skip) | `[agent] soul = """..."""` |
| 5. Skills | Accept persona defaults + add custom | `[agent] skills = [...]` |
| 6. Schedule | Briefing hour, check interval | `[loop]` |
| 7. Goals | Persona-specific starter goals | `[[initial_goals]]` |
| 8. Email | Optional IMAP/SMTP | `[email]` |
| 9. Integrations | Calendar, Contacts, Telegram, Matrix, Signal | `[calendar]`, `[telegram]`, etc. |
| 10. Intelligence | Toggle 10 heartbeat flags | `[heartbeat]` |

Every new step defaults to "accept recommended" on Enter — a user can press Enter through all 10 steps and get a fully-configured agent.

### PersonaDefaults Module

`persona_defaults.rs` provides zero-allocation `&'static` bundles for 8 personas:

```rust
pub struct PersonaBundle {
    pub skills: &'static [&'static str],
    pub goals: &'static [GoalTemplate],
    pub schedules: &'static [ScheduleTemplate],
    pub heartbeat: HeartbeatBundle,
    pub soul_template: &'static str,
    pub tool_discovery_always_include: &'static [&'static str],
}
```

Lookup: `persona_defaults::for_persona("coder")` — falls back to `assistant` for unknown names. Used by the wizard (steps 3-10), and available to the TUI for quick-setup flows.

### Config Section Formatters

`init.rs` provides TOML fragment generators consumed by the wizard:

- `format_heartbeat_section(&HeartbeatBundle)` → `[heartbeat]` block
- `format_skills_line(&[&str])` → inline TOML array
- `format_goals_section(&[GoalTemplate])` → `[[initial_goals]]` blocks
- `format_schedules_section(&[ScheduleTemplate], agent_name)` → `[[schedules]]` blocks
- `format_soul_line(&str)` → triple-quoted TOML string
- `format_persona_section(persona_name)` → `[persona]` with tuned dimensions

All formatters use `sanitize_toml_value()` for injection safety and are roundtrip-tested against `PaConfig` deserialization.

### Interactive Settings TUI

The Settings view (`views/settings.rs`) is fully interactive with all 8 cards editable:

- 8 cards in two columns (0-3 left, 4-7 right)
- `settings_card_index` / `settings_item_index` on `App` track selection
- Selected card: primary border color; selected item: `highlight()` style
- 5 popup types: `TextInput`, `MultiLineInput`, `Confirm`, `SkillManager`, `IntegrationSetup`
- Popup state machine pattern: `Option<SettingsPopup>.take()` — arms that don't re-insert close the popup

| Card | Items | Interaction |
|---|---|---|
| 0 Provider | model, base_url | TextInput popup |
| 1 Autonomy | tier, rate_limit, max_cost | Inline cycle / TextInput (UInt, Float) |
| 2 Heartbeat | enabled + 10 flags | `[x]`/`[ ]` toggle via `toggle_config_bool()` |
| 3 Schedules | dynamic (per schedule) | Enter to toggle enabled, croner next-fire time display |
| 4 Agent | name, soul, skills | TextInput, MultiLineInput, SkillManager |
| 5 Integrations | 8 integration types | Vertical `[x]`/`[ ]` list + IntegrationSetup popup |
| 7 Persona | 5 dimensions | `◄ ━━━━━━━━ ►` slider via Left/Right ±0.1 |

Settings writers in `settings.rs` (PA library):
- `toggle_config_bool()` — in-place boolean toggle
- `write_toml_string()` — quoted string value replacement
- `write_toml_number()` — unquoted numeric value replacement
- `write_toml_string_array()` — array value replacement
- `write_toml_multiline_string()` — single/triple-quoted string replacement or append
- `write_integration_config()` — append new section
- `toggle_schedule_enabled()` — scan `[[schedules]]` blocks by name, toggle `enabled` key

### Goal CRUD (Goals View)

The Goals view supports full interactive management:

- `n` create: popup with description, success criteria, priority selector
- `e` edit: popup with description, criteria, priority, deadline (YYYY-MM-DD)
- `c` complete: confirmation dialog → `BrainStore::upsert_goal()`
- `x` abandon: confirmation dialog → `BrainStore::upsert_goal()`
- Detail panel: sub-goals list, deadline, failure info (count + cooldown status)
- Popup state machine: `Option<GoalPopup>.take()` with 3 variants (Create, Edit, Confirm)

### Chat Quality (Chat View)

The Chat view includes conversation management features:

- **Context bar**: session ID, input/output tokens (K/M suffixes), cost, message count
- **Session switching** (Ctrl+S): popup listing saved sessions, create new, delete with `d`
- **System prompt preview** (Ctrl+P): scrollable overlay of composed system prompt
- **Chat export** (Ctrl+E): write conversation as timestamped markdown file
- **Streaming timeout**: 5-minute `tokio::time::timeout` wrapping `turn_stream`, error/timeout appended inline
- Popup state machine: `Option<ChatPopup>.take()` with 3 variants (SessionList, SystemPrompt, ExportDone)

### Activity Filters

The Activity view supports filtering via `[`/`]`:

| Filter | Shows |
|---|---|
| All (0) | All notifications |
| Schedule (1) | `source == "schedule"` or `source == "briefing"` |
| Heartbeat (2) | `source.contains("heartbeat")` |

### LoopContext Extension

When adding data to the background loop:

1. Add field to `LoopContext` in `lib.rs`
2. Wire in `build_loop_context()` in `runtime.rs`
3. Access in heartbeat dispatch or loop tick via `ctx.field_name`

Keep fields as `Option<T>` for backwards compatibility — existing configs that
don't set the new section should still parse and run without error.

Notable Phase 6 LoopContext fields:
- `interaction_signals: Arc<Mutex<InteractionSignals>>` — shared between TUI/API chat handler and heartbeat; tracks message patterns, notification counts
- `resource_budget: ResourceBudget` — daily token tracking with auto-reset; injected into heartbeat context when > 50% used
- `strategy_review_pending: bool` — flag set by `strategy-review` cron workflow, consumed by next heartbeat tick

### Rust 2024 Edition Notes

- Implicit binding modes: don't use `ref` in `if let` patterns when the match
  target is already a reference. Write `if let (Some(a), Some(b)) = (&x, &y)`
  instead of `if let (Some(ref a), Some(ref b)) = (&x, &y)`.
- No `&` in implicitly-borrowing patterns: `for (tool, score) in sorted.iter()`
  not `for (tool, &score) in sorted.iter()`.
