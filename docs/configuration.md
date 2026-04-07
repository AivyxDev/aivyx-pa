# Configuration Reference

All configuration lives in `~/.aivyx/config.toml`. This file is created by `aivyx init` and can be edited manually.

## Full Example

```toml
# Aivyx Personal Assistant Configuration

[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "glm-4.7-flash:latest"

[autonomy]
default_tier = "Trust"

# [[autonomy.scope_overrides]]
# scope = "shell"
# tier = "Leash"

# [[autonomy.scope_overrides]]
# scope = "custom:finance"
# tier = "Locked"

# escalation_confidence_threshold = 0.3  # Downgrade Trust/Free → Leash when domain confidence < 30%

[embedding]
type = "Ollama"
model = "nomic-embed-text"
base_url = "http://localhost:11434"

[agent]
name = "Milo"
persona = "assistant"
skills = ["email management", "scheduling", "research", "writing", "task tracking"]
soul = """
You approach your user's life the way a good chief of staff approaches
their executive — anticipate needs, handle details, surface what matters.
You are proactive, organized, and genuinely invested in making their day
run smoothly."""
# max_tokens = 4096
# greeting = "Hey boss!"

[loop]
check_interval_minutes = 15
morning_briefing = true
briefing_hour = 8

[heartbeat]
enabled = true
interval_minutes = 30
can_reflect = true
can_consolidate_memory = true
can_suggest = true
can_analyze_failures = false
can_extract_knowledge = false
can_plan_review = false
can_strategy_review = false
can_track_mood = true
can_encourage = true
can_track_milestones = false
notification_pacing = true
max_notifications_per_hour = 5
check_calendar = true
check_finance = true
check_contacts = true

[persona]
warmth = 0.7
formality = 0.4
verbosity = 0.5
humor = 0.3
confidence = 0.6
curiosity = 0.7

[[initial_goals]]
description = "Learn the user's daily routine and preferences"
success_criteria = "Can predict user needs before they ask"
priority = "high"

[[initial_goals]]
description = "Organize inbox into actionable categories"
success_criteria = "Inbox processed within 5 minutes of check"
priority = "medium"

[email]
imap_host = "imap.gmail.com"
imap_port = 993
smtp_host = "smtp.gmail.com"
smtp_port = 587
address = "you@gmail.com"
username = "you@gmail.com"

[calendar]
url = "https://caldav.example.com/dav/calendars/user/default/"
username = "you@example.com"
# Password stored in keystore as CALDAV_PASSWORD

[contacts]
url = "https://carddav.example.com/dav/addressbooks/user/default/"
username = "you@example.com"
# Password stored in keystore as CARDDAV_PASSWORD

[vault]
path = "~/Documents/vault"
extensions = ["md", "txt", "pdf"]

[finance]
currency = "USD"
receipt_folder = "receipts"

[triage]
enabled = true
max_per_tick = 10
can_auto_reply = false
can_forward = false
# forward_to = "you@example.com"
# ignore_senders = ["noreply@", "no-reply@"]
# categories = ["urgent", "personal", "receipt", "newsletter", "spam"]

# [[triage.auto_reply_rules]]
# sender_contains = "support@vendor.com"
# reply_body = "Thank you, this has been received and will be reviewed shortly."

# [signal]
# signal_cli_path = "/usr/local/bin/signal-cli"
# account = "+1234567890"

# [sms]
# gateway = "twilio"
# from_number = "+1234567890"

# [devtools]
# forge_type = "github"
# api_url = "https://api.github.com"
# default_owner = "myorg"

# [style]
# tone = "professional"
# detail_level = "concise"
# active_hours = "09:00-18:00"
# preferences = ["no emojis", "prefer code examples"]

# [resilience]
# circuit_breaker = true
# failure_threshold = 3
# fallback_providers = ["llama-fallback"]
# cache_enabled = false

# [consolidation]
# merge_threshold = 0.85
# stale_days = 90
# mine_patterns = true

# [missions]
# default_mode = "sequential"
# recipe_dir = "~/.aivyx/recipes"
# experiment_tracking = false

# [[schedules]]
# name = "daily-check"
# cron = "0 9 * * *"
# agent = "assistant"
# prompt = "Check my email and summarize anything important"
# enabled = true
# notify = true

# [desktop]
# allowed_apps = ["firefox", "gimp", "libreoffice"]
# denied_apps = ["custom-dangerous-app"]
# clipboard = true
# windows = true
# notifications = true

# [desktop.interaction]
# enabled = true               # master switch (default: false)
# [desktop.interaction.accessibility]
# enabled = true               # AT-SPI2 (requires --features accessibility)
# [desktop.interaction.browser]
# enabled = true               # CDP (requires --features browser-automation)
# debug_port = 9222
# [desktop.interaction.media]
# enabled = true               # D-Bus MPRIS (requires --features media-control)
# [desktop.interaction.input]
# enabled = true               # ydotool (always available)
```

## Sections

### [provider] (Required)

The LLM provider for chat and reasoning.

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | Provider type (see below) |
| `model` | string | yes | Model identifier |
| `base_url` | string | depends | API base URL (required for Ollama, OpenAICompatible) |
| `api_key_ref` | string | depends | Key name in encrypted store (required for cloud providers) |

