# Aivyx Personal Assistant

A private, local-first AI personal assistant that manages your life and business. One binary, one command, no cloud dependency.

```
$ aivyx
Good morning.

  Today
  - Your assistant is ready.

  Press 'c' to start chatting
```

## What It Does

Aivyx is an AI assistant that **actually does things** — not just answers questions:

- **Reads and sends email** via your own IMAP/SMTP accounts, with draft-reply-approve-send workflow
- **Manages calendar** — fetches events from CalDAV, checks conflicts, generates daily agenda
- **Manages contacts** — syncs from CardDAV, search and list from local encrypted store
- **Document vault** — indexes local files (markdown, text, PDF), semantic search via embeddings
- **Finance tracking** — transactions, budgets, bill tracking, receipt filing from email
- **Manages local files** — read, write, organize
- **Runs shell commands** with capability-gated security
- **Fetches web pages** and **searches the web** for research and monitoring
- **Sets reminders** with time-based triggers and persistence
- **Remembers context** across sessions via persistent episodic memory
- **Pursues goals** proactively — set a long-term objective and it works on it in the background
- **Runs multi-step missions** — complex tasks are decomposed, planned, and executed autonomously with encrypted checkpoints
- **Learns from experience** — maintains a self-model of what works and what doesn't, with three feedback loops connecting missions, goals, and self-assessment
- **Sends and reads messages** on Telegram, Matrix, Signal, and SMS (Twilio/Vonage), with notification forwarding for urgent items
- **Knowledge graph** — automatic triple extraction from conversations, with graph traversal, path finding, and semantic search
- **Dev tools** — local git operations (log, diff, status, branches), GitHub/Gitea integration for issues, pull requests, and CI/CD pipelines
- **Plugin system** — MCP-based plugin discovery, installation, and management with OAuth2 support
- **Multi-agent delegation** — complex tasks can be delegated to the Nonagon specialist team via `team_delegate`
- **Autonomous email triage** — rule-based fast path + LLM classification, auto-reply, forwarding, with full audit logging
- **Context fusion** — cross-source reasoning correlates calendar, email, contacts, finance, and reminders to surface proactive insights
- **Priority scoring** — heuristic urgency ranking (0.0–1.0) pre-filters context before LLM reasoning
- **Heartbeat** — periodic LLM-driven introspection that gathers context from all sources, reasons about what to do, and takes autonomous action (with context-aware skip — zero token cost when nothing changed)
- **Scoped autonomy** — per-domain trust levels (trust email reads, require approval for shell, lock finance) with confidence-based escalation
- **Workflow engine** — reusable multi-step workflow templates with cron/email/goal triggers, conditional branching, and parameterized instantiation
- **Undo system** — record and reverse destructive actions (restore files, cancel reminders, void transactions) within a 24h window
- **Style adaptation** — learns communication preferences from conversations and adapts tone, detail level, and timing
- **Failure analysis** — autonomous post-mortem when missions fail, updating self-model and domain confidence
- **Outcome tracking** — rate agent suggestions in the Activity view; ratings feed back into future behavior
- **Audit dashboard** — TUI view showing autonomous actions with HMAC chain verification, event filtering, and time-range cycling
- **Abuse detection** — sliding-window anomaly monitoring detects high-frequency calls, repeated denials, and scope escalation attempts
- **Model routing** — complexity-based routing selects cost-appropriate LLM (simple/medium/complex tiers) per request
- **Intelligent retrieval** — `RetrievalRouter` selects optimal memory recall strategy (temporal, graph, keyword, multi, vector) per query
- **Goal cascading** — completing a parent goal automatically cascades to sub-goals; progress rolls up from children to parents
- **Memory curation** — dedicated TUI view to browse, search, and delete stored memories with kind badges and access counts
- **Automated backups** — heartbeat-driven tar.gz archiving of the data directory with configurable destination and retention pruning
- **Key rotation** — rotate the master encryption key with two-phase atomic re-encryption of all stored data
- **Agent soul** — deep identity layer (core drives, self-awareness, relationship model) makes the agent a persistent evolving presence, not a stateless chatbot
- **First-launch onboarding** — on first boot, the agent introduces itself with a persona-specific welcome message and discovery questions to start learning about you
- **Session management** — switch between saved conversation sessions, create new ones, delete old ones from the TUI
- **Chat export** — export any conversation as a timestamped markdown file
- **Full settings editing** — all 8 settings cards interactive: edit model, tier, agent name, soul, skills, persona dimensions, integrations, and schedule toggles — all from the TUI
- **Goal CRUD** — create, edit, complete, and abandon goals with confirmation dialogs from the Goals view
- **Schedule visibility** — see next-fire times for all scheduled tasks, filter Activity by schedule/heartbeat source, pause/resume individual schedules
- **Self-model seeding** — each persona starts with calibrated domain confidence, strengths, and weaknesses; the agent knows what it's good at from day one
- **Growth goals** — 5 self-development goals per persona (3 universal + 2 persona-specific) that the agent pursues autonomously to improve over time

