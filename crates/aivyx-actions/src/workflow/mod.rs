//! Workflow template engine — reusable, parameterized multi-step workflows.
//!
//! A `WorkflowTemplate` defines a named sequence of steps with placeholder
//! parameters. Templates are stored encrypted via `EncryptedStore` and can
//! be instantiated into missions by replacing `{param}` placeholders with
//! concrete values.
//!
//! Storage layout: `"workflow:template:{name}"` → JSON-serialized `WorkflowTemplate`

pub mod library;

use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use zeroize::Zeroizing;

// ── Data Structures ────────────────────────────────────────────

/// A reusable workflow template with parameterized steps and optional triggers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowTemplate {
    /// Unique name (used as storage key).
    pub name: String,
    /// Human-readable description.
    pub description: String,
    /// Ordered steps in the workflow.
    pub steps: Vec<TemplateStep>,
    /// Declared parameters with optional defaults.
    pub parameters: Vec<TemplateParameter>,
    /// Triggers that auto-instantiate this template.
    #[serde(default)]
    pub triggers: Vec<WorkflowTrigger>,
    /// When this template was created.
    pub created_at: DateTime<Utc>,
    /// When this template was last modified.
    pub updated_at: DateTime<Utc>,
}

/// A single step in a workflow template.
///
/// Step descriptions and arguments support `{param}` placeholders that are
/// replaced during instantiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateStep {
    /// What this step does (supports `{param}` placeholders).
    pub description: String,
    /// Optional tool hint — if set, the agent should prefer this tool.
    /// If `None`, the agent decides which tool(s) to use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Pre-filled arguments with `{param}` placeholders.
    #[serde(default)]
    pub arguments: serde_json::Value,
    /// Whether this step requires user approval before execution.
    #[serde(default)]
    pub requires_approval: bool,
    /// Optional condition for executing this step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<StepCondition>,
    /// Step indices this step depends on (for DAG execution).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<usize>,
}

/// A declared parameter for a workflow template.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateParameter {
    /// Parameter name (referenced as `{name}` in steps).
    pub name: String,
    /// What this parameter does.
    pub description: String,
    /// Whether this parameter must be provided at instantiation time.
    #[serde(default = "default_required")]
    pub required: bool,
    /// Default value if not provided.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

fn default_required() -> bool {
    true
}

/// Conditions that control whether a step executes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StepCondition {
    /// Execute only if the previous step succeeded.
    OnSuccess,
    /// Execute only if the previous step failed.
    OnFailure,
    /// Execute if a variable equals a specific value.
    VarEquals { var: String, value: String },
    /// Execute if a variable contains a substring.
    VarContains { var: String, substring: String },
    /// All sub-conditions must be true.
    All { conditions: Vec<StepCondition> },
    /// Any sub-condition must be true.
    Any { conditions: Vec<StepCondition> },
}

impl StepCondition {
    /// Evaluate this condition against a context of variable bindings.
    ///
    /// `prev_success` indicates whether the immediately preceding step succeeded.
    pub fn evaluate(&self, context: &HashMap<String, String>, prev_success: bool) -> bool {
        match self {
            StepCondition::OnSuccess => prev_success,
            StepCondition::OnFailure => !prev_success,
            StepCondition::VarEquals { var, value } => context.get(var).is_some_and(|v| v == value),
            StepCondition::VarContains { var, substring } => context
                .get(var)
                .is_some_and(|v| v.contains(substring.as_str())),
            StepCondition::All { conditions } => {
                conditions.iter().all(|c| c.evaluate(context, prev_success))
            }
            StepCondition::Any { conditions } => {
                conditions.iter().any(|c| c.evaluate(context, prev_success))
            }
        }
    }
}

