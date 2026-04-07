# Aivyx-PA Architecture

Deep-dive into how the personal assistant is built, how data flows between subsystems, and why the design looks the way it does.

## System Diagram

```
 User
  │
  ├─ Terminal ─────────────────────────────────────────────────────┐
  │                                                                │
  │  ┌──────────────────── aivyx (binary) ───────────────────────┐ │
  │  │                                                           │ │
  │  │  main.rs ──► unlock(passphrase) ──► MasterKey             │ │
  │  │              load config.toml   ──► AivyxConfig           │ │
  │  │              create_provider    ──► Box<dyn LlmProvider>  │ │
  │  │              wrap_resilient     ──► ResilientProvider     │ │
  │  │              resolve_email      ──► Option<EmailConfig>   │ │
  │  │                    │                                      │ │
  │  │                    ▼                                      │ │
  │  │  agent.rs ► build_agent()                                 │ │
  │  │              │                                            │ │
  │  │              ├── ToolRegistry (up to 81 tools)            │ │
  │  │              ├── wire_memory() → MemoryManager            │ │
  │  │              ├── wire_brain() → Brain + BrainStore + bool │ │
  │  │              │   ├── seed_starter_goals() (first launch)  │ │
  │  │              │   ├── seed_agent_growth_goals() (first)    │ │
  │  │              │   └── seed_initial_self_model() (first)    │ │
  │  │              └── Agent::new(...)                           │ │
  │  │                    │                                      │ │
  │  │          ┌─────────┼──────────────┐                       │ │
  │  │          ▼                        ▼                       │ │
  │  │   ┌──────────┐          ┌──────────────┐                  │ │
  │  │   │  Agent   │          │  AgentLoop   │                  │ │
  │  │   │  (chat)  │◄─ Arc ──│  (background) │                 │ │
  │  │   │          │ BrainSt. │              │                  │ │
  │  │   └────┬─────┘          └──────┬───────┘                  │ │
  │  │        │                       │                          │ │
  │  │        ▼                       ▼                          │ │
  │  │   turn_stream()          run_tick()                       │ │
  │  │   ├─ LLM call            ├─ morning briefing check       │ │
  │  │   ├─ tool calls           ├─ evaluate_goals()             │ │
  │  │   ├─ memory recall        │  ├─ CheckEmail                │ │
  │  │   └─ stream tokens        │  ├─ CheckWeb                  │ │
  │  │        │                  │  └─ record outcome            │ │
  │  │        │                  ├─ heartbeat tick               │ │
  │  │        │                  │  ├─ gather context            │ │
  │  │        │                  │  ├─ skip if empty             │ │
  │  │        │                  │  └─ LLM reason → dispatch     │ │
  │  │        ▼                  │                               │ │
  │  │   ┌─────────────────┐    └──► mpsc::Notification ──┐     │ │
  │  │   │  TUI Renderer   │◄────────────────────────────┘     │ │
  │  │   │  (ratatui)      │                                    │ │
  │  │   └─────────────────┘                                    │ │
  │  └───────────────────────────────────────────────────────────┘ │
  └────────────────────────────────────────────────────────────────┘
```

## Startup Sequence

```
main()
  │
  ├── 1. Parse CLI args (clap)
  ├── 2. Load AivyxDirs (locate ~/.aivyx)
  ├── 3. ensure_initialized() — check master_key.json exists
  ├── 4. unlock() — passphrase → Argon2id → decrypt MasterKey from envelope
  ├── 5. AivyxConfig::load() — parse config.toml (provider, autonomy, embedding)
  ├── 6. PaConfig::load() — parse same config.toml for [email] section
  ├── 7. EncryptedStore::open() — open redb keystore
  ├── 8. create_provider() — instantiate LLM provider from config + decrypted API key
  ├── 9. wrap_provider_resilient() — wrap in ResilientProvider (circuit breaker + fallback) + CachingProvider (if [resilience] configured)
  ├── 10. resolve_email_config() — load EMAIL_PASSWORD from keystore, build EmailConfig
  └── 11. Dispatch to TUI or one-shot chat
```

**For TUI mode (aivyx-tui):**
```
tui::main()
  │
  ├── derive_all_keys(&master_key)     ← MUST happen before master_key is moved
  ├── resolve service configs (email, calendar, contacts, etc.)
  ├── build_agent() → BuiltAgent { agent, brain_store, mission_ctx, is_first_launch, ... }
  │     ├── capture master_key bytes     (Zeroizing<Vec<u8>> for task engine)
  │     ├── register_default_actions()    9 tools (files, shell, web, reminders)
  │     ├── register_reminder_actions()   (encrypted persistence)
  │     ├── register_email_actions()      +3 tools (if email configured)
  │     ├── register_calendar_actions()   +3 tools (if calendar configured)
  │     ├── register_contact_actions()    +2-3 tools (if contacts configured)
  │     ├── register_document_actions()   +4 tools (if vault configured)
  │     ├── register_finance_actions()    +5-6 tools (if finance configured)
  │     ├── register_triage_actions()     +2 tools (if triage enabled)
  │     ├── register_telegram_actions()  +2 tools (if Telegram configured)
  │     ├── register_matrix_actions()    +2 tools (if Matrix configured)
  │     ├── register_workflow_actions()  +4 tools (always)
  │     ├── register_undo_actions()      +3 tools (always)
  │     ├── wire_memory()                 +6 tools
  │     ├── wire_brain()                  +5 tools (4 brain + 1 self-model)
  │     │   └── first launch: seed_starter_goals(), seed_agent_growth_goals(), seed_initial_self_model()
  │     ├── AgentSession::new()           (for task engine sub-agents)
  │     └── mission tools                 +4 tools (PA-local mission_create with brain bridge)
  │
  ├── build_loop_context() (from runtime.rs) with Arc<BrainStore> + keys + configs
  │     + audit_log + mcp_pool + consolidation_config
  ├── AgentLoop::start(config, context)
  │     └── spawns tokio task: run_loop()
  │
  ├── Spawn agent turn handler task (msg_rx → agent.turn_stream → token_tx)
  ├── Initialize terminal (raw mode, alternate screen)
  └── Event loop: poll keys + drain notifications + drain tokens + render
```