**Provider types:**

| Type | base_url | api_key_ref | Notes |
|---|---|---|---|
| `Ollama` | `http://localhost:11434` | not needed | Local inference |
| `OpenAI` | not needed | `OPENAI_API_KEY` | OpenAI API |
| `Claude` | not needed | `ANTHROPIC_API_KEY` | Anthropic API |
| `OpenAICompatible` | required | varies | OpenRouter, Together, etc. |

**Suggested models:**

| Provider | Models |
|---|---|
| Ollama | `qwen3:14b`, `qwen2.5-coder:14b`, `llama3.1:8b`, `mistral:7b` |
| OpenAI | `gpt-4o`, `gpt-4o-mini`, `o1-mini` |
| Claude | `claude-sonnet-4-20250514`, `claude-haiku-4-5-20251001` |
| OpenRouter | `anthropic/claude-sonnet-4-20250514`, `openai/gpt-4o`, `google/gemini-2.0-flash-001` |

### [autonomy] (Optional)

Controls what the assistant can do without asking.

| Field | Type | Default | Description |
|---|---|---|---|
| `default_tier` | string | `"Trust"` | Default autonomy tier for all tools |

**Autonomy tiers:**

| Tier | Behavior |
|---|---|
| `Locked` | Read-only. Cannot execute any tools. |
| `Leash` | All tool calls require explicit user approval before execution. |
| `Trust` | Autonomous within defined capability boundaries. Most tools execute freely; shell commands require Shell capability scope. |
| `Free` | Full autonomy. All tools execute without approval. Use with caution. |

#### [[autonomy.scope_overrides]] (Optional, repeatable)

Per-scope autonomy tier overrides. When a tool declares a `required_scope` matching a scope listed here, that scope's tier is used instead of `default_tier`. This allows fine-grained control like "trust email reads, require approval for shell, lock finance."

| Field | Type | Required | Description |
|---|---|---|---|
| `scope` | string | yes | Scope discriminant (see table below) |
| `tier` | string | yes | Autonomy tier for tools requiring this scope |

**Scope discriminants:**

| Discriminant | Matches |
|---|---|
| `filesystem` | File read/write operations |
| `shell` | Shell command execution |
| `email` | Email send/receive operations |
| `calendar` | Calendar read/write operations |
| `network` | HTTP requests, web fetches |
| `custom:<name>` | Custom scopes (e.g., `custom:finance`, `custom:memory`) |

**Tier resolution order:** When a tool is called, the agent resolves its effective tier:
1. Look up the tool's `required_scope` discriminant
2. Check `scope_overrides` for a matching entry
3. If found → use the override tier; if not → use `default_tier`
4. Tools with no `required_scope` always use `default_tier`
5. **Confidence escalation:** If the resolved tier is Trust or Free, and the agent's domain confidence for that scope is below `escalation_confidence_threshold`, the tier is downgraded to Leash

#### escalation_confidence_threshold (Optional)

| Field | Type | Default | Description |
|---|---|---|---|
| `escalation_confidence_threshold` | float | `0.3` | Confidence threshold for tier escalation. When the agent's learned confidence in a domain is below this value, Trust/Free tiers downgrade to Leash (prompting for approval). Set to `0.0` to disable. |

Domain confidence is updated by the agent's self-model during reflection. Low confidence means the agent has had failures or limited experience in that domain, so it asks for human guidance. A `ConfidenceEscalation` audit event is emitted each time this occurs.

```toml
[autonomy]
default_tier = "Trust"
escalation_confidence_threshold = 0.3

[[autonomy.scope_overrides]]
scope = "shell"
tier = "Leash"

[[autonomy.scope_overrides]]
scope = "email"
tier = "Trust"

[[autonomy.scope_overrides]]
scope = "custom:finance"
tier = "Locked"
```

### [agent] (Optional)

Agent identity and personality configuration.

| Field | Type | Default | Description |
|---|---|---|---|
| `name` | string | `"assistant"` | How the assistant introduces itself |
| `persona` | string | `"assistant"` | Persona preset (determines personality dimensions) |
| `soul` | string | none | Custom system prompt override (replaces auto-generated soul). Supports triple-quoted TOML strings for multi-line narratives |
| `max_tokens` | integer | `4096` | Maximum tokens for LLM responses |
| `greeting` | string | none | Custom greeting template (supports `{name}` placeholder) |
| `skills` | string[] | `[]` | Declared skills/competencies. Injected into system prompt and used for tool discovery matching |

**Persona presets:** `assistant`, `coder`, `researcher`, `writer`, `coach`, `companion`, `ops`, `analyst`

Each persona generates a different system prompt with appropriate personality traits. When `soul` is set, it replaces the auto-generated prompt but persona behavioral guidelines are still appended. The genesis wizard (`aivyx init`) provides per-persona defaults for soul, skills, goals, schedules, and heartbeat flags.

The agent's name is always injected into the system prompt: `"Your name is {name}. Always introduce yourself by this name when asked."`