/// Events that can trigger automatic workflow instantiation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkflowTrigger {
    /// Fire on a cron schedule.
    Cron { expression: String },
    /// Fire when an email matches criteria.
    Email {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sender_contains: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject_contains: Option<String>,
    },
    /// Fire when a file changes in the vault.
    FileChange {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path_glob: Option<String>,
    },
    /// Fire when a goal reaches a progress threshold.
    GoalProgress { goal_match: String, threshold: f32 },
    /// Fire when a webhook is received at `/webhooks/{name}`.
    Webhook {
        /// Optional HMAC secret name in the encrypted store.
        /// When set, the receiver verifies `X-Hub-Signature-256`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secret_ref: Option<String>,
    },
    /// Only instantiated manually (via the `run_workflow` tool).
    Manual,
}

// ── Instantiation ──────────────────────────────────────────────

/// Result of instantiating a template with concrete parameters.
#[derive(Debug, Clone)]
pub struct InstantiatedWorkflow {
    /// The template name (used as `recipe_name` on the mission).
    pub template_name: String,
    /// The goal description (template description with params replaced).
    pub goal: String,
    /// Concrete steps ready for mission execution.
    pub steps: Vec<InstantiatedStep>,
}

/// A step ready for execution (all placeholders resolved).
#[derive(Debug, Clone)]
pub struct InstantiatedStep {
    pub description: String,
    pub tool_hints: Vec<String>,
    pub requires_approval: bool,
    pub depends_on: Vec<usize>,
}

impl WorkflowTemplate {
    /// Instantiate this template with the given parameters.
    ///
    /// Replaces all `{param}` placeholders in step descriptions and arguments.
    /// Returns an error if a required parameter is missing.
    pub fn instantiate(&self, params: &HashMap<String, String>) -> Result<InstantiatedWorkflow> {
        // Validate required parameters
        for p in &self.parameters {
            if p.required && !params.contains_key(&p.name) && p.default.is_none() {
                return Err(aivyx_core::AivyxError::Other(format!(
                    "missing required parameter: {}",
                    p.name
                )));
            }
        }

        // Build the effective parameter map (user values + defaults)
        let mut effective: HashMap<String, String> = HashMap::new();
        for p in &self.parameters {
            if let Some(val) = params.get(&p.name) {
                effective.insert(p.name.clone(), val.clone());
            } else if let Some(ref default) = p.default {
                effective.insert(p.name.clone(), default.clone());
            }
        }

        let replace = |s: &str| -> String {
            let mut result = s.to_string();
            for (k, v) in &effective {
                result = result.replace(&format!("{{{k}}}"), v);
            }
            result
        };

        let goal = replace(&self.description);

        let steps = self
            .steps
            .iter()
            .map(|ts| {
                let tool_hints = ts
                    .tool
                    .as_ref()
                    .map(|t| vec![replace(t)])
                    .unwrap_or_default();
                InstantiatedStep {
                    description: replace(&ts.description),
                    tool_hints,
                    requires_approval: ts.requires_approval,
                    depends_on: ts.depends_on.clone(),
                }
            })
            .collect();

        Ok(InstantiatedWorkflow {
            template_name: self.name.clone(),
            goal,
            steps,
        })
    }
}

// ── Encrypted Persistence ──────────────────────────────────────

const KEY_PREFIX: &str = "workflow:template:";

/// Save a workflow template to the encrypted store.
pub fn save_template(
    store: &EncryptedStore,
    key: &MasterKey,
    template: &WorkflowTemplate,
) -> Result<()> {
    let storage_key = format!("{KEY_PREFIX}{}", template.name);
    let json = serde_json::to_vec(template)
        .map_err(|e| aivyx_core::AivyxError::Other(format!("serialize template: {e}")))?;
    store.put(&storage_key, &json, key)?;
    Ok(())
}

/// Load a workflow template by name from the encrypted store.
pub fn load_template(
    store: &EncryptedStore,
    key: &MasterKey,
    name: &str,
) -> Result<Option<WorkflowTemplate>> {
    let storage_key = format!("{KEY_PREFIX}{name}");
    match store.get(&storage_key, key)? {
        Some(bytes) => {
            let template: WorkflowTemplate = serde_json::from_slice(&bytes).map_err(|e| {
                aivyx_core::AivyxError::Other(format!("deserialize template '{name}': {e}"))
            })?;
            Ok(Some(template))
        }
        None => Ok(None),
    }
}

