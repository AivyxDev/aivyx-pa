//! Pre-built workflow template library — common automation patterns.
//!
//! Provides a curated set of reusable workflow templates that cover the most
//! common personal-assistant patterns. Templates are installed into the
//! encrypted store on first boot (if they don't already exist) and can be
//! reinstalled on demand via the `install_workflow_library` action.
//!
//! Each template uses only tools that are part of the core PA tool set,
//! with `{param}` placeholders for user customization.

use super::{
    StepCondition, TemplateParameter, TemplateStep, WorkflowContext, WorkflowTemplate,
    WorkflowTrigger, list_templates, load_template, save_template,
};
use aivyx_core::Result;
use chrono::Utc;

// ── Template Builders ────────────────────────────────────────

/// All built-in library templates.
///
/// Each function returns a `WorkflowTemplate` with `created_at`/`updated_at`
/// set to the current time — callers should overwrite `created_at` if the
/// template already exists in the store.
pub fn all_templates() -> Vec<WorkflowTemplate> {
    vec![
        morning_briefing(),
        inbox_zero(),
        expense_report(),
        bill_pay_reminder(),
        weekly_review(),
        research_digest(),
        code_review_checklist(),
        meeting_prep(),
        monthly_budget_review(),
        project_status_report(),
        strategy_review(),
        milestone_scan(),
    ]
}

/// Names of all library templates (for identification).
pub fn library_names() -> Vec<&'static str> {
    vec![
        "morning-briefing",
        "inbox-zero",
        "expense-report",
        "bill-pay-reminder",
        "weekly-review",
        "research-digest",
        "code-review-checklist",
        "meeting-prep",
        "monthly-budget-review",
        "project-status-report",
        "strategy-review",
        "milestone-scan",
    ]
}

// ── Individual Templates ─────────────────────────────────────