```toml
[agent]
name = "Adrian"
persona = "coder"
skills = ["code review", "debugging", "architecture", "testing", "git workflow"]
soul = """
You approach code the way a surgeon approaches an operation —
precise, methodical, and always aware of the bigger picture.
You prefer clarity over cleverness and tests over trust."""
```

### [loop] (Optional)

Background agent loop timing configuration.

| Field | Type | Default | Description |
|---|---|---|---|
| `check_interval_minutes` | integer | `15` | How often to run the full check cycle |
| `morning_briefing` | boolean | `true` | Whether to run a morning briefing |
| `briefing_hour` | integer | `8` | Morning briefing hour (0-23, local time). Values > 23 are clamped to 23 with a warning. |

### [heartbeat] (Optional)

LLM-driven autonomous reasoning that runs periodically inside the agent loop. The heartbeat gathers context from all available sources (goals, reminders, email, self-model, schedules), presents it to the LLM, and lets it decide what actions to take. When nothing has changed since the last beat, the LLM call is skipped entirely (zero token cost on quiet periods).

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Whether the heartbeat is active |
| `interval_minutes` | integer | `30` | Minutes between heartbeat ticks (should be >= `check_interval_minutes`) |
| `can_reflect` | boolean | `false` | Allow the heartbeat to update the self-model via reflection |
| `can_consolidate_memory` | boolean | `false` | Allow the heartbeat to trigger memory consolidation |
| `can_suggest` | boolean | `true` | Allow the heartbeat to emit proactive suggestions |
| `can_analyze_failures` | boolean | `false` | Allow the heartbeat to analyze tool failures and learn from them |
| `can_extract_knowledge` | boolean | `false` | Allow the heartbeat to extract and store learned facts |
| `can_plan_review` | boolean | `false` | Allow multi-horizon goal planning (assigns `horizon:*` tags and deadlines) |
| `can_strategy_review` | boolean | `false` | Allow weekly strategic review of all goal progress and patterns |
| `can_track_mood` | boolean | `false` | Allow mood-aware context injection from interaction signals |
| `can_encourage` | boolean | `false` | Allow proactive celebration of completed goals and streaks |
| `can_track_milestones` | boolean | `false` | Allow anniversary detection for goals (1w, 1m, 3m, 6m, 1y) |
| `notification_pacing` | boolean | `false` | Enable Rust-level notification throttling (quiet hours, rate limit, mood gating) |
| `max_notifications_per_hour` | integer | `5` | Maximum non-urgent notifications per hour (requires `notification_pacing = true`) |
| `token_budget_daily` | integer | `0` | Daily token budget for heartbeat LLM calls (0 = unlimited). Surfaces in context when > 50% used |
| `check_calendar` | boolean | `true` | Include calendar context in heartbeat reasoning |
| `check_finance` | boolean | `true` | Include finance context (bills, budgets) in heartbeat reasoning |
| `check_contacts` | boolean | `true` | Include contact count in heartbeat context |
| `audit_retention_days` | integer | `90` | Days to keep audit log entries before automatic pruning |

**Conservative defaults:** The core learning actions (`can_reflect`, `can_consolidate_memory`, `can_analyze_failures`, `can_extract_knowledge`) and the Phase 6 intelligence actions (`can_plan_review`, `can_strategy_review`, `can_track_mood`, `can_encourage`, `can_track_milestones`, `notification_pacing`) all default to `false` — the heartbeat observes and notifies by default, but does not modify state unless explicitly opted in. The `can_suggest`, `check_calendar`, `check_finance`, and `check_contacts` flags default to `true` since they are read-only operations.

**Context fusion:** When two or more data sources are active (email, calendar, contacts, finance, vault), the heartbeat engages **cross-source reasoning** — correlating information across silos to surface insights no single source could reveal (e.g., "Sarah emailed about the deadline AND you have a meeting with her tomorrow").

**Priority scoring:** Before the LLM sees the context, items are scored by temporal urgency (0.0–1.0) using heuristics for due dates, staleness, and threshold breaches. High-priority items appear first in the prompt.

**Heartbeat actions (15):** Based on its gathered context, the LLM can:
- **Notify** — surface something important to the user
- **Suggest** — proactive insight from cross-source reasoning, tagged with contributing sources (requires `can_suggest = true`)
- **SetGoal / UpdateGoal** — create or progress goals (always enabled)
- **Reflect** — update the self-model (requires `can_reflect = true`)
- **ConsolidateMemory** — recommend memory consolidation (requires `can_consolidate_memory = true`)
- **AnalyzeFailure** — learn from tool failures, decrease domain confidence (requires `can_analyze_failures = true`)
- **ExtractKnowledge** — store learned facts as memories (requires `can_extract_knowledge = true`)
- **PruneAudit** — remove old audit entries (requires `can_prune_audit = true`)
- **Backup** — archive data directory (requires `can_backup = true`, configured via `[backup]`)
- **PlanReview** — organize goals into time horizons, assign `horizon:today/week/month/quarter` tags and deadlines (requires `can_plan_review = true`)
- **StrategyReview** — weekly deep review of all goal progress, pattern detection, domain confidence updates (requires `can_strategy_review = true`, triggered by `strategy-review` workflow cron)
- **TrackMood** — acknowledge detected mood signal (requires `can_track_mood = true`)
- **Encourage** — celebrate completed goals and streaks with persona-calibrated messages (requires `can_encourage = true`)
- **NoAction** — nothing to do (most common during quiet periods)