/// List all workflow template names in the encrypted store.
pub fn list_templates(store: &EncryptedStore) -> Result<Vec<String>> {
    let keys = store.list_keys()?;
    Ok(keys
        .into_iter()
        .filter_map(|k| k.strip_prefix(KEY_PREFIX).map(String::from))
        .collect())
}

/// Delete a workflow template by name.
pub fn delete_template(store: &EncryptedStore, name: &str) -> Result<()> {
    let storage_key = format!("{KEY_PREFIX}{name}");
    store.delete(&storage_key)
}

// ── Shared Context ─────────────────────────────────────────────

/// Shared context for workflow operations (tools + trigger engine).
///
/// Stores the domain key as raw bytes (`Zeroizing<Vec<u8>>`) because
/// `MasterKey` is `!Clone`. Reconstructed on demand via `workflow_key()`.
#[derive(Clone)]
pub struct WorkflowContext {
    pub store: Arc<EncryptedStore>,
    key_bytes: Zeroizing<Vec<u8>>,
}

impl WorkflowContext {
    pub fn new(store: Arc<EncryptedStore>, key: &MasterKey) -> Self {
        Self {
            store,
            key_bytes: Zeroizing::new(key.expose_secret().to_vec()),
        }
    }

    /// Reconstruct the workflow domain key from saved bytes.
    pub fn workflow_key(&self) -> Result<MasterKey> {
        let bytes: [u8; 32] =
            self.key_bytes.as_slice().try_into().map_err(|_| {
                aivyx_core::AivyxError::Other("workflow key must be 32 bytes".into())
            })?;
        Ok(MasterKey::from_bytes(bytes))
    }
}

// ── Action Implementations ────────────────────────────────────

/// Create or update a workflow template from a JSON definition.
pub struct CreateWorkflowAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for CreateWorkflowAction {
    fn name(&self) -> &str {
        "create_workflow"
    }

    fn description(&self) -> &str {
        "Create or update a reusable workflow template with parameterized steps and optional triggers"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name", "description", "steps"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Unique workflow name (used as storage key)"
                },
                "description": {
                    "type": "string",
                    "description": "Human-readable description (supports {param} placeholders)"
                },
                "steps": {
                    "type": "array",
                    "description": "Ordered workflow steps",
                    "items": {
                        "type": "object",
                        "required": ["description"],
                        "properties": {
                            "description": { "type": "string" },
                            "tool": { "type": "string" },
                            "arguments": {},
                            "requires_approval": { "type": "boolean" },
                            "depends_on": { "type": "array", "items": { "type": "integer" } }
                        }
                    }
                },
                "parameters": {
                    "type": "array",
                    "description": "Declared template parameters",
                    "items": {
                        "type": "object",
                        "required": ["name", "description"],
                        "properties": {
                            "name": { "type": "string" },
                            "description": { "type": "string" },
                            "required": { "type": "boolean" },
                            "default": { "type": "string" }
                        }
                    }
                },
                "triggers": {
                    "type": "array",
                    "description": "Triggers for auto-instantiation (Cron, Email, GoalProgress, Manual)"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'name'".into()))?;
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'description'".into()))?;
        let steps_val = input
            .get("steps")
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'steps'".into()))?;

        let steps: Vec<TemplateStep> = serde_json::from_value(steps_val.clone())
            .map_err(|e| aivyx_core::AivyxError::Other(format!("invalid steps: {e}")))?;

        let parameters: Vec<TemplateParameter> = input
            .get("parameters")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("invalid parameters: {e}")))?
            .unwrap_or_default();

        let triggers: Vec<WorkflowTrigger> = input
            .get("triggers")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("invalid triggers: {e}")))?
            .unwrap_or_default();

        let now = Utc::now();
        let key = self.ctx.workflow_key()?;

        // Check if template already exists (preserve created_at)
        let created_at = load_template(&self.ctx.store, &key, name)?
            .map(|existing| existing.created_at)
            .unwrap_or(now);

        let template = WorkflowTemplate {
            name: name.to_string(),
            description: description.to_string(),
            steps,
            parameters,
            triggers,
            created_at,
            updated_at: now,
        };

        save_template(&self.ctx.store, &key, &template)?;

        Ok(serde_json::json!({
            "status": "ok",
            "name": template.name,
            "steps": template.steps.len(),
            "parameters": template.parameters.len(),
            "triggers": template.triggers.len(),
        }))
    }
}