## Key Derivation Flow

```
User passphrase (string)
        │
        ▼
   ┌─────────┐
   │ Argon2id │  (salt from envelope, high memory/time cost)
   └────┬────┘
        ▼
   Master Key (32 bytes, ChaCha20 key)
        │
        ├── Envelope decrypt:  stored in ~/.aivyx/master_key.json
        │
        ├── EncryptedStore:    direct key for redb value encryption
        │   └── API keys, EMAIL_PASSWORD, other secrets
        │
        ├── HKDF("memory"):   derive_memory_key(&master_key)
        │   └── Encrypts episodic memory entries in MemoryStore
        │
        ├── HKDF("brain"):    derive_brain_key(&master_key)
        │   └── Encrypts goals, self-model in BrainStore
        │   └── Called MULTIPLE TIMES (one per consumer, MasterKey is !Clone)
        │       ├── Agent brain tools (4 calls)
        │       ├── Brain::from_store() (1 call)
        │       └── LoopContext (1 call)
        │
        ├── HKDF("task"):    derive_task_key(&master_key)
        │   └── Encrypts mission state and checkpoints in TaskStore
        │
        ├── HKDF("audit"):   derive_audit_key(&master_key)
        │   └── HMAC chain for audit log integrity
        │
        ├── HKDF("reminders"): derive_domain_key(&master_key, b"reminders")
        │   └── Encrypts reminder persistence in EncryptedStore
        │
        ├── HKDF("contacts"):  derive_domain_key(&master_key, b"contacts")
        │   └── Encrypts contact store entries
        │
        ├── HKDF("finance"):   derive_domain_key(&master_key, b"finance")
        │   └── Encrypts transactions, budgets, bills
        │
        ├── HKDF("triage"):    derive_domain_key(&master_key, b"triage")
        │   └── Encrypts triage logs, rules, cursor
        │
        ├── HKDF("vault"):     derive_domain_key(&master_key, b"vault")
        │   └── Encrypts document vault index
        │
        ├── HKDF("conversation"): derive_domain_key(&master_key, b"conversation")
        │   └── Encrypts conversation history
        │
        ├── HKDF("workflow"):  derive_domain_key(&master_key, b"workflow")
        │   └── Encrypts workflow templates
        │
        ├── HKDF("undo"):     derive_domain_key(&master_key, b"undo")
        │   └── Encrypts undo records (24h TTL)
        │
        └── Raw bytes:       Zeroizing<Vec<u8>> in MissionToolContext
            └── Reconstructed on demand for AgentSession in task engine
```

The HKDF domain separation is critical: even if one subsystem's key is somehow exposed, it cannot decrypt data from another subsystem. Each call to `derive_brain_key()` produces the same deterministic key (same master + same domain), so multiple callers can independently derive matching keys.

## Agent Construction (build_agent)

`agent.rs:build_agent()` is the central wiring point (in the `aivyx-pa` library crate). Here's the exact registration order:

