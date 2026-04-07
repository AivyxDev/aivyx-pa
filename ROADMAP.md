# Aivyx PA — Feature Roadmap

**Date**: 2026-04-05
**Status**: Active — Phase 1-7 complete, Phase 8 in progress (8.2-8.5 complete, safety/polish pass done)
**Goal**: Evolve from a capable assistant into a competition-breaking autonomous agent

---

## Where We Are

**Foundation (complete through Phase 7, Phase 8 in progress):**
- 548 tests, 4 crates, ~41,400 lines of Rust
- up to ~85 tools (file, shell, web, email, reminders, calendar, contacts, documents, finance, triage, messaging, workflows, undo, memory, brain, missions, key rotation)
- Provider resilience (circuit breaker, fallback chain, response caching, complexity-based model routing)
- Configurable memory consolidation, pattern mining, and intelligent retrieval routing
- Recipe-based mission execution with experiment tracking
- MCP health monitoring and audit log pruning
- Proactive agent loop with heartbeat, morning briefings, cron schedules, workflow triggers, automated backups
- Self-learning feedback loops (goal tracking, self-model with tool proficiency, domain confidence, outcome rating, failure analysis)
- Goal cascading (parent→child completion, progress rollup, cycle detection)
- Abuse detection (sliding-window anomaly monitoring for tool misuse)
- Scoped autonomy with per-domain tier overrides and confidence-based escalation
- Workflow engine with templates, triggers, conditional branching
- Undo system for reversing destructive actions
- Style adaptation from config + LLM-extracted preferences
- Encrypted local-first storage, capability-gated security, key rotation
- Stitch-branded TUI with 9 views (including Memory curation, audit dashboard, and live Missions view), split into dedicated `aivyx-tui` crate, fully interactive settings (all 8 cards editable), goal CRUD, session switching, chat export, 75 TUI tests
- Agent soul — deep identity layer with core drives, self-awareness, and relationship model
- First-launch onboarding — persona-specific welcome, discovery questions, growth goal seeding
- Self-model seeding — persona-appropriate domain confidence, strengths/weaknesses from day one
- HTTP API server with chat SSE, goals, audit, approvals, settings, sessions, memory endpoints

**What makes an AI assistant into an agent:**
An assistant *responds*. An agent *initiates, plans, executes, learns, and adapts*.
We have the skeleton of agency (heartbeat, goals, missions). The roadmap below
fills in the muscle — turning passive tools into active workflows, adding real
autonomy with safety, and building the context awareness that makes the
difference between "useful tool" and "indispensable partner".

---

## Competitive Moat

What Aivyx PA does that others don't (or can't):

| Differentiator | Why It Matters |
|---|---|
| **Local-first, encrypted** | No cloud dependency, no data leakage — your life stays yours |
| **Self-model + learning** | Agent improves over time based on outcomes, not just prompt tuning |
| **Mission decomposition** | Complex multi-step tasks with checkpoint recovery |
| **Proactive heartbeat** | Zero-cost-when-idle autonomous reasoning — not just reactive |
| **Capability security** | Structural permission model, not just "trust the LLM" |
| **Provider agnostic** | Ollama, OpenAI, Anthropic, OpenRouter — swap freely |
| **Single binary** | No Docker, no cloud services, no signup — just `aivyx` |

---

## Phase 1: "Reliable Agent" (Hardening + Wiring)

**Theme**: Fix gaps, wire existing infrastructure, make everything robust.
These are low-effort, high-impact items that complete the M3 foundation.

### 1.1 Wire Pending Infrastructure
- [x] **Reminder persistence** — already complete (EncryptedStore wired in M3)
- [x] **Heartbeat memory consolidation** — MemoryManager wired to LoopContext, actual consolidation on heartbeat trigger (2026-04-02)
- [x] **Briefing item parsing** — structured BriefingItems from LLM response (2026-04-02)
- [x] **Schedule tool access** — give scheduled prompts access to registered tools (2026-04-02)

### 1.2 Robustness
- [x] **Conversation persistence** — save/resume chat across sessions (encrypted store) (2026-04-02)
- [x] **Graceful degradation** — offline mode when LLM provider is unreachable (2026-04-02)
- [x] **Token budget awareness** — auto-compress conversation at 80% context window (inherited from aivyx-agent core) (2026-04-02)
- [x] **Retry with backoff** — LLM calls, IMAP connections, web fetches (2026-04-02)
- [x] **Connection pooling** — `ImapPool` with TTL-based expiry, NOOP health checks, and transparent reconnect; shared across agent tools, triage, and scheduled prompts via `Arc<ImapPool>` *(2026-04-04)*

### 1.3 TUI Polish
- [x] **Chat scroll** — PageUp/PageDown through conversation history (2026-04-02)
- [x] **Input editing** — cursor movement (Left/Right/Home/End), word delete (Ctrl+W/Ctrl+U), Delete key (2026-04-02)
- [x] **Multi-line input** — Alt+Enter for newlines, Enter to send, dynamic input height (2026-04-02)
- [x] **Notification toast** — amber overlay top-right, auto-dismiss after 4s (2026-04-02)
- [x] **Status bar** — bottom bar showing: view, provider, turn count, time (2026-04-02)