All data is encrypted locally with ChaCha20-Poly1305. Your conversations, memory, goals, and credentials never leave your machine.

## Quick Start

### Prerequisites

- Rust stable toolchain (2024 edition)
- An LLM provider: [Ollama](https://ollama.ai) (local), OpenAI, Anthropic, or OpenRouter
- [aivyx-core](https://github.com/AivyxDev/aivyx-core) checked out at `../aivyx-core`
- [aivyx](https://github.com/AivyxDev/aivyx) monorepo checked out at `../aivyx` (for task-engine)

### Install

```sh
git clone https://github.com/AivyxDev/aivyx-pa.git
cd aivyx-pa
cargo build --release
# Binary is at target/release/aivyx
```

### First-Time Setup

Just launch the assistant — the setup wizard (Genesis) runs automatically on first launch:

```sh
aivyx
```

The wizard walks you through 10 steps:

1. **Set a passphrase** — encrypts all local data (Argon2id KDF + ChaCha20-Poly1305, 8-char minimum)
2. **Choose provider + model** — Ollama (auto-detected), OpenAI, Anthropic, or OpenRouter
3. **Name + persona** — name your assistant and choose from 8 personality presets
4. **Soul** — accept, edit, or skip the agent's personality narrative
5. **Skills** — accept persona defaults + add custom skills
6. **Schedule** — configure briefing hour and check interval
7. **Goals** — persona-specific starter goals (accept/skip)
8. **Email setup** (optional) — IMAP + SMTP so the assistant can read and send on your behalf
9. **Integrations** (optional) — Calendar (CalDAV), Contacts (CardDAV), Telegram, Matrix, Signal
10. **Intelligence** — toggle 10 heartbeat features (reflection, planning, mood tracking, etc.)

You can also run setup explicitly with `aivyx init`.

### Usage

```sh
aivyx              # Launch the interactive TUI (default)
aivyx chat "..."   # One-shot chat from the terminal
aivyx status       # Show recent assistant activity
aivyx config       # View current configuration
```

**TUI keyboard shortcuts:**

| Key | Action |
|---|---|
| `Tab` | Toggle focus between Sidebar and Content area |
| `Left` | Focus sidebar |
| `Right` | Focus content area |
| `1`–`9` | Jump to view: Home, Chat, Activity, Goals, Approvals, Missions, Audit, Memory, Settings |
| `Up/Down` or `j/k` | Navigate sidebar items (sidebar focus) or select items in content (content focus) |
| `Enter` | Open selected view (sidebar) or activate item (content) |
| `[`/`]` | Cycle filter (Goals: all/active/completed/abandoned; Audit: all/tool/heartbeat/security; Activity: all/schedule/heartbeat) |
| `a`/`d` | Approve/Deny selected approval (Approvals view, content focus) |
| `Esc` | Return to sidebar from content; return to Home from sidebar |
| `q` | Quit (except in Chat view) |
| `Ctrl+C` | Quit (always) |

**Goals-specific (when Goals view is focused):**

| Key | Action |
|---|---|
| `n` | Create new goal |
| `e` | Edit selected goal |
| `c` | Complete selected goal (with confirmation) |
| `x` | Abandon selected goal (with confirmation) |

**Chat-specific (when Chat view is focused):**

| Key | Action |
|---|---|
| Type | Text input for chat message |
| `Enter` | Send message |
| `Up/Down` | Scroll conversation history |
| `PageUp/PageDown` | Fast scroll |
| `Ctrl+S` | Open session switcher (list/create/delete sessions) |
| `Ctrl+P` | Preview composed system prompt (scrollable) |
| `Ctrl+E` | Export conversation as markdown file |
| `Esc` | Return to sidebar |

### Environment Variables

| Variable | Purpose |
|---|---|
| `AIVYX_PASSPHRASE` | Skip interactive passphrase prompt (for scripts/CI) |
| `RUST_LOG` | Logging level (e.g., `info`, `debug`, `aivyx_loop=trace`) |

---

## Architecture

### Overview

```
┌──────────────────────────────────────────────────────────────────┐
│  ┌─────────────────┐   ┌────────────────┐                       │
│  │  aivyx-pa (lib)  │   │  aivyx-tui     │                       │
│  │  agent.rs  api.rs│◄──│  (9 views,     │                       │
│  │  config.rs       │   │   widgets,     │                       │
│  │  runtime.rs      │   │   theme)       │                       │
│  │  sessions.rs     │   └────────────────┘                       │
│  └────────┬─────────┘                                            │
│           │ builds Agent + starts Loop                           │
│   ┌───────┼────────────┬────────────┐                            │
│   ▼       ▼            ▼            ▼                            │
│ ┌────────────┐ ┌────────┐ ┌──────────┐ ┌─────────────┐          │
│ │aivyx-actions│ │aivyx-  │ │aivyx-core│ │aivyx-task-  │          │
│ │ (28 mods)  │ │ loop   │ │(10 crates)│ │  engine     │          │
│ │            │ │(8 mods) │ │          │ │             │          │
│ └────────────┘ └────────┘ └──────────┘ └─────────────┘          │
└──────────────────────────────────────────────────────────────────┘
```

**Single process.** The assistant runs as one binary embedding:
- A **TUI** in the foreground (ratatui + crossterm, separate `aivyx-tui` crate)
- A **background agent loop** (tokio task) that checks sources and evaluates goals
- The **agent** with LLM provider, tool registry, memory, brain, and soul

### Four Cognitive Layers

```
  ┌─────────────────────────────────────┐
  │  Missions  (aivyx-task-engine)      │  Multi-step task decomposition
  │  Background execution + checkpoints │  4 tools: create/list/status/control
  ├─────────────────────────────────────┤
  │  Brain  (aivyx-brain)               │  Goals (cascading), self-model, working memory
  │  Persistent across sessions         │  5 tools: set/list/update goals, reflect, self-model
  ├─────────────────────────────────────┤
  │  Memory  (aivyx-memory)             │  Episodic memory, knowledge triples
  │  Semantic search + retrieval router │  6 tools: store/recall/search/forget/...
  ├─────────────────────────────────────┤
  │  Actions  (aivyx-actions)           │  Real-world integrations
  │  Files, email, calendar, contacts,  │  Up to 63 action tools
  │  docs, finance, shell, web, triage, │  (varies by configuration)
  │  workflows, undo, messaging, git,   │
  │  devtools, knowledge, plugins,      │
  │  key rotation                       │
  └─────────────────────────────────────┘
```

Up to **~85 tools** when all integrations are configured:
- 9 default action tools (files, shell, web, reminders)
- 3 email tools (read_inbox, fetch_email, send_email)
- 3 calendar tools (today_agenda, fetch_events, check_conflicts)
- 3 contact tools (search_contacts, list_contacts, sync_contacts)
- 4 document tools (search_documents, read_document, list_vault_documents, index_vault)
- 6 finance tools (add_transaction, list_transactions, budget_summary, set_budget, mark_bill_paid, file_receipt)
- 2 triage tools (list_triage_log, set_triage_rule)
- 2 Telegram tools (send_telegram, read_telegram)
- 2 Matrix tools (send_matrix, read_matrix)
- 2 Signal tools (send_signal, read_signal)
- 1 SMS tool (send_sms)
- 4 knowledge graph tools (traverse_knowledge, find_knowledge_paths, search_knowledge, knowledge_graph_stats)
- 4 git tools (git_log, git_diff, git_status, git_branches)
- 8 forge tools (ci_status, ci_logs, list_issues, get_issue, create_issue, list_prs, get_pr_diff, create_pr_comment)
- 6 workflow tools (create_workflow, list_workflows, run_workflow, workflow_status, delete_workflow, install_library)
- 3 undo tools (record_undo, list_undo_history, undo_action)
- 6 plugin tools (list_plugins, enable_plugin, disable_plugin, install_plugin, uninstall_plugin, search_plugins)
- 6 memory tools (from aivyx-memory)
- 5 brain tools (from aivyx-brain + PA-local self-model)
- 5 mission tools (from aivyx-task-engine + PA-local recipe tool)
- 1 MCP tool (mcp_list_prompts)
- 1 pattern mining tool (list_discovered_patterns)
- 1 team tool (team_delegate)
- 1 security tool (key_rotate)

### Four Execution Contexts

| Context | Driver | What Happens |
|---|---|---|
| **Chat turns** | User types a message | Agent processes message, calls tools, responds with streaming |
| **Background loop** | Timer (every 15 min default) | Evaluates active goals, checks email, monitors web pages |
| **Heartbeat** | Timer (every 30 min default) | LLM-driven introspection — gathers context from all sources, priority scoring, cross-source reasoning, proactive suggestions |
| **Email triage** | Each loop tick (if enabled) | Autonomous inbox processing — rule-based fast path + LLM classification, auto-reply, forwarding |
| **Workflow triggers** | Each loop tick | Evaluate cron/email/goal triggers, instantiate workflow templates, create missions |
| **Missions** | Agent-initiated | Multi-step tasks decomposed by LLM, executed autonomously with encrypted checkpoints |
| **Brain persistence** | Cross-session | Goals survive restarts; self-model records outcomes and patterns |

### Workspace Structure

```
aivyx-pa/
├── Cargo.toml              # Workspace root — 4 member crates
├── PLAN.md                 # Product plan and milestones
├── ROADMAP.md              # Phase-based roadmap with completion tracking
├── README.md               # This file
├── rust-toolchain.toml     # Rust stable, 2024 edition
└── crates/
    ├── aivyx-pa/           # Library + binary crate — agent core, API server, CLI
    │   ├── src/
    │   │   ├── main.rs     # CLI parsing, passphrase unlock, dispatch
    │   │   ├── lib.rs      # Library root (re-exports)
    │   │   ├── agent.rs    # Agent construction, tool registration, brain seeding, self-model seeding
    │   │   ├── config.rs   # PA-specific config, system prompts, PA_SOUL_CORE, onboarding messages
    │   │   ├── api.rs      # HTTP API server (axum) — chat SSE, goals, audit, approvals, settings
    │   │   ├── runtime.rs  # Shared key derivation, loop context, loop config building
    │   │   ├── sessions.rs # Conversation session persistence (encrypted)
    │   │   ├── settings.rs # Settings snapshot + config writers (toggle, string, number, array, multiline, schedule, integration)
    │   │   ├── init.rs     # First-time setup wizard (5 steps)
    │   │   ├── oauth.rs    # OAuth2 flow for MCP plugins (PKCE, CSRF, encrypted token storage)
    │   │   └── webhook.rs  # Localhost webhook server for inbound event triggers
    │   └── tests/
    │       └── agent_integration.rs  # Integration tests (MockProvider, brain seeding, tool wiring)
    │
    ├── aivyx-tui/          # Terminal UI binary crate (ratatui + crossterm)
    │   └── src/
    │       ├── main.rs     # TUI entry point, event loop, focus-based key handler, modal intercepts
    │       ├── app.rs      # App state, View/Focus enums, popup state machines (Settings/Goal/Chat),
    │       │               # data refresh, chat streaming with timeout, session/export methods
    │       ├── theme.rs    # Stitch design system colors and styles
    │       ├── views/      # 10 view renderers (9 main + genesis wizard)
    │       │   ├── home.rs, chat.rs, activity.rs, goals.rs, approvals.rs
    │       │   ├── missions.rs, audit.rs, memory.rs, settings.rs
    │       │   ├── genesis.rs  # First-launch setup wizard (10 steps)
    │       │   └── mod.rs
    │       └── widgets/    # Reusable TUI widgets
    │           ├── sidebar.rs, header.rs, telemetry.rs
    │           └── mod.rs
    │
    ├── aivyx-actions/      # Real-world integrations (28 modules)
    │   └── src/
    │       ├── lib.rs      # Action trait, ActionRegistry, shared resolve_url utility
    │       ├── bridge.rs   # Action → Tool adapter, registration functions
    │       ├── email.rs    # IMAP inbox reader + SMTP sender (TLS), fetch_email, reply threading
    │       ├── files.rs    # ReadFile, WriteFile, ListDirectory
    │       ├── shell.rs    # RunCommand (capability-gated)
    │       ├── web.rs      # FetchPage, SearchWeb (DuckDuckGo)
    │       ├── reminders.rs # SetReminder, ListReminders, DismissReminder (encrypted persistence)
    │       ├── calendar.rs # TodayAgenda, FetchCalendarEvents, CheckConflicts (CalDAV)
    │       ├── contacts.rs # SearchContacts, ListContacts, SyncContacts (CardDAV)
    │       ├── documents.rs # SearchDocuments, ReadDocument, ListVaultDocuments, IndexVault
    │       ├── finance.rs  # AddTransaction, ListTransactions, BudgetSummary, SetBudget, MarkBillPaid, FileReceipt
    │       ├── knowledge.rs # Knowledge graph tools (traverse, paths, search, stats)
    │       ├── triage_tools.rs # ListTriageLog, SetTriageRule (user-facing triage management)
    │       ├── plugin.rs   # Plugin system state management (list, enable, install, search)
    │       ├── undo.rs     # UndoRecord, UndoAction, 3 tools (record, list, reverse)
    │       ├── retry.rs    # Retry helper for transient failures
    │       ├── messaging/  # Multi-platform messaging integrations
    │       │   ├── mod.rs  # Shared config types and Message struct
    │       │   ├── telegram.rs # Telegram Bot API (send_telegram, read_telegram)
    │       │   ├── matrix.rs   # Matrix CS API (send_matrix, read_matrix)
    │       │   ├── signal.rs   # Signal via signal-cli JSON-RPC (send_signal, read_signal)
    │       │   └── sms.rs      # SMS via Twilio/Vonage gateway (send_sms)
    │       ├── devtools/   # Developer tool integrations
    │       │   ├── mod.rs  # Dev tools module orchestration
    │       │   ├── git.rs  # Local git operations (log, diff, status, branches)
    │       │   ├── pr.rs   # Pull request tools (list, diff, comment)
    │       │   ├── issues.rs # Issue tracking (list, read, create)
    │       │   └── ci.rs   # CI/CD tools (pipeline status, log retrieval)
    │       └── workflow/   # Workflow template engine
    │           ├── mod.rs  # Templates, instantiation, triggers, conditions, 6 CRUD tools
    │           └── library.rs # Pre-built workflow templates
    │
    └── aivyx-loop/         # Proactive background agent loop (8 modules)
        └── src/
            ├── lib.rs      # AgentLoop, goal evaluation, triage orchestration, trigger evaluation
            ├── heartbeat.rs # LLM-driven autonomous reasoning, context fusion, failure analysis
            ├── briefing.rs # Morning briefing aggregator (email, calendar, reminders, bills, goals)
            ├── triage.rs   # Autonomous email triage engine (rules + LLM classification)
            ├── trigger.rs  # Workflow trigger engine (cron, email, goal, cooldown dedup)
            ├── priority.rs # Heuristic urgency scoring (0.0–1.0), priority ranking
            ├── schedule.rs # Briefing time detection
            └── sources.rs  # Reserved for future trait-based source implementations
```

### Foundation: aivyx-core (10 Crates)

The PA consumes 10 crates from [aivyx-core](https://github.com/AivyxDev/aivyx-core):

| Crate | Purpose |
|---|---|
| `aivyx-core` | Base types, error enum (22 variants), newtype IDs (AgentId, ToolId, etc.) |
| `aivyx-crypto` | Encrypted storage (redb), Argon2id KDF, ChaCha20-Poly1305, HKDF key derivation |
| `aivyx-config` | Configuration parsing (TOML), provider/autonomy/embedding settings |
| `aivyx-audit` | HMAC-SHA256 chained audit log for trust and accountability |
| `aivyx-capability` | Permission model with four autonomy tiers (Locked/Leash/Trust/Free) |
| `aivyx-llm` | LLM provider abstraction (Ollama, OpenAI, Anthropic, OpenRouter) |
| `aivyx-mcp` | MCP (Model Context Protocol) tool execution |
| `aivyx-memory` | Episodic memory + knowledge triples with embedding-based semantic search |
| `aivyx-agent` | Agent identity, persona, tool registry, turn execution, streaming |
| `aivyx-brain` | Goals (hierarchical, with cooldown/progress), self-model, working memory |

### From aivyx Monorepo

| Crate | Purpose |
|---|---|
| `aivyx-task-engine` | Multi-step mission orchestration with LLM planning, sequential execution, and encrypted checkpointing |

---

## Key Design Decisions

### Encryption Architecture

All local data is encrypted at rest:

```
Passphrase
    │
    ▼ (Argon2id)
Master Key
    │
    ├── HKDF("memory")       → Memory encryption key
    ├── HKDF("brain")        → Brain encryption key (goals, self-model)
    ├── HKDF("task")         → Task engine encryption key (missions, checkpoints)
    ├── HKDF("audit")        → Audit log HMAC key (chain integrity)
    ├── HKDF("reminders")    → Reminder persistence key
    ├── HKDF("contacts")     → Contact store key
    ├── HKDF("finance")      → Finance data key (transactions, budgets)
    ├── HKDF("triage")       → Triage log + rules key
    ├── HKDF("vault")        → Document vault encryption key
    ├── HKDF("conversation") → Conversation history key
    ├── HKDF("workflow")     → Workflow template storage key
    ├── HKDF("undo")         → Undo record storage key
    └── Direct               → EncryptedStore (API keys, email password)
```

- **One passphrase** unlocks everything — the master key is encrypted in an envelope file
- **Domain-separated keys** via HKDF ensure each subsystem has its own independent key
- **MasterKey is not Clone** — each consumer derives its own key, preventing accidental key reuse
- The task engine stores master key bytes in `Zeroizing<Vec<u8>>` and reconstructs on demand
- Credentials (API keys, email passwords) are stored in the `EncryptedStore` (redb), never in config files

### Brain ↔ Loop Architecture

The Brain (goals) is shared between two consumers:

```
                    Arc<BrainStore>
                    ┌──────┴──────┐
                    │             │
              ┌─────▼─────┐ ┌────▼────┐
              │   Agent    │ │  Loop   │
              │ (writes)   │ │ (reads) │
              └───────────┘ └─────────┘
              Brain tools     evaluate_goals()
              during chat     on timer tick
```

- The **Agent** writes goals via brain tools during chat (user says "keep an eye on my inbox")
- The **Loop** reads active goals each tick and executes corresponding checks
- Sharing is via `Arc<BrainStore>` — redb supports concurrent readers safely
- Each side gets its own HKDF-derived brain key (since `MasterKey` is not `Clone`)

### Goal → Action Classification

The loop classifies goals into automated actions using keyword heuristics:

| Goal Keywords | Action | What Happens |
|---|---|---|
| "email", "inbox", "mail" | `CheckEmail` | Fetch unread emails, filter by goal criteria |
| URL in description | `CheckWeb` | Fetch the page, emit preview as notification |
| "remind", "schedule", "every" | `CheckReminders` | Check for due reminders (TODO) |
| Everything else | `NoAction` | Evaluated during conversation only |

This is intentionally simple. Future versions will use LLM-based classification.

### Action → Tool Bridge

Actions and Tools are separate traits that serve different purposes:

```
aivyx-actions::Action          aivyx-core::Tool
  name()                         id()
  description()                  name()
  input_schema()                 description()
  execute(json) → json           input_schema()
                                 required_scope()
                                 execute(json) → json
```

`ActionTool` bridges the two — wrapping any `Action` as a `Tool` so the LLM can invoke it. This separation keeps the actions crate independent of the agent/tool infrastructure.

### Graceful Degradation

Every subsystem (Memory, Brain, Email, Calendar, Contacts, Telegram, Matrix, Signal, SMS, DevTools, Finance, Vault) is independently optional:

```rust
// Each returns None/Err on failure, agent still works
let memory_manager = match wire_memory(...) { Ok(m) => Some(m), Err(e) => { warn!(...); None } };
let (brain, store, first_launch) = match wire_brain(...) { Ok(b) => (Some(..), ..), Err(e) => { warn!(...); (None, None, false) } };
let email_config = pa_config.resolve_email_config(...); // None if not configured
```

The assistant always starts, even if memory, brain, or email are unavailable. Failed subsystems emit `tracing::warn` messages.

---

## Configuration

All configuration lives in `~/.aivyx/config.toml`, created by `aivyx init`.

### Provider Section (Required)

```toml
[provider]
type = "Ollama"                      # Ollama | OpenAI | Claude | OpenAICompatible
base_url = "http://localhost:11434"  # For Ollama / OpenRouter
model = "qwen3:14b"
# api_key_ref = "OPENAI_API_KEY"    # For cloud providers (key stored encrypted)
```

### Autonomy Section

```toml
[autonomy]
default_tier = "Trust"   # Locked | Leash | Trust | Free

# Per-scope overrides (optional)
# [[autonomy.scope_overrides]]
# scope = "shell"
# tier = "Leash"

# Confidence escalation (optional)
# escalation_confidence_threshold = 0.3
```

| Tier | Behavior |
|---|---|
| **Locked** | Read-only — cannot take any actions |
| **Leash** | All actions require explicit approval |
| **Trust** | Autonomous actions within defined boundaries |
| **Free** | Full autonomy (use with caution) |

Scope overrides let you set different tiers per tool domain. Confidence escalation automatically downgrades Trust/Free to Leash when the agent's domain confidence drops below the threshold.

### Email Section (Optional)

```toml
[email]
imap_host = "imap.gmail.com"
imap_port = 993
smtp_host = "smtp.gmail.com"
smtp_port = 587
address = "you@gmail.com"
username = "you@gmail.com"       # Defaults to address if omitted
# Password is NOT here — stored in encrypted keystore under EMAIL_PASSWORD
```

### Embedding Section (Optional)

```toml
[embedding]
type = "Ollama"                      # Same provider types as [provider]
model = "nomic-embed-text"
base_url = "http://localhost:11434"
```

Required for the memory system's semantic search. If not configured, memory operates without embedding-based recall.

---

## Data Directory

All data lives under `~/.aivyx/` with `0o700` permissions:

```
~/.aivyx/
├── config.toml         # Configuration
├── master_key.json     # Encrypted master key envelope (Argon2id)
├── store.redb          # Encrypted key-value store (API keys, passwords)
├── pa.log              # TUI log output (tracing, rotated)
├── memory/
│   └── assistant/
│       └── memory.db   # Episodic memory database (redb)
├── brain/
│   └── assistant.redb  # Goals, self-model, working memory
├── tasks/
│   └── tasks.db        # Mission state, checkpoints (redb)
├── audit/              # HMAC-chained audit log
└── backups/            # Heartbeat-driven tar.gz archives (if [backup] enabled)
```

---

## Tool Reference

### Default Action Tools (always available)

| Tool | Module | Description |
|---|---|---|
| `read_file` | files.rs | Read contents of a local file |
| `write_file` | files.rs | Write/overwrite a local file |
| `list_directory` | files.rs | List entries in a directory |
| `run_command` | shell.rs | Execute shell command (requires Trust tier, Shell capability) |
| `fetch_webpage` | web.rs | HTTP GET a URL, return text content (truncated to 32KB) |
| `search_web` | web.rs | Search the web via DuckDuckGo (no API key needed) |
| `set_reminder` | reminders.rs | Set a time-based reminder (ISO 8601, persisted) |
| `list_reminders` | reminders.rs | List all pending reminders |
| `dismiss_reminder` | reminders.rs | Dismiss a reminder by ID |

### Email Tools (if email configured)

| Tool | Description |
|---|---|
| `read_email` | Fetch recent inbox messages via IMAP (TLS), with unread-only filtering |
| `fetch_email` | Fetch full content of a specific email by IMAP sequence number |
| `send_email` | Send an email via SMTP (STARTTLS), with optional `in_reply_to` for threading |

### Calendar Tools (if CalDAV configured)

| Tool | Description |
|---|---|
| `today_agenda` | Fetch today's calendar events |
| `fetch_calendar_events` | Fetch events in a date range |
| `check_conflicts` | Check for scheduling conflicts in a time range |

### Contact Tools (if contacts configured)

| Tool | Description |
|---|---|
| `search_contacts` | Search contacts by name, email, or phone |
| `list_contacts` | List all stored contacts |
| `sync_contacts` | Sync contacts from CardDAV server (if CardDAV configured) |

### Document Tools (if vault configured)

| Tool | Description |
|---|---|
| `search_documents` | Semantic search across indexed vault documents |
| `read_document` | Read a specific document from the vault |
| `list_vault_documents` | List documents in the vault by extension filter |
| `index_vault` | Index/re-index the document vault for search |

### Finance Tools (if finance configured)

| Tool | Description |
|---|---|
| `add_transaction` | Record a financial transaction |
| `list_transactions` | List transactions with date/category filtering |
| `budget_summary` | Show budget vs. actual spending by category |
| `set_budget` | Set or update a monthly budget for a category |
| `mark_bill_paid` | Mark a recurring bill as paid for the current period |
| `file_receipt` | Extract receipt from email and file to vault (requires email + vault) |

### Triage Tools (if triage enabled)

| Tool | Description |
|---|---|
| `list_triage_log` | Show recent autonomous email triage activity |
| `set_triage_rule` | Add or update an auto-reply triage rule |

### Telegram Tools (if Telegram configured)

| Tool | Description |
|---|---|
| `send_telegram` | Send a message to a Telegram chat via Bot API |
| `read_telegram` | Read recent messages from a Telegram chat |

### Matrix Tools (if Matrix configured)

| Tool | Description |
|---|---|
| `send_matrix` | Send a message to a Matrix room via CS API |
| `read_matrix` | Read recent messages from a Matrix room |

### Signal Tools (if Signal configured)

| Tool | Description |
|---|---|
| `send_signal` | Send a message via Signal (requires signal-cli) |
| `read_signal` | Read recent Signal messages |

### SMS Tools (if SMS configured)

| Tool | Description |
|---|---|
| `send_sms` | Send an SMS via Twilio or Vonage gateway |

### Knowledge Graph Tools (always available)

| Tool | Description |
|---|---|
| `traverse_knowledge` | Traverse the knowledge graph from a starting entity |
| `find_knowledge_paths` | Find paths between two entities in the knowledge graph |
| `search_knowledge` | Semantic search across knowledge triples |
| `knowledge_graph_stats` | Show graph size, entity count, relationship distribution |

### Git Tools (always available)

| Tool | Description |
|---|---|
| `git_log` | Show recent commit history for a local repository |
| `git_diff` | Show changes in working directory or between commits |
| `git_status` | Show the current state of a git working tree |
| `git_branches` | List branches in a local repository |

### Forge Tools (if devtools configured — GitHub/Gitea)

| Tool | Description |
|---|---|
| `ci_status` | Check CI/CD pipeline status for a repository |
| `ci_logs` | Retrieve CI/CD pipeline logs |
| `list_issues` | List issues for a repository with status filtering |
| `get_issue` | Get details of a specific issue |
| `create_issue` | Create a new issue |
| `list_prs` | List pull requests for a repository |
| `get_pr_diff` | Get the diff of a specific pull request |
| `create_pr_comment` | Add a comment to a pull request |

### Workflow Tools (always available)

| Tool | Description |
|---|---|
| `create_workflow` | Create or update a reusable workflow template with steps, parameters, and triggers |
| `list_workflows` | List available workflow templates (optional detail mode) |
| `run_workflow` | Instantiate a template with parameters, producing concrete steps for mission creation |
| `workflow_status` | Inspect a template's full definition including conditions and triggers |
| `delete_workflow` | Remove a workflow template |
| `install_library` | Install a pre-built workflow template from the library |

### Undo Tools (always available)

| Tool | Description |
|---|---|
| `record_undo` | Save a recovery point before a destructive action (file overwrite, email send, etc.) |
| `list_undo_history` | Show recent undoable actions (non-expired, non-undone) |
| `undo_action` | Reverse a previous action by ID — restores files, cancels reminders, voids transactions |

### Memory Tools (from aivyx-memory)

| Tool | Description |
|---|---|
| `memory_store` | Save a fact or observation to long-term memory |
| `memory_recall` | Recall memories by semantic similarity |
| `memory_search` | Search memories by keyword |
| `memory_forget` | Remove a specific memory |
| `memory_list` | List all stored memories |
| `memory_count` | Count total memories |

### Brain Tools (from aivyx-brain + PA-local)

| Tool | Description |
|---|---|
| `brain_set_goal` | Create a new goal with description, success criteria, and optional parent |
| `brain_list_goals` | List goals filtered by status (Active/Completed/Abandoned) |
| `brain_update_goal` | Update goal status or progress notes |
| `brain_reflect` | Record a self-reflection about performance and outcomes |
| `brain_update_self_model` | Update strengths, weaknesses, domain confidence, and tool proficiency scores |

### Mission Tools (from aivyx-task-engine)

| Tool | Description |
|---|---|
| `mission_create` | Create and execute a multi-step mission — the LLM plans steps, runs them sequentially, and bridges outcomes back to brain goals + self-model |
| `mission_list` | List all missions with their current status |
| `mission_status` | Get detailed status of a specific mission including step progress |
| `mission_control` | Pause, resume, or cancel a running mission |

### Plugin Tools (always available)

| Tool | Description |
|---|---|
| `list_plugins` | List installed MCP plugins with status |
| `enable_plugin` | Enable a disabled plugin |
| `disable_plugin` | Disable a plugin without uninstalling |
| `install_plugin` | Install a new MCP plugin |
| `uninstall_plugin` | Remove an installed plugin |
| `search_plugins` | Search the plugin registry for available plugins |

### Team Tool (always available)

| Tool | Description |
|---|---|
| `team_delegate` | Delegate a complex task to the Nonagon specialist team for multi-perspective analysis |

---

## Background Agent Loop

The loop runs as a tokio task and executes a check cycle every 15 minutes (configurable).

### Tick Lifecycle

```
1. Check if it's morning briefing time (briefing_hour ± 5 min)
   → Emit Info notification if yes

2. Read all Active goals from BrainStore

3. For each goal (skipping those in cooldown):
   a. Classify: match_goal_to_action(goal)
   b. Execute:
      - CheckEmail → fetch inbox, filter by query, emit per-match notification
      - CheckWeb → fetch URL, emit preview notification
      - CheckReminders → check for related due reminders
      - NoAction → skip
   c. Record outcome → goal.record_success() or goal.record_failure()
      (exponential backoff, auto-abandon after 10 consecutive failures)

4. Run email triage (if enabled):
   a. Fetch unread emails since last cursor
   b. Fast path: rule-based matching (ignore, auto-reply) — zero LLM cost
   c. Slow path: batch LLM classification for unmatched emails
   d. Execute actions: forward urgent emails, send auto-replies
   e. Log all results to encrypted store, advance cursor

5. Run workflow trigger evaluation:
   a. Check all registered workflow triggers (cron, email, goal progress)
   b. Cooldown dedup — suppress re-fires within 300s
   c. When trigger fires → instantiate template → emit mission creation notification

6. Run heartbeat (if interval elapsed, default 30 min):
   a. Gather context from ALL sources (goals, reminders, email, calendar,
      contacts, finance, self-model, schedules)
   b. Priority scoring: rank items by temporal urgency (0.0–1.0)
   c. Context-aware skip: if nothing changed → skip LLM call (zero tokens)
   d. Build prompt with cross-source reasoning instructions
   e. LLM reasons about what actions to take
   f. Dispatch actions: notify, suggest, set/update goals, reflect, consolidate,
      analyze failures (post-mortem → self-model update)

7. Sleep until next interval or manual trigger
```

### Manual Trigger

Call `AgentLoop::trigger_check()` to run an immediate cycle (e.g., on app launch). Uses a dedicated `mpsc::UnboundedSender<()>` channel — not a fake notification.

### Lifecycle

```rust
let (agent_loop, notification_rx) = AgentLoop::start(config, context);

// Receive notifications in TUI event loop
while let Ok(notif) = notification_rx.try_recv() { /* ... */ }

// Stop on drop (or explicitly)
agent_loop.stop();
```

---

## Known Limitations / TODOs

- **Goal evaluation uses keyword matching** — future: LLM-based classification
- **Binary name collision** — the binary is named `aivyx`, same as the monorepo; may rename to `aivyx-pa` in future
- **FileChange trigger** — deferred; requires OS-level file watching (inotify/kqueue). Logs a warning when configured.
- **No startup health check** — the agent doesn't verify provider reachability or credential validity on boot (planned for Phase 8.6)
- **No in-TUI help overlay** — `?` keybind for view-specific help is planned but not yet implemented

---

## Development

### Build

```sh
cargo build                  # Debug build
cargo build --release        # Release build
```

### Run with logging

```sh
RUST_LOG=debug cargo run
RUST_LOG=aivyx_loop=trace,aivyx_loop::heartbeat=debug,aivyx_agent=debug cargo run
```

### Project Dependencies

The workspace depends on local checkouts of two sibling repositories. The `[patch]` sections in the root `Cargo.toml` redirect git dependencies to local paths:

```toml
# aivyx-core: 10 foundation crates
[patch."https://github.com/AivyxDev/aivyx-core.git"]
aivyx-core   = { path = "../aivyx-core/crates/aivyx-core" }
aivyx-crypto = { path = "../aivyx-core/crates/aivyx-crypto" }
# ... etc

# aivyx monorepo: task engine
[patch."https://github.com/AivyxDev/aivyx.git"]
aivyx-task-engine = { path = "../aivyx/crates/aivyx-task-engine" }
```

Ensure both `../aivyx-core` and `../aivyx` exist and are up to date before building.

### Logging

- **TUI mode**: logs route to `~/.aivyx/pa.log` via `tracing-appender` (non-blocking file writer) to avoid corrupting the terminal UI
- **CLI mode** (`chat`, `status`, `config`): logs route to stderr as usual
- Control log level with `RUST_LOG` (e.g., `RUST_LOG=debug`, `RUST_LOG=aivyx_task_engine=trace`)

---

## License

MIT