```
ToolRegistry
  │
  ├── register_default_actions()           (always)
  │   ├── read_file, write_file, list_directory
  │   ├── run_command                      ← CapabilityScope::Shell
  │   ├── fetch_webpage, search_web
  │
  ├── register_reminder_actions()          (always, encrypted persistence)
  │   ├── set_reminder, list_reminders, dismiss_reminder
  │
  ├── register_email_actions()             (if email configured)
  │   ├── read_email, fetch_email, send_email
  │
  ├── register_calendar_actions()          (if calendar configured)
  │   ├── today_agenda, fetch_calendar_events, check_conflicts
  │
  ├── register_contact_actions()           (if contacts configured)
  │   ├── search_contacts, list_contacts
  │   └── sync_contacts                    (only if CardDAV configured)
  │
  ├── register_document_actions()          (if vault configured)
  │   ├── search_documents, read_document, list_vault_documents, index_vault
  │
  ├── register_finance_actions()           (if finance configured)
  │   ├── add_transaction, list_transactions, budget_summary
  │   ├── set_budget, mark_bill_paid
  │   └── file_receipt                     (only if email + vault also configured)
  │
  ├── register_triage_actions()            (if triage enabled)
  │   ├── list_triage_log, set_triage_rule
  │
  ├── register_workflow_actions()          (always)
  │   ├── create_workflow, list_workflows, run_workflow, workflow_status
  │
  ├── register_undo_actions()              (always)
  │   ├── record_undo, list_undo_history, undo_action
  │
  ├── register_memory_tools()              (if embedding provider available)
  │   ├── 6 memory tools
  │
  ├── Brain tools                          (if BrainStore opens successfully)
  │   ├── brain_set_goal, brain_list_goals, brain_update_goal
  │   ├── brain_reflect
  │   └── brain_update_self_model          (PA-local)
  │
  ├── Mission tools                        (AgentSession from saved key bytes)
  │   ├── mission_create                   (PaMissionCreateTool — PA-local, brain bridge)
  │   ├── mission_list, mission_status, mission_control  (from monorepo)
  │   └── mission_from_recipe              (PA-local, loads TOML recipe templates)
  │
  ├── MCP tools                            (if MCP servers connected)
  │   └── mcp_list_prompts                 (PA-local, discovers prompt templates)
  │
  ├── Pattern mining                       (if MemoryManager available)
  │   └── list_discovered_patterns         (PA-local, surfaces mined workflow patterns)
  │
  ├── Schedule CRUD tools                  (always)
  │   ├── create_schedule, edit_schedule, delete_schedule
  │
  ├── register_desktop_actions()           (if [desktop] configured)
  │   ├── open_application                 ← CapabilityScope::Custom("desktop")
  │   ├── clipboard_read, clipboard_write  (if config.clipboard)
  │   ├── list_windows, get_active_window, focus_window  (if config.windows)
  │   ├── send_notification                (if config.notifications)
  │   │
  │   └── register_interaction_actions()   (if [desktop.interaction] enabled)
  │       ├── BackendRouter                smart routing: browser class → CDP, native → AT-SPI2, fallback → ydotool
  │       ├── InteractionContext (Arc)      shared state for all interaction tools
  │       ├── UI tools (9 always + 2 if input): ui_inspect, ui_find_element,
  │       │   ui_click, ui_type_text, ui_read_text, ui_scroll, ui_right_click,
  │       │   ui_hover, ui_drag, ui_key_combo, ui_mouse_move
  │       ├── Window tools (1 always): window_screenshot
  │       ├── Browser tools (8, if browser): browser_navigate, browser_query,
  │       │   browser_click, browser_type, browser_read_page, browser_screenshot,
  │       │   browser_scroll, browser_execute_js
  │       └── Media tools (2, if media): media_control, media_info
  │
  └── team_delegate                        (always)

Agent::new(
    id, name, system_prompt, max_tokens,
    autonomy=config.autonomy.default_tier, provider, registry, capabilities,
    rate_limiter(config.autonomy.max_tool_calls_per_minute),
    cost_tracker(config.autonomy.max_cost_per_session_usd),
    audit_log, config.autonomy.max_retries, config.autonomy.retry_base_delay_ms
)

agent.set_require_approval_for_destructive(config.autonomy.require_approval_for_destructive)
agent.set_scope_overrides(config.autonomy.scope_overrides)
agent.set_escalation_confidence_threshold(config.autonomy.escalation_confidence_threshold)
// Seed [style] config preferences into UserProfile (try_lock — sync context)
agent.set_memory_manager(mgr)  // if available
agent.set_brain(brain)          // if available
```

## Deep Application Interaction Architecture

The interaction system uses a multi-backend architecture to automate different application types:

```
[desktop.interaction] config
         ↓
   InteractionContext (Arc, shared state)
         ↓
   BackendRouter ─── route(window) ──→ best backend for that window
         │
         ├── AtSpiBackend    [feature: accessibility]
         │   └── D-Bus → AT-SPI2 registry → app → window → accessible tree
         │       Interfaces: Accessible, Action, Text, EditableText, Component
         │       Coverage: ~70-85% of GTK/Qt/Electron apps
         │
         ├── CdpBackend      [feature: browser-automation]
         │   └── WebSocket → Chrome debug port (9222) → CDP commands
         │       Methods: Page.navigate, DOM.querySelector, Runtime.evaluate,
         │       Input.dispatchMouseEvent, tab management (/json/new, /json/close)
         │       Coverage: Chromium-based browsers (Chrome, Edge, Brave, Electron)
         │
         ├── YdotoolBackend  [always available, subprocess]
         │   └── ydotool CLI → ydotoold → /dev/uinput (kernel-level input)
         │       Operations: key combos, type text, click, double-click, middle-click,
         │       right-click, scroll, drag, hover, mouse move at coordinates
         │       Coverage: 100% (coordinate-based, no semantic understanding)
         │
         ├── WindowManage    [subprocess: wmctrl/xdotool/hyprctl]
         │       Operations: minimize, maximize, restore, close, fullscreen, resize, move
         │
         ├── SystemCtl       [subprocess: wpctl/pactl/brightnessctl/dunstctl/xdg-open]
         │       Operations: volume, brightness, notifications, file manager
         │
         ├── ScreenOcr       [subprocess: grim/import → tesseract]
         │       Operations: capture region → OCR text extraction (last-resort reading)
         │
         ├── DesktopInfo     [subprocess: hyprctl/wmctrl/xprop/xlsclients]
         │       Operations: list running apps, workspace list/switch/move
         │
         ├── Documents       [subprocess: pandoc/libreoffice/pdftotext/ssconvert/weasyprint]
         │       Operations: create text/spreadsheet/PDF, edit text files,
         │       convert formats (md↔html↔pdf↔docx↔odt↔epub), read PDFs
         │
         └── D-Bus MPRIS     [feature: media-control]
             └── dbus-send → org.mpris.MediaPlayer2.Player
                 Methods: PlayPause, Next, Previous, Stop + Metadata properties
```

**Backend routing**: The `BackendRouter` selects the best backend per window using smart class detection. It detects the window class via xdotool (X11) or hyprctl (Wayland) — browser windows (Chrome, Firefox, Brave, Edge, Electron) are routed to CDP, native apps with accessibility support to AT-SPI2, and everything else to ydotool. AT-SPI2 delegates input operations (right-click, drag, hover) to ydotool after resolving element bounds.

**Feature gating**: Optional Cargo features control which backends are compiled. Without features, only ydotool (subprocess) is available — zero new binary dependencies.

