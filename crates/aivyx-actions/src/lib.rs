//! aivyx-actions — Real-world integrations for the Aivyx personal assistant.
//!
//! Each action module provides a capability the assistant can use to interact
//! with the outside world: reading email, managing files, running shell
//! commands, searching the web, and setting reminders.
//!
//! Actions are registered with the agent's tool registry so the LLM can
//! invoke them during conversation or autonomously via the agent loop.

pub mod email;
pub mod files;
pub mod reminders;
pub mod shell;
pub mod web;

use aivyx_core::{AivyxError, Result};

/// Describes an action the assistant can take in the real world.
#[async_trait::async_trait]
pub trait Action: Send + Sync {
    /// Human-readable name (e.g. "read_email", "search_web").
    fn name(&self) -> &str;

    /// One-line description for the LLM's tool list.
    fn description(&self) -> &str;

    /// JSON Schema for the action's input parameters.
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the action with the given JSON input. Returns JSON output.
    async fn execute(&self, input: serde_json::Value) -> Result<serde_json::Value>;
}

/// Collection of all available actions.
pub struct ActionRegistry {
    actions: Vec<Box<dyn Action>>,
}

impl ActionRegistry {
    pub fn new() -> Self {
        Self {
            actions: Vec::new(),
        }
    }

    pub fn register(&mut self, action: Box<dyn Action>) {
        self.actions.push(action);
    }

    pub fn list(&self) -> &[Box<dyn Action>] {
        &self.actions
    }

    pub fn find(&self, name: &str) -> Option<&dyn Action> {
        self.actions.iter().find(|a| a.name() == name).map(|a| a.as_ref())
    }

    pub async fn execute(&self, name: &str, input: serde_json::Value) -> Result<serde_json::Value> {
        let action = self.find(name).ok_or_else(|| {
            AivyxError::Other(format!("action '{name}' not found"))
        })?;
        action.execute(input).await
    }
}

impl Default for ActionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
