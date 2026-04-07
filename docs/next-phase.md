# Phase 8: "Daily Driver" — Polish, Test, Deploy

**Date**: 2026-04-04 (updated 2026-04-05)
**Status**: In Progress — 8.2–8.6 complete; P0–P2 safety/polish pass done; schedule CRUD + desktop + deep interaction added
**Predecessor**: Phase 7 "Complete Agent" (genesis wizard + interactive settings)
**Current state**: 703 tests, ~51,700 LOC, ~145 tools, 4 crates, 10-step genesis, fully interactive TUI

---

## Motivation

We've built a feature-complete personal assistant: ~85 tools, 15 heartbeat actions, 12 workflow templates, 10-step genesis, interactive settings, API server, 9 TUI views. But we haven't *used* it daily yet. Phase 8 shifts from building features to making the agent reliable, pleasant, and genuinely useful as a daily-driver.

The theme is **stabilization over expansion**. Every task should make the existing experience better rather than adding new surface area.

---

## 8.1 Real-World Testing & Bug Fixes

**Why**: The genesis wizard, heartbeat loop, email triage, and background schedules have never been exercised end-to-end in a sustained multi-day run. Bugs will surface.

- [ ] **Genesis re-test** — delete `~/.aivyx/`, run `aivyx init` through all 10 steps with real credentials, verify config.toml parses cleanly and all subsystems boot
- [ ] **48-hour soak test** — run the TUI for 2 full days with email connected, heartbeat active, schedules firing. Log and fix every crash, panic, or unexpected behavior
- [ ] **Heartbeat quality audit** — review 20+ heartbeat outputs for relevance, coherence, and action accuracy. Tune prompts if the LLM is noisy or irrelevant
- [ ] **Email triage accuracy** — process 50+ real emails through triage, verify classification accuracy, tune ignore/auto-reply rules
- [ ] **Memory growth check** — after 48 hours, inspect memory store size, consolidation effectiveness, and whether the agent's context is actually improving
- [ ] **Config migration** — verify existing 27-line configs (pre-Phase 7) still parse and run without error. The agent should boot with partial configs

**Estimated effort**: ~200 LOC fixes, 0 new features, high observational effort

---

## 8.2 Settings TUI Completion

**Why**: Phase 7 made the Heartbeat card interactive, but the other 7 cards are still read-only. Users should be able to edit all common settings without touching config.toml.

- [x] **Provider card** — edit model name (text input popup), toggle base_url
- [x] **Autonomy card** — cycle tier (Locked → Leash → Trust → Free), edit rate limit
- [x] **Agent card** — edit name (text input), edit soul (multi-line popup), manage skills (add/remove)
- [x] **Persona card** — slider-style dimension editing (←/→ adjusts by 0.1)
- [x] **Integrations card** — enable/disable individual integrations, guided setup flow
- [x] **Text input widget** — reusable single-line and multi-line input popup for settings editing
- [x] **Confirmation dialog** — "Save changes?" before destructive edits

**Estimated effort**: ~400 LOC, 0 tests (interactive TUI)

---

## 8.3 Conversation Quality

**Why**: The chat experience is the primary interface. Small improvements here have outsized impact on daily usability.

- [x] **Conversation context window** — display token count / context usage in status bar so the user can see when they're approaching limits
- [x] **Session switching** — TUI command or shortcut to list/switch between saved conversation sessions
- [x] **System prompt preview** — debug command or settings panel showing the full composed system prompt (identity + soul + persona + capabilities + integrations)
- [x] **Streaming reliability** — 5-minute `tokio::time::timeout` on agent turn, CancellationToken on timeout, error indicator inline, partial response preserved
- [x] **Chat export** — export current conversation as markdown file

**Estimated effort**: ~300 LOC, ~5 tests

---

## 8.4 Scheduled Task Visibility

**Why**: Background schedules (briefings, workflow crons, heartbeat) fire silently. The user has no visibility into what ran, when, or why.

- [x] **Schedule history** — show last 10 schedule executions in the Activity view (name, time, status, truncated result)
- [x] **Next-fire display** — show "next fires at" timestamp for each schedule in the Settings card
- [x] **Heartbeat log** — dedicated heartbeat history in Activity view showing actions taken, context summary, and skip reasons
- [x] **Schedule control** — pause/resume individual schedules from the TUI (calls `toggle_schedule_enabled` on `[[schedules]]` blocks)

**Estimated effort**: ~250 LOC, ~3 tests

---

## 8.5 Goal UX Improvements

**Why**: Goals are the agent's planning backbone, but the TUI interaction is limited to viewing and filtering. Users need to create, edit, and manage goals interactively.

- [x] **Create goal from TUI** — text input popup in Goals view (description + optional priority + optional deadline)
- [x] **Edit goal** — modify description, priority, deadline from the Goals view
- [x] **Complete/abandon from TUI** — keybinds in Goals view (e.g., `c` to complete, `x` to abandon)
- [x] **Goal detail panel** — expanded view showing sub-goals, progress, tags, created_at, last activity
- [ ] **Quick goal from chat** — when the agent creates a goal during conversation, show a notification linking to the Goals view