**Communication pacing:** When `notification_pacing = true`, the `pacing` module gates notification delivery at the Rust level before they reach the user. Rules in priority order: (1) Urgent always sends, (2) quiet hours block non-urgent, (3) hourly rate limit defers overflow, (4) frustrated mood blocks all except Urgent + ActionTaken, (5) active engagement defers Info. Deferred notifications are logged but dropped. This is a hard programmatic gate; mood-based tone adaptation is a separate soft signal via LLM context injection.

**Mood awareness:** When `can_track_mood = true`, the heartbeat estimates user mood from `InteractionSignals` (short message streaks, negative keywords, idle time, message length) and injects it as a context section. The LLM naturally adapts tone without an extra API call. Mood signals: `Neutral`, `Focused`, `Frustrated`, `Disengaged`.

**Milestone tracking:** When `can_track_milestones = true`, `check_milestones()` scans goal `created_at` dates for anniversaries at 1 week, 1 month, 3 months, 6 months, and 1 year thresholds (±1 day tolerance). Detected milestones are injected as heartbeat context for the LLM to generate celebration messages via existing Notify/Suggest actions.

```toml
[heartbeat]
enabled = true
interval_minutes = 30
can_reflect = true
can_consolidate_memory = false
can_suggest = true
can_plan_review = true
can_strategy_review = true
can_track_mood = true
can_encourage = true
can_track_milestones = true
notification_pacing = true
max_notifications_per_hour = 5
token_budget_daily = 100000
check_calendar = true
check_finance = true
check_contacts = true
```

### [[schedules]] (Optional, repeatable)

Cron-based scheduled tasks. Each entry is a recurring prompt that fires on a cron schedule.

| Field | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Human-readable schedule name |
| `cron` | string | yes | Cron expression (5-field: min hour dom month dow) |
| `agent` | string | yes | Agent profile to use |
| `prompt` | string | yes | The prompt to send to the agent when the schedule fires |
| `enabled` | boolean | yes | Whether the schedule is active |
| `notify` | boolean | no | Whether to push a notification to the TUI |

### [[initial_goals]] (Optional, repeatable)

Goals seeded into the Brain on first launch. The genesis wizard populates these from per-persona defaults.

| Field | Type | Required | Description |
|---|---|---|---|
| `description` | string | yes | What the agent should work toward |
| `success_criteria` | string | no | How to know the goal is complete |
| `priority` | string | no | Priority level: `"high"`, `"medium"`, or `"low"` (default: `"medium"`) |

```toml
[[initial_goals]]
description = "Learn the user's daily routine and preferences"
success_criteria = "Can predict user needs before they ask"
priority = "high"

[[initial_goals]]
description = "Organize inbox into actionable categories"
success_criteria = "Inbox processed within 5 minutes of check"
priority = "medium"
```

### [persona] (Optional)

Numeric personality dimension overrides. When present, these override the preset dimensions from `agent.persona`. The genesis wizard generates these from per-persona tuning values.

| Field | Type | Default | Description |
|---|---|---|---|
| `formality` | float | `0.5` | 0.0 = casual, 1.0 = formal |
| `verbosity` | float | `0.5` | 0.0 = terse, 1.0 = very detailed |
| `warmth` | float | `0.5` | 0.0 = neutral/professional, 1.0 = warm & friendly |
| `humor` | float | `0.2` | 0.0 = no humor, 1.0 = frequently humorous |
| `confidence` | float | `0.8` | 0.0 = hedging, 1.0 = assertive |
| `curiosity` | float | `0.5` | 0.0 = just answers, 1.0 = probes & explores |
| `tone` | string | none | Tone description, e.g. "precise and minimal" |
| `uses_emoji` | boolean | `false` | Whether to use emoji in responses |
| `uses_analogies` | boolean | `true` | Whether to use analogies in explanations |

```toml
[persona]
warmth = 0.4
formality = 0.3
verbosity = 0.4
humor = 0.2
confidence = 0.9
curiosity = 0.5
```

### [embedding] (Optional)

Embedding provider for the memory system's semantic search.

| Field | Type | Required | Description |
|---|---|---|---|
| `type` | string | yes | Same provider types as `[provider]` |
| `model` | string | yes | Embedding model name |
| `base_url` | string | depends | Same rules as `[provider]` |
| `api_key_ref` | string | depends | Same rules as `[provider]` |

If this section is omitted, the memory system starts without embedding-based recall. Memory still stores facts but cannot do semantic similarity search.

### [email] (Optional)

IMAP + SMTP configuration for email actions.

| Field | Type | Default | Description |
|---|---|---|---|
| `imap_host` | string | (required) | IMAP server hostname |
| `imap_port` | integer | `993` | IMAP port (TLS) |
| `smtp_host` | string | (required) | SMTP server hostname |
| `smtp_port` | integer | `587` | SMTP port (STARTTLS) |
| `address` | string | (required) | Your email address (used as From:) |
| `username` | string | same as `address` | Login username (if different from address) |

