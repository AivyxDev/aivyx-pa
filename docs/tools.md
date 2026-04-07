# Tool Reference

The assistant can use up to 145 tools during conversation or background goal evaluation. Tools are registered in `agent.rs:build_agent()` and invoked by the LLM via function calling.

## Action Tools (aivyx-actions)

These are registered via `register_default_actions()` in `bridge.rs`.

### read_file

Read the contents of a local file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute path to the file |

**Returns:** `{ "path": "...", "contents": "..." }`

**Source:** `crates/aivyx-actions/src/files.rs` — `ReadFile`

---

### write_file

Write content to a local file (creates or overwrites).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute path to write |
| `content` | string | yes | Content to write |

**Returns:** `{ "status": "written", "path": "..." }`

**Source:** `crates/aivyx-actions/src/files.rs` — `WriteFile`

---

### list_directory

List files and directories at a given path.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Directory path to list |

**Returns:** `{ "path": "...", "entries": [{ "name": "...", "is_dir": bool, "size": int }] }`

**Source:** `crates/aivyx-actions/src/files.rs` — `ListDirectory`

---

### run_command

Execute a shell command and return its output. Requires **Trust** autonomy tier and **Shell** capability scope.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `command` | string | yes | Shell command to execute |
| `working_dir` | string | no | Working directory |

**Returns:** `{ "exit_code": int, "stdout": "...", "stderr": "..." }`

Commands are executed via `sh -c` on the host system. The Shell capability scope is enforced before execution.

**Source:** `crates/aivyx-actions/src/shell.rs` — `RunCommand`

---

### fetch_webpage

Fetch a webpage and return its text content.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `url` | string | yes | URL to fetch |

**Returns:** `{ "url": "...", "status": int, "content": "..." }`

Content is truncated to 32,000 characters to avoid blowing up the LLM context window. Used both by the agent during chat and by the background loop for web monitoring goals.

**Source:** `crates/aivyx-actions/src/web.rs` — `FetchPage`

---

### search_web

Search the web using DuckDuckGo and return structured results with titles, URLs, and snippets.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `query` | string | yes | — | Search query |
| `max_results` | integer | no | 5 | Maximum results to return (max: 10) |

**Returns:**
```json
{
  "query": "rust async patterns",
  "results": [
    {
      "title": "Async Programming in Rust",
      "url": "https://example.com/async-rust",
      "snippet": "A guide to async/await patterns..."
    }
  ],
  "count": 5
}
```

Uses DuckDuckGo's HTML lite interface — no API key required. Results are parsed from the HTML response with URL decoding and HTML entity handling.

**Source:** `crates/aivyx-actions/src/web.rs` — `SearchWeb`

---

### set_reminder

Set a reminder for a specific date/time.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `message` | string | yes | What to remind about |
| `due_at` | string | yes | ISO 8601 datetime when the reminder fires |

**Returns:** `{ "status": "set", "id": "uuid", "message": "...", "due_at": "..." }`

Reminders are persisted to the encrypted store under the `HKDF("reminders")` domain key. They survive restarts.

**Source:** `crates/aivyx-actions/src/reminders.rs` — `SetReminder`

---

### list_reminders

List all pending reminders.

**Parameters:** None (empty object)

**Returns:** Array of `Reminder` objects with id, message, due_at, and status.

**Source:** `crates/aivyx-actions/src/reminders.rs` — `ListReminders`

---

### dismiss_reminder

Dismiss (remove) a reminder by its ID.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | UUID of the reminder to dismiss |

**Returns:** `{ "status": "dismissed", "id": "..." }`

**Source:** `crates/aivyx-actions/src/reminders.rs` — `DismissReminder`

---

## Email Tools

Registered via `register_email_actions()` only when email is configured in `config.toml` and the password is available in the encrypted keystore.

### read_email

Check email inbox and return a summary of recent messages via IMAP over TLS.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `limit` | integer | no | 10 | Maximum messages to fetch |
| `unread_only` | boolean | no | true | Only return unread messages |

**Returns:** Array of `EmailSummary`:
```json
[
  {
    "from": "sender@example.com",
    "subject": "Re: Proposal",
    "preview": "Sounds good, let's proceed...",
    "seq": 42,
    "message_id": "<abc123@mail.example.com>"
  }
]
```