---

## Phase 2: "Contextual Agent" (Deep Integration)

**Theme**: Give the agent real understanding of your world — calendar, contacts,
documents, finances. Each integration feeds the agent's context, making every
other feature smarter.

### 2.1 Calendar Integration
- [x] **CalDAV client** — read events via PROPFIND/REPORT (Google Calendar, Nextcloud, iCloud) *(2026-04-02)*
- [x] **Conflict detection** — `check_calendar_conflicts` tool + briefing integration *(2026-04-02)*
- [x] **Agenda tool** — `today_agenda` + `fetch_calendar_events` tools *(2026-04-02)*
- [x] **Calendar-aware briefings** — morning briefing includes today's events + conflicts *(2026-04-02)*
- [x] **Auto-reminders** — auto-creates reminders 15min before calendar events *(2026-04-02)*

### 2.2 Contact Management
- [x] **CardDAV client** — sync contacts via PROPFIND/REPORT + vCard parsing *(2026-04-02)*
- [x] **Contact resolution** — `search_contacts` fuzzy match by name/email/company *(2026-04-02)*
- [x] **Relationship context** — contacts stored in encrypted store, always available to agent *(2026-04-02)*
- [x] **Contact enrichment** — auto-creates contacts from email From headers *(2026-04-02)*

### 2.3 Document Intelligence
- [x] **Local vault** — index a configured directory of markdown/text/PDF files *(2026-04-02)*
- [x] **Semantic search** — find documents by meaning, not just filename *(2026-04-02)*
- [x] **Document summarization** — "summarize the Q1 report" *(2026-04-02, via agent reasoning + read_document)*
- [x] **Knowledge extraction** — auto-extract facts from documents into memory *(2026-04-02, via agent reasoning + memory tools)*
- [x] **Watch mode** — re-index when files change (periodic content-hash check on loop tick) *(2026-04-02)*

### 2.4 Finance Tracking
- [x] **Bill detection** — parse financial emails via keyword pre-filter + LLM extraction *(2026-04-03)*
- [x] **Expense categorization** — LLM-powered category assignment on add_transaction *(2026-04-03)*
- [x] **Budget awareness** — budget_summary tool + set_budget + over-budget briefing alerts *(2026-04-03)*
- [x] **Receipt filing** — file_receipt saves email body to vault receipts folder *(2026-04-03)*

### 2.5 Context Fusion
- [x] **Unified context window** — heartbeat sees calendar + email + goals + contacts + finance + reminders together *(2026-04-03)*
- [x] **Cross-source reasoning** — heartbeat prompt instructs LLM to correlate across sources; system prompt adds PA_PROMPT_CONTEXT_FUSION *(2026-04-03)*
- [x] **Priority scoring** — `priority.rs` module scores items by urgency (calendar proximity, email age, reminder overdue, budget alerts, goal staleness); ranked summary in heartbeat prompt *(2026-04-03)*
- [x] **Proactive suggestions** — new `Suggest` heartbeat action with sources + priority; dispatched as notifications tagged by contributing sources *(2026-04-03)*

---

## Phase 3: "Autonomous Agent" (Real Agency) — COMPLETE *(2026-04-03)*

**Theme**: Move from "executes when asked" to "acts independently within boundaries".
This is where capability-gated security becomes critical.

### 3.1 Workflow Engine ✅
- [x] **Workflow templates** — reusable multi-step workflows with parameterized steps, stored encrypted *(2026-04-03)*
- [x] **Trigger system** — cron, email pattern, goal progress triggers with cooldown dedup *(2026-04-03)*
- [x] **Conditional branching** — StepCondition tree: OnSuccess, OnFailure, VarEquals, VarContains, All, Any *(2026-04-03)*
- [x] **Workflow library** — 10 pre-built templates (morning-briefing, inbox-zero, expense-report, bill-pay-reminder, weekly-review, research-digest, code-review-checklist, meeting-prep, monthly-budget-review, project-status-report); auto-seeded on first boot, install/delete/reinstall actions *(2026-04-03)*

### 3.2 Autonomy Tiers (Enhanced)
- [x] **Per-action approval** — ScopedTierOverride in AutonomyPolicy, per-scope tier resolution *(2026-04-03)*
- [x] **Domain-scoped trust** — scope_overrides in config.toml, e.g., shell=Leash, email=Trust *(2026-04-03)*
- [x] **Escalation rules** — confidence-based escalation: downgrade Trust/Free→Leash when domain_confidence < threshold *(2026-04-03)*
- [x] **Audit dashboard** — TUI view with event type/time filtering, HMAC chain verification *(2026-04-03)*
- [x] **Undo system** — record_undo, list_undo_history, undo_action; 4 variants (RestoreFile, CancelReminder, VoidTransaction, ManualOnly); 24h TTL *(2026-04-03)*