/// Daily morning briefing — calendar, email summary, reminders, weather.
fn morning_briefing() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "morning-briefing".into(),
        description: "Generate a morning briefing with today's schedule, unread emails, and pending reminders".into(),
        steps: vec![
            TemplateStep {
                step_id: "fetch-calendar".into(),
                description: "Fetch today's calendar events".into(),
                tool: Some("list_events".into()),
                arguments: serde_json::json!({"date": "today"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "check-unread-email".into(),
                description: "Check unread emails from the last 12 hours".into(),
                tool: Some("read_email".into()),
                arguments: serde_json::json!({"unread_only": true, "limit": 20}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "list-reminders".into(),
                description: "List pending reminders".into(),
                tool: Some("list_reminders".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "compile-briefing".into(),
                description: "Compile briefing summary: prioritize urgent items, flag conflicts, suggest time blocks for deep work".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![0, 1, 2],
                acceptance: vec![],
            },
        ],
        parameters: vec![],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 7 * * *".into() },
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Inbox zero — triage unread emails, draft replies, archive handled items.
fn inbox_zero() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "inbox-zero".into(),
        description: "Process unread emails: categorize by urgency, draft replies for {category} emails, summarize the rest".into(),
        steps: vec![
            TemplateStep {
                step_id: "fetch-unread".into(),
                description: "Fetch all unread emails".into(),
                tool: Some("read_email".into()),
                arguments: serde_json::json!({"unread_only": true, "limit": 50}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "categorize".into(),
                description: "Categorize emails by urgency: urgent (needs reply today), important (this week), informational (archive candidate)".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "draft-urgent-replies".into(),
                description: "Draft replies for urgent emails — present each draft for approval before sending".into(),
                tool: Some("send_email".into()),
                arguments: serde_json::Value::Null,
                requires_approval: true,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "summarize-informational".into(),
                description: "Summarize informational emails in a brief digest".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "category".into(),
                description: "Which urgency category to draft replies for (urgent, important, or all)".into(),
                required: false,
                default: Some("urgent".into()),
            },
        ],
        triggers: vec![WorkflowTrigger::Manual],
        created_at: now,
        updated_at: now,
    }
}

/// Expense report processing — fetch receipt email, extract amounts, file receipt.
fn expense_report() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "expense-report".into(),
        description: "Process expense receipt from {sender}: extract amounts, categorize, and file to {folder}".into(),
        steps: vec![
            TemplateStep {
                step_id: "fetch-receipt".into(),
                description: "Fetch the receipt email from {sender}".into(),
                tool: Some("fetch_email".into()),
                arguments: serde_json::json!({"query": "from:{sender}"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "extract-amounts".into(),
                description: "Extract expense amounts, vendor name, and date from the email body and attachments".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "record-transaction".into(),
                description: "Record the transaction in the finance tracker".into(),
                tool: Some("add_transaction".into()),
                arguments: serde_json::json!({
                    "category": "{category}",
                    "description": "Expense from {sender}"
                }),
                requires_approval: true,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "file-receipt".into(),
                description: "File the receipt document to {folder}".into(),
                tool: Some("file_receipt".into()),
                arguments: serde_json::json!({"folder": "{folder}"}),
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![2],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "sender".into(),
                description: "Email address or name of the expense sender".into(),
                required: true,
                default: None,
            },
            TemplateParameter {
                name: "folder".into(),
                description: "Folder to file the receipt into".into(),
                required: false,
                default: Some("receipts".into()),
            },
            TemplateParameter {
                name: "category".into(),
                description: "Expense category (e.g. travel, meals, software)".into(),
                required: false,
                default: Some("general".into()),
            },
        ],
        triggers: vec![
            WorkflowTrigger::Email {
                sender_contains: None,
                subject_contains: Some("receipt".into()),
            },
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Bill payment reminder — check upcoming bills and send reminders.
fn bill_pay_reminder() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "bill-pay-reminder".into(),
        description: "Check for upcoming bills due within {days} days and send payment reminders"
            .into(),
        steps: vec![
            TemplateStep {
                step_id: "budget-summary".into(),
                description: "Review budget summary for bills due within {days} days".into(),
                tool: Some("budget_summary".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "list-recent-transactions".into(),
                description: "List recent transactions to check which bills have already been paid"
                    .into(),
                tool: Some("list_transactions".into()),
                arguments: serde_json::json!({"days": "{days}"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "identify-unpaid".into(),
                description:
                    "Identify unpaid bills by comparing budget due dates with recent payments"
                        .into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0, 1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "set-reminders".into(),
                description: "Set reminders for each unpaid bill with the due date and amount"
                    .into(),
                tool: Some("set_reminder".into()),
                arguments: serde_json::Value::Null,
                requires_approval: true,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![2],
                acceptance: vec![],
            },
        ],
        parameters: vec![TemplateParameter {
            name: "days".into(),
            description: "Number of days ahead to check for upcoming bills".into(),
            required: false,
            default: Some("7".into()),
        }],
        triggers: vec![
            WorkflowTrigger::Cron {
                expression: "0 9 * * 1".into(),
            }, // Monday 9am
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Weekly review — goals, accomplishments, upcoming week planning.
fn weekly_review() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "weekly-review".into(),
        description: "Conduct a weekly review: summarize accomplishments, review goals, plan next week".into(),
        steps: vec![
            TemplateStep {
                step_id: "fetch-this-week".into(),
                description: "Fetch this week's calendar events to review what happened".into(),
                tool: Some("list_events".into()),
                arguments: serde_json::json!({"range": "this_week"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "review-sent-email".into(),
                description: "Review sent emails from this week for context on completed work".into(),
                tool: Some("fetch_email".into()),
                arguments: serde_json::json!({"folder": "sent", "limit": 30}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "budget-summary".into(),
                description: "Check budget summary for the week's financial activity".into(),
                tool: Some("budget_summary".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "compile-weekly-summary".into(),
                description: "Compile weekly summary: key accomplishments, blockers encountered, decisions made".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![0, 1, 2],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "fetch-next-week".into(),
                description: "Fetch next week's calendar to identify upcoming commitments and free blocks".into(),
                tool: Some("list_events".into()),
                arguments: serde_json::json!({"range": "next_week"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![3],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "draft-next-week-priorities".into(),
                description: "Draft next week's priorities and time-block suggestions based on review".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![3, 4],
                acceptance: vec![],
            },
        ],
        parameters: vec![],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 17 * * 5".into() }, // Friday 5pm
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Research digest — search the web for a topic, summarize findings, save to file.
fn research_digest() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "research-digest".into(),
        description: "Research {topic}: search the web, read top results, compile a digest and save to {output_file}".into(),
        steps: vec![
            TemplateStep {
                step_id: "search-web".into(),
                description: "Search the web for recent information on {topic}".into(),
                tool: Some("search_web".into()),
                arguments: serde_json::json!({"query": "{topic}"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "fetch-top-results".into(),
                description: "Fetch and read the top 3 most relevant results".into(),
                tool: Some("fetch_webpage".into()),
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "synthesize-digest".into(),
                description: "Synthesize findings into a structured digest with key takeaways, sources, and open questions".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "save-digest".into(),
                description: "Save the research digest to {output_file}".into(),
                tool: Some("write_file".into()),
                arguments: serde_json::json!({"path": "{output_file}"}),
                requires_approval: true,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![2],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "topic".into(),
                description: "Research topic or question to investigate".into(),
                required: true,
                default: None,
            },
            TemplateParameter {
                name: "output_file".into(),
                description: "File path to save the research digest".into(),
                required: false,
                default: Some("research-digest.md".into()),
            },
        ],
        triggers: vec![WorkflowTrigger::Manual],
        created_at: now,
        updated_at: now,
    }
}

/// Code review checklist — check PR diff, review changes, post comments.
fn code_review_checklist() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "code-review-checklist".into(),
        description: "Review PR #{pr_number}: fetch diff, check for issues, post review comments".into(),
        steps: vec![
            TemplateStep {
                step_id: "fetch-diff".into(),
                description: "Fetch the PR diff for #{pr_number}".into(),
                tool: Some("get_pr_diff".into()),
                arguments: serde_json::json!({"number": "{pr_number}", "full_diff": true}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "fetch-context".into(),
                description: "Fetch the PR's linked issue or description for context".into(),
                tool: Some("list_prs".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "analyze-diff".into(),
                description: "Analyze the diff for: correctness, security issues, error handling, test coverage, naming clarity, and unnecessary complexity".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0, 1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "post-comments".into(),
                description: "Post review comments on the PR — one comment per finding".into(),
                tool: Some("create_pr_comment".into()),
                arguments: serde_json::json!({"number": "{pr_number}"}),
                requires_approval: true,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![2],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "pr_number".into(),
                description: "Pull request number to review".into(),
                required: true,
                default: None,
            },
        ],
        triggers: vec![
            WorkflowTrigger::Webhook { secret_ref: None },
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Meeting prep — gather context before a meeting.
fn meeting_prep() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "meeting-prep".into(),
        description: "Prepare for meeting about {subject}: gather context, draft agenda, set reminders".into(),
        steps: vec![
            TemplateStep {
                step_id: "search-email-threads".into(),
                description: "Search emails for recent threads about {subject}".into(),
                tool: Some("fetch_email".into()),
                arguments: serde_json::json!({"query": "{subject}", "limit": 10}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "search-vault-docs".into(),
                description: "Check for related documents in the vault about {subject}".into(),
                tool: Some("search_documents".into()),
                arguments: serde_json::json!({"query": "{subject}"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "lookup-attendees".into(),
                description: "Look up contacts for {attendees} to understand roles and recent interactions".into(),
                tool: Some("search_contacts".into()),
                arguments: serde_json::json!({"query": "{attendees}"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "draft-agenda".into(),
                description: "Synthesize context and draft a meeting agenda with talking points and open questions".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![0, 1, 2],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "set-prep-reminder".into(),
                description: "Set a reminder {minutes_before} minutes before the meeting".into(),
                tool: Some("set_reminder".into()),
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![3],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "subject".into(),
                description: "Meeting topic or project name".into(),
                required: true,
                default: None,
            },
            TemplateParameter {
                name: "attendees".into(),
                description: "Names or emails of meeting attendees".into(),
                required: false,
                default: Some("".into()),
            },
            TemplateParameter {
                name: "minutes_before".into(),
                description: "Minutes before the meeting to set a reminder".into(),
                required: false,
                default: Some("15".into()),
            },
        ],
        triggers: vec![WorkflowTrigger::Manual],
        created_at: now,
        updated_at: now,
    }
}

/// Monthly budget review — comprehensive financial health check.
fn monthly_budget_review() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "monthly-budget-review".into(),
        description: "Monthly financial review: analyze spending vs budget, identify trends, flag overages".into(),
        steps: vec![
            TemplateStep {
                step_id: "budget-summary".into(),
                description: "Pull the full budget summary for the current month".into(),
                tool: Some("budget_summary".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "list-30d-transactions".into(),
                description: "List all transactions for the past 30 days".into(),
                tool: Some("list_transactions".into()),
                arguments: serde_json::json!({"days": "30"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "analyze-by-category".into(),
                description: "Analyze spending by category: compare actual vs budgeted amounts, calculate percentage used".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![0, 1],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "identify-anomalies".into(),
                description: "Identify anomalies: categories over budget, unusual transactions, recurring charges that changed".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![2],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "generate-monthly-report".into(),
                description: "Generate a concise monthly report with recommendations for next month's budget adjustments".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![3],
                acceptance: vec![],
            },
        ],
        parameters: vec![],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 10 1 * *".into() }, // 1st of month, 10am
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Project status report — git activity, open issues, CI status.
fn project_status_report() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "project-status-report".into(),
        description: "Generate a project status report: recent commits, open issues, CI health, PR activity".into(),
        steps: vec![
            TemplateStep {
                step_id: "git-log".into(),
                description: "Fetch recent git log for the past {days} days".into(),
                tool: Some("git_log".into()),
                arguments: serde_json::json!({"limit": 20}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "list-issues".into(),
                description: "List open issues".into(),
                tool: Some("list_issues".into()),
                arguments: serde_json::json!({"state": "open"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "ci-status".into(),
                description: "Check CI pipeline status for the default branch".into(),
                tool: Some("ci_status".into()),
                arguments: serde_json::json!({}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "list-open-prs".into(),
                description: "List open pull requests".into(),
                tool: Some("list_prs".into()),
                arguments: serde_json::json!({"state": "open"}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "compile-status-report".into(),
                description: "Compile a project status report: development velocity, blockers, CI health, review bottlenecks".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![0, 1, 2, 3],
                acceptance: vec![],
            },
        ],
        parameters: vec![
            TemplateParameter {
                name: "days".into(),
                description: "Number of days of git history to include".into(),
                required: false,
                default: Some("7".into()),
            },
        ],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 9 * * 1".into() }, // Monday 9am
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Strategy review — weekly self-review of all goals and progress patterns.
///
/// Triggered every Sunday at 9 AM. Sets the `strategy_review_pending` flag
/// on the loop context so the next heartbeat tick gathers extended context
/// (all goals including recently completed/abandoned) and generates a
/// `StrategyReview` action.
fn strategy_review() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "strategy-review".into(),
        description: "Weekly strategic review: analyze all goal progress, detect patterns, suggest adjustments".into(),
        steps: vec![
            TemplateStep {
                step_id: "list-goals".into(),
                description: "Review all active, completed, and stalled goals from the past week".into(),
                tool: Some("list_goals".into()),
                arguments: serde_json::json!({"include_completed": true}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "analyze-patterns".into(),
                description: "Analyze progress patterns: completion velocity, recurring blockers, stalled items, shifting priorities".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "generate-adjustments".into(),
                description: "Generate strategic adjustments: reprioritize goals, flag at-risk deadlines, update domain confidence".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
        ],
        parameters: vec![],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 9 * * 0".into() }, // Sunday 9am
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

/// Milestone scan — monthly deep scan for goal anniversaries and achievements.
///
/// Triggered on the 1st of each month at 9 AM. Complements the daily
/// milestone checks in the heartbeat by scanning for longer-term
/// anniversaries (monthly, quarterly, yearly).
fn milestone_scan() -> WorkflowTemplate {
    let now = Utc::now();
    WorkflowTemplate {
        name: "milestone-scan".into(),
        description: "Monthly milestone scan: detect goal anniversaries, celebrate long-term achievements".into(),
        steps: vec![
            TemplateStep {
                step_id: "list-all-goals".into(),
                description: "List all goals including completed ones to scan for anniversaries".into(),
                tool: Some("list_goals".into()),
                arguments: serde_json::json!({"include_completed": true}),
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "identify-anniversaries".into(),
                description: "Identify milestone anniversaries: 1 month, 3 months, 6 months, 1 year since creation or completion".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![0],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "generate-celebrations".into(),
                description: "Generate celebration messages for detected milestones — calibrate tone to persona".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: Some(StepCondition::OnSuccess),
                depends_on: vec![1],
                acceptance: vec![],
            },
        ],
        parameters: vec![],
        triggers: vec![
            WorkflowTrigger::Cron { expression: "0 9 1 * *".into() }, // 1st of month, 9am
            WorkflowTrigger::Manual,
        ],
        created_at: now,
        updated_at: now,
    }
}

// ── Seeding ──────────────────────────────────────────────────

/// Install library templates into the encrypted store.
///
/// Only installs templates that don't already exist — existing templates
/// (including user-modified versions) are preserved. Returns the count of
/// newly installed templates.
pub fn seed_library(ctx: &WorkflowContext) -> Result<usize> {
    let key = ctx.workflow_key()?;
    let existing = list_templates(&ctx.store)?;
    let mut installed = 0;

    for template in all_templates() {
        if !existing.contains(&template.name) {
            save_template(&ctx.store, &key, &template)?;
            installed += 1;
        }
    }

    Ok(installed)
}

/// Force-install all library templates, overwriting any existing ones.
///
/// Preserves the original `created_at` if the template already exists.
/// Returns the total count of templates installed.
pub fn reinstall_library(ctx: &WorkflowContext) -> Result<usize> {
    let key = ctx.workflow_key()?;
    let mut count = 0;

    for mut template in all_templates() {
        // Preserve original creation timestamp if it already exists
        if let Some(existing) = load_template(&ctx.store, &key, &template.name)? {
            template.created_at = existing.created_at;
        }
        save_template(&ctx.store, &key, &template)?;
        count += 1;
    }

    Ok(count)
}

// ── Actions ──────────────────────────────────────────────────

/// Install (or reinstall) the built-in workflow template library.
pub struct InstallLibraryAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for InstallLibraryAction {
    fn name(&self) -> &str {
        "install_workflow_library"
    }

    fn description(&self) -> &str {
        "Install the built-in workflow template library. Use force=true to \
         overwrite existing library templates with fresh versions."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "force": {
                    "type": "boolean",
                    "description": "If true, overwrite existing library templates (preserves created_at). Default: false (skip existing)."
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let force = input
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let count = if force {
            reinstall_library(&self.ctx)?
        } else {
            seed_library(&self.ctx)?
        };

        let total = library_names().len();

        Ok(serde_json::json!({
            "status": "ok",
            "installed": count,
            "total_library_templates": total,
            "mode": if force { "reinstall" } else { "seed" },
            "templates": library_names(),
        }))
    }
}

/// Delete a workflow template by name.
pub struct DeleteWorkflowAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for DeleteWorkflowAction {
    fn name(&self) -> &str {
        "delete_workflow"
    }

    fn description(&self) -> &str {
        "Delete a workflow template by name. This is irreversible — the template \
         and all its triggers will be removed."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the workflow template to delete"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'name'".into()))?;

        // Verify it exists first
        let key = self.ctx.workflow_key()?;
        let existed = load_template(&self.ctx.store, &key, name)?.is_some();

        if !existed {
            return Err(aivyx_core::AivyxError::Other(format!(
                "workflow template '{name}' not found"
            )));
        }

        super::delete_template(&self.ctx.store, name)?;

        Ok(serde_json::json!({
            "status": "ok",
            "deleted": name,
        }))
    }
}

// ── Tests ────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn all_templates_have_unique_names() {
        let templates = all_templates();
        let mut names: Vec<&str> = templates.iter().map(|t| t.name.as_str()).collect();
        let original_len = names.len();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), original_len, "duplicate template names found");
    }

    #[test]
    fn all_templates_names_match_index() {
        let templates = all_templates();
        let names = library_names();
        assert_eq!(templates.len(), names.len());
        for (t, n) in templates.iter().zip(names.iter()) {
            assert_eq!(t.name, *n);
        }
    }

    #[test]
    fn all_templates_have_steps() {
        for t in all_templates() {
            assert!(!t.steps.is_empty(), "template '{}' has no steps", t.name);
        }
    }

    #[test]
    fn all_templates_have_descriptions() {
        for t in all_templates() {
            assert!(
                !t.description.is_empty(),
                "template '{}' has no description",
                t.name
            );
            for (i, s) in t.steps.iter().enumerate() {
                assert!(
                    !s.description.is_empty(),
                    "template '{}' step {} has no description",
                    t.name,
                    i
                );
            }
        }
    }

    #[test]
    fn all_templates_have_at_least_manual_trigger() {
        for t in all_templates() {
            assert!(
                !t.triggers.is_empty(),
                "template '{}' has no triggers",
                t.name,
            );
            let has_manual = t
                .triggers
                .iter()
                .any(|tr| matches!(tr, WorkflowTrigger::Manual));
            assert!(
                has_manual,
                "template '{}' should have a Manual trigger fallback",
                t.name
            );
        }
    }

    #[test]
    fn all_templates_json_roundtrip() {
        for t in all_templates() {
            let json = serde_json::to_string(&t).unwrap();
            let parsed: WorkflowTemplate = serde_json::from_str(&json).unwrap();
            assert_eq!(parsed.name, t.name);
            assert_eq!(parsed.steps.len(), t.steps.len());
            assert_eq!(parsed.parameters.len(), t.parameters.len());
            assert_eq!(parsed.triggers.len(), t.triggers.len());
        }
    }

    #[test]
    fn depends_on_indices_valid() {
        for t in all_templates() {
            let step_count = t.steps.len();
            for (i, s) in t.steps.iter().enumerate() {
                for &dep in &s.depends_on {
                    assert!(
                        dep < i,
                        "template '{}' step {} depends on step {} which is not before it",
                        t.name,
                        i,
                        dep,
                    );
                    assert!(
                        dep < step_count,
                        "template '{}' step {} depends on non-existent step {}",
                        t.name,
                        i,
                        dep,
                    );
                }
            }
        }
    }

    #[test]
    fn parameterized_templates_instantiate() {
        // Templates with required params should fail without them
        // and succeed with them
        for t in all_templates() {
            let has_required = t
                .parameters
                .iter()
                .any(|p| p.required && p.default.is_none());

            if has_required {
                // Should fail with empty params
                let result = t.instantiate(&HashMap::new());
                assert!(
                    result.is_err(),
                    "template '{}' should require params",
                    t.name
                );
            }

            // Should succeed with all params provided
            let mut params = HashMap::new();
            for p in &t.parameters {
                if p.default.is_none() {
                    params.insert(p.name.clone(), format!("test_{}", p.name));
                }
            }
            let result = t.instantiate(&params);
            assert!(
                result.is_ok(),
                "template '{}' failed to instantiate: {:?}",
                t.name,
                result.err()
            );

            // Verify placeholders were replaced
            let inst = result.unwrap();
            for step in &inst.steps {
                for p in &t.parameters {
                    if let Some(val) = params.get(&p.name) {
                        // If the original step used this param, the instantiated version
                        // should have the test value, not the placeholder
                        if t.steps
                            .iter()
                            .any(|s| s.description.contains(&format!("{{{}}}", p.name)))
                        {
                            assert!(
                                !step.description.contains(&format!("{{{}}}", p.name))
                                    || !t.steps.iter().any(|s| s.description == step.description),
                                "template '{}' has unreplaced placeholder for '{}'",
                                t.name,
                                p.name,
                            );
                        }
                        let _ = val; // suppress unused warning
                    }
                }
            }
        }
    }

    #[test]
    fn library_has_expected_count() {
        assert_eq!(all_templates().len(), 12, "expected 12 library templates");
        assert_eq!(library_names().len(), 12, "expected 12 library names");
    }

    #[test]
    fn all_templates_have_non_empty_step_ids() {
        // Slice A invariant: every library step must have an explicit,
        // non-empty `step_id`. The load-path and instantiate-path both
        // backfill empty IDs with positional defaults, but library
        // templates are authored code — they should be canonical so that
        // reviewers reading the source see a meaningful identifier
        // rather than relying on the backfill.
        for t in all_templates() {
            for (i, s) in t.steps.iter().enumerate() {
                assert!(
                    !s.step_id.is_empty(),
                    "template '{}' step {} ({}) has empty step_id",
                    t.name,
                    i,
                    s.description,
                );
            }
        }
    }

    #[test]
    fn all_templates_have_unique_step_ids() {
        // Slice A invariant: within a single template, step IDs must be
        // unique. Mission plan cursors and acceptance reports reference
        // steps by `step_id`, so duplicates would make resolution
        // ambiguous.
        for t in all_templates() {
            let mut ids: Vec<&str> = t.steps.iter().map(|s| s.step_id.as_str()).collect();
            let original = ids.len();
            ids.sort();
            ids.dedup();
            assert_eq!(
                ids.len(),
                original,
                "template '{}' has duplicate step_id(s)",
                t.name,
            );
        }
    }

    #[test]
    fn instantiated_steps_carry_library_step_ids() {
        // End-to-end check: the `step_id` that the library authored on
        // each `TemplateStep` must reach every `InstantiatedStep` after
        // `instantiate()` runs.
        for t in all_templates() {
            // Build params that satisfy any required placeholders.
            let mut params = HashMap::new();
            for p in &t.parameters {
                if p.default.is_none() {
                    params.insert(p.name.clone(), format!("test_{}", p.name));
                }
            }
            let inst = t
                .instantiate(&params)
                .unwrap_or_else(|e| panic!("template '{}' instantiate failed: {e}", t.name));
            assert_eq!(inst.steps.len(), t.steps.len());
            for (template_step, instantiated_step) in t.steps.iter().zip(inst.steps.iter()) {
                assert_eq!(
                    instantiated_step.step_id, template_step.step_id,
                    "template '{}' dropped step_id during instantiation",
                    t.name,
                );
            }
        }
    }

    #[test]
    fn cron_templates_have_valid_expressions() {
        for t in all_templates() {
            for trigger in &t.triggers {
                if let WorkflowTrigger::Cron { expression } = trigger {
                    let parts: Vec<&str> = expression.split_whitespace().collect();
                    assert_eq!(
                        parts.len(),
                        5,
                        "template '{}' has invalid cron expression '{}' (expected 5 fields)",
                        t.name,
                        expression,
                    );
                }
            }
        }
    }
}