/// List available workflow templates.
pub struct ListWorkflowsAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for ListWorkflowsAction {
    fn name(&self) -> &str {
        "list_workflows"
    }

    fn description(&self) -> &str {
        "List all available workflow templates with their descriptions and trigger counts"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "details": {
                    "type": "boolean",
                    "description": "If true, include step/parameter/trigger details for each template"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let details = input
            .get("details")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let names = list_templates(&self.ctx.store)?;

        if !details {
            // Quick listing — just names
            let key = self.ctx.workflow_key()?;
            let mut workflows = Vec::new();
            for name in &names {
                let desc = load_template(&self.ctx.store, &key, name)?
                    .map(|t| t.description.clone())
                    .unwrap_or_default();
                workflows.push(serde_json::json!({
                    "name": name,
                    "description": desc,
                }));
            }
            return Ok(serde_json::json!({
                "count": workflows.len(),
                "workflows": workflows,
            }));
        }

        // Detailed listing
        let key = self.ctx.workflow_key()?;
        let mut workflows = Vec::new();
        for name in &names {
            if let Some(t) = load_template(&self.ctx.store, &key, name)? {
                workflows.push(serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "steps": t.steps.len(),
                    "parameters": t.parameters.iter().map(|p| {
                        serde_json::json!({
                            "name": p.name,
                            "description": p.description,
                            "required": p.required,
                            "default": p.default,
                        })
                    }).collect::<Vec<_>>(),
                    "triggers": t.triggers.len(),
                    "created_at": t.created_at.to_rfc3339(),
                    "updated_at": t.updated_at.to_rfc3339(),
                }));
            }
        }

        Ok(serde_json::json!({
            "count": workflows.len(),
            "workflows": workflows,
        }))
    }
}

/// Instantiate and run a workflow template with parameters.
pub struct RunWorkflowAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for RunWorkflowAction {
    fn name(&self) -> &str {
        "run_workflow"
    }

    fn description(&self) -> &str {
        "Instantiate a workflow template with parameters, returning the concrete steps for mission creation"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the workflow template to instantiate"
                },
                "params": {
                    "type": "object",
                    "description": "Parameter values to substitute into the template (key-value pairs)",
                    "additionalProperties": { "type": "string" }
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'name'".into()))?;

        let key = self.ctx.workflow_key()?;
        let template = load_template(&self.ctx.store, &key, name)?.ok_or_else(|| {
            aivyx_core::AivyxError::Other(format!("workflow template '{name}' not found"))
        })?;

        let params: HashMap<String, String> = input
            .get("params")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("invalid params: {e}")))?
            .unwrap_or_default();

        let instantiated = template.instantiate(&params)?;

        Ok(serde_json::json!({
            "status": "ok",
            "template_name": instantiated.template_name,
            "goal": instantiated.goal,
            "steps": instantiated.steps.iter().enumerate().map(|(i, s)| {
                serde_json::json!({
                    "index": i,
                    "description": s.description,
                    "tool_hints": s.tool_hints,
                    "requires_approval": s.requires_approval,
                    "depends_on": s.depends_on,
                })
            }).collect::<Vec<_>>(),
        }))
    }
}

/// Inspect a workflow template's definition, triggers, and parameters.
pub struct WorkflowStatusAction {
    pub ctx: WorkflowContext,
}

#[async_trait::async_trait]
impl crate::Action for WorkflowStatusAction {
    fn name(&self) -> &str {
        "workflow_status"
    }