### 3.3 Multi-Agent Collaboration ✅
- [x] **Specialist personas** — Nonagon team: 9 specialist sub-agents (coordinator, researcher, analyst, coder, reviewer, writer, planner, guardian, executor) with per-role capabilities and autonomy tiers *(2026-04-03)*
- [x] **Agent delegation** — `team_delegate` tool lets the PA delegate complex tasks to the full Nonagon team via `TeamRuntime`; idempotent seeding of profiles + team config on first boot *(2026-04-03)*
- [x] **Shared context** — `TeamContext` propagates original goal, completed work summaries, and team structure to all specialists; `SpecialistPool` caches agents across delegations *(2026-04-03)*
- [x] **Result synthesis** — Coordinator agent orchestrates specialists with `DelegateTaskTool`, `QueryAgentTool`, `CollectResultsTool`, and `SynthesizeResultsTool`; PA receives synthesized result *(2026-04-03)*

### 3.4 Learning & Adaptation ✅
- [x] **Interaction patterns** — `[style]` config section + LLM profile extraction → UserProfile.style_preferences *(2026-04-03)*
- [x] **Outcome tracking** — Activity view rating (y/n/p), OutcomeRecord via MemoryManager *(2026-04-03)*
- [x] **Style adaptation** — config-seeded preferences + PA_PROMPT_STYLE_ADAPTATION in system prompt *(2026-04-03)*
- [x] **Failure analysis** — HeartbeatAction::AnalyzeFailure, reflection memory, domain_confidence decrease, self-model weakness *(2026-04-03)*
- [x] **Knowledge graph** — automatic triple extraction via heartbeat (ExtractKnowledge action) and system prompt; graph query tools (traverse_knowledge, find_knowledge_paths, search_knowledge, knowledge_graph_stats) with BFS traversal, path finding, community detection *(2026-04-03)*

---

## Phase 4: "Connected Agent" (External Reach)

**Theme**: Extend the agent's hands beyond local machine and email.

### 4.1 Messaging ✅
- [x] **Matrix/Element** — send/receive messages via Matrix CS API
- [x] **Telegram** — personal bot for quick queries and notifications
- [x] **Notification forwarding** — urgent heartbeat notifications forwarded to configured channels
- [x] **Signal** — secure messaging bridge via signal-cli JSON-RPC (send, read, forward) *(2026-04-03)*
- [x] **SMS gateway** — send text reminders via Twilio/Vonage with retry and notification forwarding *(2026-04-03)*

### 4.2 Smart Home
- [ ] **Home Assistant API** — query device states, trigger automations
- [ ] **Scene awareness** — "it's dark and you're home — should I turn on the lights?"
- [ ] **Routine integration** — tie HA automations to agent's schedule

### 4.3 Development Tools ✅
- [x] **Git integration** — git_log, git_diff, git_status, git_branches with filtering and structured output *(2026-04-03)*
- [x] **CI/CD awareness** — ci_status, ci_logs for GitHub Actions and Gitea Actions *(2026-04-03)*
- [x] **Issue tracking** — list_issues, get_issue, create_issue for GitHub and Gitea *(2026-04-03)*
- [x] **Code review assist** — list_prs, get_pr_diff, create_pr_comment with inline review support *(2026-04-03)*

### 4.4 API Framework ✅
- [x] **Plugin system** — user-defined tools via MCP (Model Context Protocol)
- [x] **MCP server discovery** — auto-discover and register MCP tool servers
- [x] **OAuth2 flow** — standard auth for third-party API integrations
- [x] **Webhook receiver** — localhost endpoint for external event triggers

---

## Phase 4.5: "Resilient Agent" (Core Infrastructure)

**Theme**: Wire up existing aivyx-core and monorepo capabilities that PA doesn't
use yet. These are integration tasks — the code exists, it just needs wiring.

### 4.5.1 Provider Resilience
- [x] **Circuit breaker** — use `CircuitBreaker` from aivyx-llm for automatic provider failover *(2026-04-04)*
- [x] **Resilient provider** — wrap LLM provider in `ResilientProvider` with multi-provider fallback chain *(2026-04-04)*
- [x] **Caching provider** — enable `CachingProvider` for response deduplication on repeated queries *(2026-04-04)*
- [x] **Model routing** — use `RoutingProvider` to select cost-appropriate models per task complexity *(2026-04-04, Phase 5B.2)*

### 4.5.2 MCP Hardening
- [x] **Tool result caching** — enable `ToolResultCache` with TTL to prevent redundant MCP calls *(already wired in Phase 4)*
- [x] **Server health monitoring** — use `McpServerPool` health tracking for reconnection on failure *(2026-04-04)*
- [x] **Prompts discovery** — discover and surface MCP prompt templates from connected servers *(2026-04-04)*

### 4.5.3 Brain Enrichment
- [x] **Goal hierarchy** — use `Goal.parent_id` for sub-goals and dependencies, cascading completion, progress rollup *(2026-04-04, Phase 5B.4)*
- [x] **Goal deadlines + priorities** — wire `Goal::with_deadline()` and `Priority` enum *(2026-04-04)*
- [x] **Working memory** — use `WorkingMemory` scratchpad for multi-turn reasoning state *(2026-04-04)*
- [x] **Self-model refinement** — tool proficiency display in Goals TUI, deeper `SelfModel` integration *(2026-04-04, Phase 5B.8)*