## Action → Tool Bridge

The `Action` trait (aivyx-actions) and `Tool` trait (aivyx-core) have similar shapes but serve different layers:

```
Action (aivyx-actions)              Tool (aivyx-core)
├── name() → &str                   ├── id() → ToolId          ← unique UUID
├── description() → &str            ├── name() → &str
├── input_schema() → Value          ├── description() → &str
└── execute(Value) → Value          ├── input_schema() → Value
                                    ├── required_scope() → Option<CapabilityScope>
                                    └── execute(Value) → Value
```

`ActionTool` adapts any `Action` into a `Tool`:
- Generates a random `ToolId` on construction
- Delegates `name()`, `description()`, `input_schema()`, `execute()` to the inner `Action`
- Adds an optional `CapabilityScope` (e.g., `Shell { allowed_commands }` for `run_command`)

This design keeps `aivyx-actions` independent of `aivyx-agent` — actions don't need to know about tool IDs or capability scopes.

## Scoped Autonomy

The autonomy system supports per-scope tier overrides, allowing different trust levels for different tool categories:

```
Tool call arrives at execute_single_tool()
    │
    ├─ resolve_effective_tier(tool_name)
    │   ├─ resolve_scope_tier(tool_name)
    │   │   ├─ no overrides & no confidence map? → return default_tier
    │   │   ├─ tool has no required_scope? → return default_tier
    │   │   └─ scope_discriminant(scope) → match against scope_overrides
    │   │       ├─ match found → override tier
    │   │       └─ no match → default_tier
    │   │
    │   └─ confidence escalation (if tier is Trust/Free)
    │       ├─ domain_confidence[scope] < threshold?
    │       │   ├─ yes → downgrade to Leash + emit ConfidenceEscalation audit event
    │       │   └─ no → keep tier as-is
    │       └─ no confidence data for domain → keep tier (optimistic)
    │
    └─ match effective_tier {
        Locked  → deny + audit
        Leash   → prompt user via channel → approve/deny
        Trust   → auto-execute (check destructive approval if configured)
        Free    → auto-execute always
    }
```

**Scope discriminants** are the simplified category names used for matching:

| CapabilityScope variant | Discriminant |
|---|---|
| `Filesystem { root }` | `filesystem` |
| `Shell { allowed_commands }` | `shell` |
| `Email { allowed_recipients }` | `email` |
| `Calendar` | `calendar` |
| `Network { hosts, ports }` | `network` |
| `Custom("finance")` | `custom:finance` |

This differs from `scope_to_string()` (used in audit logs) which includes parameters — discriminants strip parameters for human-friendly config matching.

## Workflow Engine

### Templates (`aivyx-actions/src/workflow.rs`)

Reusable, parameterized multi-step workflows stored encrypted in the vault:

- **WorkflowTemplate** — name, description, steps, parameters, triggers
- **TemplateStep** — description, optional tool hint, arguments, approval gate, condition, dependencies
- **TemplateParameter** — name, description, required/optional, default value
- **StepCondition** — recursive condition tree: `OnSuccess`, `OnFailure`, `VarEquals`, `VarContains`, `All`, `Any`

Storage: `"workflow:template:{name}"` → JSON in EncryptedStore (workflow domain key).

### Instantiation

`WorkflowTemplate::instantiate(params)` validates required params, applies defaults, and replaces `{param}` placeholders in descriptions and arguments, producing an `InstantiatedWorkflow` with concrete steps.

### Workflow Tools

Four tools registered via `register_workflow_actions()`:

| Tool | Description |
|---|---|
| `create_workflow` | Create or update a template from JSON |
| `list_workflows` | List templates (optional detail mode) |
| `run_workflow` | Instantiate a template with parameters |
| `workflow_status` | Inspect a template's full definition |

### Trigger System (`aivyx-loop/src/trigger.rs`)

Evaluated each loop tick after triage, before heartbeat:

| Trigger Type | Fires When |
|---|---|
| `Cron { expression }` | Cron schedule matched since last trigger evaluation (uses `last_evaluated_at` baseline) |
| `Email { sender_contains, subject_contains }` | Recent email matches sender/subject patterns |
| `GoalProgress { goal_match, threshold }` | Active goal matching substring reaches progress threshold |
| `FileChange { path_glob }` | *(Deferred — requires OS-level file watching)* |
| `Manual` | Only fired explicitly via `run_workflow` tool |

**Cooldown dedup:** Each `(template_name, trigger_index)` pair tracks its last fire time. Re-fires within `cooldown_secs` (default 300s) are suppressed.

When a trigger fires, the loop emits an `ActionTaken` notification for mission creation.

## Background Loop Internals

### Loop Architecture

```
AgentLoop::start()
  │
  ├── Creates 3 channels:
  │   ├── notification_tx/rx  (mpsc::unbounded) → notifications to TUI
  │   ├── shutdown_tx/rx      (oneshot)         → stop signal
  │   └── trigger_tx/rx       (mpsc::unbounded) → manual trigger signal
  │
  └── Spawns: run_loop(config, notification_tx, shutdown_rx, trigger_rx, context)
              │
              ├── Initial 5-second delay (let TUI render first)
              ├── run_tick()  ← first tick
              └── loop {
                    select! {
                      sleep(interval) => run_tick()
                      trigger_rx.recv() => run_tick()     ← manual trigger
                      shutdown => break
                    }
                  }
```

### Goal Evaluation Pipeline