**The email password is NOT stored in config.toml.** It is stored encrypted in the keystore (`~/.aivyx/store.redb`) under the key `EMAIL_PASSWORD`. Set it during `aivyx init` or manually via the encrypted store API.

**Common provider settings:**

| Provider | IMAP Host | SMTP Host | Notes |
|---|---|---|---|
| Gmail | `imap.gmail.com:993` | `smtp.gmail.com:587` | Requires app password (not your Google password) |
| Outlook | `outlook.office365.com:993` | `smtp.office365.com:587` | |
| ProtonMail | `127.0.0.1:1143` | `127.0.0.1:1025` | Via ProtonMail Bridge |
| Fastmail | `imap.fastmail.com:993` | `smtp.fastmail.com:587` | |
| Hostinger | `imap.hostinger.com:993` | `smtp.hostinger.com:587` | Standard email credentials |

### [calendar] (Optional)

CalDAV calendar integration for agenda, event fetching, and conflict detection.

| Field | Type | Required | Description |
|---|---|---|---|
| `url` | string | yes | CalDAV calendar URL (full path to the calendar resource) |
| `username` | string | yes | CalDAV username |

**The CalDAV password is stored in the encrypted keystore** under `CALDAV_PASSWORD`.

```toml
[calendar]
url = "https://caldav.example.com/dav/calendars/user/default/"
username = "you@example.com"
```

### [contacts] (Optional)

CardDAV contact sync + local encrypted contact store.

| Field | Type | Required | Description |
|---|---|---|---|
| `url` | string | yes | CardDAV addressbook URL |
| `username` | string | yes | CardDAV username |

**The CardDAV password is stored in the encrypted keystore** under `CARDDAV_PASSWORD`.

Even without CardDAV, the `search_contacts` and `list_contacts` tools work against the local encrypted store. The `sync_contacts` tool is only available when CardDAV is configured.

```toml
[contacts]
url = "https://carddav.example.com/dav/addressbooks/user/default/"
username = "you@example.com"
```

### [vault] (Optional)

Local document vault for semantic search and document management.

| Field | Type | Default | Description |
|---|---|---|---|
| `path` | string | (required) | Path to the vault directory |
| `extensions` | string[] | `["md", "txt", "pdf"]` | File extensions to index |

Documents are indexed using the configured embedding provider. The vault encryption key is derived via `HKDF("vault")`.

```toml
[vault]
path = "~/Documents/vault"
extensions = ["md", "txt", "pdf"]
```

### [finance] (Optional)

Personal finance tracking — transactions, budgets, bills, and receipt filing.

| Field | Type | Default | Description |
|---|---|---|---|
| `currency` | string | `"USD"` | Display currency symbol |
| `receipt_folder` | string | `"receipts"` | Subfolder in vault for filed receipts |

Financial data is encrypted under the `HKDF("finance")` domain key. The `file_receipt` tool additionally requires both email and vault to be configured.

```toml
[finance]
currency = "USD"
receipt_folder = "receipts"
```

### [triage] (Optional)

Autonomous email triage — processes the inbox each loop tick using rule-based matching and LLM classification.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `false` | Whether autonomous triage is active |
| `max_per_tick` | integer | `10` | Maximum emails to process per tick |
| `can_auto_reply` | boolean | `false` | Allow sending auto-replies to matched rules |
| `can_forward` | boolean | `false` | Allow forwarding urgent emails |
| `forward_to` | string | none | Email address to forward urgent emails to |
| `ignore_senders` | string[] | `[]` | Sender patterns to silently ignore (e.g., `"noreply@"`) |
| `categories` | string[] | `["urgent", "personal", "receipt", "newsletter", "spam"]` | Categories for LLM classification |

**Two-speed processing:**
1. **Rule-based fast path** — known patterns (ignore senders, auto-reply rules) are handled instantly with zero LLM cost
2. **LLM classification** — remaining emails are batched into a single LLM call for categorization and urgency assessment

**Permission layering:** Classification is always safe (read-only). Auto-reply requires `can_auto_reply = true`. Forwarding requires `can_forward = true` and `forward_to` to be set. All actions are logged to the encrypted store.

```toml
[triage]
enabled = true
max_per_tick = 10
can_auto_reply = true
can_forward = true
forward_to = "you@example.com"
ignore_senders = ["noreply@", "no-reply@", "mailer-daemon@"]
categories = ["urgent", "personal", "receipt", "newsletter", "spam"]

[[triage.auto_reply_rules]]
sender_contains = "support@vendor.com"
reply_body = "Thank you, this has been received and will be reviewed shortly."

[[triage.auto_reply_rules]]
subject_contains = "meeting request"
reply_body = "Thanks for the meeting request. I'll review my calendar and get back to you."
```

Additional auto-reply rules can be added at runtime via the `set_triage_rule` tool — these are stored in the encrypted store and merged with config-file rules.

### [telegram] (Optional)

Telegram Bot API integration for sending and reading messages.

| Field | Type | Required | Description |
|---|---|---|---|
| `default_chat_id` | string | no | Default chat ID for notifications and quick messages |