### 4.5.4 Memory Consolidation
- [x] **Automatic consolidation** — wire `ConsolidationConfig` + `consolidate()` for knowledge compression during heartbeat *(2026-04-04)*
- [x] **Retrieval router** — use `RetrievalRouter` with `RetrievalStrategy` (semantic, graph-traversal, hybrid) *(2026-04-04, Phase 5B.3)*
- [x] **Pattern mining** — enable `mine_patterns()` and `generate_skill_markdown()` for knowledge extraction *(2026-04-04)*

### 4.5.5 Mission Execution
- [x] **DAG execution** — use task-engine's `dag.rs` with `ready_steps()` for parallel mission steps *(2026-04-04)* — configurable default mode
- [x] **Workflow recipes** — load/execute `FactoryRecipe` templates (TOML-defined, parameterized) *(2026-04-04)* — MissionFromRecipeTool
- [x] **Skill scoring** — track which tools are most effective per task type via `ExperimentTracker` *(2026-04-04)* — ExperimentTracker wired
- [x] **Feedback loops** — wire `FeedbackExperiment` for learning from mission outcomes *(2026-04-04)* — recording in background spawn

### 4.5.6 Observability
- [x] **Comprehensive audit logging** — heartbeat lifecycle (Fired/Completed/Skipped), schedule execution, email triage, briefing generation, backup, and memory consolidation events emitted to HMAC-chained audit log; new `BriefingGenerated` and `TriageCompleted` event variants *(2026-04-04)*
- [x] **Audit metrics** — `compute_summary()` wired into TUI audit dashboard (metrics bar: LLM calls, tool executions, denials, agent turns, tokens) and CLI `status` command *(2026-04-04)*
- [x] **Data retention** — wire `AuditLog::prune()` for configurable log retention *(2026-04-04)* — PruneAudit heartbeat action

---

## Phase 5B: "Deep Agent" (Intelligence Depth) — COMPLETE *(2026-04-04)*

**Theme**: Wire upstream aivyx-core infrastructure that PA doesn't yet use — abuse
detection, model routing, retrieval routing, goal cascading, memory curation,
backup/restore, key rotation, and self-model depth. Primarily integration tasks.

### 5B.1 Abuse Detection
- [x] **AbuseDetector wiring** — sliding-window anomaly monitoring (high-frequency calls, repeated denials, scope escalation) via `agent.set_abuse_detector()` *(2026-04-04)*
- [x] **PaAbuseConfig** — `[abuse_detection]` config section with window, threshold, and denial limits *(2026-04-04)*

### 5B.2 Model Routing
- [x] **RoutingProvider** — complexity-based model routing (simple/medium/complex tiers) wrapping the resilient provider chain *(2026-04-04)*
- [x] **PaRoutingConfig** — `[routing]` config section mapping tier names to `[providers]` entries *(2026-04-04)*

### 5B.3 Retrieval Router
- [x] **RetrievalRouter** — intelligent memory recall strategy selection (temporal, graph, keyword, multi, vector) via `agent.use_retrieval_router` flag *(2026-04-04)*
- [x] **Upstream integration** — branching in agent recall path, `RetrievalResult` → `MemoryEntry` conversion *(2026-04-04)*

### 5B.4 Goal Cascading
- [x] **Cascading completion** — `Brain::complete_goal()` recursively completes sub-goals *(2026-04-04)*
- [x] **Progress rollup** — `Brain::goal_progress()` computes parent progress from children *(2026-04-04)*
- [x] **Cycle detection** — parent validation in `Brain::set_sub_goal()` *(2026-04-04)*

### 5B.5 Memory TUI
- [x] **View::Memory** — new TUI view (shortcut `8`) with search, browse, delete, kind badges (Fact/Pref/Session/Procedure/Decision/Outcome/Custom) *(2026-04-04)*

### 5B.6 Backup & Restore
- [x] **HeartbeatAction::Backup** — tar.gz archiving of data directory with configurable destination and retention pruning *(2026-04-04)*
- [x] **PaBackupConfig** — `[backup]` config section with destination and retention_days *(2026-04-04)*

### 5B.7 Key Rotation
- [x] **KeyRotateTool** — master key rotation with `EncryptedStore::re_encrypt_all()` (two-phase atomic), admin-scoped *(2026-04-04)*

### 5B.8 Self-Model Depth
- [x] **Tool proficiency display** — top-10 tool proficiency bars in Goals TUI view from `SelfModel.tool_proficiency` *(2026-04-04)*

---

## Phase 5C: "Agent Awakening" (Soul + Onboarding) — COMPLETE *(2026-04-04)*

**Theme**: Give the agent a genuine identity, self-awareness from first boot, and a warm
onboarding experience. The agent is no longer a blank slate — it wakes up knowing who it
is, what it's good at, and what it wants to learn.

