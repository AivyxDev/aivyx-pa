# Aivyx Personal Assistant — Product Plan

**Date**: 2026-04-01 (updated 2026-04-05)
**Status**: Active — M1-M3 complete, Phase 1-7 complete, Phase 8 in progress (8.2-8.5 done)
**Vision**: A private, local-first AI personal assistant that manages your life and business.

---

## The Problem

The current Aivyx ecosystem (8 repos, 24 crates, enterprise fleet tooling) was built for an enterprise agent orchestration platform that nobody asked for. The original goal was simple: **a secure, private AI assistant that actually does things for you**.

Getting an agent running today requires: genesis wizards, systemd units, encrypted keystores, TOML configs per agent, fleet deploy scripts, PTY hacks for credential injection, and a factory dashboard showing metrics for work that never happens.

The end user experience should be:
```
$ aivyx
Good morning, Julian.
3 new emails — 1 needs a reply (Sarah re: proposal, she accepted).
Electric bill due Apr 7 ($142.30) — reminder set for tomorrow.
No calendar conflicts today.
> draft a reply to Sarah
```

---

## What We Keep (from aivyx-core)

These crates are battle-tested and directly serve the personal assistant:

| Crate | Why |
|---|---|
| `aivyx-core` | Base types, error handling, IDs |
| `aivyx-crypto` | Encrypted storage, Argon2id KDF, ChaCha20 |
| `aivyx-config` | Config parsing (simplified) |
| `aivyx-audit` | HMAC-chained audit log — trust/accountability |
| `aivyx-llm` | Provider abstraction (Ollama, OpenAI, Anthropic, OpenRouter) |
| `aivyx-memory` | Episodic memory + knowledge triples — the moat |
| `aivyx-mcp` | MCP tool execution |
| `aivyx-agent` | Agent identity, persona, soul |
| `aivyx-capability` | Permission model (Locked/Leash/Trust/Free) |

## What We Shelve

These served the enterprise vision. Not deleted — just not in v1.0:

| Crate/App | Why shelved |
|---|---|
| `aivyx-federation` | Nexus social network — not a PA feature |
| `aivyx-nexus` | Same |
| `aivyx-billing` | Multi-tenant billing — single user doesn't need it |
| `aivyx-tenant` | Multi-tenancy |
| `aivyx-team` | Team topology |
| `aivyx-registry` | Agent marketplace |
| `aivyx-sso` | Enterprise SSO/OIDC |
| ~~`aivyx-task-engine`~~ | ~~DAG orchestration — overkill for PA~~ — **ADOPTED** (2026-04-02), missions enable multi-step autonomous tasks |
| `aivyx-eval` | Evaluation framework |
| `aivyx-cognition` | Over-engineered reasoning layer |
| `nexus-web` | Nexus web app |
| `aivyx-creator` | Agent builder wizard site |
| `aivyx-hub` | Hub infrastructure |
| `aivyx-ecosystem` | Ecosystem meta-repo |
| `aivyx-agent-node` | 8-agent fleet deploy |

## What We Build New

### 1. `aivyx-actions` — Real-world integrations

The critical missing piece. The assistant needs hands, not just a mouth.

**Phase 1 (MVP):**
- Email (read inbox, draft/send replies) — reuse existing IMAP/SMTP code from aivyx-server
- Files (read, write, organize local files)
- Shell (run commands with capability gating)
- Reminders (time-based triggers stored locally)
- Web (fetch pages, search)

**Phase 2:**
- Calendar (Google Calendar / CalDAV)
- Contacts (CardDAV / local)
- Notes (local markdown vault)
- Finance (read bank notifications, track bills)

**Phase 3:**
- Smart home (Home Assistant API)
- Messaging (send texts via configured provider)
- Shopping (price checks, order tracking)

### 2. `aivyx-loop` — The proactive agent loop

This is what makes it an assistant instead of a chatbot.

```
loop {
    // 1. Wake on schedule (morning briefing, hourly check, etc.)
    // 2. Check all registered sources (email, calendar, reminders, feeds)
    // 3. Assess what's new, urgent, or due
    // 4. Prepare briefing or take autonomous action (per autonomy tier)
    // 5. Present to user when they open the app, or notify via configured channel
    // 6. Sleep until next trigger or user interaction
}
```

Key behaviors:
- **Morning briefing**: Email summary, calendar for today, reminders due, weather
- **Continuous monitoring**: New emails, calendar changes, reminder triggers
- **Autonomous actions** (Trust/Free tier): Auto-file receipts, draft routine replies, reschedule conflicts
- **Approval queue** (Leash/Trust tier): "I drafted a reply to Sarah. Send it?"

### 3. Simplified TUI — `aivyx` CLI

