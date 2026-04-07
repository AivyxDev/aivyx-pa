# Aivyx Personal Assistant — User Manual

A comprehensive guide to using your AI personal assistant.

---

## Table of Contents

1. [Getting Started](#getting-started)
2. [The TUI Interface](#the-tui-interface)
3. [Chatting with Your Assistant](#chatting-with-your-assistant)
4. [Memory & Knowledge](#memory--knowledge)
5. [Goals & Self-Improvement](#goals--self-improvement)
6. [Reminders & Schedules](#reminders--schedules)
7. [Missions](#missions)
8. [Email](#email)
9. [Calendar](#calendar)
10. [Contacts](#contacts)
11. [Messaging](#messaging)
12. [Document Vault](#document-vault)
13. [Finance Tracking](#finance-tracking)
14. [Web Research](#web-research)
15. [Desktop Interaction](#desktop-interaction)
16. [Developer Tools](#developer-tools)
17. [Plugins & MCP](#plugins--mcp)
18. [Workflows](#workflows)
19. [Multi-Agent Delegation](#multi-agent-delegation)
20. [Autonomy & Security](#autonomy--security)
21. [The Heartbeat](#the-heartbeat)
22. [Undo System](#undo-system)
23. [Settings & Personalization](#settings--personalization)
24. [Keyboard Shortcuts](#keyboard-shortcuts)
25. [Troubleshooting](#troubleshooting)

---

## Getting Started

### First Launch

Run `aivyx` in your terminal. On the first launch, the **Genesis wizard** walks you through setup:

1. **Passphrase** — All your data is encrypted locally. Choose a strong passphrase (8+ characters). You'll enter this each time you launch.
2. **LLM Provider** — Pick your AI backend: Ollama (local, free), OpenAI, Anthropic, or OpenRouter.
3. **Name & Persona** — Give your assistant a name and choose a personality: assistant, coder, researcher, writer, coach, companion, ops, or analyst.
4. **Soul** — Review (and optionally edit) your assistant's personality narrative.
5. **Skills** — Confirm the default skill set or add custom skills.
6. **Schedule** — Set your morning briefing time and background check interval.
7. **Goals** — Accept persona-specific starter goals for the agent to work on.
8. **Email** (optional) — Enter IMAP/SMTP credentials so the assistant can read and send email.
9. **Integrations** (optional) — Connect Calendar (CalDAV), Contacts (CardDAV), Telegram, Matrix, or Signal.
10. **Intelligence** — Toggle heartbeat features: reflection, memory consolidation, mood tracking, and more.

You can re-run setup any time with `aivyx init`.

### Launch Modes

```
aivyx              # Interactive TUI (default)
aivyx chat "..."   # Quick one-shot question from the terminal
aivyx status       # Show recent assistant activity
aivyx config       # View current configuration
```

### Unlocking

Every launch requires your passphrase. To skip the prompt in scripts, set the `AIVYX_PASSPHRASE` environment variable.

---

## The TUI Interface

The TUI has three regions:

```
┌──────────┬───────────────────────────────┐
│ Sidebar  │                               │
│          │         Content Area          │
│ ◆ HOME   │                               │
│ ◇ CHAT   │   (changes per view)         │
│ ⚡ ACTIV  │                               │
│ ◎ GOALS  │                               │
│ ⊕ APPROV │                               │
│ ──────── │                               │
│ ▣ MISSN  │                               │
│ ◉ MEMORY │                               │
│ ◈ AUDIT  │                               │
│ ──���───── │                               │
│ ? HELP   │                               │
│ ⚙ SETTNGS│                               │
│          ├───────────────────────────────┤
│ ⊕ MILO   │         Status Bar           │
│ ASST·TRUST                               │
└──────────┴───────────────────────────��───┘
```

**Navigation:** Press `Tab` to toggle focus between the Sidebar and Content area. Use `Up`/`Down` (or `j`/`k`) to move through sidebar items, `Enter` to select. Press number keys `1`–`9`, `0` to jump directly to a view.

### Views

| # | View | What It Shows |
|---|------|---------------|
| 1 | **Home** | Dashboard with stat cards (goals, missions, approvals, memories), system health indicators, and an activity feed |
| 2 | **Chat** | Conversation with your assistant. Type messages, see streaming responses, manage sessions |
| 3 | **Activity** | Notification log — everything the assistant has done autonomously (heartbeat actions, schedule results, triage decisions). Filter by source with `[`/`]` |
| 4 | **Goals** | All goals with status, priority, and progress. Create, edit, complete, or abandon goals |
| 5 | **Approvals** | Pending approval requests — when the assistant wants to do something that requires your OK. Approve with `a`, deny with `d` |
| 6 | **Missions** | Multi-step background tasks with progress tracking and step details |
| 7 | **Memory** | Browse, search, and manage stored memories with kind badges and access counts |
| 8 | **Audit** | Tamper-evident log of all autonomous actions. Filter by event type with `[`/`]`. Cycle time range |
| 9 | **Help** | This manual, rendered in a scrollable view |
| 0 | **Settings** | Edit all configuration: model, autonomy tier, agent name, soul, skills, persona dimensions, integrations, and schedules |

---

## Chatting with Your Assistant

The Chat view is where most interaction happens. Type your message at the bottom and press `Enter` to send. The assistant streams its response in real-time.

### What You Can Ask

Your assistant can do far more than answer questions. Try asking it to:

- **"Check my email"** — reads your inbox and summarizes what's new
- **"Reply to Sarah's email about the project"** — drafts and sends a reply (with approval if needed)
- **"What's on my calendar today?"** — fetches today's events
- **"Set a reminder for 3pm: call the dentist"** — creates a persistent reminder
- **"Research quantum computing and write a summary"** — searches the web, reads pages, synthesizes a report
- **"Open Firefox"** — launches an application on your desktop
- **"What do we know about Project X?"** — searches memory, knowledge graph, documents, and email
- **"Create a goal: finish the quarterly report by Friday"** — sets a tracked goal with deadline
- **"Run the code-review recipe on this repo"** — kicks off a multi-step mission

### Sessions

You can maintain multiple conversation threads:

- `Ctrl+S` — Open the session switcher (list, create, delete sessions)
- `Ctrl+E` — Export the current conversation as a markdown file
- `Ctrl+P` — Preview the full system prompt the assistant is using

Each session preserves its conversation history. Memory and goals are shared across all sessions.

---

## Memory & Knowledge

Your assistant remembers things across conversations using two complementary systems.

### Episodic Memory

The assistant stores facts, observations, and preferences as searchable memory entries. These persist across sessions and inform future conversations.

You can interact with memory naturally:

- **"Remember that I prefer Python over JavaScript"** — stores a preference
- **"What do you remember about my vacation plans?"** — searches memories
- **"Forget the old office address"** — removes outdated information
- **"What patterns have you noticed?"** — analyzes recurring themes

Behind the scenes, the assistant uses semantic search (via embeddings) to find relevant memories, so it doesn't need exact keyword matches.

### Knowledge Graph

Beyond simple memories, the assistant builds a structured knowledge graph of relationships:

- **(Sarah, works_at, Acme Corp)**
- **(Quarterly Report, deadline_is, March 31)**
- **(Project X, related_to, Budget Review)**

The graph is built automatically from conversations, emails, and documents. You can ask relational questions:

- **"How does Alice connect to Project X?"** — finds paths through the graph
- **"What do we know about Acme Corp?"** — traverses connected entities
- **"Show me knowledge graph stats"** — entity/edge counts and clusters

---

## Goals & Self-Improvement

Goals are persistent objectives that the assistant actively works toward.

### Managing Goals

In the **Goals** view:

| Key | Action |
|-----|--------|
| `n` | Create a new goal |
| `e` | Edit the selected goal |
| `c` | Complete the selected goal |
| `x` | Abandon the selected goal |
| `[`/`]` | Filter: all / active / completed / abandoned |

Or just tell the assistant in Chat:

- **"Set a goal to organize my email by end of week"**
- **"How are my goals going?"**
- **"Mark the research goal as complete"**

### Goal Cascading

Goals can have sub-goals. When a parent goal is completed, its sub-goals automatically cascade. Progress from children rolls up to parents.

### Self-Model

The assistant maintains a model of its own capabilities — what it's good at, where it struggles, and its confidence in different domains. This evolves over time:

- After successful actions, confidence increases
- After failures, confidence decreases and the assistant may ask for approval more often
- Periodic reflection (during heartbeats) updates the self-model

Ask **"What's your self-assessment?"** to see how the assistant views its own capabilities.

---

## Reminders & Schedules

### Reminders

One-time alerts triggered at a specific time:

- **"Remind me at 2pm to call the dentist"**
- **"What reminders do I have?"**
- **"Cancel the dentist reminder"**

Reminders persist across sessions and survive restarts.

### Schedules

Recurring tasks on a cron schedule. These fire automatically in the background:

- **"Check my email every morning at 7 AM"** — creates a cron schedule
- **"Summarize the news every weekday at 9"** — periodic prompt execution
- **"Show my schedules"** — list all active schedules

Schedules can be managed in the **Settings** view (toggle on/off, see next fire time) or via Chat (the assistant can create, edit, and delete schedules).

The difference: reminders are one-time notifications; schedules are recurring tasks that the assistant actively executes.

---

## Missions

Missions are complex, multi-step tasks that run autonomously in the background.

### How They Work

When you give the assistant a complex task, it decomposes it into steps and executes them one by one:

- **"Research AI safety and write a comprehensive report"** — might become 5-6 steps: search the web, read multiple pages, synthesize findings, write draft, review, save file
- **"Set up my project with tests, CI, and documentation"** — decomposed into setup, scaffolding, writing, and verification steps

### Factory Recipes

Pre-built mission templates for common workflows:

- **code-review** — review a codebase with multi-stage analysis
- **documentation** — generate comprehensive docs
- **research** — systematic research with synthesis
- **data-pipeline-qa** — quality assurance for data pipelines
- **incident-response** — structured incident handling
- **onboarding** — new team member onboarding checklist

Ask: **"Run the code-review recipe"** or **"What recipes are available?"**

### Monitoring

The **Missions** view shows all missions with their status, progress, and individual steps. Active missions show a live badge in the sidebar.

---

## Email

Requires: `[email]` section in config.toml (set up during Genesis or via Settings).

Your assistant can:

- **Read your inbox** — "Check my email", "Any new messages?"
- **Read specific emails** — "Read the email from Sarah about the project"
- **Draft and send replies** — "Reply to that email saying we'll meet Thursday"
- **Forward messages** — "Forward this to the team"
- **File receipts** — automatically detects purchase emails and files them

### Approval Workflow

At **Leash** autonomy, the assistant always shows you a draft before sending. At **Trust**, routine replies go automatically while important emails get a draft review. At **Free**, all emails are sent autonomously.

### Email Triage

When enabled (`[triage]` in config), the assistant automatically processes your inbox:

- Classifies emails by category and urgency
- Applies auto-reply rules for matching senders
- Forwards important messages
- Logs all decisions in the Activity view

Ask **"Show triage log"** to see what was processed autonomously.

---

## Calendar

Requires: `[calendar]` section in config.toml (CalDAV).

- **"What's on my calendar today?"** — daily agenda
- **"Do I have anything on Thursday?"** — fetch events for a date range
- **"Am I free at 3pm tomorrow?"** — check for conflicts
- **"Schedule a meeting..."** — the assistant checks conflicts before suggesting times

Automatic reminders are created 15 minutes before each calendar event.

At **Trust** autonomy and above, the assistant proactively connects calendar events to relevant context — if you mention a person, it checks if there's an upcoming meeting with them and offers to prep.

---

## Contacts

Contacts are always available (populated from email interactions even without CardDAV).

- **"Find Sarah's email"** — searches by name, email, or company
- **"List all my contacts"** — browse the address book
- **"Sync contacts"** — pull latest from CardDAV server (if configured)

When you say **"email Sarah"**, the assistant resolves her email address from contacts before composing.

---

## Messaging

### Telegram

Requires: `[telegram]` section in config.toml.

- **"Send a Telegram message to chat 12345: meeting at 3"**
- **"Read recent Telegram messages from that chat"**

### Matrix

Requires: `[matrix]` section in config.toml.

- **"Send a message to the project room on Matrix"**
- **"Read the latest messages from !room:example.com"**

### Signal

Requires: `[signal]` section in config.toml (uses signal-cli).

- **"Message +1234567890 on Signal: I'll be late"**
- **"Check for new Signal messages"**

### SMS

Requires: `[sms]` section in config.toml (Twilio or Vonage gateway).

- **"Text +1234567890: Running 10 minutes late"**

---

## Document Vault

Requires: `[vault]` section in config.toml.

Your vault is a local directory of markdown, text, and PDF files that the assistant indexes for semantic search.

- **"Search my documents for quarterly revenue"** — semantic search (meaning, not just keywords)
- **"Read the project-plan.md file"** — full document content
- **"What documents do I have?"** — browse the vault
- **"Re-index my vault"** — after adding new files

### Document Creation

With desktop interaction enabled, the assistant can create documents:

- **"Create a PDF report about the meeting"** — generates formatted PDF
- **"Make a spreadsheet of these expenses"** — creates CSV/XLSX/ODS
- **"Convert my notes to HTML"** — format conversion via pandoc

---

## Finance Tracking

Requires: `[finance]` section in config.toml.

- **"Add a $45 dining expense"** — records a transaction
- **"How much did I spend on groceries this month?"** — queries by category
- **"What's my budget looking like?"** — spending vs budget limits
- **"Set my dining budget to $500"** — category limits
- **"Mark the electricity bill as paid"** — tracks bill status

The assistant automatically detects bills and expenses from emails when email triage is active.

---

## Web Research

Always available (no extra configuration needed).

- **"Search the web for latest AI news"** — returns titles, URLs, and snippets
- **"Read this page: https://example.com/article"** — fetches full page content
- **"Research topic X and summarize"** — multi-step: search, read several pages, synthesize

For complex research, the assistant may launch a mission to handle it systematically.

---

## Desktop Interaction

Requires: `[desktop]` section in config.toml.

### Application Control

- **"Open Firefox"** — launches any application
- **"Open this PDF"** — opens a file with its default handler
- **"Open the Downloads folder"** — shows it in the file manager

### Deep Interaction (requires `[desktop.interaction]`)

With deep interaction enabled, the assistant can see, click, and type into any application:

**UI Automation:**
- Inspect application interfaces (buttons, labels, fields)
- Click, right-click, double-click, middle-click elements
- Type text into fields, read text from elements
- Scroll, hover, drag-and-drop
- Select dropdown options, clear fields
- Multi-select with Ctrl+click
- Take window screenshots
- Extract text via OCR

**Browser Automation (Chrome DevTools Protocol):**
- Navigate to URLs, click DOM elements, type into forms
- Read page content, take screenshots
- Search text on pages, save as PDF
- Run JavaScript, manage tabs
- Requires Chrome running with `--remote-debugging-port=9222`

**System Controls:**
- Manage windows (minimize, maximize, move, resize)
- Control audio volume and display brightness
- Read desktop notifications
- Control media playback (play/pause/next/previous)

---

## Developer Tools

Requires: `[devtools]` section in config.toml.

### Git

- **"What's the git status?"** — branch, staged/unstaged changes
- **"Show recent commits"** — git log with filtering
- **"What changed in the last commit?"** — git diff
- **"List branches"** — branches with last commit info

### GitHub/Gitea Integration (requires `forge_type` in devtools)

- **"Show open issues"** — list and filter issues
- **"What's in PR #42?"** — view PR diffs and metadata
- **"Comment on that PR about the security concern"** — post review comments
- **"Check CI status"** — pipeline status and build logs
- **"Create an issue: fix the login bug"** — create issues with labels

---

## Plugins & MCP

The assistant supports [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) plugins to extend its capabilities.

- **"What plugins do I have?"** — list installed plugins
- **"Search for a weather plugin"** — browse the MCP registry
- **"Install the weather plugin"** — adds new capabilities
- **"Disable the news plugin"** — toggle without removing
- **"What prompts are available?"** — discover plugin-provided prompt templates

Plugins are configured in `config.toml` under `[[mcp_servers]]` or installed dynamically via the install command. Plugin tools appear alongside built-in tools — the assistant uses them identically.

---

## Workflows

Pre-built multi-step workflow templates for common tasks:

| Workflow | Description | Trigger |
|----------|-------------|---------|
| morning-briefing | Daily summary of calendar, emails, reminders | Daily 7am |
| inbox-zero | Triage unread emails, draft replies | On demand |
| expense-report | Process receipts, record transactions | On demand |
| bill-pay-reminder | Check upcoming bills, set reminders | Monday 9am |
| weekly-review | Summarize the week, plan next week | Friday 5pm |
| research-digest | Search web, synthesize findings | On demand |
| code-review-checklist | Fetch PR diff, analyze, comment | On demand |
| meeting-prep | Gather context, draft agenda | On demand |
| monthly-budget-review | Analyze spending vs budget | 1st of month |
| project-status-report | Git log, issues, CI summary | Monday 9am |

**Commands:**
- **"Run the morning briefing workflow"**
- **"What workflows are available?"**
- **"Create a custom workflow for..."**
- **"Check the status of the running workflow"**

---

## Multi-Agent Delegation

For truly complex tasks, the assistant can delegate to a team of 9 specialist sub-agents (the Nonagon):

| Specialist | Expertise |
|------------|-----------|
| Coordinator | Orchestrates multi-agent workflows |
| Researcher | Gathers information, evaluates sources |
| Analyst | Data analysis, pattern recognition |
| Coder | Writes and tests code, manages git |
| Reviewer | Code review, security audit |
| Writer | Documentation, tutorials, technical writing |
| Planner | Task decomposition, complexity estimation |
| Guardian | Security monitoring, capability validation |
| Executor | Shell commands, file management, deployment |

Ask: **"Delegate this to the team: research and write a report on..."** — the coordinator receives your task and orchestrates the specialists.

---

## Autonomy & Security

### Autonomy Tiers

Your assistant operates at one of four independence levels:

| Tier | Behavior |
|------|----------|
| **Locked** | Read-only. Cannot execute tools. Observation and advice only. |
| **Leash** | Proposes actions but requires your approval before executing. |
| **Trust** | Acts autonomously for routine tasks. Confirms high-stakes actions. |
| **Free** | Full autonomy. Acts decisively without approval. |

Change the tier in Settings or `config.toml`. Per-domain overrides let you fine-tune: "trust email reads, require approval for shell commands, lock finance tools."

### Confidence Escalation

When the assistant's learned confidence in a domain drops below a threshold (default 30%), it automatically downgrades from Trust/Free to Leash — asking for your approval until it builds confidence back up.

### Encryption

All data is encrypted locally with ChaCha20-Poly1305. Your passphrase derives the master key via Argon2id. Conversations, memory, goals, credentials, and audit logs never leave your machine.

### Audit Trail

Every autonomous action is logged in a tamper-evident HMAC-chained audit log. View it in the **Audit** view, filter by event type, and verify the integrity chain.

### Key Rotation

Rotate your master encryption key with `aivyx rotate-key`. This atomically re-encrypts all stored data with a new key derived from your new passphrase.

---

## The Heartbeat

The heartbeat is your assistant's autonomous reasoning cycle — it runs periodically (default: every 30 minutes) and decides what, if anything, needs attention.

### What It Does

Each heartbeat:
1. Gathers context from all sources (goals, reminders, email, calendar, finance, contacts, self-model)
2. Scores items by urgency (0.0–1.0)
3. Cross-references sources for insights
4. Decides on actions: notify, suggest, update goals, reflect, consolidate memory, or do nothing

**Smart skip:** When nothing has changed since the last beat, the LLM call is skipped entirely — zero token cost during quiet periods.

### Configurable Features

Enable or disable in Settings or `[heartbeat]` in config.toml:

| Feature | What It Does |
|---------|--------------|
| Reflection | Updates the self-model based on recent outcomes |
| Memory consolidation | Merges and organizes memories |
| Suggestions | Proactive insights from cross-source reasoning |
| Failure analysis | Learns from tool failures, adjusts confidence |
| Knowledge extraction | Stores facts from interactions as memories |
| Plan review | Organizes goals into time horizons (today/week/month/quarter) |
| Strategy review | Weekly deep review of all goal progress |
| Mood tracking | Adapts communication tone to detected user mood |
| Encouragement | Celebrates completed goals and streaks |
| Milestone tracking | Notes goal anniversaries (1 week, 1 month, etc.) |

### Notification Pacing

When enabled, the assistant throttles notifications to avoid overwhelming you:
- Urgent notifications always come through
- Quiet hours block non-urgent notifications
- Rate limiting defers overflow beyond the hourly cap
- Mood-aware gating pauses non-critical notifications when you seem frustrated

---

## Undo System

Before performing destructive actions, the assistant saves recovery points:

- **"Undo that"** — reverses the last destructive action
- **"What can I undo?"** — shows the undo history

Supported undo types:
- **File overwrites** — restores the original content
- **Sent emails** — can't recall, but provides follow-up instructions
- **Reminders** — cancels created reminders
- **Financial transactions** — voids recorded transactions

Recovery points expire after 24 hours.

---

## Settings & Personalization

The **Settings** view lets you edit everything without leaving the TUI:

### Settings Cards

| Card | What You Can Change |
|------|-------------------|
| Model | LLM provider, model name, base URL |
| Autonomy | Default tier and per-scope overrides |
| Agent | Name, persona preset, greeting |
| Soul | Edit the agent's personality narrative |
| Skills | Add, remove, reorder skills |
| Persona | Warmth, formality, verbosity, humor, confidence, curiosity |
| Integrations | Set up email, calendar, contacts, Telegram, Matrix, Signal, dev tools |
| Schedules | Toggle schedules on/off, see next fire times |

### Persona Presets

Each preset comes with tuned personality dimensions, starter goals, and recommended schedules:

| Persona | Personality | Best For |
|---------|-------------|----------|
| **Assistant** | Warm, proactive, organized | General-purpose daily management |
| **Coder** | Precise, methodical, concise | Software development workflow |
| **Researcher** | Curious, thorough, systematic | Deep research and knowledge work |
| **Writer** | Creative, detail-oriented, stylish | Writing, editing, content creation |
| **Coach** | Motivating, empathetic, structured | Goal tracking, habit building |
| **Companion** | Warm, conversational, supportive | Casual interaction, emotional support |
| **Ops** | Efficient, alert, data-driven | Infrastructure monitoring, CI/CD |
| **Analyst** | Quantitative, structured, insightful | Data analysis, reporting |

---

## Keyboard Shortcuts

### Global

| Key | Action |
|-----|--------|
| `Tab` | Toggle focus: Sidebar ↔ Content |
| `Left` | Focus sidebar |
| `Right` | Focus content |
| `1`–`9`, `0` | Jump to view (1=Home, 2=Chat, ..., 9=Help, 0=Settings) |
| `Up`/`Down` or `j`/`k` | Navigate items |
| `Enter` | Select/activate |
| `[`/`]` | Cycle filter (Goals, Audit, Activity) |
| `Esc` | Back to sidebar / back to Home |
| `q` | Quit (except in Chat) |
| `Ctrl+C` | Quit (always) |

### Chat View

| Key | Action |
|-----|--------|
| Type | Enter message text |
| `Enter` | Send message |
| `Up`/`Down` | Scroll conversation |
| `PageUp`/`PageDown` | Fast scroll |
| `Ctrl+S` | Session switcher |
| `Ctrl+P` | Preview system prompt |
| `Ctrl+E` | Export conversation as markdown |

### Goals View

| Key | Action |
|-----|--------|
| `n` | Create new goal |
| `e` | Edit selected goal |
| `c` | Complete selected goal |
| `x` | Abandon selected goal |

### Approvals View

| Key | Action |
|-----|--------|
| `a` | Approve selected item |
| `d` | Deny selected item |

---

## Troubleshooting

### "The assistant says it can't do X"

The assistant has many tools — sometimes it doesn't realize it has a capability. Try being more specific:

- Instead of "show me my files" → "use list_directory to show /home/user/Documents"
- Instead of "check the build" → "use ci_status to check the latest CI run"

### LLM is not responding

Check the Health Bar on the Home view. If LLM shows "degraded":

- **Ollama**: Is the Ollama server running? (`ollama serve`)
- **Cloud providers**: Is your API key valid? Check with `aivyx config`
- **Network**: Can you reach the API endpoint?

### Email not working

- Verify IMAP/SMTP credentials in Settings → Integrations
- Gmail requires an App Password (not your regular password)
- Check the Health Bar — Email status shows connection state

### Desktop tools not working

- **AT-SPI2**: Ensure accessibility is enabled in your desktop environment
- **Browser automation**: Chrome must be running with `--remote-debugging-port=9222`
- **ydotool**: Requires the ydotool daemon running (`ydotoold`)

### "Tool call failed"

Check the **Audit** view for details. Common causes:

- **Permission denied**: The tool requires a higher autonomy tier or capability scope
- **Not found**: A file path or element selector was wrong
- **Timeout**: A network request or subprocess timed out

The assistant has built-in error recovery — it will diagnose transient failures and retry, and suggest fixes for permanent ones.

### Resetting

- **Re-run setup**: `aivyx init` (preserves existing data)
- **Rotate key**: `aivyx rotate-key` (re-encrypts all data with a new passphrase)
- **Fresh start**: Delete `~/.aivyx/` and run `aivyx` again

---

*Aivyx Personal Assistant — Your data stays yours.*