The IMAP connection uses TLS on port 993 (configurable). Messages are fetched newest-first. The `seq` field is the IMAP sequence number. The `message_id` field contains the RFC 2822 Message-ID for reply threading (may be `null` if the server didn't provide it).

**Source:** `crates/aivyx-actions/src/email.rs` — `ReadInbox`

---

### fetch_email

Fetch the full content of a specific email by its IMAP sequence number. Use this after `read_email` to get the complete body for drafting replies.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `seq` | integer | yes | IMAP sequence number from `read_email` results |

**Returns:**
```json
{
  "from": "sender@example.com",
  "to": "you@example.com",
  "subject": "Re: Proposal",
  "date": "Wed, 02 Apr 2026 10:30:00 +0000",
  "message_id": "<abc123@mail.example.com>",
  "body": "Full email body text...",
  "seq": 42
}
```

Body is returned up to 32,000 characters. Opens a fresh IMAP connection, fetches the single message, and logs out.

**Source:** `crates/aivyx-actions/src/email.rs` — `FetchEmail`

---

### send_email

Send an email message via SMTP with STARTTLS.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `to` | string | yes | Recipient email address |
| `subject` | string | yes | Email subject |
| `body` | string | yes | Email body (plain text) |
| `in_reply_to` | string | no | Message-ID of the email being replied to (for threading) |

**Returns:** `{ "status": "sent", "to": "..." }`

Emails are sent with `From: Aivyx Assistant <your-configured-address>`. Uses STARTTLS on port 587 (configurable). When `in_reply_to` is provided, the email includes `In-Reply-To` and `References` headers so the reply threads correctly in the recipient's mail client.

**Source:** `crates/aivyx-actions/src/email.rs` — `SendEmail`

---

## Calendar Tools

Registered via `register_calendar_actions()` only when `[calendar]` is configured in `config.toml` with CalDAV credentials.

### today_agenda

Fetch today's calendar events in chronological order.

**Parameters:** None (empty object)

**Returns:**
```json
{
  "date": "2026-04-03",
  "events": [
    {
      "summary": "Team standup",
      "start": "2026-04-03T09:00:00",
      "end": "2026-04-03T09:30:00",
      "location": "Zoom"
    }
  ],
  "count": 3
}
```

**Source:** `crates/aivyx-actions/src/calendar.rs` — `TodayAgenda`

---

### fetch_calendar_events

Fetch calendar events within a date range.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `start` | string | yes | Start date (ISO 8601, e.g., `2026-04-01`) |
| `end` | string | yes | End date (ISO 8601, e.g., `2026-04-07`) |

**Returns:** Array of events with summary, start, end, and location.

**Source:** `crates/aivyx-actions/src/calendar.rs` — `FetchCalendarEvents`

---

### check_conflicts

Check for scheduling conflicts (overlapping events) in a time range.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `start` | string | yes | Range start (ISO 8601 datetime) |
| `end` | string | yes | Range end (ISO 8601 datetime) |

**Returns:** `{ "conflicts": [...], "count": int }` — pairs of overlapping events.

**Source:** `crates/aivyx-actions/src/calendar.rs` — `CheckConflicts`

---

## Contact Tools

Registered via `register_contact_actions()`. Search and list always work against the local encrypted store. `sync_contacts` is only available when CardDAV is configured.

### search_contacts

Search contacts by name, email, or phone number.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Search string (matched case-insensitively against all fields) |

**Returns:** Array of matching contact objects.

**Source:** `crates/aivyx-actions/src/contacts.rs` — `SearchContacts`

---

### list_contacts

List all stored contacts.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `limit` | integer | no | 50 | Maximum contacts to return |

**Returns:** Array of contact objects with name, email, phone, and organization.

**Source:** `crates/aivyx-actions/src/contacts.rs` — `ListContacts`

---

### sync_contacts

Sync contacts from the configured CardDAV server to the local encrypted store.

**Parameters:** None (empty object)

**Returns:** `{ "status": "synced", "added": int, "updated": int, "total": int }`

Only available when `[contacts]` includes CardDAV credentials. Uses the `HKDF("contacts")` domain key for encryption.

**Source:** `crates/aivyx-actions/src/contacts.rs` — `SyncContacts`

---

## Document Tools

Registered via `register_document_actions()` when `[vault]` is configured. The vault is a local directory of documents (markdown, text, PDF) that can be indexed for semantic search.

### search_documents

Search across indexed vault documents using semantic similarity.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Natural language search query |
| `limit` | integer | no | Maximum results (default 10) |

**Returns:** Array of matches with document path, relevance score, and matching excerpt.

Requires an embedding provider to be configured. Uses the memory system's embedding infrastructure for vector search.

**Source:** `crates/aivyx-actions/src/documents.rs` — `SearchDocuments`

---

### read_document

Read the full content of a specific document from the vault.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Relative path within the vault directory |

**Returns:** `{ "path": "...", "content": "..." }`

**Source:** `crates/aivyx-actions/src/documents.rs` — `ReadDocument`

---

### list_vault_documents

List documents in the vault filtered by file extension.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `extensions` | string[] | no | File extensions to include (default: `["md", "txt", "pdf"]`) |

**Returns:** Array of document paths with size and modification time.

**Source:** `crates/aivyx-actions/src/documents.rs` — `ListVaultDocuments`

---

### index_vault

Index or re-index the document vault for semantic search. Reads all matching documents, generates embeddings, and stores them for future `search_documents` queries.

**Parameters:** None (empty object)

**Returns:** `{ "status": "indexed", "documents": int, "chunks": int }`

**Source:** `crates/aivyx-actions/src/documents.rs` — `IndexVault`

---

## Finance Tools

Registered via `register_finance_actions()` when `[finance]` is configured. All financial data is encrypted under the `HKDF("finance")` domain key.

### add_transaction

Record a financial transaction.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `amount` | number | yes | Transaction amount (positive = income, negative = expense) |
| `category` | string | yes | Category (e.g., "groceries", "utilities", "salary") |
| `description` | string | yes | Description of the transaction |
| `date` | string | no | Date (ISO 8601, defaults to today) |

**Returns:** `{ "status": "recorded", "id": "uuid", "balance_change": "..." }`

**Source:** `crates/aivyx-actions/src/finance.rs` — `AddTransaction`

---

### list_transactions

List transactions with optional filtering.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `limit` | integer | no | Maximum transactions (default 20) |
| `category` | string | no | Filter by category |
| `start_date` | string | no | Filter from date (ISO 8601) |
| `end_date` | string | no | Filter to date (ISO 8601) |

**Returns:** Array of transaction objects with amount, category, description, and date.

**Source:** `crates/aivyx-actions/src/finance.rs` — `ListTransactions`

---

### budget_summary

Show budget vs. actual spending by category for the current month.

**Parameters:** None (empty object)

**Returns:**
```json
{
  "month": "2026-04",
  "categories": [
    { "name": "groceries", "budget": 500.00, "spent": 312.50, "remaining": 187.50 }
  ],
  "total_budget": 2000.00,
  "total_spent": 1234.50
}
```

**Source:** `crates/aivyx-actions/src/finance.rs` — `BudgetSummary`

---

### set_budget

Set or update a monthly budget for a category.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `category` | string | yes | Budget category |
| `amount` | number | yes | Monthly budget amount |

**Returns:** `{ "status": "set", "category": "...", "amount": "..." }`

**Source:** `crates/aivyx-actions/src/finance.rs` — `SetBudget`

---

### mark_bill_paid

Mark a recurring bill as paid for the current billing period.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Bill name (e.g., "rent", "electricity") |
| `amount` | number | no | Actual amount paid (if different from expected) |

**Returns:** `{ "status": "paid", "name": "...", "period": "..." }`

**Source:** `crates/aivyx-actions/src/finance.rs` — `MarkBillPaid`

---

### file_receipt

Extract a receipt from an email and file it to the document vault. Requires both email and vault to be configured.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `seq` | integer | yes | IMAP sequence number of the receipt email |
| `category` | string | no | Expense category for the receipt |

**Returns:** `{ "status": "filed", "path": "...", "amount": "..." }`

**Source:** `crates/aivyx-actions/src/finance.rs` — `FileReceipt`

---

## Triage Tools

Registered via `register_triage_actions()` when `[triage]` is enabled and email is configured. These tools let the user inspect what the agent has done autonomously and manage auto-reply rules.

### list_triage_log

Show recent email triage activity — what the agent did autonomously with incoming emails.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `limit` | integer | no | 20 | Maximum entries to return |

**Returns:**
```json
{
  "count": 5,
  "entries": [
    {
      "seq": 42,
      "from": "noreply@service.com",
      "subject": "Your receipt",
      "action": "classified",
      "category": "receipt",
      "urgency": "low",
      "timestamp": "2026-04-03T10:30:00Z"
    }
  ]
}
```

Entries are sorted by timestamp (most recent first).

**Source:** `crates/aivyx-actions/src/triage_tools.rs` — `ListTriageLog`

---

### set_triage_rule

Add or update an auto-reply triage rule. Rules are matched on sender and/or subject.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Rule name (unique identifier) |
| `sender_contains` | string | no | Match emails where sender contains this string |
| `subject_contains` | string | no | Match emails where subject contains this string |
| `reply_body` | string | yes | Auto-reply message body |

At least one of `sender_contains` or `subject_contains` must be provided. Rules are stored in the encrypted store under the `HKDF("triage")` domain key and merged with config-file rules at runtime.

**Returns:** `{ "status": "saved", "name": "...", "total_rules": int }`

**Source:** `crates/aivyx-actions/src/triage_tools.rs` — `SetTriageRule`

---

## Messaging Tools

### Telegram Tools

Registered via `register_telegram_actions()` only when `[telegram]` is configured and `TELEGRAM_BOT_TOKEN` is in the keystore.

### send_telegram

Send a message via Telegram Bot API.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `chat_id` | string | no | Telegram chat ID (defaults to `default_chat_id` from config) |
| `text` | string | yes | Message text to send |

**Returns:** `{ "status": "sent", "chat_id": "..." }`

**Source:** `crates/aivyx-actions/src/messaging/telegram.rs` — `SendTelegram`

---

### read_telegram

Read recent messages from a Telegram chat.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `chat_id` | string | no | `default_chat_id` | Telegram chat ID |
| `limit` | integer | no | 10 | Maximum messages to return |

**Returns:** Array of `Message` objects with id, from, text, and timestamp.

**Source:** `crates/aivyx-actions/src/messaging/telegram.rs` — `ReadTelegram`

---

### Matrix Tools

Registered via `register_matrix_actions()` only when `[matrix]` is configured and `MATRIX_ACCESS_TOKEN` is in the keystore.

### send_matrix

Send a message to a Matrix room.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `room_id` | string | no | Matrix room ID (defaults to `default_room_id` from config) |
| `text` | string | yes | Message text to send |

**Returns:** `{ "status": "sent", "room_id": "..." }`

**Source:** `crates/aivyx-actions/src/messaging/matrix.rs` — `SendMatrix`

---

### read_matrix

Read recent messages from a Matrix room.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `room_id` | string | no | `default_room_id` | Matrix room ID |
| `limit` | integer | no | 10 | Maximum messages to return |

**Returns:** Array of `Message` objects with id, from, text, and timestamp.

**Source:** `crates/aivyx-actions/src/messaging/matrix.rs` — `ReadMatrix`

---

### Signal Tools

Registered via `register_signal_actions()` only when `[signal]` is configured and `signal-cli` is available on the host.

### send_signal

Send a message via signal-cli.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `recipient` | string | yes | Phone number or Signal username of the recipient |
| `message` | string | yes | Message text to send |

**Returns:** `{ "status": "sent", "recipient": "..." }`

**Source:** `crates/aivyx-actions/src/messaging/signal.rs` — `SendSignal`

---

### read_signal

Read recent Signal messages.

| Parameter | Type | Required | Default | Description |
|---|---|---|---|---|
| `limit` | integer | no | 10 | Maximum messages to return |

**Returns:** Array of `Message` objects with id, from, text, and timestamp.

**Source:** `crates/aivyx-actions/src/messaging/signal.rs` — `ReadSignal`

---

### SMS Tools

Registered via `register_sms_actions()` only when `[sms]` is configured with Twilio or Vonage credentials.

### send_sms

Send an SMS message via Twilio/Vonage.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `to` | string | yes | Recipient phone number (E.164 format) |
| `message` | string | yes | Message text to send |

**Returns:** `{ "status": "sent", "to": "..." }`

**Source:** `crates/aivyx-actions/src/messaging/sms.rs` — `SendSms`

---

## Workflow Tools

Registered via `register_workflow_actions()`. Always available — workflow templates are stored encrypted under the `HKDF("workflow")` domain key.

### create_workflow

Create or update a reusable workflow template.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `template` | object | yes | Full workflow template JSON (name, description, steps, parameters, triggers) |

Each step can specify: description, optional tool hint, arguments with `{param}` placeholders, `requires_approval` flag, and an optional `StepCondition` (OnSuccess, OnFailure, VarEquals, VarContains, All, Any).

**Returns:** `{ "status": "created"|"updated", "name": "...", "steps": int }`

**Source:** `crates/aivyx-actions/src/workflow.rs` — `CreateWorkflowAction`

---

### list_workflows

List available workflow templates.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `detailed` | boolean | no | Include full step/parameter/trigger details (default: false) |

**Returns:** Array of template summaries or detailed definitions.

**Source:** `crates/aivyx-actions/src/workflow.rs` — `ListWorkflowsAction`

---

### run_workflow

Instantiate a workflow template with parameters, producing concrete steps for mission creation.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Template name to instantiate |
| `params` | object | no | Parameter values to fill `{param}` placeholders |

Validates required parameters, applies defaults, and replaces placeholders in step descriptions and arguments.

**Returns:** `{ "workflow": "...", "steps": [...], "step_count": int }`

**Source:** `crates/aivyx-actions/src/workflow.rs` — `RunWorkflowAction`

---

### workflow_status

Inspect a template's full definition including serialized conditions and triggers.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Template name to inspect |

**Returns:** Full template definition with steps, parameters, triggers, and timestamps.

**Source:** `crates/aivyx-actions/src/workflow.rs` — `WorkflowStatusAction`

---

### delete_workflow

Delete a workflow template by name.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Template name to delete |

**Returns:** `{ "status": "deleted", "name": "..." }`

**Source:** `crates/aivyx-actions/src/workflow.rs` — `DeleteWorkflowAction`

---

### install_workflow_library

Install or reinstall the built-in workflow template library (12 templates). Skips templates that already exist unless `force = true`.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `force` | boolean | no | If true, overwrite existing library templates (preserves `created_at`). Default: false |

**Returns:** `{ "status": "ok", "installed": int, "total_library_templates": 12, "mode": "seed"|"reinstall", "templates": [...] }`

**Source:** `crates/aivyx-actions/src/workflow.rs` — `InstallLibraryAction`

#### Built-in Workflow Templates (12)

The library ships with 12 pre-built templates, installed on first boot via `seed_library()`:

| Template | Trigger | Description |
|---|---|---|
| `morning-briefing` | Cron: daily 7 AM | Calendar + email + reminders digest |
| `inbox-zero` | Manual | Triage unread emails, draft replies |
| `expense-report` | Email (subject: "receipt") | Extract amounts, file receipt, record transaction |
| `bill-pay-reminder` | Cron: Monday 9 AM | Check upcoming bills, set payment reminders |
| `weekly-review` | Cron: Friday 5 PM | Goals, accomplishments, next week planning |
| `research-digest` | Manual | Web search + summarize + save to file |
| `code-review-checklist` | Webhook / Manual | Fetch PR diff, review, post comments |
| `meeting-prep` | Manual | Gather email/vault/contact context, draft agenda |
| `monthly-budget-review` | Cron: 1st of month 10 AM | Spending vs budget analysis, trend detection |
| `project-status-report` | Cron: Monday 9 AM | Git log, issues, CI status, PR activity |
| `strategy-review` | Cron: Sunday 9 AM | Weekly strategic review of all goal progress and patterns (sets `strategy_review_pending` flag for heartbeat) |
| `milestone-scan` | Cron: 1st of month 9 AM | Deep monthly/yearly goal anniversary scan for milestones |

**Source:** `crates/aivyx-actions/src/workflow/library.rs`

---

## Undo Tools

Registered via `register_undo_actions()`. Always available — undo records are stored encrypted under the `HKDF("undo")` domain key with a configurable TTL (default 24h).

### record_undo

Record an undo entry before performing a destructive action so it can be reversed later.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `tool_name` | string | yes | Name of the tool performing the destructive action |
| `action_summary` | string | yes | Brief description of what will be done |
| `undo_type` | string | yes | One of: `restore_file`, `cancel_reminder`, `void_transaction`, `manual_only` |
| `undo_data` | object | yes | Data for reversal (varies by type — see below) |
| `ttl_hours` | integer | no | Hours until expiry (default: 24) |

**undo_data by type:**
- `restore_file`: `{ "path": "...", "original_content": "..." }`
- `cancel_reminder`: `{ "reminder_id": "..." }`
- `void_transaction`: `{ "transaction_id": "..." }`
- `manual_only`: `{ "instructions": "..." }`

**Returns:** `{ "status": "recorded", "undo_id": "uuid", "expires_at": "..." }`

**Source:** `crates/aivyx-actions/src/undo.rs` — `RecordUndoAction`

---

### list_undo_history

List recent actions that can be undone (non-expired, non-undone).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `limit` | integer | no | Maximum entries (default: 20) |

**Returns:** `{ "count": int, "entries": [{ "id", "tool", "summary", "undo_type", "performed_at", "expires_at" }] }`

**Source:** `crates/aivyx-actions/src/undo.rs` — `ListUndoHistoryAction`

---

### undo_action

Undo a previous action by its undo ID. For `restore_file`, the original content is written back automatically. For other types, instructions are returned for the agent to complete the reversal.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `undo_id` | string | yes | The undo record ID to reverse |

**Returns:** `{ "undo_id": "...", "result": { "status": "restored"|"pending"|"manual", ... } }`

**Source:** `crates/aivyx-actions/src/undo.rs` — `UndoActionTool`

---

## Memory Tools (aivyx-memory)

Registered via `register_memory_tools()` from the aivyx-memory crate. Only available if an embedding provider is configured (see `[embedding]` in config.toml).

These tools give the assistant persistent memory across sessions. The memory system uses embedding-based semantic search for intelligent recall.

### memory_store

Save a fact, observation, or user preference to long-term memory.

### memory_recall

Recall memories by semantic similarity to a query. Uses the configured embedding model to find relevant memories.

### memory_search

Search memories by exact keyword matching.

### memory_forget

Remove a specific memory by ID.

### memory_list

List all stored memories.

### memory_count

Return the total number of stored memories.

**Note:** Full parameter schemas for memory tools are defined in the aivyx-memory crate. See [aivyx-core documentation](https://github.com/AivyxDev/aivyx-core) for details.

---

## Brain Tools (aivyx-brain)

Registered individually in `build_agent()`. Each tool gets its own HKDF-derived brain key (since `MasterKey` is `!Clone`). Only available if the BrainStore opens successfully.

### brain_set_goal

Create a new persistent goal.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `description` | string | yes | What the goal is about |
| `success_criteria` | string | yes | How to know when the goal is achieved |
| `parent_id` | string | no | Parent goal ID (for sub-goals) |

Goals persist across sessions and are evaluated by the background loop. Use sub-goals to break complex missions into steps.

### brain_list_goals

List goals filtered by status.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `status` | string | no | Filter: "Active", "Completed", "Abandoned" |

### brain_update_goal

Update a goal's status or add progress notes.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `goal_id` | string | yes | Goal ID to update |
| `status` | string | no | New status |
| `progress` | string | no | Progress note to append |

### brain_reflect

Record a self-reflection about performance and outcomes. This feeds the assistant's self-model — patterns of what works, what fails, and how to improve.

**Note:** Full parameter schemas for brain tools are defined in the aivyx-brain crate. See [aivyx-core documentation](https://github.com/AivyxDev/aivyx-core) for details.

### brain_update_self_model

Update the assistant's self-model based on learned outcomes. This is a **PA-local tool** (defined in `tui.rs`, not in aivyx-brain) that directly modifies the persisted self-model: strengths, weaknesses, domain confidence scores, and tool proficiency ratings.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `add_strengths` | string[] | no | Strengths to add (e.g., `["email-drafting", "research-synthesis"]`) |
| `remove_strengths` | string[] | no | Strengths to remove if no longer accurate |
| `add_weaknesses` | string[] | no | Weaknesses to add (e.g., `["complex-math", "visual-design"]`) |
| `remove_weaknesses` | string[] | no | Weaknesses to remove (e.g., after improving) |
| `domain_confidence` | object | no | Domain → score (0.0–1.0). E.g., `{"coding": 0.8, "cooking": 0.3}` |
| `tool_proficiency` | object | no | Tool → score (0.0–1.0). E.g., `{"fetch_webpage": 0.9, "send_email": 0.7}` |

All parameters are optional — pass only the fields you want to change. Scores are clamped to `[0.0, 1.0]`.

**Returns:** `{ "status": "updated", "changes": [...], "strengths": int, "weaknesses": int, "domain_count": int, "tool_count": int }`

**Source:** `crates/aivyx-pa/src/tui.rs` — `BrainUpdateSelfModelTool`

---

## Mission Tools (aivyx-task-engine)

Registered via `create_mission_tools()` from the aivyx-task-engine crate. These enable multi-step autonomous task execution with LLM-based planning and encrypted checkpointing.

Each mission tool shares a `MissionToolContext` that holds an `Arc<AgentSession>` and `Zeroizing<Vec<u8>>` (master key bytes). The `TaskEngine` is constructed on demand per tool call, avoiding persistent redb locks.

### mission_create

Create a new mission — the LLM decomposes the goal into sequential steps and executes them autonomously in the background.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `goal` | string | yes | High-level description of what to accomplish |
| `agent` | string | no | Agent profile to use for execution (defaults to current agent) |

**Returns:** `{ "mission_id": "uuid", "status": "running", "steps": [...] }`

Missions run in the background. Each step is executed as a separate agent turn. Between steps, the entire mission state is checkpointed to encrypted storage. If the process crashes, the mission can be resumed from the last checkpoint.

**Mission→Brain Bridge:** When a BrainStore is available, this tool is replaced by `PaMissionCreateTool` (defined in `tui.rs`), which wraps the standard mission_create with a post-completion hook. On mission completion, the bridge:
1. Matches the mission goal to active brain goals using keyword overlap (tokenize, filter words >3 chars, require 40% overlap or 2+ matching words)
2. Records success/failure on matched goals (exponential backoff on failure, auto-abandon after 10 consecutive failures)
3. Updates the self-model: adjusts domain confidence (±5%) and `mission_create` tool proficiency (±2–3%)

### mission_list

List all missions with their current status.

**Parameters:** None

**Returns:** Array of mission summaries: `[{ "id": "uuid", "goal": "...", "status": "running|completed|failed|cancelled", "progress": "2/5 steps" }]`

### mission_status

Get detailed status of a specific mission including individual step progress.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `mission_id` | string | yes | UUID of the mission to inspect |

**Returns:** Full mission state including step-by-step status, outputs, and timing.

### mission_control

Control a running mission — pause, resume, or cancel.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `mission_id` | string | yes | UUID of the mission |
| `action` | string | yes | One of: `pause`, `resume`, `cancel` |

**Returns:** Updated mission status.

**Source:** `aivyx/crates/aivyx-task-engine/src/tools.rs`

---

### mission_from_recipe

Create a mission from a TOML recipe template. Recipes are reusable multi-stage pipeline definitions with specialist assignments, reflect gates, approval stages, and DAG dependencies.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `recipe` | string | yes | Recipe filename (without `.toml`) or `"list"` to see available recipes |
| `goal` | string | conditional | Goal to accomplish using this recipe (required when creating, not when listing) |

**List mode:** When `recipe` is `"list"`, returns all available `.toml` files in the configured recipe directory with their descriptions, stage counts, and tags.

**Create mode:** Loads the recipe, validates its dependency graph, converts stages to task engine steps, and starts execution.

**Returns (list):** `{ "recipes": [{ "name": "...", "description": "...", "stages": int, "tags": [...] }], "recipe_dir": "..." }`

**Returns (create):** `{ "task_id": "uuid", "recipe": "...", "stages": int, "specialists": [...], "status": "executing" }`

**Source:** `crates/aivyx-pa/src/tui.rs` — `MissionFromRecipeTool`

---

## MCP Tools

### mcp_list_prompts

List prompt templates discovered from connected MCP servers.

**Parameters:** None

**Returns:** `{ "prompts": [{ "server": "...", "name": "...", "description": "..." }] }`

Iterates all servers in the `McpServerPool`, calls `list_prompts()` on each, and returns the aggregated list. Useful for discovering reusable prompt templates exposed by MCP tool servers.

**Source:** `crates/aivyx-pa/src/tui.rs` — `McpListPromptsTool`

---

## Pattern Mining Tools

### list_discovered_patterns

List workflow patterns automatically discovered by the memory consolidation system.

**Parameters:** None

**Returns:** `{ "patterns": [{ "description": "...", "occurrences": int, "success_rate": float, "first_seen": "...", "last_seen": "..." }] }`

Patterns are mined during heartbeat memory consolidation when `mine_patterns = true` in the `[consolidation]` config. They represent recurring sequences of tool usage or task approaches that have been observed across multiple missions or conversations.

**Source:** `crates/aivyx-pa/src/tui.rs` — `ListDiscoveredPatternsTool`

---

## Knowledge Graph Tools

Registered via `register_knowledge_actions()`. Always available — the knowledge graph is stored encrypted under the `HKDF("knowledge")` domain key.

### traverse_knowledge

Traverse the knowledge graph from a given entity, returning connected triples.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `entity` | string | yes | Starting entity name |
| `depth` | integer | no | Traversal depth (default 1) |

**Returns:** `{ "entity": "...", "triples": [{ "subject": "...", "predicate": "...", "object": "..." }], "count": int }`

**Source:** `crates/aivyx-actions/src/knowledge.rs` — `TraverseKnowledge`

---

### find_knowledge_paths

Find paths between two entities in the knowledge graph.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `from` | string | yes | Starting entity |
| `to` | string | yes | Target entity |
| `max_depth` | integer | no | Maximum path length (default 5) |

**Returns:** `{ "from": "...", "to": "...", "paths": [[...]], "count": int }`

**Source:** `crates/aivyx-actions/src/knowledge.rs` — `FindKnowledgePaths`

---

### search_knowledge

Semantic search across knowledge graph triples.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Natural language search query |
| `limit` | integer | no | Maximum results (default 10) |

**Returns:** Array of matching triples with relevance scores.

**Source:** `crates/aivyx-actions/src/knowledge.rs` — `SearchKnowledge`

---

### knowledge_graph_stats

Return knowledge graph size and statistics.

**Parameters:** None (empty object)

**Returns:** `{ "entities": int, "triples": int, "predicates": int, "most_connected": [...] }`

**Source:** `crates/aivyx-actions/src/knowledge.rs` — `KnowledgeGraphStats`

---

## Git Tools

Registered via `register_git_actions()`. Always available — operates on the local git repository in the current working directory or a specified path.

### git_log

Show recent commit history.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Repository path (defaults to working directory) |
| `limit` | integer | no | Maximum commits to return (default 10) |

**Returns:** Array of commit objects with hash, author, date, and message.

**Source:** `crates/aivyx-actions/src/devtools/git.rs` — `GitLog`

---

### git_diff

Show working directory changes (staged and unstaged).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Repository path (defaults to working directory) |
| `staged` | boolean | no | Show only staged changes (default false) |

**Returns:** `{ "diff": "..." }`

**Source:** `crates/aivyx-actions/src/devtools/git.rs` — `GitDiff`

---

### git_status

Show the working tree state.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Repository path (defaults to working directory) |

**Returns:** `{ "branch": "...", "files": [{ "path": "...", "status": "..." }] }`

**Source:** `crates/aivyx-actions/src/devtools/git.rs` — `GitStatus`

---

### git_branches

List branches in the repository.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | no | Repository path (defaults to working directory) |

**Returns:** `{ "current": "...", "branches": [...] }`

**Source:** `crates/aivyx-actions/src/devtools/git.rs` — `GitBranches`

---

## Forge Tools

Registered via `register_forge_actions()` only when `[devtools]` is configured with forge credentials (GitHub, Gitea, or GitLab).

### ci_status

Get the CI pipeline status for a branch or commit.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `branch` | string | no | Branch name (defaults to current branch) |
| `commit` | string | no | Commit SHA to check |

**Returns:** `{ "status": "...", "pipelines": [...] }`

**Source:** `crates/aivyx-actions/src/devtools/ci.rs` — `CiStatus`

---

### ci_logs

Get CI pipeline logs for a specific run.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `run_id` | string | yes | CI run identifier |

**Returns:** `{ "run_id": "...", "logs": "..." }`

**Source:** `crates/aivyx-actions/src/devtools/ci.rs` — `CiLogs`

---

### list_issues

List repository issues with optional filtering.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `state` | string | no | Filter by state: "open", "closed", "all" (default "open") |
| `limit` | integer | no | Maximum issues to return (default 20) |

**Returns:** Array of issue objects with number, title, state, and labels.

**Source:** `crates/aivyx-actions/src/devtools/issues.rs` — `ListIssues`

---

### get_issue

Get detailed information about a specific issue.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `number` | integer | yes | Issue number |

**Returns:** Full issue object with title, body, comments, labels, and assignees.

**Source:** `crates/aivyx-actions/src/devtools/issues.rs` — `GetIssue`

---

### create_issue

Create a new issue in the repository.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `title` | string | yes | Issue title |
| `body` | string | no | Issue body (markdown) |
| `labels` | string[] | no | Labels to apply |

**Returns:** `{ "status": "created", "number": int, "url": "..." }`

**Source:** `crates/aivyx-actions/src/devtools/issues.rs` — `CreateIssue`

---

### list_prs

List pull requests with optional filtering.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `state` | string | no | Filter by state: "open", "closed", "merged", "all" (default "open") |
| `limit` | integer | no | Maximum PRs to return (default 20) |

**Returns:** Array of PR objects with number, title, state, and branch info.

**Source:** `crates/aivyx-actions/src/devtools/pr.rs` — `ListPrs`

---

### get_pr_diff

Get the diff for a specific pull request.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `number` | integer | yes | PR number |

**Returns:** `{ "number": int, "diff": "..." }`

**Source:** `crates/aivyx-actions/src/devtools/pr.rs` — `GetPrDiff`

---

### create_pr_comment

Comment on a pull request.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `number` | integer | yes | PR number |
| `body` | string | yes | Comment body (markdown) |

**Returns:** `{ "status": "created", "number": int }`

**Source:** `crates/aivyx-actions/src/devtools/pr.rs` — `CreatePrComment`

---

## Plugin Tools

Registered via `register_plugin_actions()`. Always available — the plugin registry is stored encrypted under the `HKDF("plugins")` domain key.

### list_plugins

List all installed plugins with their enabled/disabled status.

**Parameters:** None (empty object)

**Returns:** Array of plugin objects with name, version, enabled status, and description.

**Source:** `crates/aivyx-actions/src/plugin.rs` — `ListPlugins`

---

### enable_plugin

Enable a previously disabled plugin.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Plugin name to enable |

**Returns:** `{ "status": "enabled", "name": "..." }`

**Source:** `crates/aivyx-actions/src/plugin.rs` — `EnablePlugin`

---

### disable_plugin

Disable an active plugin without uninstalling it.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Plugin name to disable |

**Returns:** `{ "status": "disabled", "name": "..." }`

**Source:** `crates/aivyx-actions/src/plugin.rs` — `DisablePlugin`

---

### install_plugin

Install a plugin from the registry or a local path.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `source` | string | yes | Plugin name (from registry) or local path |

**Returns:** `{ "status": "installed", "name": "...", "version": "..." }`

**Source:** `crates/aivyx-actions/src/plugin.rs` — `InstallPlugin`

---

### uninstall_plugin

Uninstall a plugin and remove its data.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Plugin name to uninstall |

**Returns:** `{ "status": "uninstalled", "name": "..." }`

**Source:** `crates/aivyx-actions/src/plugin.rs` — `UninstallPlugin`

---

### search_plugins

Search the plugin registry for available plugins.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Search query |

**Returns:** Array of matching plugin objects with name, version, and description.

**Source:** `crates/aivyx-actions/src/plugin.rs` — `SearchPlugins`

---

## Team Tool (aivyx-team)

Registered via `register_team_actions()`. Always available when the aivyx-team crate is linked.

### team_delegate

Delegate a task to a Nonagon specialist team. The team selects the appropriate specialist agent based on the task domain and executes collaboratively.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `task` | string | yes | Description of the task to delegate |
| `team` | string | no | Specific team to target (auto-selected if omitted) |
| `context` | string | no | Additional context for the specialist |

**Returns:** `{ "status": "delegated", "team": "...", "specialist": "...", "result": "..." }`

**Source:** `aivyx/crates/aivyx-team/src/lib.rs` — `TeamDelegate`

---

## Security Tools (Phase 5B)

Registered directly in `build_agent()`. Admin-scoped — requires explicit capability grant.

### key_rotate

Rotate the master encryption key. All encrypted data is re-encrypted with the new key
in a two-phase atomic operation: all entries are decrypted/re-encrypted in memory first,
then committed in a single transaction.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `new_passphrase` | string | yes | New passphrase (minimum 8 characters) |
| `confirm` | boolean | yes | Must be `true` to proceed with rotation |

**Returns:** `{ "status": "rotated", "keys_migrated": 42, "errors": 0 }`

**Required scope:** `Custom("admin")` — only agents with explicit admin capability can rotate keys.

**Source:** `crates/aivyx-pa/src/tui.rs` — `KeyRotateTool`

---

## Schedule CRUD Tools (aivyx-pa)

Registered directly in `build_agent()`. Requires `admin` capability scope.

### create_schedule

Create a new `[[schedules]]` entry in config.toml.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Slug name (1-64 chars, alphanumeric/hyphens/underscores) |
| `cron` | string | yes | Cron expression (e.g., `0 9 * * 1-5`) |
| `prompt` | string | yes | What the agent should do when the schedule fires |
| `notify` | boolean | no | Send notification on completion (default: true) |

**Returns:** `{ "status": "created", "name": "..." }`

**Source:** `crates/aivyx-pa/src/schedule_tools.rs` — `ScheduleCreateTool`

---

### edit_schedule

Edit fields of an existing schedule by name.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Schedule name to edit |
| `cron` | string | no | New cron expression |
| `prompt` | string | no | New prompt |
| `enabled` | boolean | no | Enable/disable the schedule |
| `notify` | boolean | no | Toggle notification on completion |

**Returns:** `{ "status": "updated", "name": "...", "fields_changed": [...] }`

**Source:** `crates/aivyx-pa/src/schedule_tools.rs` — `ScheduleEditTool`

---

### delete_schedule

Delete a schedule from config.toml by name.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `name` | string | yes | Schedule name to delete |

**Returns:** `{ "status": "deleted", "name": "..." }`

**Source:** `crates/aivyx-pa/src/schedule_tools.rs` — `ScheduleDeleteTool`

---

## Desktop Interaction Tools (aivyx-actions)

Registered via `register_desktop_actions()` in `bridge.rs` when `[desktop]` is configured. All tools are scoped with `CapabilityScope::Custom("desktop")`.

### open_application

Open a file, URL, or application on the desktop. Uses `xdg-open` for files/URLs, direct process spawn for named applications.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `target` | string | yes | URL, file path, or application name |
| `args` | string[] | no | Additional arguments for the application |

**Returns:** `{ "status": "launched", "target": "...", "method": "xdg-open"|"direct" }`

Blocked apps: shells, terminal emulators, package managers, privilege escalation tools. If `allowed_apps` is configured, only those apps are permitted.

**Source:** `crates/aivyx-actions/src/desktop/open.rs` — `OpenApplication`

---

### clipboard_read

Read the current text content from the system clipboard.

No parameters.

**Returns:** `{ "text": "..." }`

Uses `xclip` (X11) or `wl-paste` (Wayland). Output capped at 64 KB.

**Source:** `crates/aivyx-actions/src/desktop/clipboard.rs` — `ClipboardRead`

---

### clipboard_write

Write text to the system clipboard.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `text` | string | yes | Text to write (max 1 MB) |

**Returns:** `{ "status": "written", "bytes": N }`

Uses `xclip` (X11) or `wl-copy` (Wayland). Pipes text via stdin (no shell injection).

**Source:** `crates/aivyx-actions/src/desktop/clipboard.rs` — `ClipboardWrite`

---

### list_windows

List all open windows with ID, desktop number, PID, and title. X11 only.

No parameters.

**Returns:** `{ "windows": [{ "id": "0x...", "desktop": "0", "pid": "...", "title": "..." }], "count": N }`

**Source:** `crates/aivyx-actions/src/desktop/windows.rs` — `ListWindows`

---

### get_active_window

Get the title and class of the currently focused window. X11 only.

No parameters.

**Returns:** `{ "window_id": "...", "title": "...", "class": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/windows.rs` — `GetActiveWindow`

---

### focus_window

Bring a window to the front by title substring or window ID. X11 only.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `title` | string | no | Substring of window title to match |
| `window_id` | string | no | Window ID from `list_windows` |

At least one parameter is required. Window ID takes precedence if both are provided.

**Returns:** `{ "status": "focused", "target": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/windows.rs` — `FocusWindow`

---

### send_notification

Send a desktop notification via `notify-send`.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `title` | string | yes | Notification title |
| `body` | string | yes | Notification body text |
| `urgency` | string | no | `low`, `normal` (default), or `critical` |
| `icon` | string | no | Icon name or path |
| `timeout_ms` | integer | no | Auto-dismiss timeout in ms (0 = persistent) |

**Returns:** `{ "status": "sent", "title": "...", "urgency": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/notify.rs` — `SendNotification`

---

## Deep Application Interaction Tools (aivyx-actions)

Registered via `register_interaction_actions()` in `bridge.rs` when `[desktop.interaction]` is configured with `enabled = true`. All tools are scoped with `CapabilityScope::Custom("desktop")`. Backend selection (AT-SPI2, CDP, ydotool) is automatic via the `BackendRouter`.

### UI Automation Tools (AT-SPI2 + ydotool fallback)

### ui_inspect

Read the accessibility tree of a window. Returns structured JSON of UI elements (buttons, labels, text fields, menus) with roles, names, states, bounds, and text content.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title (substring match). Omit for active window |
| `window_id` | string | no | Window ID (overrides title if both given) |
| `max_depth` | integer | no | Maximum tree depth (default: 5) |

**Returns:** `{ "backend": "at-spi2", "element_count": N, "tree": [...] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiInspect`

---

### ui_find_element

Find a specific UI element by role, name, or text content. Returns element paths usable with `ui_click` and `ui_type_text`.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title (substring match) |
| `window_id` | string | no | Window ID |
| `role` | string | no | Element role filter (e.g., "button", "text_field") |
| `name` | string | no | Element name filter (substring, case-insensitive) |
| `text` | string | no | Element text content filter (substring) |

At least one of `role`, `name`, or `text` is required.

**Returns:** `{ "backend": "at-spi2", "count": N, "elements": [{ "path": "0/3/1", "role": "push button", "name": "OK", ... }] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiFindElement`

---

### ui_click

Click a UI element by element path (from `ui_find_element`) or by role+name direct lookup. Falls back to ydotool coordinate-based clicking if the accessibility backend can't perform the action.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title |
| `element` | string | no | Element path from `ui_find_element` |
| `role` | string | no | Element role (with `name` for direct lookup) |
| `name` | string | no | Element name (with `role` for direct lookup) |

Either `element` or `role`+`name` is required.

**Returns:** `{ "status": "clicked", "backend": "at-spi2", "element": "OK" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiClick`

---

### ui_type_text

Type text into a focused text field or a specific UI element. Uses the accessibility backend's `EditableText` interface when available, falls back to ydotool keyboard simulation.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `text` | string | yes | Text to type (max 10 KB) |
| `element` | string | no | Element path (types into focused element if omitted) |

**Returns:** `{ "status": "typed", "backend": "at-spi2", "length": N }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiTypeText`

---

### ui_read_text

Read text content from a UI element using the accessibility `Text` interface.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `element` | string | yes | Element path from `ui_find_element` |
| `window` | string | no | Window title |

**Returns:** `{ "backend": "at-spi2", "text": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiReadText`

---

### ui_key_combo

Send a keyboard shortcut via ydotool. Works on both X11 and Wayland. Requires `ydotoold` daemon running.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `keys` | string | yes | Key combination (e.g., "ctrl+s", "alt+F4", "ctrl+shift+t", "super") |

**Returns:** `{ "status": "sent", "keys": "ctrl+s" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiKeyCombo`

---

### Browser Automation Tools (Chrome DevTools Protocol)

Requires Chrome/Chromium launched with `--remote-debugging-port=9222` and `[desktop.interaction.browser]` enabled.

### browser_navigate

Navigate a browser tab to a URL. Only `http://`, `https://`, and `file://` schemes allowed.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `url` | string | yes | URL to navigate to |
| `tab` | integer | no | Tab index (default: 0 = active tab) |

**Returns:** `{ "status": "navigated", "url": "...", "tab": 0, "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserNavigate`

---

### browser_query

Query DOM elements in a browser tab by CSS selector.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector (e.g., "button.submit", "#login-form input") |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "selector": "...", "count": N, "elements": [{ "nodeId": ..., "outerHTML": "..." }] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserQuery`

---

### browser_click

Click a DOM element in the browser by CSS selector. Scrolls into view and clicks.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector for the element |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "clicked", "selector": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserClick`

---

### browser_type

Type text into a form field in the browser selected by CSS selector.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector for the input field |
| `text` | string | yes | Text to type (max 10 KB) |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "typed", "selector": "...", "length": N, "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserType`

---

### browser_read_page

Read the visible text content of a browser page or a specific element. Returns `innerText`, capped at 64 KB.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | no | CSS selector to scope reading (full page if omitted) |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "text": "...", "length": N }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserReadPage`

---

### browser_screenshot

Take a screenshot of the current browser page. Returns a base64-encoded image (max 5 MB).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `format` | string | no | "png" (default) or "jpeg" |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "format": "png", "data_base64": "...", "size_bytes": N }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserScreenshot`

---

### Media Control Tools (D-Bus MPRIS2)

Control media players via D-Bus MPRIS2. Requires `[desktop.interaction.media]` enabled. Supported by most Linux players (Spotify, VLC, Firefox, Chromium, mpv, Rhythmbox).

### media_control

Control the active media player.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | One of: `play`, `pause`, `toggle`, `next`, `previous`, `stop` |
| `player` | string | no | Player name (e.g., "spotify", "vlc"). Auto-detects if omitted |

**Returns:** `{ "status": "ok", "detail": "toggle sent to spotify" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `MediaControl`

---

### media_info

Get current playback info from the active media player.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `player` | string | no | Player name. Auto-detects if omitted |

**Returns:** `{ "player": "spotify", "status": "Playing", "title": "...", "artist": "...", "album": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `MediaInfo`

---

### ui_scroll

Scroll within any application window using mouse wheel injection.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `direction` | string | yes | Scroll direction: "up", "down", "left", "right" |
| `amount` | integer | no | Scroll units (default: 3, each ≈ one wheel click) |
| `window` | string | no | Window title (optional — scrolls wherever cursor is) |

**Returns:** `{ "status": "scrolled", "direction": "down", "amount": 3, "backend": "ydotool" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiScroll`

---

### ui_right_click

Right-click a UI element to open its context menu.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `element` | string | no | Element path from ui_find_element |
| `role` | string | no | Element role (for direct lookup) |
| `name` | string | no | Element name (for direct lookup) |
| `x` | integer | no | Absolute X coordinate (if no element) |
| `y` | integer | no | Absolute Y coordinate (if no element) |
| `window` | string | no | Window title |

**Returns:** `{ "status": "right_clicked", "backend": "at-spi2", "element": "file.txt" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiRightClick`

---

### ui_hover

Hover over a UI element to trigger tooltips or hover menus.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `element` | string | no | Element path from ui_find_element |
| `role` | string | no | Element role (for lookup) |
| `name` | string | no | Element name (for lookup) |
| `x` | integer | no | Absolute X coordinate |
| `y` | integer | no | Absolute Y coordinate |
| `window` | string | no | Window title |

**Returns:** `{ "status": "hovered", "backend": "ydotool", "x": 100, "y": 200 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiHover`

---

### ui_drag

Drag from one position to another. Supports both coordinate-based and element-based dragging.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `from_x` | integer | no | Source X coordinate |
| `from_y` | integer | no | Source Y coordinate |
| `to_x` | integer | no | Destination X coordinate |
| `to_y` | integer | no | Destination Y coordinate |
| `from_element` | string | no | Source element path (alternative to coordinates) |
| `to_element` | string | no | Destination element path |

**Returns:** `{ "status": "dragged", "backend": "ydotool", "from": [100, 200], "to": [300, 400] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiDrag`

---

### ui_mouse_move

Move the mouse cursor to absolute screen coordinates.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `x` | integer | yes | Absolute X coordinate |
| `y` | integer | yes | Absolute Y coordinate |

**Returns:** `{ "status": "moved", "x": 100, "y": 200 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiMouseMove`

---

### window_screenshot

Take a screenshot of a specific window or the full screen. Uses grim (Wayland) or import (X11).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title (full screen if omitted) |
| `format` | string | no | "png" or "jpeg" (default: "png") |

**Returns:** `{ "format": "png", "data_base64": "...", "size_bytes": 12345 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `WindowScreenshot`

---

### browser_scroll

Scroll within a browser page or a specific element.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `direction` | string | yes | Scroll direction: "up", "down", "left", "right" |
| `amount` | integer | no | Scroll units (default: 3, each ≈ 100px) |
| `selector` | string | no | CSS selector to scope scrolling |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "scrolled", "direction": "down", "amount": 3, "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserScroll`

---

### browser_execute_js

Execute arbitrary JavaScript in a browser tab. Max 10KB expression.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `expression` | string | yes | JavaScript to evaluate |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "executed", "result": { "type": "string", "value": "..." } }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserExecuteJs`

---

### ui_double_click

Double-click a UI element. Useful for opening files, selecting words.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title |
| `element` | string | no | Element path from ui_find_element |
| `role` | string | no | Element role (for lookup) |
| `name` | string | no | Element name (for lookup) |
| `x` | integer | no | Absolute X coordinate |
| `y` | integer | no | Absolute Y coordinate |

**Returns:** `{ "status": "double_clicked", "backend": "...", ... }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiDoubleClick`

---

### ui_middle_click

Middle-click a UI element. Pastes primary selection on Linux, opens links in new tabs.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `window` | string | no | Window title |
| `element` | string | no | Element path |
| `role` | string | no | Element role (for lookup) |
| `name` | string | no | Element name (for lookup) |
| `x` | integer | no | Absolute X coordinate |
| `y` | integer | no | Absolute Y coordinate |

**Returns:** `{ "status": "middle_clicked", "backend": "...", ... }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiMiddleClick`

---

### browser_list_tabs

List all open browser tabs with their titles and URLs.

| Parameter | Type | Required | Description |
|---|---|---|---|
| *(none)* | | | |

**Returns:** `{ "count": 5, "tabs": [{ "index": 0, "title": "...", "url": "..." }, ...] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserListTabs`

---

### browser_new_tab

Open a new browser tab, optionally navigating to a URL.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `url` | string | no | URL to open (default: about:blank) |

**Returns:** `{ "status": "opened", "url": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserNewTab`

---

### browser_close_tab

Close a browser tab by index.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `tab` | integer | no | Tab index to close (default: 0) |

**Returns:** `{ "status": "closed", "tab": 0 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserCloseTab`

---

### browser_wait_for

Wait for a CSS selector to appear on the page. Polls every 200ms.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector to wait for |
| `timeout` | integer | no | Max wait in ms (default: 5000, max: 30000) |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "found" | "timeout", "selector": "...", "timeout_ms": 5000 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserWaitFor`

---

### window_manage

Manage desktop windows: minimize, maximize, restore, close, fullscreen, resize, move.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | One of: minimize, maximize, restore, close, fullscreen, resize, move |
| `window` | string | no | Window title (default: active window) |
| `width` | integer | no | New width (for resize) |
| `height` | integer | no | New height (for resize) |
| `x` | integer | no | New X position (for move) |
| `y` | integer | no | New Y position (for move) |

**Returns:** `{ "status": "ok", "action": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `WindowManage`

---

### system_volume

Control system audio volume via wpctl (PipeWire) or pactl (PulseAudio).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | One of: get, set, up, down, mute, unmute, toggle_mute |
| `value` | integer | no | Percentage (for set: 0-150) or increment (for up/down, default: 5) |

**Returns:** `{ "volume": 75, "muted": false, "backend": "wpctl" }` (for get) or status object

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `SystemVolume`

---

### system_brightness

Control display brightness via brightnessctl.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | One of: get, set, up, down |
| `value` | integer | no | Percentage (for set: 0-100) or increment (for up/down, default: 5) |

**Returns:** `{ "brightness": 80, "device": "..." }` (for get) or status object

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `SystemBrightness`

---

### ui_select_option

Select an option in a dropdown/combobox by value or visible text.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector for the `<select>` element |
| `value` | string | no | Option value to select |
| `text` | string | no | Visible text to match |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "selected", "selector": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiSelectOption`

---

### ui_clear_field

Clear a text input field. Uses JavaScript for browser fields, Ctrl+A+Delete for native.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `selector` | string | yes | CSS selector (browser) or element path (native) |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "status": "cleared", "backend": "cdp" | "ydotool" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiClearField`

---

### notification_list

List recent desktop notifications from dunst, SwayNC, or mako.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `count` | integer | no | Max notifications to return (default: 10) |

**Returns:** `{ "count": 3, "daemon": "dunst", "notifications": [{ "app": "...", "summary": "...", "body": "..." }] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `NotificationList`

---

### file_manager_show

Open a file or folder in the system file manager, optionally revealing (highlighting) a file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute path to a file or directory |
| `reveal` | boolean | no | Highlight the file in parent folder (default: false) |

**Returns:** `{ "status": "opened", "path": "...", "reveal": false }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `FileManagerShow`

---

### screen_ocr

Extract text from a screen region using OCR (Tesseract). Last resort for apps that don't expose accessibility data.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `region` | string | yes | Screen region as `x,y widthxheight` (e.g., `100,200 800x600`) |
| `language` | string | no | OCR language (default: eng). E.g., deu, fra, jpn, chi_sim |

**Returns:** `{ "status": "ok", "region": "...", "text": "...", "char_count": 42 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `ScreenOcr`

---

### list_running_apps

List all running GUI applications with window titles, classes, PIDs, and workspaces.

| Parameter | Type | Required | Description |
|---|---|---|---|
| *(none)* | | | |

**Returns:** `{ "count": 5, "backend": "hyprctl", "apps": [{ "title": "...", "class": "...", "pid": 1234, "workspace": "1" }] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `ListRunningApps`

---

### desktop_workspace

Manage desktop workspaces: list, get current, switch, or move windows between workspaces.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `action` | string | yes | One of: list, current, switch, move_window |
| `target` | string | no | Target workspace name/number (for switch/move_window) |
| `window` | string | no | Window title (for move_window — defaults to active) |

**Returns:** Varies by action. List returns workspaces array, switch returns status.

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DesktopWorkspace`

---

### browser_pdf

Save the current browser page as a PDF document.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `tab` | integer | no | Tab index (default: 0) |
| `landscape` | boolean | no | Landscape orientation (default: false) |

**Returns:** `{ "status": "ok", "format": "pdf", "data_base64": "...", "size_bytes": 12345 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserPdf`

---

### browser_find_text

Find text on a browser page (Ctrl+F equivalent). Case-insensitive search with surrounding context.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `query` | string | yes | Text to search for (max 1000 chars) |
| `tab` | integer | no | Tab index (default: 0) |

**Returns:** `{ "query": "...", "count": 3, "matches": [{ "index": 42, "context": "...text..." }] }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `BrowserFindText`

---

### ui_multi_select

Ctrl+click multiple positions for batch selection (files, list items, table rows).

| Parameter | Type | Required | Description |
|---|---|---|---|
| `positions` | array | yes | Array of `[x, y]` coordinate pairs (max 100) |

**Returns:** `{ "status": "multi_selected", "count": 3, "backend": "ydotool" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `UiMultiSelect`

---

### doc_create_text

Create a text, markdown, HTML, or other text-format file.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute output path |
| `content` | string | yes | File content (max 1MB) |

**Returns:** `{ "status": "created", "path": "...", "bytes": 1234 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocCreateText`

---

### doc_create_spreadsheet

Create a spreadsheet from structured data. CSV natively, XLSX/ODS via conversion.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute output path (.csv, .xlsx, .xls, .ods) |
| `headers` | string[] | yes | Column headers |
| `rows` | string[][] | yes | Data rows (array of arrays) |

**Returns:** `{ "status": "created", "path": "...", "columns": 3, "rows": 10 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocCreateSpreadsheet`

---

### doc_create_pdf

Create a PDF from inline markdown, HTML, LaTeX, or RST content.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute output path for PDF |
| `content` | string | yes | Document content |
| `format` | string | no | Source format: markdown, html, latex, rst (default: markdown) |

**Returns:** `{ "status": "created", "path": "...", "format": "pdf", "source_format": "markdown" }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocCreatePdf`

---

### doc_edit_text

Edit a text file with structured operations.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute path to edit |
| `operation` | string | yes | find_replace, insert_at, append, prepend, delete_lines |
| `find` | string | no | Text to find (find_replace) |
| `replace` | string | no | Replacement text (find_replace) |
| `all` | boolean | no | Replace all occurrences (default: false) |
| `line` | integer | no | Line number, 1-based (insert_at) |
| `text` | string | no | Text to insert/append/prepend |
| `from` | integer | no | Start line, 1-based (delete_lines) |
| `to` | integer | no | End line, inclusive (delete_lines) |

**Returns:** `{ "status": "edited", "path": "...", "operation": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocEditText`

---

### doc_convert

Convert documents between formats using pandoc or LibreOffice.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `input_path` | string | yes | Source document path |
| `output_path` | string | yes | Destination path (format from extension) |
| `from_format` | string | no | Override source format |
| `to_format` | string | no | Override target format |

**Returns:** `{ "status": "converted", "input": "...", "output": "...", "detail": "..." }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocConvert`

---

### doc_read_pdf

Extract text from a PDF file using pdftotext. More accurate than OCR.

| Parameter | Type | Required | Description |
|---|---|---|---|
| `path` | string | yes | Absolute path to PDF |
| `first_page` | integer | no | First page to extract (1-based) |
| `last_page` | integer | no | Last page to extract (1-based) |

**Returns:** `{ "path": "...", "text": "...", "char_count": 5000 }`

**Source:** `crates/aivyx-actions/src/desktop/interaction/tools.rs` — `DocReadPdf`

---

## Tool Registration Order

Tools are registered in `build_agent()` in this order. Conditional tools are only registered when their integration is configured.

```
Index  Tool                    Source              Condition
─────  ────                    ──────              ─────────
0      read_file               files.rs            Always
1      write_file              files.rs            Always
2      list_directory          files.rs            Always
3      run_command             shell.rs            Always (capability-gated)
4      fetch_webpage           web.rs              Always
5      search_web              web.rs              Always
6      set_reminder            reminders.rs        Always (encrypted persistence)
7      list_reminders          reminders.rs        Always
8      dismiss_reminder        reminders.rs        Always
9      read_email              email.rs            Email configured
10     fetch_email             email.rs            Email configured
11     send_email              email.rs            Email configured
12     today_agenda            calendar.rs         Calendar configured
13     fetch_calendar_events   calendar.rs         Calendar configured
14     check_conflicts         calendar.rs         Calendar configured
15     search_contacts         contacts.rs         Contacts configured
16     list_contacts           contacts.rs         Contacts configured
17     sync_contacts           contacts.rs         CardDAV configured
18     search_documents        documents.rs        Vault configured
19     read_document           documents.rs        Vault configured
20     list_vault_documents    documents.rs        Vault configured
21     index_vault             documents.rs        Vault configured
22     add_transaction         finance.rs          Finance configured
23     list_transactions       finance.rs          Finance configured
24     budget_summary          finance.rs          Finance configured
25     set_budget              finance.rs          Finance configured
26     mark_bill_paid          finance.rs          Finance configured
27     file_receipt            finance.rs          Finance + Email + Vault
28     list_triage_log         triage_tools.rs     Triage enabled
29     set_triage_rule         triage_tools.rs     Triage enabled
30     send_telegram           telegram.rs         Telegram configured
31     read_telegram           telegram.rs         Telegram configured
32     send_matrix             matrix.rs           Matrix configured
33     read_matrix             matrix.rs           Matrix configured
34     send_signal             signal.rs           Signal configured
35     read_signal             signal.rs           Signal configured
36     send_sms                sms.rs              SMS configured
37     create_workflow         workflow.rs         Always
38     list_workflows          workflow.rs         Always
39     run_workflow            workflow.rs         Always
40     workflow_status         workflow.rs         Always
41     delete_workflow         workflow.rs         Always
42     install_library         workflow.rs         Always
43     record_undo             undo.rs             Always
44     list_undo_history       undo.rs             Always
45     undo_action             undo.rs             Always
46-51  memory_*                aivyx-memory        Embedding provider available
52     brain_set_goal          aivyx-brain         BrainStore opens
53     brain_list_goals        aivyx-brain         BrainStore opens
54     brain_update_goal       aivyx-brain         BrainStore opens
55     brain_reflect           aivyx-brain         BrainStore opens
56     brain_update_self_model tui.rs (PA-local)   BrainStore opens
57     mission_create *        tui.rs (PA-local)   BrainStore opens (falls back to engine version)
58     mission_list            aivyx-task-engine   Always
59     mission_status          aivyx-task-engine   Always
60     mission_control         aivyx-task-engine   Always
61     mission_from_recipe     tui.rs (PA-local)   Always
62     mcp_list_prompts        tui.rs (PA-local)   MCP servers connected
63     list_discovered_patterns tui.rs (PA-local)  Memory manager available
64     traverse_knowledge      knowledge.rs        Always
65     find_knowledge_paths    knowledge.rs        Always
66     search_knowledge        knowledge.rs        Always
67     knowledge_graph_stats   knowledge.rs        Always
68     git_log                 devtools/git.rs     Always
69     git_diff                devtools/git.rs     Always
70     git_status              devtools/git.rs     Always
71     git_branches            devtools/git.rs     Always
72     ci_status               devtools/ci.rs      Devtools configured
73     ci_logs                 devtools/ci.rs      Devtools configured
74     list_issues             devtools/issues.rs  Devtools configured
75     get_issue               devtools/issues.rs  Devtools configured
76     create_issue            devtools/issues.rs  Devtools configured
77     list_prs                devtools/pr.rs      Devtools configured
78     get_pr_diff             devtools/pr.rs      Devtools configured
79     create_pr_comment       devtools/pr.rs      Devtools configured
80     list_plugins            plugin.rs           Always
81     enable_plugin           plugin.rs           Always
82     disable_plugin          plugin.rs           Always
83     install_plugin          plugin.rs           Always
84     uninstall_plugin        plugin.rs           Always
85     search_plugins          plugin.rs           Always
86     team_delegate           aivyx-team          Always
87     key_rotate              tui.rs              Always (admin scope)
88     create_schedule         schedule_tools.rs   Always (admin scope)
89     edit_schedule           schedule_tools.rs   Always (admin scope)
90     delete_schedule         schedule_tools.rs   Always (admin scope)
91     open_application        desktop/open.rs     [desktop] configured
92     clipboard_read          desktop/clipboard.rs [desktop] + clipboard=true
93     clipboard_write         desktop/clipboard.rs [desktop] + clipboard=true
94     list_windows            desktop/windows.rs  [desktop] + windows=true
95     get_active_window       desktop/windows.rs  [desktop] + windows=true
96     focus_window            desktop/windows.rs  [desktop] + windows=true
97     send_notification       desktop/notify.rs   [desktop] + notifications=true
98     ui_inspect              interaction/tools.rs [desktop.interaction] + enabled
99     ui_find_element         interaction/tools.rs [desktop.interaction] + enabled
100    ui_click                interaction/tools.rs [desktop.interaction] + enabled
101    ui_type_text            interaction/tools.rs [desktop.interaction] + enabled
102    ui_read_text            interaction/tools.rs [desktop.interaction] + enabled
103    ui_scroll               interaction/tools.rs [desktop.interaction] + enabled
104    ui_right_click          interaction/tools.rs [desktop.interaction] + enabled
105    ui_hover                interaction/tools.rs [desktop.interaction] + enabled
106    ui_drag                 interaction/tools.rs [desktop.interaction] + enabled
107    ui_key_combo            interaction/tools.rs [desktop.interaction] + input=true
108    ui_mouse_move           interaction/tools.rs [desktop.interaction] + input=true
109    window_screenshot       interaction/tools.rs [desktop.interaction] + enabled
110    browser_navigate        interaction/tools.rs [desktop.interaction] + browser=true
111    browser_query           interaction/tools.rs [desktop.interaction] + browser=true
112    browser_click           interaction/tools.rs [desktop.interaction] + browser=true
113    browser_type            interaction/tools.rs [desktop.interaction] + browser=true
114    browser_read_page       interaction/tools.rs [desktop.interaction] + browser=true
115    browser_screenshot      interaction/tools.rs [desktop.interaction] + browser=true
116    browser_scroll          interaction/tools.rs [desktop.interaction] + browser=true
117    browser_execute_js      interaction/tools.rs [desktop.interaction] + browser=true
118    ui_double_click         interaction/tools.rs [desktop.interaction] + enabled
119    ui_middle_click         interaction/tools.rs [desktop.interaction] + enabled
120    ui_select_option        interaction/tools.rs [desktop.interaction] + enabled
121    ui_clear_field          interaction/tools.rs [desktop.interaction] + enabled
122    window_manage           interaction/tools.rs [desktop.interaction] + enabled
123    system_volume           interaction/tools.rs [desktop.interaction] + enabled
124    system_brightness       interaction/tools.rs [desktop.interaction] + enabled
125    notification_list       interaction/tools.rs [desktop.interaction] + enabled
126    file_manager_show       interaction/tools.rs [desktop.interaction] + enabled
127    browser_list_tabs       interaction/tools.rs [desktop.interaction] + browser=true
128    browser_new_tab         interaction/tools.rs [desktop.interaction] + browser=true
129    browser_close_tab       interaction/tools.rs [desktop.interaction] + browser=true
130    browser_wait_for        interaction/tools.rs [desktop.interaction] + browser=true
131    ui_multi_select         interaction/tools.rs [desktop.interaction] + enabled
132    screen_ocr              interaction/tools.rs [desktop.interaction] + enabled
133    list_running_apps       interaction/tools.rs [desktop.interaction] + enabled
134    desktop_workspace       interaction/tools.rs [desktop.interaction] + enabled
135    browser_pdf             interaction/tools.rs [desktop.interaction] + browser=true
136    browser_find_text       interaction/tools.rs [desktop.interaction] + browser=true
137    doc_create_text         interaction/tools.rs [desktop.interaction] + enabled
138    doc_create_spreadsheet  interaction/tools.rs [desktop.interaction] + enabled
139    doc_create_pdf          interaction/tools.rs [desktop.interaction] + enabled
140    doc_edit_text           interaction/tools.rs [desktop.interaction] + enabled
141    doc_convert             interaction/tools.rs [desktop.interaction] + enabled
142    doc_read_pdf            interaction/tools.rs [desktop.interaction] + enabled
143    media_control           interaction/tools.rs [desktop.interaction] + media=true
144    media_info              interaction/tools.rs [desktop.interaction] + media=true
```

\* When BrainStore is available, `mission_create` is the PA-local `PaMissionCreateTool` with the brain bridge. Without BrainStore, it's the standard engine version.

Minimum: 42 tools (no integrations, no memory, no brain — defaults + workflow + undo + mission + recipe + knowledge + git + plugin + team + security + schedule tools)
Maximum: 119 tools (all integrations, memory, brain, desktop, interaction, MCP servers)