    fn description(&self) -> &str {
        "Get detailed status of a workflow template including steps, parameters, and trigger definitions"
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["name"],
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Name of the workflow template to inspect"
                }
            }
        })
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let name = input
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| aivyx_core::AivyxError::Other("missing 'name'".into()))?;

        let key = self.ctx.workflow_key()?;
        let template = load_template(&self.ctx.store, &key, name)?.ok_or_else(|| {
            aivyx_core::AivyxError::Other(format!("workflow template '{name}' not found"))
        })?;

        Ok(serde_json::json!({
            "name": template.name,
            "description": template.description,
            "created_at": template.created_at.to_rfc3339(),
            "updated_at": template.updated_at.to_rfc3339(),
            "steps": template.steps.iter().enumerate().map(|(i, s)| {
                serde_json::json!({
                    "index": i,
                    "description": s.description,
                    "tool": s.tool,
                    "requires_approval": s.requires_approval,
                    "condition": s.condition.as_ref().map(|c| serde_json::to_value(c).ok()),
                    "depends_on": s.depends_on,
                })
            }).collect::<Vec<_>>(),
            "parameters": template.parameters.iter().map(|p| {
                serde_json::json!({
                    "name": p.name,
                    "description": p.description,
                    "required": p.required,
                    "default": p.default,
                })
            }).collect::<Vec<_>>(),
            "triggers": template.triggers.iter().map(|t| {
                serde_json::to_value(t).unwrap_or(serde_json::Value::Null)
            }).collect::<Vec<_>>(),
        }))
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_template() -> WorkflowTemplate {
        WorkflowTemplate {
            name: "expense-report".into(),
            description: "Process expense report for {employee}".into(),
            steps: vec![
                TemplateStep {
                    description: "Fetch email from {employee}".into(),
                    tool: Some("fetch_email".into()),
                    arguments: serde_json::json!({"query": "from:{employee}"}),
                    requires_approval: false,
                    condition: None,
                    depends_on: vec![],
                },
                TemplateStep {
                    description: "Extract expense amounts".into(),
                    tool: None,
                    arguments: serde_json::Value::Null,
                    requires_approval: false,
                    condition: Some(StepCondition::OnSuccess),
                    depends_on: vec![0],
                },
                TemplateStep {
                    description: "File receipt to {folder}".into(),
                    tool: Some("file_receipt".into()),
                    arguments: serde_json::json!({"folder": "{folder}"}),
                    requires_approval: true,
                    condition: Some(StepCondition::OnSuccess),
                    depends_on: vec![1],
                },
            ],
            parameters: vec![
                TemplateParameter {
                    name: "employee".into(),
                    description: "Employee email address".into(),
                    required: true,
                    default: None,
                },
                TemplateParameter {
                    name: "folder".into(),
                    description: "Receipt folder name".into(),
                    required: false,
                    default: Some("receipts".into()),
                },
            ],
            triggers: vec![WorkflowTrigger::Manual],
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    #[test]
    fn instantiate_replaces_placeholders() {
        let template = sample_template();
        let mut params = HashMap::new();
        params.insert("employee".into(), "alice@example.com".into());

        let result = template.instantiate(&params).unwrap();
        assert_eq!(result.template_name, "expense-report");
        assert_eq!(result.goal, "Process expense report for alice@example.com");
        assert_eq!(
            result.steps[0].description,
            "Fetch email from alice@example.com"
        );
        assert_eq!(result.steps[2].description, "File receipt to receipts"); // default
        assert!(result.steps[2].requires_approval);
    }

    #[test]
    fn instantiate_with_explicit_default_override() {
        let template = sample_template();
        let mut params = HashMap::new();
        params.insert("employee".into(), "bob@co.com".into());
        params.insert("folder".into(), "expenses-2026".into());

        let result = template.instantiate(&params).unwrap();
        assert_eq!(result.steps[2].description, "File receipt to expenses-2026");
    }

    #[test]
    fn instantiate_missing_required_param_errors() {
        let template = sample_template();
        let params = HashMap::new(); // missing "employee"
        assert!(template.instantiate(&params).is_err());
    }

    #[test]
    fn step_condition_on_success() {
        let ctx = HashMap::new();
        assert!(StepCondition::OnSuccess.evaluate(&ctx, true));
        assert!(!StepCondition::OnSuccess.evaluate(&ctx, false));
    }

    #[test]
    fn step_condition_on_failure() {
        let ctx = HashMap::new();
        assert!(!StepCondition::OnFailure.evaluate(&ctx, true));
        assert!(StepCondition::OnFailure.evaluate(&ctx, false));
    }

    #[test]
    fn step_condition_var_equals() {
        let mut ctx = HashMap::new();
        ctx.insert("status".into(), "approved".into());

        let cond = StepCondition::VarEquals {
            var: "status".into(),
            value: "approved".into(),
        };
        assert!(cond.evaluate(&ctx, true));

        let cond_miss = StepCondition::VarEquals {
            var: "status".into(),
            value: "rejected".into(),
        };
        assert!(!cond_miss.evaluate(&ctx, true));
    }

    #[test]
    fn step_condition_var_contains() {
        let mut ctx = HashMap::new();
        ctx.insert("body".into(), "Please review the attached invoice".into());

        let cond = StepCondition::VarContains {
            var: "body".into(),
            substring: "invoice".into(),
        };
        assert!(cond.evaluate(&ctx, true));
    }

    #[test]
    fn step_condition_all_and_any() {
        let mut ctx = HashMap::new();
        ctx.insert("a".into(), "1".into());
        ctx.insert("b".into(), "2".into());

        let all = StepCondition::All {
            conditions: vec![
                StepCondition::VarEquals {
                    var: "a".into(),
                    value: "1".into(),
                },
                StepCondition::VarEquals {
                    var: "b".into(),
                    value: "2".into(),
                },
            ],
        };
        assert!(all.evaluate(&ctx, true));

        let any = StepCondition::Any {
            conditions: vec![
                StepCondition::VarEquals {
                    var: "a".into(),
                    value: "WRONG".into(),
                },
                StepCondition::VarEquals {
                    var: "b".into(),
                    value: "2".into(),
                },
            ],
        };
        assert!(any.evaluate(&ctx, true));

        let all_fail = StepCondition::All {
            conditions: vec![
                StepCondition::VarEquals {
                    var: "a".into(),
                    value: "1".into(),
                },
                StepCondition::VarEquals {
                    var: "b".into(),
                    value: "WRONG".into(),
                },
            ],
        };
        assert!(!all_fail.evaluate(&ctx, true));
    }

    #[test]
    fn template_roundtrip_json() {
        let template = sample_template();
        let json = serde_json::to_string(&template).unwrap();
        let parsed: WorkflowTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.name, "expense-report");
        assert_eq!(parsed.steps.len(), 3);
        assert_eq!(parsed.parameters.len(), 2);
        assert_eq!(parsed.triggers.len(), 1);
    }

    #[test]
    fn trigger_variants_serialize() {
        let triggers = vec![
            WorkflowTrigger::Cron {
                expression: "0 9 * * *".into(),
            },
            WorkflowTrigger::Email {
                sender_contains: Some("vendor@".into()),
                subject_contains: None,
            },
            WorkflowTrigger::FileChange {
                path_glob: Some("*.pdf".into()),
            },
            WorkflowTrigger::GoalProgress {
                goal_match: "quarterly review".into(),
                threshold: 0.8,
            },
            WorkflowTrigger::Manual,
        ];
        let json = serde_json::to_string(&triggers).unwrap();
        let parsed: Vec<WorkflowTrigger> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 5);
    }

    #[test]
    fn depends_on_preserved_in_instantiation() {
        let template = sample_template();
        let mut params = HashMap::new();
        params.insert("employee".into(), "x@y.com".into());

        let result = template.instantiate(&params).unwrap();
        assert!(result.steps[0].depends_on.is_empty());
        assert_eq!(result.steps[1].depends_on, vec![0]);
        assert_eq!(result.steps[2].depends_on, vec![1]);
    }
}