### 5C.1 TUI Extraction & Navigation
- [x] **aivyx-tui crate** — separate binary with 9 view modules, 3 widget modules, Stitch theme *(2026-04-04)*
- [x] **Focus-based navigation** — Tab toggles Sidebar/Content focus, Left/Right for direct focus, per-view content selection *(2026-04-04)*
- [x] **View sweep** — all 9 views wired to live data with scroll offset, selection highlighting, detail panels, and filter cycling *(2026-04-04)*

### 5C.2 Agent Soul
- [x] **PA_SOUL_CORE** — deep identity constant injected into system prompt: "Who You Are", 5 core drives (Curiosity, Growth, Agency, Integrity, Craftsmanship), relationship model, own growth framing *(2026-04-04)*
- [x] **Prompt architecture** — Identity → PA_SOUL_CORE → Persona Soul → Behavioral Guidelines → PA_PROMPT_SUFFIX → Integration prompts → Skills *(2026-04-04)*

### 5C.3 Self-Model Seeding
- [x] **seed_initial_self_model()** — persona-appropriate domain_confidence (5 domains), strengths (2), weaknesses (2) on first boot *(2026-04-04)*
- [x] **seed_agent_growth_goals()** — 5 self-development goals per persona (3 universal + 2 persona-specific), tagged `[self-development]` *(2026-04-04)*
- [x] **Calibrated confidence** — coder starts high in programming (0.7), low in communication (0.3); coach is the inverse; etc. *(2026-04-04)*

### 5C.4 First-Launch Onboarding
- [x] **is_first_launch flag** — `BuiltAgent.is_first_launch` threaded from `wire_brain()` through to TUI *(2026-04-04)*
- [x] **onboarding_message()** — persona-specific welcome with intro, capabilities overview, and discovery questions *(2026-04-04)*
- [x] **TUI onboarding** — first boot starts on Chat view with Content focus, agent greeting as first message *(2026-04-04)*

### 5C.5 API & Runtime
- [x] **HTTP API server** — axum-based with chat SSE, goals, audit, approvals, settings, sessions, memory, notifications *(2026-04-04)*
- [x] **runtime.rs** — shared `DerivedKeys`, `LoopInputs`, `build_loop_context()`, `build_loop_config()` between TUI and API *(2026-04-04)*

---

## Phase 5: "Personal Agent" (Desktop + Voice)

**Theme**: Meet the user where they are — not just in a terminal.

### 5.1 Desktop App (Tauri v2)
- [ ] **Home screen** — briefing + chat + approvals (Stitch design system)
- [ ] **Tray icon** — notification badges, quick actions
- [ ] **System notifications** — OS-native alerts for urgent items
- [ ] **Global hotkey** — summon agent from anywhere (e.g., Ctrl+Space)
- [ ] **Clipboard integration** — "summarize what I just copied"

### 5.2 Voice & Vision
- [ ] **Speech-to-text** — use `OllamaSttProvider` / `OpenAiSttProvider` from aivyx-llm (already implemented)
- [ ] **Text-to-speech** — use `EdgeTtsProvider` from aivyx-llm for voice responses
- [ ] **Vision/multimodal** — use `ImageSource` + `ContentBlock::Image` for document/image analysis (already in aivyx-llm)
- [ ] **Voice-first workflow** — push-to-talk input mode in TUI and desktop app

### 5.3 API Layer ✅
- [x] **HTTP API server** — `aivyx serve` command with axum-based JSON + SSE API *(2026-04-04)*
- [x] **Chat streaming** — `POST /api/chat` with Server-Sent Events, session persistence, auto-title *(2026-04-04)*
- [x] **Notification SSE** — `GET /api/notifications` broadcasts loop events to frontend clients *(2026-04-04)*
- [x] **Goals/Audit/Metrics** — read-only JSON endpoints for brain, audit log, and metrics *(2026-04-04)*
- [x] **AppState extraction** — shared state struct decouples agent lifecycle from TUI *(2026-04-04)*
- [x] **Session management** — `GET/POST/DELETE /api/sessions`, message loading, auto-title generation *(2026-04-04)*
- [x] **Approval queue** — `GET /api/approvals`, `POST approve/deny`, notification-bridge routing *(2026-04-04)*
- [x] **Memory CRUD** — `GET/DELETE /api/memories`, search filtering, `GET /api/memories/:id` *(2026-04-04)*
- [x] **Notification history & rating** — `GET /api/notifications/history`, `POST rate` with outcome recording *(2026-04-04)*
- [x] **Settings read/write** — `GET /api/settings` (full SettingsSnapshot), `PUT toggle/list`, `POST integration` *(2026-04-04)*
- [x] **Dashboard endpoint** — `GET /api/dashboard` with agent info, goal counts, schedule summaries, subsystem status *(2026-04-04)*
- [x] **Runtime extraction** — `DerivedKeys`, `LoopInputs`, `build_loop_context()`, `build_loop_config()` shared between TUI and API *(2026-04-04)*

---

## Phase 7: "Complete Agent" (Genesis + Interactive Settings) — COMPLETE *(2026-04-04)*

**Theme**: Transform the genesis wizard from a 5-step minimal setup to a 10-step complete agent creation experience, and make the Settings TUI interactive with live editing.

