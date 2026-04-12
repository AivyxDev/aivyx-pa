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
    /// Stable identifier for this step, used by acceptance reports and
    /// mission plan cursors to reference the step unambiguously across
    /// reorderings.
    ///
    /// Defaults to an empty string during deserialization of legacy
    /// templates; the loader ([`load_template`]) and template constructor
    /// ([`WorkflowTemplate::instantiate`]) backfill empty IDs with
    /// `s{NN}` based on positional index. New templates written in code
    /// should always set this explicitly.
    #[serde(default)]
    pub step_id: String,
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
    /// Acceptance checks this step must satisfy before it can be marked
    /// complete. An empty vec means the step has no structured contract
    /// and is treated as "vacuously passing" by the executor (see
    /// [`AcceptanceReport::overall_or_vacuous`]). Declared checks are
    /// evaluated against the `StepEvidence` collected at runtime by the
    /// pure evaluator in [`workflow::acceptance`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance: Vec<AcceptanceCheck>,
}

/// Generate a default positional step id for a step at the given index.
///
/// Used by the deserialization backfill path to assign stable IDs to
/// legacy templates that were saved before `step_id` existed.
pub(crate) fn default_step_id(index: usize) -> String {
    format!("s{index:02}")
}

/// Backfill empty `step_id` fields on a template's steps with positional
/// defaults. Idempotent — steps that already have a non-empty id are left
/// untouched.
fn backfill_step_ids(steps: &mut [TemplateStep]) {
    for (i, step) in steps.iter_mut().enumerate() {
        if step.step_id.is_empty() {
            step.step_id = default_step_id(i);
        }
    }
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

// ── Acceptance Checks (re-exported from aivyx-task-engine) ────
//
// The canonical types live in `aivyx_task_engine::acceptance` so the
// mission executor can evaluate them without depending on this crate.
// We re-export here so existing `aivyx_actions::workflow::AcceptanceCheck`
// imports and `TemplateStep.acceptance: Vec<AcceptanceCheck>` continue to
// resolve transparently.
pub use aivyx_task_engine::acceptance::{
    AcceptanceCheck, AcceptanceReport, CheckKind, CheckOutcome, MemoryHit, SelfAssertionEvidence,
    StepEvidence, ToolExpectation, ToolOutcome, evaluate as evaluate_acceptance,
};

// ── Workflow Triggers ─────────────────────────────────────────

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
    /// Stable step identifier carried over from the source `TemplateStep`.
    /// Used by acceptance reports and mission plan cursors to reference
    /// this step unambiguously.
    pub step_id: String,
    pub description: String,
    pub tool_hints: Vec<String>,
    pub requires_approval: bool,
    pub depends_on: Vec<usize>,
    /// Acceptance checks carried over from the source `TemplateStep`.
    /// The mission executor evaluates these against the step's
    /// `StepEvidence` to decide whether the step may advance.
    pub acceptance: Vec<AcceptanceCheck>,
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

        // Propagate step IDs, falling back to positional defaults for any
        // steps that were constructed in code without an explicit id. This
        // mirrors the `load_template` backfill so every `InstantiatedStep`
        // has a non-empty, stable `step_id` regardless of its origin.
        let steps = self
            .steps
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let tool_hints = ts
                    .tool
                    .as_ref()
                    .map(|t| vec![replace(t)])
                    .unwrap_or_default();
                let step_id = if ts.step_id.is_empty() {
                    default_step_id(i)
                } else {
                    ts.step_id.clone()
                };
                InstantiatedStep {
                    step_id,
                    description: replace(&ts.description),
                    tool_hints,
                    requires_approval: ts.requires_approval,
                    depends_on: ts.depends_on.clone(),
                    acceptance: ts.acceptance.clone(),
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
///
/// Deserialized templates are passed through [`backfill_step_ids`] so that
/// legacy records written before `step_id` existed receive stable
/// positional IDs (`s00`, `s01`, ...). The backfill is idempotent — templates
/// that already carry explicit ids are left untouched — and it runs in memory
/// only, so the stored blob is not rewritten until the next `save_template`.
pub fn load_template(
    store: &EncryptedStore,
    key: &MasterKey,
    name: &str,
) -> Result<Option<WorkflowTemplate>> {
    let storage_key = format!("{KEY_PREFIX}{name}");
    match store.get(&storage_key, key)? {
        Some(bytes) => {
            let mut template: WorkflowTemplate = serde_json::from_slice(&bytes).map_err(|e| {
                aivyx_core::AivyxError::Other(format!("deserialize template '{name}': {e}"))
            })?;
            backfill_step_ids(&mut template.steps);
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
                            "step_id": {
                                "type": "string",
                                "description": "Stable step identifier (used by acceptance reports and mission plan cursors). If omitted, a positional default like 's00' is assigned at load time."
                            },
                            "description": { "type": "string" },
                            "tool": { "type": "string" },
                            "arguments": {},
                            "requires_approval": { "type": "boolean" },
                            "depends_on": { "type": "array", "items": { "type": "integer" } },
                            "acceptance": {
                                "type": "array",
                                "description": "Acceptance checks this step must satisfy. Each check is one of: {\"type\":\"ToolCall\",\"tool\":\"...\",\"expect\":{\"kind\":\"Ok\"|\"JsonEquals\"|\"JsonPresent\",...}}, {\"type\":\"MemoryExists\",\"kind\":\"...\",\"query\":\"...\"}, or {\"type\":\"SelfAssertion\",\"criterion\":\"...\",\"dimension\":\"...\"}. Empty or omitted means the step has no structured contract.",
                                "items": { "type": "object" }
                            }
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

        let mut steps: Vec<TemplateStep> = serde_json::from_value(steps_val.clone())
            .map_err(|e| aivyx_core::AivyxError::Other(format!("invalid steps: {e}")))?;
        // Assign positional IDs to any step the agent submitted without
        // an explicit `step_id`. Keeps the saved blob canonical so future
        // loads don't rely on the load-path backfill.
        backfill_step_ids(&mut steps);

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
                    "step_id": s.step_id,
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
                    "step_id": s.step_id,
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
                    step_id: "fetch".into(),
                    description: "Fetch email from {employee}".into(),
                    tool: Some("fetch_email".into()),
                    arguments: serde_json::json!({"query": "from:{employee}"}),
                    requires_approval: false,
                    condition: None,
                    depends_on: vec![],
                    acceptance: vec![],
                },
                TemplateStep {
                    step_id: "extract".into(),
                    description: "Extract expense amounts".into(),
                    tool: None,
                    arguments: serde_json::Value::Null,
                    requires_approval: false,
                    condition: Some(StepCondition::OnSuccess),
                    depends_on: vec![0],
                    acceptance: vec![],
                },
                TemplateStep {
                    step_id: "file".into(),
                    description: "File receipt to {folder}".into(),
                    tool: Some("file_receipt".into()),
                    arguments: serde_json::json!({"folder": "{folder}"}),
                    requires_approval: true,
                    condition: Some(StepCondition::OnSuccess),
                    depends_on: vec![1],
                    acceptance: vec![],
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

    // ── step_id propagation (Slice A) ──────────────────────────

    #[test]
    fn instantiate_preserves_explicit_step_ids() {
        let template = sample_template();
        let mut params = HashMap::new();
        params.insert("employee".into(), "x@y.com".into());

        let result = template.instantiate(&params).unwrap();
        assert_eq!(result.steps[0].step_id, "fetch");
        assert_eq!(result.steps[1].step_id, "extract");
        assert_eq!(result.steps[2].step_id, "file");
    }

    #[test]
    fn instantiate_backfills_empty_step_ids_positionally() {
        // A template constructed in code without explicit step IDs —
        // e.g. by an early caller we haven't migrated yet — should still
        // emit instantiated steps with stable positional IDs.
        let now = Utc::now();
        let template = WorkflowTemplate {
            name: "anon".into(),
            description: "anon".into(),
            steps: vec![
                TemplateStep {
                    step_id: String::new(),
                    description: "first".into(),
                    tool: None,
                    arguments: serde_json::Value::Null,
                    requires_approval: false,
                    condition: None,
                    depends_on: vec![],
                    acceptance: vec![],
                },
                TemplateStep {
                    step_id: String::new(),
                    description: "second".into(),
                    tool: None,
                    arguments: serde_json::Value::Null,
                    requires_approval: false,
                    condition: None,
                    depends_on: vec![],
                    acceptance: vec![],
                },
            ],
            parameters: vec![],
            triggers: vec![WorkflowTrigger::Manual],
            created_at: now,
            updated_at: now,
        };

        let result = template.instantiate(&HashMap::new()).unwrap();
        assert_eq!(result.steps[0].step_id, "s00");
        assert_eq!(result.steps[1].step_id, "s01");
    }

    #[test]
    fn legacy_json_without_step_id_backfills_on_load() {
        // Hand-crafted legacy JSON: no `step_id` field at all on the step
        // object. This simulates a template that was saved to the encrypted
        // store before Slice A landed. After passing through the same
        // backfill that `load_template` applies, the steps must have
        // stable positional IDs.
        let legacy_json = serde_json::json!({
            "name": "legacy",
            "description": "old template",
            "steps": [
                {
                    "description": "step one",
                    "depends_on": []
                },
                {
                    "description": "step two",
                    "depends_on": [0]
                },
                {
                    "description": "step three",
                    "depends_on": [1]
                }
            ],
            "parameters": [],
            "triggers": [{ "type": "Manual" }],
            "created_at": Utc::now().to_rfc3339(),
            "updated_at": Utc::now().to_rfc3339(),
        });

        let mut template: WorkflowTemplate =
            serde_json::from_value(legacy_json).expect("legacy JSON should deserialize");
        // Pre-backfill: all step IDs empty (serde default).
        assert!(template.steps.iter().all(|s| s.step_id.is_empty()));

        // Exercise the same helper `load_template` uses.
        backfill_step_ids(&mut template.steps);

        assert_eq!(template.steps[0].step_id, "s00");
        assert_eq!(template.steps[1].step_id, "s01");
        assert_eq!(template.steps[2].step_id, "s02");
    }

    #[test]
    fn backfill_is_idempotent_and_leaves_explicit_ids_alone() {
        let mut steps = vec![
            TemplateStep {
                step_id: "alpha".into(),
                description: "a".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                // mixed: this one is empty and should be backfilled
                step_id: String::new(),
                description: "b".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
            TemplateStep {
                step_id: "gamma".into(),
                description: "c".into(),
                tool: None,
                arguments: serde_json::Value::Null,
                requires_approval: false,
                condition: None,
                depends_on: vec![],
                acceptance: vec![],
            },
        ];

        backfill_step_ids(&mut steps);
        assert_eq!(steps[0].step_id, "alpha");
        assert_eq!(steps[1].step_id, "s01"); // positional, not s00
        assert_eq!(steps[2].step_id, "gamma");

        // Idempotent: running again changes nothing.
        backfill_step_ids(&mut steps);
        assert_eq!(steps[0].step_id, "alpha");
        assert_eq!(steps[1].step_id, "s01");
        assert_eq!(steps[2].step_id, "gamma");
    }

    #[test]
    fn template_roundtrip_preserves_explicit_step_ids() {
        let template = sample_template();
        let json = serde_json::to_string(&template).unwrap();
        let parsed: WorkflowTemplate = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.steps[0].step_id, "fetch");
        assert_eq!(parsed.steps[1].step_id, "extract");
        assert_eq!(parsed.steps[2].step_id, "file");
    }
}