```
run_tick()
  │
  ├── Check morning briefing time
  │   └── is_briefing_time(hour) → within first 5 minutes of the hour
  │
  └── evaluate_goals()
      │
      ├── BrainStore::list_goals(filter: Active)
      │
      └── For each goal:
          │
          ├── Check cooldown (goal.is_in_cooldown())
          │   └── Skip if recently evaluated
          │
          ├── match_goal_to_action(goal) → GoalAction
          │   ├── Keywords: email/inbox/mail → CheckEmail { query }
          │   ├── URL in text → CheckWeb { url }
          │   ├── Keywords: remind/schedule/every → CheckReminders
          │   └── Default → NoAction
          │
          ├── Execute action:
          │   │
          │   ├── CheckEmail:
          │   │   ├── ReadInbox.execute({ limit: 10, unread_only: true })
          │   │   ├── Filter: from/subject/preview contains query (case-insensitive)
          │   │   └── Emit Notification per match
          │   │
          │   └── CheckWeb:
          │       ├── FetchPage.execute({ url })
          │       ├── Take first 500 chars as preview
          │       └── Emit Notification if non-empty
          │
          └── Record outcome → BrainStore:
              ├── Success → goal.record_success() (reset failures, clear cooldown)
              ├── Failure → goal.record_failure() (exponential backoff: 5min × 2^n, cap 24h)
              │   └── Auto-abandon after 10 consecutive failures
              ├── NoMatch → no state change (check ran, found nothing)
              └── Skipped → no state change (no applicable check)

  Email triage (every tick, if enabled):
  │
  ├── fetch unread emails since last cursor (IMAP)
  ├── Rule-based fast path:
  │   ├── should_ignore() → skip (sender in ignore list)
  │   ├── find_auto_reply_rule() → send reply (if can_auto_reply)
  │   └── load_custom_rules() → merge config + stored rules
  ├── LLM classification (batch, one call for all unmatched):
  │   ├── build_classification_prompt() → structured prompt
  │   ├── parse_classification_response() → category, urgency, suggested_reply
  │   └── Forward urgent emails (if can_forward + forward_to set)
  ├── save_triage_log() → encrypted store (triage-log:{seq})
  └── save_cursor() → advance IMAP cursor

  Workflow trigger evaluation (every tick, after triage):
  │
  ├── load all templates with triggers from EncryptedStore
  ├── evaluate each trigger:
  │   ├── Cron: croner::Cron::find_next_occurrence() since last_evaluated_at
  │   ├── Email: match sender/subject against recent triage results
  │   └── GoalProgress: BrainStore::list_goals() → match substring, check threshold
  ├── cooldown dedup: skip if (template, trigger_index) fired within cooldown_secs
  └── fire → emit ActionTaken notification for mission creation

  Heartbeat (every 30 min, runs after goal eval + triage + triggers):
  │
  ├── gather_context() → sections from ALL sources:
  │   ├── goals, reminders, email, self-model, schedules (Phase 1)
  │   ├── calendar events + conflicts (Phase 2)
  │   ├── finance: upcoming bills, over-budget categories (Phase 2)
  │   ├── contacts count (Phase 2)
  │   ├── mood signal: estimate from InteractionSignals → Neutral/Focused/Frustrated/Disengaged (Phase 6)
  │   ├── resource budget: token usage + quiet hours (Phase 6, only when noteworthy)
  │   ├── milestones: goal anniversary detection at 1w/1m/3m/6m/1y thresholds (Phase 6)
  │   ├── achievements: recently completed goals + streak counts (Phase 6)
  │   └── strategy review: extended context when strategy_review_pending flag set (Phase 6)
  ├── build_priority_items() → score items by urgency (0.0–1.0)
  │   ├── score_calendar_event(), score_reminder(), score_upcoming_bill()
  │   ├── score_over_budget(), score_stale_goal(), score_email_age()
  │   └── rank() → sorted by score descending
  ├── format_priority_summary() → top items for prompt
  ├── MCP health check → list_tools() on each pool server, log unreachable
  ├── is_empty()? → SKIP (zero tokens, no LLM call)
  ├── build_heartbeat_prompt() → structured prompt with cross-source reasoning
  ├── LLM chat() → JSON response
  ├── parse_response() → HeartbeatResponse { reasoning, actions[] }
  ├── dispatch_actions() → pacing gate (Phase 6): non-urgent notifications checked
  │   against pacing::should_send() before delivery
  └── 15 action types:
      ├── notify → store Notification for TUI
      ├── suggest → source-tagged notification (from cross-source correlation)
      ├── set_goal → create new brain goal
      ├── update_goal → update progress/status by description match
      ├── reflect → update self-model (strengths, weaknesses, domain confidence)
      ├── consolidate_memory → store recommendation notification
      ├── analyze_failure → store reflection memory, decrease domain_confidence, add weakness
      ├── extract_knowledge → store learned facts as memories
      ├── prune_audit → remove audit entries older than retention period (configurable days)
      ├── backup → archive data directory to configured destination
      ├── plan_review → assign horizon tags (today/week/month/quarter) + deadlines to goals
      ├── strategy_review → weekly deep review, store as memory, update domain confidence
      ├── track_mood → informational mood logging (real adaptation via LLM context)
      ├── encourage → celebrate completed goals and streaks, persona-calibrated
      └── no_action → log and skip
```

## Email System

### Configuration Flow

```
aivyx init
  │
  ├── Prompt: "Set up email? (y/N)"
  ├── Collect: address, IMAP host/port, SMTP host/port, username, password
  ├── Smart defaults: guess_imap_host("user@gmail.com") → "imap.gmail.com"
  │
  ├── Store password: EncryptedStore::put("EMAIL_PASSWORD", password, &master_key)
  └── Append to config.toml:
      [email]
      imap_host = "imap.gmail.com"
      imap_port = 993
      ...
```

