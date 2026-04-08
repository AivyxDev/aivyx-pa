//! Bridge between aivyx-actions `Action` and aivyx-core `Tool` trait.
//!
//! Wraps any `Action` as a `Tool` so it can be registered in the agent's
//! `ToolRegistry` and invoked by the LLM during turns.

use crate::Action;
use aivyx_core::{CapabilityScope, ToolId};

/// Wraps an `Action` as an aivyx-core `Tool`.
pub struct ActionTool {
    id: ToolId,
    action: Box<dyn Action>,
    scope: Option<CapabilityScope>,
}

impl ActionTool {
    pub fn new(action: Box<dyn Action>) -> Self {
        Self {
            id: ToolId::new(),
            action,
            scope: None,
        }
    }

    pub fn with_scope(mut self, scope: CapabilityScope) -> Self {
        self.scope = Some(scope);
        self
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for ActionTool {
    fn id(&self) -> ToolId {
        self.id
    }

    fn name(&self) -> &str {
        self.action.name()
    }

    fn description(&self) -> &str {
        self.action.description()
    }

    fn input_schema(&self) -> serde_json::Value {
        self.action.input_schema()
    }

    fn required_scope(&self) -> Option<CapabilityScope> {
        self.scope.clone()
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        self.action.execute(input).await
    }
}

/// Register stateless default actions into a `ToolRegistry`.
///
/// Reminder actions are NOT included here — they require an EncryptedStore
/// and MasterKey. Use `register_reminder_actions` separately.
pub fn register_default_actions(registry: &mut aivyx_core::ToolRegistry) {
    use crate::files::{ListDirectory, ReadFile, WriteFile};
    use crate::shell::RunCommand;
    use crate::web::{FetchPage, SearchWeb};

    // File tools — no capability scope needed (governed by autonomy tier)
    registry.register(Box::new(ActionTool::new(Box::new(ReadFile))));
    registry.register(Box::new(ActionTool::new(Box::new(WriteFile))));
    registry.register(Box::new(ActionTool::new(Box::new(ListDirectory))));

    // Shell — requires Shell capability scope
    registry.register(Box::new(
        ActionTool::new(Box::new(RunCommand))
            .with_scope(CapabilityScope::Shell {
                allowed_commands: vec![],
            }),
    ));

    // Web
    registry.register(Box::new(ActionTool::new(Box::new(FetchPage))));
    registry.register(Box::new(ActionTool::new(Box::new(SearchWeb))));
}

/// Register reminder actions that persist to the encrypted store.
pub fn register_reminder_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
) {
    use crate::reminders::{DismissReminder, ListReminders, SetReminder, UpdateReminder};

    registry.register(Box::new(ActionTool::new(Box::new(SetReminder {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"reminders"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListReminders {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"reminders"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DismissReminder {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"reminders"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(UpdateReminder {
        store,
        key: aivyx_crypto::derive_domain_key(key, b"reminders"),
    }))));
}

/// Register a read-only subset of tools suitable for autonomous scheduled
/// prompts. Includes reading (not writing) files, web search/fetch, and
/// email reading (not sending). No shell access.
pub fn register_schedule_actions(
    registry: &mut aivyx_core::ToolRegistry,
    email_config: Option<crate::email::EmailConfig>,
    imap_pool: Option<std::sync::Arc<crate::email::ImapPool>>,
    telegram_config: Option<crate::messaging::TelegramConfig>,
    matrix_config: Option<crate::messaging::MatrixConfig>,
    signal_config: Option<crate::messaging::SignalConfig>,
) {
    use crate::files::{ListDirectory, ReadFile};
    use crate::web::{FetchPage, SearchWeb};

    // Read-only file access
    registry.register(Box::new(ActionTool::new(Box::new(ReadFile))));
    registry.register(Box::new(ActionTool::new(Box::new(ListDirectory))));

    // Web (read-only by nature)
    registry.register(Box::new(ActionTool::new(Box::new(FetchPage))));
    registry.register(Box::new(ActionTool::new(Box::new(SearchWeb))));

    // Email reading (not sending)
    if let Some(config) = email_config {
        use crate::email::{FetchEmail, ReadInbox};
        registry.register(Box::new(ActionTool::new(Box::new(ReadInbox {
            config: config.clone(),
            pool: imap_pool.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(FetchEmail {
            config,
            pool: imap_pool,
        }))));
    }

    // Messaging reading (not sending)
    if let Some(config) = telegram_config {
        use crate::messaging::telegram::ReadTelegram;
        registry.register(Box::new(ActionTool::new(Box::new(ReadTelegram { config }))));
    }
    if let Some(config) = matrix_config {
        use crate::messaging::matrix::ReadMatrix;
        registry.register(Box::new(ActionTool::new(Box::new(ReadMatrix { config }))));
    }
    if let Some(config) = signal_config {
        use crate::messaging::signal::ReadSignal;
        registry.register(Box::new(ActionTool::new(Box::new(ReadSignal { config }))));
    }
}

/// Register email actions if email config is available.
pub fn register_email_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::email::EmailConfig,
    pool: Option<std::sync::Arc<crate::email::ImapPool>>,
) {
    use crate::email::{
        ArchiveEmail, DeleteEmail, FetchEmail, MarkEmailRead, ReadInbox, SendEmail,
    };

    registry.register(Box::new(ActionTool::new(Box::new(ReadInbox {
        config: config.clone(),
        pool: pool.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(FetchEmail {
        config: config.clone(),
        pool: pool.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(MarkEmailRead {
        config: config.clone(),
        pool: pool.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ArchiveEmail {
        config: config.clone(),
        pool: pool.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteEmail {
        config: config.clone(),
        pool,
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SendEmail { config }))));
}

/// Register Telegram messaging actions.
pub fn register_telegram_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::messaging::TelegramConfig,
) {
    use crate::messaging::telegram::{ReadTelegram, SendTelegram};

    registry.register(Box::new(ActionTool::new(Box::new(ReadTelegram {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SendTelegram { config }))));
}

/// Register Matrix messaging actions.
pub fn register_matrix_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::messaging::MatrixConfig,
) {
    use crate::messaging::matrix::{ReadMatrix, SendMatrix};

    registry.register(Box::new(ActionTool::new(Box::new(ReadMatrix {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SendMatrix { config }))));
}

/// Register calendar actions if calendar config is available.
pub fn register_calendar_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::calendar::CalendarConfig,
) {
    use crate::calendar::{
        CheckConflicts, CreateCalendarEvent, DeleteCalendarEvent, FetchCalendarEvents,
        TodayAgenda, UpdateCalendarEvent,
    };

    registry.register(Box::new(ActionTool::new(Box::new(TodayAgenda {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(FetchCalendarEvents {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(CheckConflicts {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(CreateCalendarEvent {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(UpdateCalendarEvent {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteCalendarEvent {
        config,
    }))));
}

/// Register contact actions.
///
/// When `carddav_config` is `Some`, includes the `sync_contacts` tool that
/// can pull contacts from the CardDAV server. Search/list always work against
/// the local encrypted store.
pub fn register_contact_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
    carddav_config: Option<crate::contacts::ContactsConfig>,
) -> Result<(), aivyx_core::AivyxError> {
    use crate::contacts::{
        AddContact, DeleteContact, ListContacts, SearchContacts, SyncContacts, UpdateContact,
    };

    // Derive the contacts domain key once, then copy bytes for each tool
    // (MasterKey is not Clone — intentionally, to prevent accidental leaks).
    let contacts_key_bytes: [u8; 32] = aivyx_crypto::derive_domain_key(key, b"contacts")
        .expose_secret()
        .try_into()
        .map_err(|_| aivyx_core::AivyxError::Crypto("domain key is not 32 bytes".into()))?;

    registry.register(Box::new(ActionTool::new(Box::new(SearchContacts {
        store: store.clone(),
        key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListContacts {
        store: store.clone(),
        key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
    }))));

    registry.register(Box::new(ActionTool::new(Box::new(AddContact {
        store: store.clone(),
        key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(UpdateContact {
        store: store.clone(),
        key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteContact {
        store: store.clone(),
        key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
    }))));

    // Sync tool only available when CardDAV is configured.
    if let Some(config) = carddav_config {
        registry.register(Box::new(ActionTool::new(Box::new(SyncContacts {
            config,
            store,
            key: aivyx_crypto::MasterKey::from_bytes(contacts_key_bytes),
        }))));
    }

    Ok(())
}

/// Register document vault tools if vault config is available.
pub fn register_document_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::documents::VaultConfig,
    memory: std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    vault_key: aivyx_crypto::MasterKey,
) {
    use crate::documents::{
        DeleteDocument, IndexVault, ListVaultDocuments, ReadDocument, SearchDocuments,
    };

    // vault_key is not Clone; copy the bytes to share between tools.
    let key_bytes: [u8; 32] = vault_key.expose_secret().try_into().expect("vault key 32 bytes");

    registry.register(Box::new(ActionTool::new(Box::new(SearchDocuments {
        memory: memory.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ReadDocument {
        vault_path: config.path.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListVaultDocuments {
        vault_path: config.path.clone(),
        extensions: if config.extensions.is_empty() {
            vec!["md".into(), "txt".into(), "pdf".into()]
        } else {
            config.extensions.clone()
        },
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteDocument {
        vault_path: config.path.clone(),
        memory: memory.clone(),
        store: store.clone(),
        vault_key: aivyx_crypto::MasterKey::from_bytes(key_bytes),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(IndexVault {
        config,
        memory,
        store,
        vault_key: aivyx_crypto::MasterKey::from_bytes(key_bytes),
    }))));
}

/// Register finance tracking tools.
///
/// When `email_config` and `vault_path` are provided, the `file_receipt` tool
/// is also registered (it needs email access to fetch receipt bodies and
/// vault access to write receipt files).
pub fn register_finance_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
    email_config: Option<&crate::email::EmailConfig>,
    vault_path: Option<&std::path::Path>,
    receipt_folder: &str,
) {
    use crate::finance::{
        AddTransaction, BudgetSummary, DeleteBudget, DeleteTransactionAction,
        FileReceipt, ListTransactions, MarkBillPaid, SetBudget, UpdateTransaction,
    };

    let finance_key = aivyx_crypto::derive_domain_key(key, b"finance");

    registry.register(Box::new(ActionTool::new(Box::new(AddTransaction {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListTransactions {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(BudgetSummary {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SetBudget {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(MarkBillPaid {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteTransactionAction {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(UpdateTransaction {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"finance"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteBudget {
        store: store.clone(),
    }))));

    // FileReceipt needs both email access and vault path.
    if let (Some(ec), Some(vp)) = (email_config, vault_path) {
        registry.register(Box::new(ActionTool::new(Box::new(FileReceipt {
            store,
            key: finance_key,
            email_config: ec.clone(),
            vault_path: vp.to_path_buf(),
            receipt_folder: receipt_folder.to_string(),
        }))));
    }
}

/// Register workflow management tools (create, list, run, status, delete, library).
pub fn register_workflow_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
) {
    use crate::workflow::{
        CreateWorkflowAction, ListWorkflowsAction, RunWorkflowAction,
        WorkflowContext, WorkflowStatusAction,
    };
    use crate::workflow::library::{DeleteWorkflowAction, InstallLibraryAction};

    let workflow_key = aivyx_crypto::derive_domain_key(key, b"workflow");
    let ctx = WorkflowContext::new(store, &workflow_key);

    registry.register(Box::new(ActionTool::new(Box::new(CreateWorkflowAction {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListWorkflowsAction {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(RunWorkflowAction {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(WorkflowStatusAction {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteWorkflowAction {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(InstallLibraryAction {
        ctx,
    }))));
}

/// Register knowledge graph query tools (traverse, find paths, search, stats).
pub fn register_knowledge_actions(
    registry: &mut aivyx_core::ToolRegistry,
    memory: std::sync::Arc<tokio::sync::Mutex<aivyx_memory::MemoryManager>>,
) {
    use crate::knowledge::{
        DeleteKnowledgeTriple, FindKnowledgePaths, KnowledgeContext, KnowledgeGraphStats,
        SearchKnowledgeEntities, TraverseKnowledgeGraph,
    };

    let ctx = KnowledgeContext { memory };

    registry.register(Box::new(ActionTool::new(Box::new(TraverseKnowledgeGraph {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(FindKnowledgePaths {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SearchKnowledgeEntities {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(KnowledgeGraphStats {
        ctx: ctx.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(DeleteKnowledgeTriple {
        ctx,
    }))));
}

/// Register email triage tools (log viewer + rule manager).
///
/// Only registers when triage is enabled. These tools let the user inspect
/// what the agent has done autonomously and add/modify auto-reply rules.
pub fn register_triage_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
) {
    use crate::triage_tools::{ListTriageLog, SetTriageRule};

    let triage_key = aivyx_crypto::derive_domain_key(key, b"triage");

    registry.register(Box::new(ActionTool::new(Box::new(ListTriageLog {
        store: store.clone(),
        key: aivyx_crypto::derive_domain_key(key, b"triage"),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SetTriageRule {
        store,
        key: triage_key,
    }))));
}

/// Register plugin management tools (list, install, enable/disable, uninstall, search).
pub fn register_plugin_actions(
    registry: &mut aivyx_core::ToolRegistry,
    state: crate::plugin::PluginState,
) {
    use crate::plugin::{
        InstallPlugin, ListPlugins, SearchPluginRegistry, TogglePlugin, UninstallPlugin,
    };

    registry.register(Box::new(ActionTool::new(Box::new(ListPlugins {
        state: state.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(InstallPlugin {
        state: state.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(TogglePlugin {
        state: state.clone(),
        enable: true,
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(TogglePlugin {
        state: state.clone(),
        enable: false,
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(UninstallPlugin {
        state,
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SearchPluginRegistry))));
}

/// Register Signal messaging actions.
pub fn register_signal_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::messaging::SignalConfig,
) {
    use crate::messaging::signal::{ReadSignal, SendSignal};

    registry.register(Box::new(ActionTool::new(Box::new(ReadSignal {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SendSignal { config }))));
}

/// Register SMS gateway actions.
pub fn register_sms_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::messaging::SmsConfig,
) {
    use crate::messaging::sms::SendSms;

    registry.register(Box::new(ActionTool::new(Box::new(SendSms { config }))));
}

/// Register dev tools (git + CI/CD when forge is configured).
pub fn register_devtools_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::devtools::DevToolsConfig,
) {
    use crate::devtools::git::{GitBranches, GitDiff, GitLog, GitStatus};

    // Local git tools — always registered
    registry.register(Box::new(ActionTool::new(Box::new(GitLog {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(GitDiff {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(GitStatus {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(GitBranches {
        config: config.clone(),
    }))));

    // Forge-dependent tools — CI/CD, issues, PRs (only when forge is configured)
    if config.forge.is_some() {
        use crate::devtools::ci::{CiLogs, CiStatus};
        use crate::devtools::issues::{CreateIssue, GetIssue, ListIssues};
        use crate::devtools::pr::{CreatePrComment, GetPrDiff, ListPrs};

        registry.register(Box::new(ActionTool::new(Box::new(CiStatus {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(CiLogs {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(ListIssues {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(GetIssue {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(CreateIssue {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(ListPrs {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(GetPrDiff {
            config: config.clone(),
        }))));
        registry.register(Box::new(ActionTool::new(Box::new(CreatePrComment { config }))));
    }
}

/// Register undo system tools (record, list, execute undo).
///
/// Provides three tools:
/// - `record_undo` — record a reversible action before executing it
/// - `list_undo_history` — show recent undoable actions
/// - `undo_action` — reverse a previous action by ID
pub fn register_undo_actions(
    registry: &mut aivyx_core::ToolRegistry,
    store: std::sync::Arc<aivyx_crypto::EncryptedStore>,
    key: &aivyx_crypto::MasterKey,
) {
    use crate::undo::{ListUndoHistoryAction, RecordUndoAction, UndoActionTool, UndoContext};

    let undo_key = aivyx_crypto::derive_domain_key(key, b"undo");
    let ctx = UndoContext::new(store, &undo_key);

    registry.register(Box::new(ActionTool::new(Box::new(RecordUndoAction::new(
        ctx.clone(),
    )))));
    registry.register(Box::new(ActionTool::new(Box::new(
        ListUndoHistoryAction::new(ctx.clone()),
    ))));
    registry.register(Box::new(ActionTool::new(Box::new(UndoActionTool::new(
        ctx,
    )))));
}

/// Register desktop interaction tools (app launching, clipboard, windows, notifications).
///
/// All tools are gated by `CapabilityScope::Custom("desktop")`. Sub-features
/// (clipboard, windows, notifications) are individually toggled via config.
pub fn register_desktop_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::desktop::DesktopConfig,
) {
    use aivyx_core::CapabilityScope;
    use crate::desktop::open::OpenApplication;

    let scope = || CapabilityScope::Custom("desktop".into());

    // App launching is always registered when [desktop] is present
    registry.register(Box::new(
        ActionTool::new(Box::new(OpenApplication { config: config.clone() }))
            .with_scope(scope()),
    ));

    if config.clipboard {
        use crate::desktop::clipboard::{ClipboardRead, ClipboardWrite};
        registry.register(Box::new(
            ActionTool::new(Box::new(ClipboardRead)).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(ClipboardWrite)).with_scope(scope()),
        ));
    }

    if config.windows {
        use crate::desktop::windows::{FocusWindow, GetActiveWindow, ListWindows};
        registry.register(Box::new(
            ActionTool::new(Box::new(ListWindows)).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(GetActiveWindow)).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(FocusWindow)).with_scope(scope()),
        ));
    }

    if config.notifications {
        use crate::desktop::notify::SendNotification;
        registry.register(Box::new(
            ActionTool::new(Box::new(SendNotification)).with_scope(scope()),
        ));
    }

    // Deep interaction tools (AT-SPI2/UIA, CDP, MPRIS/SMTC, ydotool/SendInput).
    if let Some(ref ic) = config.interaction {
        if ic.enabled {
            register_interaction_actions(registry, ic.clone(), config.clone());
        }
    }
}

/// Register deep application interaction tools.
///
/// 48 tools across semantic UI (AT-SPI2/UIA), browser (CDP), media (MPRIS/SMTC),
/// input injection (ydotool/SendInput), window management, system controls, OCR,
/// desktop awareness, and document creation/editing.
/// Cross-platform: Linux uses AT-SPI2 + ydotool + D-Bus; Windows uses UIA + SendInput + SMTC.
/// All gated by `CapabilityScope::Custom("desktop")`.
fn register_interaction_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::desktop::interaction::InteractionConfig,
    desktop_config: crate::desktop::DesktopConfig,
) {
    use aivyx_core::CapabilityScope;
    use crate::desktop::interaction::{InteractionContext, tools::*};

    let scope = || CapabilityScope::Custom("desktop".into());
    let ctx = InteractionContext::new(config.clone(), desktop_config);

    // Semantic UI tools (always registered — fall back to ydotool on Linux, SendInput on Windows).
    registry.register(Box::new(
        ActionTool::new(Box::new(UiInspect { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiFindElement { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiClick { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiTypeText { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiReadText { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiScroll { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiRightClick { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiHover { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiDrag { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Key combo + mouse move (ydotool on Linux, SendInput on Windows — always available).
    if config.input.enabled {
        registry.register(Box::new(
            ActionTool::new(Box::new(UiKeyCombo { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(UiMouseMove { ctx: ctx.clone() })).with_scope(scope()),
        ));
    }

    // Window screenshot (subprocess — always available).
    registry.register(Box::new(
        ActionTool::new(Box::new(WindowScreenshot { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Browser automation tools (CDP).
    if config.browser.enabled {
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserNavigate { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserQuery { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserClick { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserType { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserReadPage { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserScreenshot { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserScroll { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserExecuteJs { ctx: ctx.clone() })).with_scope(scope()),
        ));
    }

    // Double-click + middle-click (semantic UI, always available).
    registry.register(Box::new(
        ActionTool::new(Box::new(UiDoubleClick { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiMiddleClick { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Select dropdown + clear field (browser or native).
    registry.register(Box::new(
        ActionTool::new(Box::new(UiSelectOption { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(UiClearField { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Window management (wmctrl/xdotool/hyprctl on Linux, Win32 on Windows).
    registry.register(Box::new(
        ActionTool::new(Box::new(WindowManage { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // System controls (wpctl/brightnessctl on Linux, WASAPI/WMI on Windows).
    registry.register(Box::new(
        ActionTool::new(Box::new(SystemVolume { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(SystemBrightness { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(NotificationList { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(FileManagerShow { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Browser tab management + wait (CDP).
    if config.browser.enabled {
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserListTabs { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserNewTab { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserCloseTab { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserWaitFor { ctx: ctx.clone() })).with_scope(scope()),
        ));
    }

    // Multi-select (Ctrl+click via input backend — always available).
    registry.register(Box::new(
        ActionTool::new(Box::new(UiMultiSelect { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Screen OCR (tesseract on Linux, Windows.Media.Ocr on Windows).
    registry.register(Box::new(
        ActionTool::new(Box::new(ScreenOcr { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Desktop awareness (running apps + workspaces — always available).
    registry.register(Box::new(
        ActionTool::new(Box::new(ListRunningApps { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DesktopWorkspace { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Browser PDF + find text (CDP).
    if config.browser.enabled {
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserPdf { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(BrowserFindText { ctx: ctx.clone() })).with_scope(scope()),
        ));
    }

    // Document tools (subprocess-based — always available).
    registry.register(Box::new(
        ActionTool::new(Box::new(DocCreateText { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DocCreateSpreadsheet { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DocCreatePdf { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DocEditText { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DocConvert { ctx: ctx.clone() })).with_scope(scope()),
    ));
    registry.register(Box::new(
        ActionTool::new(Box::new(DocReadPdf { ctx: ctx.clone() })).with_scope(scope()),
    ));

    // Media tools (D-Bus MPRIS on Linux, SMTC on Windows).
    if config.media.enabled {
        registry.register(Box::new(
            ActionTool::new(Box::new(MediaControl { ctx: ctx.clone() })).with_scope(scope()),
        ));
        registry.register(Box::new(
            ActionTool::new(Box::new(MediaInfo { ctx })).with_scope(scope()),
        ));
    }
}

/// Register OS background task management tools.
pub fn register_task_actions(
    registry: &mut aivyx_core::ToolRegistry,
    task_registry: crate::tasks::TaskRegistry,
    persist_path: Option<std::sync::Arc<std::path::PathBuf>>,
) {
    use crate::tasks::{CancelTask, GetTaskStatus, ListTasks, SpawnTask};
    
    registry.register(Box::new(ActionTool::new(Box::new(SpawnTask {
        registry: task_registry.clone(),
        persist_path: persist_path.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(GetTaskStatus {
        registry: task_registry.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(ListTasks {
        registry: task_registry.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(CancelTask {
        registry: task_registry,
        persist_path,
    }))));
}

/// Register read-only system monitoring tools.
pub fn register_monitor_actions(registry: &mut aivyx_core::ToolRegistry) {
    use crate::monitor::{CheckDiskSpace, CheckProcess, CheckUrlHealth, SystemStats, TailLog};

    registry.register(Box::new(ActionTool::new(Box::new(CheckDiskSpace))));
    registry.register(Box::new(ActionTool::new(Box::new(CheckProcess))));
    registry.register(Box::new(ActionTool::new(Box::new(TailLog))));
    registry.register(Box::new(ActionTool::new(Box::new(CheckUrlHealth))));
    registry.register(Box::new(ActionTool::new(Box::new(SystemStats))));
}
