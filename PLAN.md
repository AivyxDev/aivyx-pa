# Aivyx Personal Assistant — Product Plan

**Date**: 2026-04-01
**Status**: Planning
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
| `aivyx-task-engine` | DAG orchestration — overkill for PA |
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

---

## Architecture

```
aivyx-pa/
  crates/
    aivyx-pa/         # Main binary — TUI + embedded server + agent loop
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

### M1: "It talks and remembers" (1-2 weeks)
- [ ] New repo scaffolded with workspace
- [ ] Core deps wired (crypto, config, llm, memory, agent)
- [ ] Simplified `aivyx init` (3 steps: passphrase, provider, model)
- [ ] `aivyx chat` — interactive REPL with memory persistence
- [ ] Memory recall across sessions ("you mentioned last week...")

### M2: "It checks my email" (1-2 weeks)
- [ ] `aivyx-actions` crate with Email action (IMAP read, SMTP send)
- [ ] `aivyx-loop` crate with basic schedule (check email every N minutes)
- [ ] Morning briefing: email summary on TUI launch
- [ ] "Draft a reply to X" → generates draft → asks for approval → sends

### M3: "It manages my day" (2-3 weeks)
- [ ] Reminders (local, time-triggered, persisted)
- [ ] File actions (read, organize, search local files)
- [ ] Shell actions (with capability gating)
- [ ] Web search/fetch actions
- [ ] Approval queue in TUI ("2 actions pending your approval")

### M4: "It has a face" (2 weeks)
- [ ] Tauri desktop app with simplified GUI
- [ ] Home screen (briefing + chat + approvals)
- [ ] Tray icon with notification badges
- [ ] System notifications for urgent items

### M5: "It runs my life" (ongoing)
- [ ] Calendar integration
- [ ] Contacts
- [ ] Finance tracking
- [ ] Smart home
- [ ] Mobile companion (future)

---

## Non-Goals for v1.0

- Multi-agent fleets / constellations
- Federation / Nexus social network
- Multi-tenant billing
- DAG recipe orchestration
- Enterprise SSO
- Kubernetes deployment
- Agent marketplace
- Voice interface (future)

---

## Open Questions

1. **Repo strategy**: Fresh repo (`aivyx-pa`) or refactor within `aivyx`?
2. **aivyx-core dependency**: Git submodule, path dependency, or vendor the 9 crates?
3. **Desktop-first or TUI-first?** (Recommendation: TUI-first, GUI follows)
4. **Model default**: Ollama + qwen3:14b as the zero-config local option?
5. **Name**: Keep "Aivyx" or give the PA a distinct name/personality?