```
main.rs startup
  │
  ├── PaConfig::load(config.toml) → parse [email] section
  └── resolve_email_config(&store, &master_key)
      ├── Read EMAIL_PASSWORD from EncryptedStore
      ├── Build EmailConfig { ..., password }
      └── None if password missing or email not configured
```

### IMAP Protocol Flow (ReadInbox)

```
1. TLS connect to imap_host:993
2. A001 LOGIN "username" "password"
3. A002 SELECT INBOX
4. A003 SEARCH UNSEEN          (or SEARCH ALL if unread_only=false)
5. Parse sequence numbers from "* SEARCH 1 2 3 ..."
6. For last N messages (reversed):
   F00x FETCH seq (BODY[HEADER.FIELDS (FROM SUBJECT)] BODY[TEXT])
   → Parse From, Subject, body preview (first 200 chars)
7. A099 LOGOUT
```

**Timeout protection:** All IMAP tag reads are wrapped in `tokio::time::timeout(30s)`. If the server stops responding mid-conversation, the connection fails with a clear error rather than hanging indefinitely.

### SMTP Flow (SendEmail)

Uses `lettre` with STARTTLS:
```
1. Connect to smtp_host:587
2. STARTTLS upgrade
3. Authenticate with username + password
4. Build message: From (Aivyx Assistant <address>), To, Subject, Body
5. Send via AsyncSmtpTransport
```

## Knowledge Graph

Conversations automatically extract `(subject, predicate, object)` triples during heartbeat consolidation. When the heartbeat LLM processes accumulated conversation history, it identifies factual relationships and stores them as triples in the encrypted memory store alongside episodic memories (using the same memory domain key).

**Query tools** (registered with memory tools when embedding provider is available):

| Tool | Description |
|---|---|
| `kg_traverse` | Walk the graph from a starting entity, following edges up to N hops |
| `kg_paths` | Find shortest paths between two entities |
| `kg_search` | Full-text search across entity names and predicates |
| `kg_stats` | Summary statistics: entity count, edge count, most-connected nodes |

During heartbeat context gathering, the knowledge graph is queried to surface cross-domain connections — for example, linking a contact mentioned in an email to a goal that references the same project. This enables cross-domain reasoning that pure episodic memory search would miss.

## Developer Tools Integration

Local git operations and remote forge integration for software development workflows. Git operations use `git2` (libgit2 bindings) where possible, falling back to command invocation for complex operations.

**Configuration** (`config.toml`):
```toml
[devtools]
forge_type = "github"          # "github" | "gitea"
api_url = "https://api.github.com"
token = "keystore:DEVTOOLS_TOKEN"   # resolved from EncryptedStore
```

**Git tools** (4, always registered when devtools configured):

| Tool | Description |
|---|---|
| `git_status` | Show working tree status (staged, unstaged, untracked) |
| `git_diff` | Diff working tree or between refs |
| `git_log` | Commit history with optional path filter |
| `git_commit` | Stage and commit changes with message |

**Forge tools** (8, conditional on devtools config):

| Tool | Description |
|---|---|
| `forge_list_issues` | List issues with label/state filters |
| `forge_get_issue` | Get issue details and comments |
| `forge_create_issue` | Create issue with title, body, labels |
| `forge_list_prs` | List pull requests with state filter |
| `forge_get_pr` | Get PR details, diff stats, review status |
| `forge_create_pr` | Create PR from branch with title and body |
| `forge_ci_status` | Get CI/CD pipeline status for a ref |
| `forge_merge_pr` | Merge a pull request (requires approval) |

Forge tools follow the same graceful degradation pattern: if the token is missing or the API is unreachable, the tools return clear errors without crashing.

## Messaging Integrations

Four messaging platforms are supported, each independently configured and optional:

| Platform | Protocol | Tools | Notes |
|---|---|---|---|
| Telegram | Bot API (HTTPS polling) | `send_telegram`, `read_telegram` | Requires bot token from BotFather |
| Matrix | Client-Server API (CS API) | `send_matrix`, `read_matrix` | Homeserver URL + access token |
| Signal | signal-cli JSON-RPC | `send_signal`, `read_signal` | Requires local signal-cli daemon |
| SMS | Twilio/Vonage HTTP API | `send_sms` | Send-only, no read capability |

Each platform is registered independently via its own `register_*_actions()` call during `build_agent()`. The send tools accept a recipient and message body; read tools return recent messages with sender and timestamp. All credentials are stored in the EncryptedStore under platform-specific keys.

SMS is intentionally send-only — inbound SMS would require a webhook endpoint, which conflicts with the local-first architecture.

## Plugin System

Plugins extend the assistant's capabilities through the Model Context Protocol (MCP). The plugin system handles discovery, lifecycle management, and authentication for external tool servers.

**Key features:**
- MCP-based plugin discovery — plugins expose tool schemas via the standard MCP handshake
- OAuth2 support for authenticated plugins (token refresh handled automatically)
- Plugin state persisted in EncryptedStore (installed plugins survive restarts)

**Management tools** (6, always registered):

| Tool | Description |
|---|---|
| `plugin_list` | List installed plugins and their status |
| `plugin_install` | Install a plugin from URL or local path |
| `plugin_remove` | Uninstall a plugin and clean up state |
| `plugin_enable` | Enable a disabled plugin |
| `plugin_disable` | Disable a plugin without uninstalling |
| `plugin_info` | Show plugin details, exposed tools, auth status |