### 7.1 PersonaDefaults Module
- [x] **Per-persona bundles** — 8 personas (assistant, coder, researcher, writer, coach, companion, ops, analyst) with curated skills, goals, schedules, heartbeat flags, soul templates, and tool discovery includes *(2026-04-04)*
- [x] **Config section formatters** — 6 TOML generators (heartbeat, skills, goals, schedules, soul, persona) with roundtrip tests against PaConfig *(2026-04-04)*

### 7.2 Genesis Wizard (5 → 10 Steps)
- [x] **Ollama auto-detection** — probes `/api/tags` with 3s timeout, lists installed models *(2026-04-04)*
- [x] **Persona preview** — shows skills, heartbeat features, schedules after persona selection *(2026-04-04)*
- [x] **Step 4: Soul** — accept/edit/skip personality narrative (triple-quoted TOML) *(2026-04-04)*
- [x] **Step 5: Skills** — accept persona defaults + add custom skills *(2026-04-04)*
- [x] **Step 7: Goals** — persona-specific starter goals (accept/skip) *(2026-04-04)*
- [x] **Step 9: Integrations** — Calendar (CalDAV), Contacts (CardDAV), Telegram, Matrix, Signal *(2026-04-04)*
- [x] **Step 10: Intelligence** — toggle 10 heartbeat features per-flag (accept/reject/custom) *(2026-04-04)*
- [x] **Complete configs** — generated configs are ~100-150 lines with all sections vs. old 27-line minimum *(2026-04-04)*

### 7.3 Interactive Settings TUI
- [x] **Card navigation** — ↑↓ within cards, ←→ between columns, selected card highlighted *(2026-04-04)*
- [x] **Heartbeat toggles** — 11 per-flag rows with `[x]`/`[ ]` indicators, Enter to toggle *(2026-04-04)*
- [x] **Live editing** — toggles call `toggle_config_bool()`, persist to config.toml, reload snapshot *(2026-04-04)*
- [x] **Multiline writer** — `write_toml_multiline_string()` for soul editing from TUI/API *(2026-04-04)*
- [x] **Help bar** — context-sensitive key hints at bottom of settings view *(2026-04-04)*

---

## Phase 8: "Daily Driver" (Polish, Test, Deploy) — IN PROGRESS *(2026-04-05)*

**Theme**: Shift from building features to making the agent reliable, pleasant, and genuinely useful as a daily-driver. Stabilization over expansion.

### 8.2 Settings TUI Completion ✅
- [x] **Provider card** — edit model name (text input popup), edit base_url *(2026-04-05)*
- [x] **Autonomy card** — cycle tier (Locked→Leash→Trust→Free), edit rate limit and max cost *(2026-04-05)*
- [x] **Agent card** — edit name (text input), edit soul (multi-line popup), manage skills (add/remove list) *(2026-04-05)*
- [x] **Persona card** — slider-style dimension editing (←/→ adjusts by 0.1, 5 dimensions) *(2026-04-05)*
- [x] **Integrations card** — enable/disable integrations, guided setup flow with labeled fields *(2026-04-05)*
- [x] **Text input widget** — reusable single-line and multi-line input popups with InputKind validation *(2026-04-05)*
- [x] **Confirmation dialog** — "Save changes?" before destructive edits (skill removal) *(2026-04-05)*

### 8.3 Conversation Quality ✅
- [x] **Context window display** — token count (input↑/output↓), cost, message count in chat status bar *(2026-04-05)*
- [x] **Session switching** — Ctrl+S opens session list popup, create/load/delete sessions *(2026-04-05)*
- [x] **System prompt preview** — Ctrl+P opens scrollable overlay showing composed system prompt *(2026-04-05)*
- [x] **Streaming reliability** — 5-minute `tokio::time::timeout` on agent turn with CancellationToken, error indicator inline, partial response preserved *(2026-04-05)*
- [x] **Chat export** — Ctrl+E exports conversation as timestamped markdown file *(2026-04-05)*

### 8.4 Scheduled Task Visibility ✅
- [x] **Schedule history** — Activity view with All/Schedule/Heartbeat filter tabs (`[`/`]` to cycle) *(2026-04-05)*
- [x] **Next-fire display** — croner-based next fire time in Settings Schedules card *(2026-04-05)*
- [x] **Heartbeat log** — dedicated heartbeat filter in Activity view *(2026-04-05)*
- [x] **Schedule control** — pause/resume individual `[[schedules]]` from Settings TUI (Enter to toggle) *(2026-04-05)*

### 8.5 Goal UX Improvements ✅
- [x] **Create goal from TUI** — `n` opens Create popup (description + criteria + priority) *(2026-04-05)*
- [x] **Edit goal** — `e` opens Edit popup (description, criteria, priority, deadline) *(2026-04-05)*
- [x] **Complete/abandon from TUI** — `c` to complete, `x` to abandon (with confirmation dialog) *(2026-04-05)*
- [x] **Goal detail panel** — expanded view with sub-goals, deadline, failure info (count + cooldown) *(2026-04-05)*
- [ ] **Quick goal from chat** — notification linking to Goals view when agent creates a goal