**Estimated effort**: ~250 LOC, ~3 tests

---

## 8.6 Error Recovery & Resilience

**Why**: When things go wrong (Ollama down, email password expired, disk full), the agent should degrade gracefully and tell the user what happened and how to fix it.

- [x] **Startup health check** — on boot, verify: Ollama reachable (if configured), email credentials valid (quick IMAP login test), disk space adequate, all config sections parse. Show summary in Dashboard
- [x] **Provider status indicator** — Dashboard health bar shows LLM provider, email, config, disk status with degradation details
- [x] **Credential expiry detection** — if email IMAP login fails 3 times, show a persistent Urgent notification with guidance ("Email password may have expired — run `aivyx init` to update")
- [x] **Graceful heartbeat degradation** — exponential backoff on consecutive LLM failures (1x → 2x → 4x → 8x cap), clear logging, automatic recovery on success
- [x] **Config validation on save** — `validated_atomic_write` parses TOML before committing; rejects corrupt writes and preserves original file

**Estimated effort**: ~300 LOC, ~5 tests

---

## 8.7 Documentation & Onboarding

**Why**: A daily-driver needs good first-run documentation. Users who aren't us need to understand what the agent can do.

- [ ] **Quick start guide** — standalone `docs/quickstart.md` covering install → init → first conversation → first goal → first briefing
- [ ] **Persona guide** — document each of the 8 personas with use cases, example conversations, and recommended configurations
- [ ] **Troubleshooting guide** — common errors and fixes (Ollama 404, email auth, "memory system unavailable", passphrase forgotten)
- [ ] **In-TUI help** — `?` keybind shows view-specific help overlay with available actions and keybinds
- [ ] **Agent capabilities summary** — on first boot, the agent's greeting should mention its configured capabilities (e.g., "I have email access, 3 scheduled tasks, and heartbeat reflection enabled")

**Estimated effort**: ~200 LOC code, ~500 lines documentation

---

## Implementation Order

```
8.1 Real-World Testing ──────────────────────────────── (do first, drives all other priorities)
     │
     ├──► 8.6 Error Recovery (fix what breaks in 8.1)
     │
     ├──► 8.3 Conversation Quality (improve the primary interface)
     │
     ├──► 8.4 Schedule Visibility (understand background behavior)
     │
     └──► 8.5 Goal UX (interactive goal management)

8.2 Settings Completion ─────────────────────────────── (independent track, can interleave)

8.7 Documentation ───────────────────────────────────── (after features stabilize)
```

**8.1 must come first** — it generates the bug list and priority signals for everything else. The 48-hour soak test will likely surface 5-15 issues that reshape the rest of the phase.

---

## What This Phase is NOT

- **No new integrations** — no Home Assistant, no new messaging platforms, no new tool registrations
- **No desktop app** — Tauri is Phase 5 on the roadmap, after daily-driver is solid
- **No voice/vision** — interesting but premature; the text experience needs to be flawless first
- **No federation** — multi-agent networking is Phase 6+ territory
- **No performance optimization** — premature until we have real usage data from the soak test

---

## Success Criteria

Phase 8 is complete when:

1. The agent runs for 7 consecutive days without crashes or data corruption
2. All 10 genesis steps work with real credentials (email, calendar, Telegram, etc.)
3. Heartbeat outputs are consistently relevant (>80% useful by manual review)
4. A new user can go from `cargo install` to a working assistant in under 10 minutes
5. All 8 settings cards are interactive (view + edit)
6. The Goals view supports full CRUD (create, read, update, complete/abandon)

---

## Estimated Totals

| Section | New LOC | Tests | Priority | Status |
|---|---|---|---|---|
| 8.1 Real-world testing | ~200 | 0 | **Critical** | Pending |
| 8.2 Settings completion | ~400 | 0 | High | **Done** |
| 8.3 Conversation quality | ~300 | 5 | High | **Done** |
| 8.4 Schedule visibility | ~250 | 3 | Medium | **Done** |
| 8.5 Goal UX | ~250 | 3 | Medium | **Done** |
| 8.6 Error recovery | ~300 | 7 | High | **Done** |
| 8.7 Documentation | ~200 + docs | 0 | Medium | Pending |
| Safety/polish pass (P0–P2) | ~120 | 0 | High | **Done** |
| Mission TUI wiring | ~340 | 7 | High | **Done** |
| TUI test coverage | ~400 | 75 | Medium | **Done** |
| Schedule CRUD tools | ~290 | 9 | High | **Done** |
| Desktop interaction (7 tools) | ~560 | 21 | High | **Done** |
| Deep interaction (22 tools) | ~3,200 | 49 | High | **Done** |
| Extended interaction (14 tools) | ~1,800 | 14 | High | **Done** |
| High-impact interaction (6 tools) | ~1,500 | 20 | High | **Done** |
| Document tools (6 tools) | ~1,500 | 21 | High | **Done** |
| TUI bug fixes (mutex, scroll) | ~20 | 0 | High | **Done** |
| Tools & Extensions card | ~30 | 1 | Medium | **Done** |
| **Total** | **~11,660** | **~233** | | |