Plugin-provided tools are merged into the main ToolRegistry and subject to the same autonomy and capability scope rules as built-in tools.

## TUI Event Loop

```
loop {
    // 1. Render frame (ratatui, ~60fps for smooth animations)
    terminal.draw(|frame| render(&app, frame))

    // 2. Tick animation counter
    app.frame_count += 1

    // 3. Drain streaming tokens from agent turn
    poll_chat_tokens():
      while token_rx.try_recv() → append to last assistant message
      "[[DONE]]" sentinel → set streaming = false

    // 4. Periodic data refresh (every 2 seconds)
    if last_refresh > 2s → refresh_data().await:
      - Agent: total_input_tokens, total_output_tokens, cost, conversation length
      - Goals, approvals, audit, notifications, memories, settings snapshot

    // 5. Poll for keyboard input (16ms timeout for 60fps)
    if event::poll(16ms) → handle_key(app, key)
      - Modal popup intercepts: Chat > Settings > Goals (before global Esc)
      - Chat content focus: Ctrl+S/P/E shortcuts, then char input
      - Global: Tab, Left/Right, number keys, filter cycling, goal CRUD keys
}
```

### Views

| View | Shortcut | Content |
|---|---|---|
| Home | `1` | Dashboard with stat cards, telemetry sidebar |
| Chat | `2` / `c` | Scrollable chat, streaming, context bar (tokens/cost), session switching (Ctrl+S), prompt preview (Ctrl+P), export (Ctrl+E) |
| Activity | `3` | Notification timeline with detail panel, filter tabs (All/Schedule/Heartbeat) |
| Goals | `4` | Goal list with CRUD: create (`n`), edit (`e`), complete (`c`), abandon (`x`), detail panel with sub-goals/deadline/failures |
| Approvals | `5` | Pending Leash-tier approval requests (a/d) |
| Missions | `6` | Running/completed multi-step missions |
| Audit | `7` | HMAC-chained audit log with filter/time cycling, chain verification |
| Memory | `8` | Browse, search, delete stored memories with kind badges |
| Settings | `9` | Fully interactive: 8 cards with text input, multi-line editor, skill manager, integration setup, schedule toggle, persona sliders |

### Streaming Architecture

```
User types message → Enter → send_chat_message()
  │
  ├── Push user ChatMessage to chat_messages
  ├── Push empty assistant ChatMessage
  ├── Create mpsc channel (token_tx, token_rx)
  │
  └── tokio::spawn background task:
      │
      ├── agent.lock().await
      ├── tokio::time::timeout(300s, agent.turn_stream())
      │     │
      │     ├── LLM streaming response → token_tx.send(chunk)
      │     └── Tool calls executed inline
      │
      ├── Ok(Ok(_)) → send "[[DONE]]"
      ├── Ok(Err(e)) → send "⚠ Error: {e}" + "[[DONE]]"
      └── Err(timeout) → send "⚠ Response timed out" + "[[DONE]]"
```

The TUI drains `token_rx` non-blockingly each frame (16ms poll), appending tokens to the last assistant message. The `streaming` flag prevents user input during generation. On timeout or error, partial response is preserved with a visible error indicator.

## Graceful Degradation Model

Every optional subsystem follows the same pattern:

```rust
let subsystem = match wire_subsystem(...) {
    Ok(s) => Some(s),
    Err(e) => {
        tracing::warn!("Subsystem unavailable: {e}");
        None
    }
};
```

| Subsystem | Fails When | Effect |
|---|---|---|
| Memory | No embedding provider configured | No semantic recall, but chat still works |
| Brain | BrainStore can't open (filesystem issue) | No goals or self-model, but chat still works |
| Email | Not configured or password missing | No email tools registered, loop skips email checks, triage disabled |
| Calendar | Not configured or CalDAV unreachable | No calendar tools, heartbeat skips calendar context |
| Contacts | Not configured or CardDAV unreachable | No contact tools (sync unavailable), search/list still work on local store |
| Documents | Vault not configured | No document tools, no semantic document search |
| Finance | Not configured | No finance tools, heartbeat skips finance context |
| Triage | Not enabled or email unavailable | No triage tools, no autonomous inbox processing |
| Telegram | Not configured or bot token missing | No Telegram tools, no notification forwarding to Telegram |
| Matrix | Not configured or access token missing | No Matrix tools, no notification forwarding to Matrix |
| Missions | AgentSession construction fails | Mission tools still registered but will return errors on use |
| Loop context | Brain unavailable | Loop runs but only does morning briefing checks |

| Resilience | Not configured | No circuit breaker or fallback; single provider failure = error |
| MCP pool | No servers connected | No MCP tools, no prompts discovery |

The assistant always starts. The worst case is a basic chatbot with file/shell/web/reminder/mission tools.

## Provider Resilience

When `[resilience]` and/or `[routing]` are configured, both the agent and loop providers are wrapped in a decorator chain:

```
Box<dyn LlmProvider>
  │
  └── ResilientProvider (circuit breaker + fallback)
      ├── Primary provider (from [provider] config)
      ├── CircuitBreaker: opens after N failures, half-open after recovery timeout
      ├── Fallback chain: tries each [providers.*] entry in order
      └── ProviderEvent tracing: state transitions logged
          │
          └── CachingProvider (optional, if cache_enabled)
          │   ├── L1: SHA-256 prompt hash → exact match (O(1))
          │   └── L2: semantic embedding similarity (O(n), if embedding configured)
          │       └── Scoped by system prompt hash (different agents don't share cache)
          │
          └── RoutingProvider (optional, if [routing] enabled)
              ├── Heuristic complexity classifier (Simple/Medium/Complex)
              ├── Per-tier providers from [providers.*] table
              └── Falls back to default provider for unmatched tiers
```

