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

/// Register all default actions into a `ToolRegistry`.
pub fn register_default_actions(registry: &mut aivyx_core::ToolRegistry) {
    use crate::files::{ListDirectory, ReadFile, WriteFile};
    use crate::reminders::{ListReminders, SetReminder};
    use crate::shell::RunCommand;
    use crate::web::FetchPage;

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

    // Reminders
    registry.register(Box::new(ActionTool::new(Box::new(SetReminder))));
    registry.register(Box::new(ActionTool::new(Box::new(ListReminders))));
}

/// Register email actions if email config is available.
pub fn register_email_actions(
    registry: &mut aivyx_core::ToolRegistry,
    config: crate::email::EmailConfig,
) {
    use crate::email::{ReadInbox, SendEmail};

    registry.register(Box::new(ActionTool::new(Box::new(ReadInbox {
        config: config.clone(),
    }))));
    registry.register(Box::new(ActionTool::new(Box::new(SendEmail { config }))));
}