One binary. One command.

```
$ aivyx init          # First run: passphrase, pick model, done (3 steps, not 8)
$ aivyx               # Launch TUI — shows briefing + chat
$ aivyx chat "..."    # One-shot from terminal
$ aivyx status        # What has the assistant been doing
$ aivyx config        # Edit settings
```

No `server start`. No port numbers. No fleet management. The TUI IS the app.

### 4. Simplified GUI — Desktop app

Tauri v2 + lightweight frontend (keep Stitch design system).

**Home screen** (replaces Factory Hub):
```
Good morning, Julian.                    [Settings]

--- Today -------------------------------------------
  3 new emails (1 needs reply)         [View Inbox]
  Dentist at 2pm                       [View Calendar]
  Electric bill due Apr 7 ($142.30)    [Set Reminder]

--- Assistant Activity ------------------------------
  Drafted reply to Sarah (awaiting approval)  [Review]
  Filed 2 receipts to /finances/2026/         [Undo]

--- Chat --------------------------------------------
  > _                                        [Send]
```

**Sidebar** (simplified):
- Home (briefing + chat)
- Inbox (emails + approvals)
- Memory (what the assistant knows about you)
- Activity (audit log — what it's been doing)
- Settings

That's 5 views, not 9. No Factory Hub, no Fleet, no DAG Recipes, no Nexus.

**Note:** The shipped TUI has 9 views (Home, Chat, Activity, Goals, Approvals, Missions, Audit, Memory, Settings) — the additional views proved necessary for operational visibility.

---

## Architecture

```
aivyx-pa/
  crates/
    aivyx-pa/         # Library + binary — agent core, config, API server, sessions
    aivyx-tui/        # Terminal UI binary (ratatui, 9 views, widgets, theme)
    aivyx-actions/    # Real-world integrations (email, files, shell, web, etc.)
    aivyx-loop/       # Proactive agent loop (briefings, monitoring, triggers)
  gui/                # Tauri desktop app (optional — TUI is primary)

  # Dependencies (from aivyx-core, vendored or path-dep):
  # aivyx-core, aivyx-crypto, aivyx-config, aivyx-audit
  # aivyx-llm, aivyx-memory, aivyx-mcp, aivyx-agent, aivyx-capability
```

**Single process.** The assistant runs as one binary that embeds:
- The agent loop (background thread)
- A minimal HTTP API (localhost only, for GUI communication)
- The TUI (foreground, ratatui)

No systemd units. No deploy scripts. `aivyx` runs, `Ctrl+C` stops it.

---

## Milestones

### M1: "It talks and remembers" ~~(1-2 weeks)~~ COMPLETE
- [x] New repo scaffolded with workspace
- [x] Core deps wired (crypto, config, llm, memory, agent)
- [x] Simplified `aivyx init` (3 steps: passphrase, provider, model)
- [x] Auto-init: Genesis wizard runs on first TUI launch (no `aivyx init` required)
- [x] `aivyx chat` — interactive REPL with memory persistence
- [x] Memory recall across sessions ("you mentioned last week...")
- [x] Agent identity: knows its configured name, uses persona-driven soul
- [x] TUI log routing: tracing → file (pa.log) instead of corrupting terminal
- [x] Brain system: goals seeded per persona, self-model, reflection
- [x] 84 tests passing across 3 crates + integration tests

### M2: "It checks my email and runs missions" — COMPLETE
- [x] `aivyx-actions` crate with Email action (IMAP read, SMTP send)
- [x] `aivyx-loop` crate with basic schedule (check email every N minutes)
- [x] Task-engine integration: 4 mission tools (create, list, status, control)
- [x] AgentSession + MissionToolContext wired in build_agent()
- [x] System prompt documents mission capabilities
- [x] Self-learning: 3 feedback loops wired end-to-end
  - Goal outcome feedback (record_success/failure, exponential backoff, auto-abandon)
  - Self-model update tool (brain_update_self_model — strengths, weaknesses, domain confidence)
  - Mission→brain bridge (PaMissionCreateTool bridges mission outcomes to goals + self-model)
- [x] Heartbeat: LLM-driven autonomous reasoning with context-aware skip (11 tests)
  - Gathers context (goals, reminders, email, self-model, schedules)
  - Skips LLM call when nothing changed (zero token cost)
  - Actions: notify, set/update goals, reflect, consolidate
  - Configurable via `[heartbeat]` in config.toml
- [x] Morning briefing: email summary on TUI launch (briefing_on_launch flag, structured notifications)
- [x] "Draft a reply to X" → generates draft → asks for approval → sends (fetch_email tool, in_reply_to threading, system prompt workflow)
- [x] Mission TUI view (dedicated Ctrl+N tab — active/completed/failed with step progress)

### M3: "It manages my day" — COMPLETE
- [x] Reminders (local, time-triggered, persisted to EncryptedStore, loop fires due reminders)
- [x] File actions (ReadFile, WriteFile, ListDirectory — registered and capability-authorized)
- [x] Shell actions (RunCommand with Shell capability scope)
- [x] Web fetch (FetchPage — raw page retrieval, 32k char limit)
- [x] Capability grants: all tool scopes (self-improvement, memory, missions, shell, filesystem) wired
- [x] Web search action (DuckDuckGo HTML search — search_web tool with result parsing)
- [x] Approval queue in TUI (^6 Approvals view — pending badge, approve/deny, auto-cleanup)
- [x] Memory extractor: already handles empty LLM responses gracefully (known Ollama model-quality issue, no crash)

### M4: "It has a face"
- [ ] Tauri desktop app with simplified GUI
- [ ] Home screen (briefing + chat + approvals)
- [ ] Tray icon with notification badges
- [ ] System notifications for urgent items

### M5: "It runs my life" — PARTIAL (Deep integrations complete, connected phase next)
- [x] Calendar integration (CalDAV — agenda, events, conflicts, auto-reminders) *(2026-04-02)*
- [x] Contacts (CardDAV sync + local store, search, enrichment) *(2026-04-02)*
- [x] Finance tracking (transactions, budgets, bills, receipts) *(2026-04-03)*
- [ ] Smart home
- [ ] Mobile companion (future)

### Phase 3: "Autonomous Agent" — COMPLETE *(2026-04-03)*
- [x] Audit dashboard TUI view (filter by type/time, HMAC chain verification)
- [x] Scoped autonomy (per-domain tier overrides, confidence-based escalation)
- [x] Workflow engine (templates, triggers, conditional branching, 4 tools)
- [x] Trigger system (cron, email, goal progress, cooldown dedup)
- [x] Undo system (record/list/reverse destructive actions, 24h TTL)
- [x] Outcome tracking (rate suggestions in Activity view, OutcomeRecord)
- [x] Failure analysis (heartbeat post-mortem, self-model update)
- [x] Style adaptation ([style] config, profile seeding, LLM-extracted preferences)
- [x] 427 tests, ~30,500 LOC, up to 80 tools (at time of completion)

### Phase 4.5: "Resilient Agent" — COMPLETE *(2026-04-04)*
- [x] Provider resilience: circuit breaker + multi-provider fallback + response caching
- [x] Configurable memory consolidation (merge threshold, stale days, pattern mining)
- [x] Brain enrichment: goal deadlines (ISO 8601), working memory population
- [x] MCP hardening: heartbeat health checks, prompts discovery tool
- [x] Mission execution: configurable default mode (sequential/DAG), recipe-based missions, experiment tracking
- [x] Observability: audit log pruning via heartbeat (configurable retention)
- [x] Settings TUI: resilience, consolidation, and mission config displayed in settings view
- [x] 427 tests, up to 80 tools

### Phase 5B: "Deep Agent" — COMPLETE *(2026-04-04)*
- [x] Abuse detection: `AbuseDetector` wired via `agent.set_abuse_detector()`, `[abuse_detection]` config
- [x] Model routing: `RoutingProvider` for complexity-based model selection (simple/medium/complex tiers), `[routing]` config
- [x] Retrieval router: intelligent memory recall strategy (temporal/graph/keyword/multi/vector) via `agent.use_retrieval_router`
- [x] Goal cascading: recursive sub-goal completion, progress rollup, cycle detection in `Brain`
- [x] Memory TUI: new View::Memory with search, browse, delete, kind badges
- [x] Backup & restore: heartbeat-driven `Backup` action with tar.gz archiving, retention pruning, `[backup]` config
- [x] Key rotation: `KeyRotateTool` with two-phase atomic re-encryption, admin-scoped
- [x] Self-model depth: tool proficiency bars in Goals TUI from `SelfModel.tool_proficiency`
- [x] 427 tests, ~31,000 LOC, up to 81 tools, 9 TUI views

### Phase 5C: "Agent Awakening" — COMPLETE *(2026-04-04)*
- [x] TUI crate extraction: `aivyx-tui` separated from `aivyx-pa` with 9 view modules, widgets, and theme
- [x] Focus-based navigation: Tab toggles Sidebar/Content focus, Left/Right for direct focus, per-view content selection
- [x] TUI view sweep: all 9 views wired to live data with scroll, selection, detail panels, and filter cycling
- [x] PA Soul: `PA_SOUL_CORE` constant injected into every agent's system prompt — core drives (Curiosity, Growth, Agency, Integrity, Craftsmanship), relationship with user, self-growth framing
- [x] Self-model seeding: `seed_initial_self_model()` primes domain confidence, strengths, and weaknesses per persona on first boot
- [x] Growth goals: `seed_agent_growth_goals()` creates 5 self-development goals per persona (3 universal + 2 persona-specific) tagged `[self-development]`
- [x] First-launch onboarding: `is_first_launch` flag on `BuiltAgent`, persona-specific welcome message, TUI starts on Chat view with agent greeting
- [x] API server: HTTP API (`aivyx serve`) with chat SSE, goals, audit, approvals, settings, sessions, memory, notifications
- [x] Runtime extraction: `runtime.rs` with `DerivedKeys`, `LoopInputs`, `build_loop_context()`, `build_loop_config()` shared between TUI and API
- [x] 429 tests, ~32,750 LOC, up to 81 tools, 4 crates

### Phase 6: "Smarter Agent" — COMPLETE *(2026-04-04)*
- [x] InteractionSignals: ephemeral per-session user activity tracking (message patterns, short streaks, negative keywords, notification counts)
- [x] MoodSignal: heuristic mood estimation (Neutral/Focused/Frustrated/Disengaged) from interaction signals — no extra LLM call
- [x] PlanReview heartbeat action: LLM organizes goals into time horizons (today/week/month/quarter), applies tags and deadlines
- [x] StrategyReview heartbeat action: weekly cron-triggered deep review of all goal progress, pattern detection, domain confidence updates
- [x] ResourceBudget: daily token tracking with auto-reset, quiet hours detection, context injection when budget > 50%
- [x] Communication pacing: `pacing` module with 5-rule notification throttling (urgent bypass, quiet hours, rate limit, mood gating, engagement deferral)
- [x] Milestone tracking: `check_milestones()` scans goal `created_at` for 1w/1m/3m/6m/1y anniversaries, ±1 day tolerance
- [x] Proactive encouragement: `Encourage` heartbeat action detects completed goals and streaks, persona-calibrated celebration
- [x] TrackMood heartbeat action: informational mood logging, real adaptation via LLM reading mood context section
- [x] 2 new workflow templates: `strategy-review` (Sunday 9 AM cron) and `milestone-scan` (1st of month cron)
- [x] Settings TUI: Heartbeat card expanded with "SMART" features row and pacing display
- [x] 459 tests, ~33,750 LOC, 81 tools, 15 heartbeat actions, 12 workflow templates

### Phase 7: "Complete Agent" — COMPLETE *(2026-04-04)*
- [x] PersonaDefaults module: 8 persona bundles (skills, goals, schedules, heartbeat flags, soul templates, tool discovery)
- [x] Config section formatters: 6 TOML generators (heartbeat, skills, goals, schedules, soul, persona) with roundtrip tests
- [x] Genesis wizard expanded: 5 → 10 steps (soul, skills, goals, integrations, intelligence)
- [x] Ollama model auto-detection: probes `/api/tags` with 3s timeout, lists installed models
- [x] Persona preview: shows skills, heartbeat features, schedules after persona selection
- [x] Integration setup: Calendar (CalDAV), Contacts (CardDAV), Telegram, Matrix, Signal — secrets stored in EncryptedStore
- [x] Heartbeat intelligence tuning: toggle 10 autonomous features per-flag (accept/reject/custom)
- [x] `write_toml_multiline_string()` settings writer for soul editing from TUI/API
- [x] Interactive Settings TUI: card navigation (↑↓←→), heartbeat toggle rows, Enter to toggle, help bar
- [x] Generated configs: ~100-150 lines with all sections vs. old 27-line minimum
- [x] 473 tests, ~35,000 LOC, 81 tools, 15 heartbeat actions, 12 workflow templates

---

## Non-Goals for v1.0

- Multi-agent fleets / constellations
- Federation / Nexus social network
- Multi-tenant billing
- Enterprise SSO
- Kubernetes deployment
- Agent marketplace
- Voice interface (future)

---

## Resolved Questions

1. **Repo strategy**: Fresh repo (`aivyx-pa`) — separate from the monorepo
2. **aivyx-core dependency**: Git + `[patch]` to local paths (both aivyx-core and aivyx monorepo)
3. **Desktop-first or TUI-first?** TUI-first, GUI follows
4. **Model default**: User chooses during Genesis wizard — Ollama models work well locally
5. **Name**: Configurable per-user via `[agent] name = "Milo"` in config.toml

## Open Questions

1. **Binary name**: Rename from `aivyx` to `aivyx-pa` to avoid collision with monorepo binary?
2. **Release distribution**: GitHub Releases (carried over from monorepo CI)?
3. **Desktop app**: When to start the Tauri GUI layer?