Agent and loop get independent provider instances with independent circuit breaker state. The wrapping happens in `main.rs` BEFORE `master_key` moves into `build_agent()`.

## Learning & Safety (Phase 3C)

### Outcome Tracking

The Activity view lets users rate agent suggestions:
- `y` = Useful (1.0), `p` = Partial (0.5), `n` = Not useful (0.0)
- Ratings are stored via `MemoryManager::record_outcome()` as `OutcomeRecord`
- Heartbeat uses outcome data to adjust future suggestion relevance

### Failure Analysis

When `can_analyze_failures = true` in heartbeat config, the heartbeat LLM can emit `AnalyzeFailure` actions:
- Stores a reflection memory (root cause + remediation)
- Decreases `domain_confidence` for the relevant domain by 0.1
- Adds weakness to the agent's self-model
- Sends notification to user

### Style Adaptation

Two mechanisms work together:
1. **Explicit config** (`[style]` section) — tone, detail_level, active_hours, free-text preferences
2. **LLM-extracted** — profile extractor learns from conversations, adds to `UserProfile.style_preferences`

Config preferences are seeded into the UserProfile at startup via `try_lock()`. Both are injected into the system prompt as `[USER PROFILE]` blocks.

### Undo System (`aivyx-actions/src/undo.rs`)

Records reversible actions in EncryptedStore (domain key `"undo"`):

| Tool | Description |
|---|---|
| `record_undo` | Save recovery point before destructive action |
| `list_undo_history` | Show undoable actions (non-expired, non-undone) |
| `undo_action` | Reverse action by ID (restore file, cancel reminder, etc.) |

**Undo variants:**
- `RestoreFile` — writes original content back (fully automatic)
- `CancelReminder` — returns instruction to use `delete_reminder`
- `VoidTransaction` — returns instruction to use finance tools
- `ManualOnly` — human instructions (e.g., sent emails can't be recalled)

Records expire after a configurable TTL (default 24h).

## Deep Agent Infrastructure (Phase 5B)

### Abuse Detection

The `AbuseDetector` from `aivyx-audit` is wired into the agent via `agent.set_abuse_detector()`.
It monitors tool calls in a sliding window and fires alerts for three anomaly types:

| Alert Type | Trigger |
|---|---|
| `HighFrequency` | Calls exceed `max_calls_per_window` |
| `RepeatedDenials` | Denied calls exceed `max_denials_per_window` |
| `ScopeEscalation` | Unique tool scopes exceed `max_unique_tools_per_window` |

Configuration via `[abuse_detection]` with per-field defaults. The detector runs inline
during agent turns — no background thread needed.

### Model Routing

When `[routing]` is enabled, a `RoutingProvider` wraps the resilient provider chain:

```
User message → ComplexityClassifier (heuristic)
  │
  ├── Simple → cheap/fast provider (e.g., Haiku)
  ├── Medium → balanced provider (e.g., Sonnet)
  └── Complex → powerful provider (e.g., Opus)
```

Each tier maps to a named entry in the `[providers]` table. Unmatched tiers
fall back to the default provider. The classifier is a heuristic built into
`aivyx-llm` — no LLM call needed for routing.

### Retrieval Router

When `agent.use_retrieval_router` is `true`, memory recall uses intelligent
strategy selection instead of simple vector similarity:

```
User query → RetrievalRouter::route(query) → RetrievalStrategy
  │
  ├── Temporal  — "what did I say last week"
  ├── Graph     — "who is connected to Alice"
  ├── Keyword   — "find the API key note"
  ├── Multi     — combines multiple strategies
  └── Vector    — default semantic similarity
```

The router runs in the agent's recall path (`agent.rs` ~line 798). Results are
converted from `RetrievalResult` to `MemoryEntry` for downstream compatibility.

### Goal Cascading

`Brain::complete_goal()` recursively cascades completion to sub-goals:

```
complete_goal(parent)
  ├── mark parent Completed
  └── for each child (via get_sub_goals):
      └── if not Completed → complete_goal(child)
```

`Brain::goal_progress()` computes parent progress from children:
- Leaf goal: `1.0` if completed, else `goal.progress`
- Parent: `completed_children / total_children`

`Brain::set_sub_goal()` validates that the parent exists before inserting,
preventing orphaned sub-goal relationships.

### Heartbeat Backup

A new `HeartbeatAction::Backup` variant triggers data directory archiving:

```
Heartbeat LLM decides to backup
  → dispatch_backup()
    → tar czf ~/.aivyx/backups/pa_backup_YYYYMMDD_HHMMSS.tar.gz
    → prune_old_backups(retention_days)
    → emit Notification
```

Uses system `tar` (no Rust crate dependency). Retention pruning parses
timestamps from filenames — self-describing, no metadata DB needed.
Gated by `can_backup` flag in `HeartbeatConfig`.

### Key Rotation

The `KeyRotateTool` provides in-agent master key rotation:

```
Agent receives "rotate my encryption key" request
  → KeyRotateTool::execute({ new_passphrase, confirm: true })
    → EncryptedStore::re_encrypt_all(old_key, new_key) // two-phase atomic
    → MasterKey::encrypt_to_envelope(new_passphrase)   // write new envelope
    → Return { keys_migrated, errors }
```

Scoped to `Custom("admin")` — only granted explicitly in `build_agent()`.
The re-encryption is two-phase atomic: all entries are re-encrypted in memory
first, then committed in a single transaction.