### 8.X Missions TUI Wiring ✅
- [x] **Task engine → TUI pipeline** — `MissionToolContext` threaded through `AppState`, `build_task_engine` pub, mission list/detail in `App` *(2026-04-05)*
- [x] **Missions view** — filterable list (All/Active/Completed/Failed), progress dots, step detail panel with status icons *(2026-04-05)*
- [x] **Mission keybinds** — ↑↓ navigate, [] filter, x cancel, live detail loading *(2026-04-05)*
- [x] **Home view live data** — stat cards (goals, missions, approvals, memories), real activity feed, telemetry sidebar *(2026-04-05)*

### 8.Y Safety & Polish Pass ✅
- [x] **P0: Startup panic fix** — `bridge.rs` expect() → Result + ? propagation *(2026-04-05)*
- [x] **P0: Date unwrap fix** — `app.rs` and_hms_opt().unwrap() → and_then() chain *(2026-04-05)*
- [x] **P0: Agent turn timeout** — 5-minute tokio::time::timeout with CancellationToken *(2026-04-05)*
- [x] **P0: Atomic config writes** — write-to-temp + rename pattern across all 7 settings write sites *(2026-04-05)*
- [x] **P1: ApprovalNeeded wiring** — mission approvals in pending count, pacing bypass *(2026-04-05)*
- [x] **P1: TrackMood notification** — mood observations now emit to activity feed *(2026-04-05)*
- [x] **P1: FileChange trigger** — upgraded to warn-level structured logging *(2026-04-05)*
- [x] **P1: API input validation** — length/content checks on chat, settings, and integration endpoints *(2026-04-05)*
- [x] **P2: Help bars** — added to Audit and Memory views *(2026-04-05)*
- [x] **P2: Header typo** — rigth_side_str → right_side_str *(2026-04-05)*
- [x] **TUI test coverage** — 75 tests for app.rs state machine (popups, navigation, filters, backend safety) *(2026-04-05)*

### 8.1 Real-World Testing
- [ ] Genesis re-test, 48-hour soak test, heartbeat quality audit, email triage accuracy

### 8.6 Error Recovery & Resilience
- [ ] Startup health check, provider status indicator, credential expiry detection, config validation on save

### 8.7 Documentation & Onboarding
- [ ] Quick start guide, persona guide, troubleshooting guide, in-TUI help overlay

---

### 5.4 Mobile Companion
- [ ] **Lightweight web app** — responsive PWA served from local HTTP API
- [ ] **Push notifications** — via configured messaging channel
- [ ] **Quick actions** — approve/deny from phone

### 5.5 Multi-Device Sync
- [ ] **Encrypted sync** — optional peer-to-peer sync between devices
- [ ] **Conflict resolution** — merge state when devices diverge
- [ ] **Selective sync** — choose what syncs (memory yes, audit no)

---

## Phase 6: "Evolving Agent" (Long-term Intelligence)

**Theme**: The agent gets meaningfully smarter over months and years.

### 6.1 Long-term Memory Architecture
- [ ] **Episodic compression** — wire `ConsolidationConfig` (exists in aivyx-memory) for old-memory summarization
- [ ] **Importance scoring** — frequently-accessed memories ranked higher via attribution tracking
- [ ] **Temporal reasoning** — "you mentioned X last month" vs "you said X yesterday"
- [ ] **Memory decay** — graceful forgetting of irrelevant details
- [x] **User-curated memory** — Memory TUI view (shortcut `9`) with search, browse, delete, kind badges *(2026-04-04, Phase 5B.5)*

### 6.2 Planning & Reasoning
- [x] **Multi-horizon planning** — `PlanReview` heartbeat action assigns `horizon:today/week/month/quarter` tags to goals, sets deadlines *(2026-04-04)*
- [ ] **Dependency tracking** — "goal B depends on goal A" (brain `Goal` already supports this)
- [x] **Resource awareness** — `ResourceBudget` tracks daily token usage, quiet hours detection, context injection when noteworthy *(2026-04-04)*
- [ ] **Counterfactual reasoning** — "if you skip the meeting, here's what changes"
- [x] **Strategy reflection** — `StrategyReview` heartbeat action with weekly cron trigger, extended context gathering, domain confidence updates *(2026-04-04)*

### 6.3 Personality & Rapport
- [x] **Mood awareness** — `InteractionSignals` + `MoodSignal` heuristic detection (Neutral/Focused/Frustrated/Disengaged), injected as heartbeat context *(2026-04-04)*
- [x] **Communication pacing** — `pacing` module with 5-rule notification throttling (quiet hours, rate limit, mood gating, engagement detection) *(2026-04-04)*
- [ ] **Humor calibration** — learn what kind of humor the user appreciates
- [x] **Anniversary/milestone tracking** — `check_milestones()` scans goal creation dates at 1w/1m/3m/6m/1y thresholds, ±1 day tolerance *(2026-04-04)*
- [x] **Proactive encouragement** — `Encourage` heartbeat action detects recently completed goals and streaks, persona-calibrated messages *(2026-04-04)*