**The bot token is stored in the encrypted keystore** under `TELEGRAM_BOT_TOKEN`. Create a bot via [@BotFather](https://t.me/BotFather) to obtain your token.

When configured, urgent heartbeat notifications are automatically forwarded to the default chat.

```toml
[telegram]
default_chat_id = "123456789"
```

### [matrix] (Optional)

Matrix messaging integration for sending and reading messages in Matrix rooms.

| Field | Type | Required | Description |
|---|---|---|---|
| `homeserver` | string | yes | Matrix homeserver URL (e.g., `https://matrix.org`) |
| `default_room_id` | string | no | Default room ID for notifications (e.g., `!abc123:matrix.org`) |

**The access token is stored in the encrypted keystore** under `MATRIX_ACCESS_TOKEN`.

When configured, urgent heartbeat notifications are automatically forwarded to the default room.

```toml
[matrix]
homeserver = "https://matrix.org"
default_room_id = "!abc123:matrix.org"
```

### [signal] (Optional)

Signal messaging integration via [signal-cli](https://github.com/AsamK/signal-cli). Requires `signal-cli` to be installed and linked to a phone number.

| Field | Type | Required | Description |
|---|---|---|---|
| `signal_cli_path` | string | yes | Path to the `signal-cli` binary |
| `account` | string | yes | Your Signal phone number (E.164 format) |

```toml
[signal]
signal_cli_path = "/usr/local/bin/signal-cli"
account = "+1234567890"
```

### [sms] (Optional)

SMS gateway integration for sending and receiving text messages. Supports Twilio and Vonage backends.

| Field | Type | Required | Description |
|---|---|---|---|
| `gateway` | string | yes | Gateway provider: `"twilio"` or `"vonage"` |
| `from_number` | string | yes | Your sending phone number (E.164 format) |

**The account credentials are stored in the encrypted keystore** under `SMS_ACCOUNT_SID` and `SMS_AUTH_TOKEN`.

```toml
[sms]
gateway = "twilio"
from_number = "+1234567890"
```

### [devtools] (Optional)

Developer tool integrations for GitHub and Gitea forges. Enables repository management, issue tracking, and PR workflows.

| Field | Type | Required | Description |
|---|---|---|---|
| `forge_type` | string | yes | Forge provider: `"github"` or `"gitea"` |
| `api_url` | string | yes | API base URL (e.g., `https://api.github.com` or your Gitea instance URL) |
| `default_owner` | string | no | Default repository owner/organization |

**The API token is stored in the encrypted keystore** under `FORGE_API_TOKEN`.

```toml
[devtools]
forge_type = "github"
api_url = "https://api.github.com"
default_owner = "myorg"
```

### [style] (Optional)

Explicit communication style preferences. These are seeded into the user profile on startup so the agent has them from the first turn. The profile extractor will learn additional preferences automatically from conversations.

| Field | Type | Default | Description |
|---|---|---|---|
| `tone` | string | none | Preferred tone: "professional", "casual", "friendly", "formal" |
| `detail_level` | string | none | Response detail: "concise", "balanced", "thorough" |
| `active_hours` | string | none | Working hours (HH:MM-HH:MM). Outside these, non-urgent notifications are batched |
| `preferences` | string[] | `[]` | Additional free-text preferences (e.g., "no emojis", "prefer code examples") |

```toml
[style]
tone = "professional"
detail_level = "concise"
active_hours = "09:00-18:00"
preferences = ["no emojis", "always include sources"]
```

Style preferences from config are merged with LLM-detected preferences. Config preferences take priority (they won't be overwritten by profile extraction).

### [resilience] (Optional)

Provider resilience configuration — circuit breaker, multi-provider fallback, and response caching.

| Field | Type | Default | Description |
|---|---|---|---|
| `circuit_breaker` | boolean | `true` | Enable circuit breaker on LLM provider (auto-opens after repeated failures) |
| `failure_threshold` | integer | `3` | Consecutive failures before circuit opens |
| `recovery_timeout_secs` | integer | `30` | Seconds before half-open retry |
| `success_threshold` | integer | `1` | Consecutive successes to close circuit after recovery |
| `fallback_providers` | string[] | `[]` | Named providers from `[providers.*]` to use as fallback chain |
| `cache_enabled` | boolean | `false` | Enable response caching (SHA-256 prompt hash + optional semantic cache) |

When `fallback_providers` is set, define the fallback providers as named entries in the top-level config:

```toml
[providers.llama-fallback]
type = "Ollama"
base_url = "http://localhost:11434"
model = "llama3.1:8b"

[resilience]
circuit_breaker = true
failure_threshold = 3
fallback_providers = ["llama-fallback"]
cache_enabled = true
```

Both the primary agent provider and the background loop provider are independently wrapped with their own circuit breaker state.

### [consolidation] (Optional)

Memory consolidation tuning — controls how the heartbeat merges, prunes, and mines patterns from episodic memory.

| Field | Type | Default | Description |
|---|---|---|---|
| `merge_threshold` | float | `0.85` | Cosine similarity threshold for merging duplicate memories |
| `stale_days` | integer | `90` | Days after which unreinforced memories are candidates for pruning |
| `batch_size` | integer | `200` | Maximum memories to process per consolidation pass |
| `triple_decay_factor` | float | `0.95` | Confidence decay rate for knowledge graph triples |
| `mine_patterns` | boolean | `true` | Enable automatic pattern mining during consolidation |
| `pattern_min_occurrences` | integer | `3` | Minimum occurrences before a pattern is surfaced |
| `pattern_min_success_rate` | float | `0.6` | Minimum success rate for a pattern to be considered useful |
| `retrieval_router` | boolean | `false` | Use intelligent retrieval strategy selection (temporal/graph/keyword/multi/vector) instead of simple vector recall |

```toml
[consolidation]
merge_threshold = 0.85
stale_days = 60
mine_patterns = true
pattern_min_occurrences = 5
retrieval_router = true
```

### [missions] (Optional)

Mission execution configuration — default execution mode, recipe templates, and experiment tracking.

| Field | Type | Default | Description |
|---|---|---|---|
| `default_mode` | string | `"sequential"` | Default execution mode: `"sequential"` or `"dag"` |
| `recipe_dir` | string | `~/.aivyx/recipes/` | Directory containing TOML recipe templates |
| `experiment_tracking` | boolean | `false` | Enable A/B experiment tracking for mission outcomes |

When `default_mode` is `"dag"`, missions created without an explicit mode use parallel DAG execution (steps run concurrently when their dependencies are met). Sequential mode runs steps one at a time.

Recipe templates are TOML files that define multi-stage pipelines with specialist assignments, reflect gates, and approval stages. Use the `mission_from_recipe` tool to list and execute recipes.

```toml
[missions]
default_mode = "dag"
recipe_dir = "~/.aivyx/recipes"
experiment_tracking = true
```

### [abuse_detection] (Optional)

Sliding-window anomaly monitoring for tool usage. Detects high-frequency bursts,
repeated authorization denials, and scope escalation attempts.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Whether abuse detection is active |
| `window_secs` | integer | `60` | Sliding window duration in seconds |
| `max_calls_per_window` | integer | `50` | Maximum tool calls before `HighFrequency` alert |
| `max_denials_per_window` | integer | `5` | Maximum denied calls before `RepeatedDenials` alert |
| `max_unique_tools_per_window` | integer | `10` | Maximum distinct tool scopes before `ScopeEscalation` alert |

```toml
[abuse_detection]
enabled = true
window_secs = 60
max_calls_per_window = 50
max_denials_per_window = 5
max_unique_tools_per_window = 10
```

### [routing] (Optional)

Complexity-based model routing. Classifies each request as simple, medium, or complex
and routes to a cost-appropriate LLM provider from the `[providers]` table.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `false` | Whether routing is active |
| `simple` | string | *(none)* | Provider name for simple requests (short Q&A, lookups) |
| `medium` | string | *(none)* | Provider name for medium requests (standard tool use) |
| `complex` | string | *(none)* | Provider name for complex requests (multi-step reasoning) |

Unspecified tiers fall back to the default `[provider]`. Provider names reference
entries in the `[providers.*]` table (same as `[resilience]` fallback providers).

```toml
[routing]
enabled = true
simple = "haiku"
medium = "sonnet"
complex = "opus"
```

### [backup] (Optional)

Heartbeat-driven encrypted backup of the PA data directory. The heartbeat LLM
autonomously decides when to trigger backups based on context (e.g., daily or
after significant changes). Old archives are pruned based on retention.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `false` | Whether heartbeat-driven backups are active |
| `destination` | string | `~/.aivyx/backups/` | Directory for backup archives |
| `retention_days` | integer | `30` | Days to keep old backup archives before pruning |

```toml
[backup]
enabled = true
destination = "/mnt/backups/aivyx"
retention_days = 30
```

Backups are gzipped tarballs named `pa_backup_YYYYMMDD_HHMMSS.tar.gz`. The
heartbeat must have `can_backup` permission (auto-set when `[backup]` is enabled).

### [desktop] (Optional)

Desktop interaction — app launching, clipboard, window management, and desktop notifications. When absent, no desktop tools are registered. All tools use subprocess calls to standard Linux CLI tools.

| Field | Type | Default | Description |
|---|---|---|---|
| `allowed_apps` | string[] | `[]` | Strict allowlist of launchable apps (empty = all non-denied) |
| `denied_apps` | string[] | `[]` | Additional app names to deny beyond built-in denylist |
| `clipboard` | boolean | `true` | Register clipboard read/write tools |
| `windows` | boolean | `true` | Register window management tools (X11 only) |
| `notifications` | boolean | `true` | Register desktop notification tool |

```toml
[desktop]
allowed_apps = ["firefox", "gimp", "libreoffice"]
clipboard = true
windows = true
notifications = true
```

**7 tools registered** (when all sub-features enabled): `open_application`, `clipboard_read`, `clipboard_write`, `list_windows`, `get_active_window`, `focus_window`, `send_notification`.

**Built-in denylist** blocks shells, terminal emulators, package managers, and privilege escalation tools. All tools are gated by `CapabilityScope::Custom("desktop")`.

**Required CLI tools**: `xdg-open`, `xclip` or `wl-copy`/`wl-paste` (clipboard), `wmctrl` + `xdotool` (windows, X11 only), `notify-send` (notifications).

### [desktop.interaction] (Optional)

Deep application interaction — semantic UI automation via AT-SPI2, browser automation via Chrome DevTools Protocol, media control via D-Bus MPRIS2, and universal input injection via ydotool. Opt-in: disabled by default.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `false` | Master switch for all interaction tools |

#### [desktop.interaction.accessibility]

AT-SPI2 accessibility backend for native GTK/Qt/Electron apps. Reads the accessibility tree to discover buttons, text fields, menus by role and name. Requires the `accessibility` Cargo feature.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Enable AT-SPI2 backend (when interaction enabled) |

#### [desktop.interaction.browser]

Chrome DevTools Protocol backend for browser automation. Connects via WebSocket to Chrome's remote debugging port. Requires the `browser-automation` Cargo feature.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Enable CDP browser tools |
| `debug_port` | integer | `9222` | Chrome remote debugging port |

#### [desktop.interaction.media]

D-Bus MPRIS2 media control for Linux media players. Requires the `media-control` Cargo feature.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Enable media control tools |

#### [desktop.interaction.input]

ydotool universal input injection (Wayland + X11). Always available as subprocess — no Cargo feature required.

| Field | Type | Default | Description |
|---|---|---|---|
| `enabled` | boolean | `true` | Enable key combo tool (ydotool) |

```toml
[desktop.interaction]
enabled = true

[desktop.interaction.accessibility]
enabled = true

[desktop.interaction.browser]
enabled = true
debug_port = 9222

[desktop.interaction.media]
enabled = true

[desktop.interaction.input]
enabled = true
```

**Up to 14 tools registered** (when all sub-features enabled): 6 UI tools (`ui_inspect`, `ui_find_element`, `ui_click`, `ui_type_text`, `ui_read_text`, `ui_key_combo`), 6 browser tools (`browser_navigate`, `browser_query`, `browser_click`, `browser_type`, `browser_read_page`, `browser_screenshot`), 2 media tools (`media_control`, `media_info`).

**Cargo features**: Build with `--features desktop-full` for all backends, or selectively: `--features accessibility` (AT-SPI2 + zbus), `--features browser-automation` (tokio-tungstenite), `--features media-control` (zbus). Without features, only ydotool (subprocess) is available.

**Required CLI tools**: `ydotool` + `ydotoold` (input injection), `dbus-send` (media control). Chrome must be launched with `--remote-debugging-port=9222` for browser tools. AT-SPI2 requires the accessibility daemon (`at-spi2-core`).

**Safety**: All tools share `CapabilityScope::Custom("desktop")`. CDP only connects to `localhost`. Browser navigation restricted to `http://`, `https://`, `file://` schemes. Text input capped at 10 KB. Screenshots capped at 5 MB.

## Encrypted Keystore

Secrets are stored in `~/.aivyx/store.redb`, encrypted with ChaCha20-Poly1305 under the master key.

| Key | Set By | Used By |
|---|---|---|
| `OPENAI_API_KEY` | `aivyx init` | LLM provider (OpenAI) |
| `ANTHROPIC_API_KEY` | `aivyx init` | LLM provider (Claude) |
| `OPENROUTER_API_KEY` | `aivyx init` | LLM provider (OpenRouter) |
| `EMAIL_PASSWORD` | `aivyx init` | Email IMAP login + SMTP authentication |
| `CALENDAR_PASSWORD` | `aivyx init` or manual | CalDAV calendar authentication |
| `CONTACTS_PASSWORD` | `aivyx init` or manual | CardDAV contact sync authentication |
| `TELEGRAM_BOT_TOKEN` | `aivyx init` or manual | Telegram Bot API authentication |
| `MATRIX_ACCESS_TOKEN` | `aivyx init` or manual | Matrix homeserver authentication |
| `SMS_ACCOUNT_SID` | Manual | SMS gateway account SID (Twilio/Vonage) |
| `SMS_AUTH_TOKEN` | Manual | SMS gateway auth token (Twilio/Vonage) |
| `FORGE_API_TOKEN` | Manual | GitHub/Gitea API token |

## Data Directory Layout

```
~/.aivyx/                      (0o700 permissions)
├── config.toml                Configuration file
├── master_key.json            Encrypted master key envelope
│                              {salt, nonce, ciphertext, params}
├── store.redb                 Encrypted key-value store
│                              API keys, passwords
├── pa.log                     TUI log output (tracing-appender)
├── memory/
│   └── assistant/
│       └── memory.db          Episodic memory (redb, HKDF("memory") key)
├── brain/
│   └── assistant.redb         Goals, self-model (redb, HKDF("brain") key)
├── tasks/
│   └── tasks.db               Mission state, checkpoints (redb, HKDF("task") key)
├── audit/                     HMAC-chained audit log (HKDF("audit") key)
└── backups/                   Heartbeat-driven tar.gz archives (if [backup] enabled)
    └── pa_backup_YYYYMMDD_HHMMSS.tar.gz
```