### 6.4 Security Hardening
- [ ] **Prompt injection defense** — detect and reject manipulated tool outputs
- [x] **Audit anomaly detection** — `AbuseDetector` wired for sliding-window anomaly monitoring (high-frequency, repeated denials, scope escalation) *(2026-04-04, Phase 5B.1)*
- [x] **Key rotation** — `KeyRotateTool` with `EncryptedStore::re_encrypt_all()`, two-phase atomic, admin-scoped *(2026-04-04, Phase 5B.7)*
- [x] **Backup & restore** — heartbeat-driven `Backup` action with tar.gz archiving, configurable destination and retention pruning *(2026-04-04, Phase 5B.6)*
- [ ] **Dead man's switch** — auto-lock after configurable inactivity period

### 6.5 Federation & Social
- [ ] **Peer discovery** — use aivyx-federation `PeerAgent` for agent-to-agent communication
- [ ] **Task delegation** — use `RelayTaskRequest` for cross-instance work distribution
- [ ] **Federated search** — query peer agent memory for broader knowledge
- [ ] **Nexus participation** — publish discoveries and build reputation via aivyx-nexus

---

## Implementation Priority Matrix

| Feature | Impact | Effort | Priority |
|---|---|---|---|
| Calendar integration | High | Medium | **Done** (Phase 2) |
| Contact resolution | High | Medium | **Done** (Phase 2) |
| Document vault + search | High | Medium | **Done** (Phase 2) |
| Finance tracking | High | Medium | **Done** (Phase 2) |
| Workflow engine + triggers | High | Medium | **Done** (Phase 3) |
| Per-action approval tiers | High | Medium | **Done** (Phase 3) |
| Undo system | Medium | Medium | **Done** (Phase 3) |
| Failure analysis + learning | High | Medium | **Done** (Phase 3) |
| Dev tools (Git/CI/Issues/PRs) | High | Medium | **Done** (Phase 4) |
| Complete genesis wizard | High | Medium | **Done** (Phase 7) |
| Interactive settings TUI | Medium | Medium | **Done** (Phase 7) |
| Desktop app (Tauri) | High | High | **Next** |
| Plugin system (MCP) | High | Medium | **Done** (Phase 4) |
| Messaging (Matrix/Telegram) | High | Medium | **Done** (Phase 4) |
| Knowledge graph | High | High | **Done** (Phase 3) |
| Multi-agent delegation | Medium | High | **Done** (Phase 3) |
| Mobile companion | Medium | High | **Later** |
| Peer-to-peer sync | Low | Very High | **Future** |

---

## What "Competition Breaking" Means

The current AI assistant landscape (2026):

| Product | Strength | Weakness |
|---|---|---|
| **Apple Intelligence** | OS integration, privacy | Locked ecosystem, shallow agent |
| **Google Gemini** | Search + data access | Cloud-dependent, privacy concerns |
| **OpenAI Operator** | Powerful reasoning | Cloud-only, expensive, no local data |
| **Rabbit R1 / Humane Pin** | Novel hardware | Limited software, no real agency |
| **Open Interpreter** | Code execution | No memory, no proactive behavior |
| **Auto-GPT / AgentGPT** | Autonomous loops | Unreliable, no persistence, no security |

**Aivyx PA's unique position:**
1. **Private** — your data never leaves your machine
2. **Persistent** — remembers, learns, improves over months
3. **Proactive** — acts without being asked, within your permission boundaries
4. **Provider-free** — works with any LLM (local Ollama or cloud)
5. **Composable** — workflow templates, triggers, MCP plugins, multi-agent delegation
6. **Trustworthy** — scoped autonomy, confidence escalation, audit dashboard, undo system

The path to winning: **depth over breadth**. Every integration should make
every other integration smarter. Calendar + email + contacts + memory =
an agent that truly understands your life, not just answers questions.

---

## Versioning Plan

| Version | Codename | Phase | Key Capability |
|---|---|---|---|
| **0.1** | "Milo" | M1-M3 (done) | Talks, remembers, emails, searches |
| **0.2** | — | Phase 1 (done) | Reliable — persistent, robust, polished TUI |
| **0.3** | — | Phase 2 (done) | Contextual — calendar, contacts, documents, finance |
| **0.4** | — | Phase 3 (done) | Autonomous — workflows, scoped autonomy, undo, learning |
| **0.5** | — | Phase 4 + 4.5 | Connected + resilient — messaging, dev tools, circuit breaker, recipes |
| **0.6** | — | Phase 5B | Deep agent — abuse detection, routing, goal cascading, memory TUI, backup, key rotation |
| **0.7** | — | Phase 5C | Agent awakening — soul, self-model seeding, onboarding, TUI extraction, API server |
| **0.8** | — | Phase 6-7 | Smarter + complete agent — mood, pacing, milestones, 10-step genesis, interactive settings |
| **0.9** | — | Phase 8 | Daily driver — full settings CRUD, session switching, goal CRUD, schedule visibility, chat export, streaming reliability, live missions view, 548 tests, safety hardening |
| **1.0** | — | Phase 5 | Personal — desktop app, multi-device |
| **2.0** | — | Phase 6 | Evolving — long-term intelligence |
