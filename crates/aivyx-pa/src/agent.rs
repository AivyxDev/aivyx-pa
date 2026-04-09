//! Agent construction and lifecycle.
//!
//! Builds the PA agent with up to 97 tools, resilient providers, brain,
//! memory, MCP plugins, and all service integrations. This module is the
//! shared core used by both the API server and CLI commands.

use aivyx_agent::agent::Agent;
use aivyx_agent::brain_tools::{
    BrainListGoalsTool, BrainReflectTool, BrainSetGoalTool, BrainUpdateGoalTool,
};
use aivyx_agent::cost_tracker::CostTracker;
use aivyx_agent::rate_limiter::RateLimiter;
use aivyx_audit::AuditLog;
use aivyx_brain::{Brain, BrainStore, GoalFilter, GoalStatus};
use aivyx_capability::{ActionPattern, Capability, CapabilityScope, CapabilitySet};
use aivyx_config::{AivyxConfig, AivyxDirs, McpServerConfig};
use aivyx_core::{AgentId, CapabilityId, Principal, ToolRegistry};
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key, derive_brain_key};
use aivyx_llm::{LlmProvider, create_embedding_provider};
use aivyx_memory::{MemoryManager, MemoryStore};
use aivyx_task_engine::{
    ExecutionMode, ExperimentTracker, FactoryRecipe, Mission, MissionToolContext, StepStatus,
    TaskEngine, TaskStatus, TaskStore, create_mission_tools,
};

use crate::config::PaConfig;
use aivyx_actions::bridge::register_default_actions;
use aivyx_actions::plugin::PluginState;
use aivyx_mcp::{McpClient, McpProxyTool, McpServerPool, ToolResultCache};

use std::sync::Arc;
use tokio::sync::Mutex;
use zeroize::Zeroizing;

// ── Self-model update tool ──────────────────────────────────────

/// Tool that lets the agent update its own self-model (strengths,
/// weaknesses, domain confidence, tool proficiency).
///
/// This closes the self-learning feedback loop: the agent reflects on
/// outcomes, then writes what it learned back to persistent storage.
/// The updated self-model is injected into the system prompt on every
/// subsequent turn, so the agent benefits from its own learning.
struct BrainUpdateSelfModelTool {
    id: aivyx_core::ToolId,
    store: Arc<BrainStore>,
    key: MasterKey,
}

impl BrainUpdateSelfModelTool {
    fn new(store: Arc<BrainStore>, key: MasterKey) -> Self {
        Self {
            id: aivyx_core::ToolId::new(),
            store,
            key,
        }
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for BrainUpdateSelfModelTool {
    fn id(&self) -> aivyx_core::ToolId {
        self.id
    }

    fn name(&self) -> &str {
        "brain_update_self_model"
    }

    fn description(&self) -> &str {
        "Update your self-model based on what you've learned. You can add or replace \
         strengths, weaknesses, domain confidence scores, and tool proficiency ratings. \
         This information persists across sessions and shapes your future behavior."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "add_strengths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Strengths to add (e.g., ['email-drafting', 'research-synthesis'])"
                },
                "remove_strengths": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Strengths to remove if no longer accurate"
                },
                "add_weaknesses": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Weaknesses to add (e.g., ['complex-math', 'visual-design'])"
                },
                "remove_weaknesses": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Weaknesses to remove (e.g., after improving)"
                },
                "domain_confidence": {
                    "type": "object",
                    "additionalProperties": { "type": "number" },
                    "description": "Set domain confidence scores (0.0-1.0). E.g., {\"coding\": 0.8, \"cooking\": 0.3}"
                },
                "tool_proficiency": {
                    "type": "object",
                    "additionalProperties": { "type": "number" },
                    "description": "Set tool proficiency scores (0.0-1.0). E.g., {\"fetch_webpage\": 0.9, \"send_email\": 0.7}"
                }
            }
        })
    }

    fn required_scope(&self) -> Option<aivyx_capability::CapabilityScope> {
        Some(aivyx_capability::CapabilityScope::Custom(
            "self-improvement".into(),
        ))
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        // Load current self-model (or create empty)
        let mut model = self.store.load_self_model(&self.key)?.unwrap_or_default();

        let mut changes = Vec::new();

        // Add strengths
        if let Some(items) = input["add_strengths"].as_array() {
            for item in items {
                if let Some(s) = item.as_str()
                    && !model.strengths.contains(&s.to_string())
                {
                    model.strengths.push(s.to_string());
                    changes.push(format!("added strength: {s}"));
                }
            }
        }

        // Remove strengths
        if let Some(items) = input["remove_strengths"].as_array() {
            for item in items {
                if let Some(s) = item.as_str() {
                    let before = model.strengths.len();
                    model.strengths.retain(|x| x != s);
                    if model.strengths.len() < before {
                        changes.push(format!("removed strength: {s}"));
                    }
                }
            }
        }

        // Add weaknesses
        if let Some(items) = input["add_weaknesses"].as_array() {
            for item in items {
                if let Some(s) = item.as_str()
                    && !model.weaknesses.contains(&s.to_string())
                {
                    model.weaknesses.push(s.to_string());
                    changes.push(format!("added weakness: {s}"));
                }
            }
        }

        // Remove weaknesses
        if let Some(items) = input["remove_weaknesses"].as_array() {
            for item in items {
                if let Some(s) = item.as_str() {
                    let before = model.weaknesses.len();
                    model.weaknesses.retain(|x| x != s);
                    if model.weaknesses.len() < before {
                        changes.push(format!("removed weakness: {s}"));
                    }
                }
            }
        }

        // Update domain confidence
        if let Some(obj) = input["domain_confidence"].as_object() {
            for (domain, score) in obj {
                if let Some(s) = score.as_f64() {
                    let clamped = s.clamp(0.0, 1.0) as f32;
                    model.domain_confidence.insert(domain.clone(), clamped);
                    changes.push(format!(
                        "domain '{domain}' confidence: {:.0}%",
                        clamped * 100.0
                    ));
                }
            }
        }

        // Update tool proficiency
        if let Some(obj) = input["tool_proficiency"].as_object() {
            for (tool, score) in obj {
                if let Some(s) = score.as_f64() {
                    let clamped = s.clamp(0.0, 1.0) as f32;
                    model.tool_proficiency.insert(tool.clone(), clamped);
                    changes.push(format!(
                        "tool '{tool}' proficiency: {:.0}%",
                        clamped * 100.0
                    ));
                }
            }
        }

        if changes.is_empty() {
            return Ok(serde_json::json!({
                "status": "no_changes",
                "message": "No valid updates provided"
            }));
        }

        // Update timestamp and persist
        model.updated_at = chrono::Utc::now();
        self.store.save_self_model(&model, &self.key)?;

        tracing::info!("Self-model updated: {} changes", changes.len());

        Ok(serde_json::json!({
            "status": "updated",
            "changes": changes,
            "strengths": model.strengths,
            "weaknesses": model.weaknesses,
            "domain_count": model.domain_confidence.len(),
            "tool_count": model.tool_proficiency.len()
        }))
    }
}

// ── MCP Prompts Discovery Tool ─────────────────────────────────

/// Lists prompt templates from connected MCP servers.
struct McpListPromptsTool {
    id: aivyx_core::ToolId,
    pool: Arc<McpServerPool>,
}

impl McpListPromptsTool {
    fn new(pool: Arc<McpServerPool>) -> Self {
        Self {
            id: aivyx_core::ToolId::new(),
            pool,
        }
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for McpListPromptsTool {
    fn id(&self) -> aivyx_core::ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "mcp_list_prompts"
    }
    fn description(&self) -> &str {
        "List prompt templates available from connected MCP servers. \
         These are pre-built prompt templates the servers provide."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "server": {
                    "type": "string",
                    "description": "Optional: filter to a specific server name"
                }
            }
        })
    }
    fn required_scope(&self) -> Option<aivyx_capability::CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let server_filter = input["server"].as_str();
        let mut all_prompts = Vec::new();

        for name in self.pool.server_names().await {
            if let Some(filter) = server_filter
                && name != filter
            {
                continue;
            }
            if let Some(client) = self.pool.get(&name).await {
                match client.list_prompts().await {
                    Ok(prompts) => {
                        for p in prompts {
                            all_prompts.push(serde_json::json!({
                                "server": name,
                                "name": p.name,
                                "description": p.description,
                            }));
                        }
                    }
                    Err(e) => {
                        tracing::debug!("Failed to list prompts from '{name}': {e}");
                    }
                }
            }
        }

        Ok(serde_json::json!({
            "prompts": all_prompts,
            "count": all_prompts.len()
        }))
    }
}

// ── Pattern Mining Tool ────────────────────────────────────────

/// Exposes discovered workflow patterns from memory consolidation.
/// Pattern mining runs during heartbeat consolidation when `mine_patterns` is enabled.
struct ListDiscoveredPatternsTool {
    id: aivyx_core::ToolId,
    memory_manager: Arc<Mutex<aivyx_memory::MemoryManager>>,
}

impl ListDiscoveredPatternsTool {
    fn new(mm: Arc<Mutex<aivyx_memory::MemoryManager>>) -> Self {
        Self {
            id: aivyx_core::ToolId::new(),
            memory_manager: mm,
        }
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for ListDiscoveredPatternsTool {
    fn id(&self) -> aivyx_core::ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "list_discovered_patterns"
    }
    fn description(&self) -> &str {
        "List workflow patterns automatically discovered from your tool usage history. \
         Patterns are sequences of tools that frequently succeed together."
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "min_occurrences": {
                    "type": "integer",
                    "description": "Minimum times the pattern appeared. Default: 2"
                },
                "min_success_rate": {
                    "type": "number",
                    "description": "Minimum success rate (0.0-1.0). Default: 0.5"
                }
            }
        })
    }
    fn required_scope(&self) -> Option<aivyx_capability::CapabilityScope> {
        None
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        let min_occ = input["min_occurrences"].as_u64().unwrap_or(2) as u32;
        let min_rate = input["min_success_rate"].as_f64().unwrap_or(0.5) as f32;

        let mm = self.memory_manager.lock().await;
        let filter = aivyx_memory::pattern::PatternFilter {
            min_occurrences: Some(min_occ),
            min_success_rate: Some(min_rate),
            ..Default::default()
        };
        let patterns = mm.query_patterns(&filter).unwrap_or_default();

        let results: Vec<serde_json::Value> = patterns
            .iter()
            .map(|p| {
                serde_json::json!({
                    "id": p.id.to_string(),
                    "tool_sequence": p.tool_sequence,
                    "occurrences": p.occurrence_count,
                    "success_rate": p.success_rate,
                    "avg_duration_ms": p.avg_duration_ms,
                    "keywords": p.goal_keywords,
                })
            })
            .collect();

        Ok(serde_json::json!({
            "patterns": results,
            "count": results.len()
        }))
    }
}

// ── Mission → Brain bridge ──────────────────────────────────────

/// PA-local mission_create tool that wraps the standard engine logic
/// but bridges mission outcomes back into brain goals and self-model.
///
/// When a mission completes or fails in the background, this tool:
/// 1. Searches active brain goals whose description overlaps with the mission goal
/// 2. Records success/failure on matching goals (circuit breaker, cooldown)
/// 3. Updates progress on matching goals based on step completion ratio
/// 4. Feeds the outcome into the self-model (domain confidence)
struct PaMissionCreateTool {
    id: aivyx_core::ToolId,
    ctx: MissionToolContext,
    agent_name: String,
    brain_store: Arc<BrainStore>,
    /// Shared master key — derive brain key on demand instead of storing raw bytes.
    master_key: Arc<MasterKey>,
    /// Configured default execution mode (from [missions] config).
    default_mode: ExecutionMode,
    /// Optional experiment tracker for A/B feedback scoring.
    experiment_tracker: Option<Arc<tokio::sync::Mutex<ExperimentTracker>>>,
}

impl PaMissionCreateTool {
    fn new(
        ctx: MissionToolContext,
        agent_name: String,
        brain_store: Arc<BrainStore>,
        master_key: Arc<MasterKey>,
        default_mode: ExecutionMode,
        experiment_tracker: Option<Arc<tokio::sync::Mutex<ExperimentTracker>>>,
    ) -> Self {
        Self {
            id: aivyx_core::ToolId::new(),
            ctx,
            agent_name,
            brain_store,
            master_key,
            default_mode,
            experiment_tracker,
        }
    }

    /// Build a TaskEngine from the MissionToolContext fields.
    fn build_engine(&self) -> aivyx_core::Result<TaskEngine> {
        build_task_engine(&self.ctx)
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for PaMissionCreateTool {
    fn id(&self) -> aivyx_core::ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "mission_create"
    }

    fn description(&self) -> &str {
        "Create a new mission from a goal. The goal is decomposed into steps and \
         executed in the background. On completion, matching brain goals are \
         automatically updated with the outcome."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "goal": {
                    "type": "string",
                    "description": "The goal to accomplish"
                },
                "agent": {
                    "type": "string",
                    "description": "Agent profile to use for execution (default: current agent)"
                },
                "mode": {
                    "type": "string",
                    "description": "'sequential' or 'dag' (default: sequential)"
                }
            },
            "required": ["goal"]
        })
    }

    fn required_scope(&self) -> Option<aivyx_capability::CapabilityScope> {
        Some(aivyx_capability::CapabilityScope::Custom("missions".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        use aivyx_core::AivyxError;

        let goal = input["goal"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("mission_create: missing 'goal'".into()))?;
        let agent = input["agent"].as_str().unwrap_or(&self.agent_name);
        let mode = match input["mode"].as_str() {
            Some("dag") => Some(ExecutionMode::Dag),
            Some("sequential") => Some(ExecutionMode::Sequential),
            Some(_) => None, // unknown → fall through to default
            None => Some(self.default_mode),
        };

        if goal.len() > 4096 {
            return Err(AivyxError::Agent(
                "mission_create: goal exceeds 4096 characters".into(),
            ));
        }

        let engine = self.build_engine()?;
        let task_id = engine
            .create_mission_with_mode(goal, agent, None, mode)
            .await?;

        // Spawn background execution with brain bridge on completion.
        let bg_ctx = self.ctx.clone();
        let bg_task_id = task_id;
        let bg_brain_store = Arc::clone(&self.brain_store);
        let bg_master_key = Arc::clone(&self.master_key);
        let bg_goal = goal.to_string();
        let bg_tracker = self.experiment_tracker.clone();

        tokio::spawn(async move {
            // Build engine from context fields (mirrors build_engine helper).
            let bg_engine = match (|| -> aivyx_core::Result<TaskEngine> {
                let mk = {
                    let bytes: [u8; 32] =
                        bg_ctx.master_key_bytes.as_slice().try_into().map_err(|_| {
                            aivyx_core::AivyxError::Crypto("invalid master key length".into())
                        })?;
                    MasterKey::from_bytes(bytes)
                };
                let task_key = aivyx_crypto::derive_task_key(&mk);
                let store = TaskStore::open(bg_ctx.dirs.tasks_dir().join("tasks.db"))?;
                let audit_key = derive_audit_key(&mk);
                let audit_log = AuditLog::new(bg_ctx.dirs.audit_path(), &audit_key);
                let mut engine =
                    TaskEngine::new(bg_ctx.session.clone(), store, task_key, Some(audit_log));
                if let Some(ref mm) = bg_ctx.memory_manager {
                    engine = engine.with_memory_manager(mm.clone());
                }
                Ok(engine)
            })() {
                Ok(e) => e,
                Err(e) => {
                    tracing::error!("Failed to build engine for background mission: {e}");
                    return;
                }
            };

            let timeout = std::time::Duration::from_secs(1800);
            let result =
                tokio::time::timeout(timeout, bg_engine.execute_mission(&bg_task_id, None, None))
                    .await;

            // Load the final mission state for the bridge.
            let mission = match bg_engine.get_mission(&bg_task_id) {
                Ok(Some(m)) => m,
                Ok(None) => {
                    tracing::warn!("Mission {bg_task_id} not found after execution");
                    return;
                }
                Err(e) => {
                    tracing::warn!("Failed to load mission {bg_task_id}: {e}");
                    return;
                }
            };

            // Derive brain key from shared master key — no raw byte copies.
            let brain_key = derive_brain_key(&bg_master_key);

            // Bridge the outcome into brain goals + self-model.
            bridge_mission_to_brain(&mission, &bg_goal, &bg_brain_store, &brain_key);

            // Record experiment data if tracking is enabled.
            if let Some(ref tracker) = bg_tracker {
                let success = matches!(mission.status, TaskStatus::Completed);
                let mut t = tracker.lock().await;
                // Use the recipe name as pattern description if available,
                // otherwise fall back to a truncated goal.
                let desc = mission
                    .recipe_name
                    .as_deref()
                    .unwrap_or_else(|| &bg_goal[..bg_goal.len().min(80)]);
                t.record(
                    aivyx_core::PatternId::new(),
                    desc,
                    false, // feedback_injected: no A/B split yet
                    success,
                );
            }

            // Log the overall result.
            match result {
                Ok(Err(e)) => tracing::error!("Background mission failed: {e}"),
                Err(_) => tracing::error!("Background mission timed out"),
                Ok(Ok(_)) => {}
            }
        });

        Ok(serde_json::json!({
            "task_id": task_id.to_string(),
            "status": "executing",
            "agent": agent,
        }))
    }
}

// ── Recipe-based mission creation ──────────────────────────────────

/// Tool that creates missions from TOML recipe templates.
///
/// Recipes are reusable pipeline definitions (multi-stage DAGs with
/// specialist assignments, reflect gates, and approval stages). This tool
/// lists available recipes, validates them, and creates missions from them.
struct MissionFromRecipeTool {
    id: aivyx_core::ToolId,
    ctx: MissionToolContext,
    agent_name: String,
    recipe_dir: std::path::PathBuf,
}

impl MissionFromRecipeTool {
    fn new(ctx: MissionToolContext, agent_name: String, recipe_dir: std::path::PathBuf) -> Self {
        Self {
            id: aivyx_core::ToolId::new(),
            ctx,
            agent_name,
            recipe_dir,
        }
    }
}

#[async_trait::async_trait]
impl aivyx_core::Tool for MissionFromRecipeTool {
    fn id(&self) -> aivyx_core::ToolId {
        self.id
    }
    fn name(&self) -> &str {
        "mission_from_recipe"
    }

    fn description(&self) -> &str {
        "Create a mission from a TOML recipe template. \
         Pass 'list' as the recipe to see available recipes, \
         or pass a recipe name to create a mission from it."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "recipe": {
                    "type": "string",
                    "description": "Recipe filename (without .toml) or 'list' to see available recipes"
                },
                "goal": {
                    "type": "string",
                    "description": "Goal to accomplish using this recipe (required when creating)"
                }
            },
            "required": ["recipe"]
        })
    }

    fn required_scope(&self) -> Option<aivyx_capability::CapabilityScope> {
        Some(aivyx_capability::CapabilityScope::Custom("missions".into()))
    }

    async fn execute(&self, input: serde_json::Value) -> aivyx_core::Result<serde_json::Value> {
        use aivyx_core::AivyxError;

        let recipe_name = input["recipe"]
            .as_str()
            .ok_or_else(|| AivyxError::Agent("mission_from_recipe: missing 'recipe'".into()))?;

        // List mode: enumerate available recipes.
        if recipe_name == "list" {
            let mut recipes = Vec::new();
            if self.recipe_dir.is_dir()
                && let Ok(entries) = std::fs::read_dir(&self.recipe_dir)
            {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().is_some_and(|e| e == "toml")
                        && let Ok(r) = FactoryRecipe::load(&path)
                    {
                        recipes.push(serde_json::json!({
                            "name": path.file_stem().unwrap_or_default().to_string_lossy(),
                            "description": r.factory.description,
                            "stages": r.stages.len(),
                            "tags": r.factory.tags,
                        }));
                    }
                }
            }
            return Ok(serde_json::json!({
                "recipes": recipes,
                "recipe_dir": self.recipe_dir.display().to_string(),
            }));
        }

        // Create mode: load recipe and build mission.
        let goal = input["goal"].as_str().ok_or_else(|| {
            AivyxError::Agent(
                "mission_from_recipe: 'goal' is required when creating a mission".into(),
            )
        })?;

        let recipe_path = self.recipe_dir.join(format!("{recipe_name}.toml"));
        let recipe = FactoryRecipe::load(&recipe_path)?;
        recipe.validate()?;

        let mission = recipe.to_mission(goal, &self.agent_name)?;
        let task_id = mission.id;
        let stage_count = recipe.stages.len();
        let specialists = recipe
            .specialists()
            .into_iter()
            .map(String::from)
            .collect::<Vec<_>>();

        // Save and execute via TaskEngine.
        let engine = build_task_engine(&self.ctx)?;
        engine.save_and_execute_mission(mission, None, None).await?;

        Ok(serde_json::json!({
            "task_id": task_id.to_string(),
            "recipe": recipe_name,
            "stages": stage_count,
            "specialists": specialists,
            "status": "executing",
        }))
    }
}

/// Bridge a completed mission's outcome into the brain.
///
/// 1. Find active goals whose description overlaps with the mission goal
/// 2. Record success/failure (circuit breaker + cooldown)
/// 3. Update progress based on step completion ratio
/// 4. Update self-model domain confidence
fn bridge_mission_to_brain(
    mission: &Mission,
    original_goal: &str,
    brain_store: &BrainStore,
    brain_key: &MasterKey,
) {
    // Only bridge terminal missions.
    if !mission.status.is_terminal() {
        return;
    }

    let success = matches!(mission.status, TaskStatus::Completed);

    // Calculate step completion ratio for progress updates.
    let total_steps = mission.steps.len() as f32;
    let completed_steps = mission
        .steps
        .iter()
        .filter(|s| s.status == StepStatus::Completed)
        .count() as f32;
    let step_ratio = if total_steps > 0.0 {
        completed_steps / total_steps
    } else {
        0.0
    };

    // Find matching brain goals by keyword overlap.
    let active_goals = match brain_store.list_goals(
        &GoalFilter {
            status: Some(GoalStatus::Active),
            ..Default::default()
        },
        brain_key,
    ) {
        Ok(goals) => goals,
        Err(e) => {
            tracing::warn!("Failed to load goals for mission bridge: {e}");
            return;
        }
    };

    // Tokenize the mission goal into significant words (>3 chars, lowercased).
    let goal_words: Vec<String> = original_goal
        .split_whitespace()
        .map(|w| {
            w.to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string()
        })
        .filter(|w| w.len() > 3)
        .collect();

    let mut matched = 0u32;
    for goal in &active_goals {
        let desc_lower = goal.description.to_lowercase();
        let overlap = goal_words
            .iter()
            .filter(|w| desc_lower.contains(w.as_str()))
            .count();

        // Require at least 2 overlapping words or 40% of goal words.
        let threshold = (goal_words.len() as f32 * 0.4).max(2.0) as usize;
        if overlap < threshold {
            continue;
        }

        let mut updated = goal.clone();

        if success {
            updated.record_success();
            // Advance progress proportionally to step completion.
            let new_progress = (updated.progress + step_ratio * 0.5).min(1.0);
            updated.set_progress(new_progress);
            if step_ratio >= 1.0 {
                // All steps completed — consider the goal done.
                updated.set_status(GoalStatus::Completed);
            }
        } else {
            updated.record_failure();
        }

        if let Err(e) = brain_store.upsert_goal(&updated, brain_key) {
            tracing::warn!(
                "Failed to update goal '{}' from mission bridge: {e}",
                goal.description
            );
        } else {
            matched += 1;
            tracing::info!(
                "Mission bridge: {} goal '{}' (progress: {:.0}%)",
                if success {
                    "advanced"
                } else {
                    "recorded failure on"
                },
                goal.description,
                updated.progress * 100.0,
            );
        }
    }

    // Update self-model with mission outcome.
    if let Ok(Some(mut model)) = brain_store.load_self_model(brain_key) {
        // Extract a domain hint from the mission goal (first significant word).
        let domain = goal_words
            .first()
            .cloned()
            .unwrap_or_else(|| "general".into());

        let current = model.domain_confidence.get(&domain).copied().unwrap_or(0.5);
        let delta = if success { 0.05 } else { -0.05 };
        let new_conf = (current + delta).clamp(0.0, 1.0);
        model.domain_confidence.insert(domain.clone(), new_conf);

        // Update tool proficiency for mission_create itself.
        let tool_current = model
            .tool_proficiency
            .get("mission_create")
            .copied()
            .unwrap_or(0.5);
        let tool_delta = if success { 0.02 } else { -0.03 };
        model.tool_proficiency.insert(
            "mission_create".into(),
            (tool_current + tool_delta).clamp(0.0, 1.0),
        );

        model.updated_at = chrono::Utc::now();
        if let Err(e) = brain_store.save_self_model(&model, brain_key) {
            tracing::warn!("Failed to update self-model from mission bridge: {e}");
        } else {
            tracing::info!(
                "Mission bridge: self-model updated (domain '{domain}': {:.0}%, mission_create: {:.0}%)",
                new_conf * 100.0,
                model.tool_proficiency.get("mission_create").unwrap_or(&0.5) * 100.0,
            );
        }
    }

    tracing::info!(
        "Mission bridge complete: {} goals matched, success={}",
        matched,
        success
    );
}

// ── Key Rotation ─────────────────────────────────────────────
//
// SECURITY: Key rotation is handled via `aivyx rotate-key` CLI command,
// NOT as an LLM tool. Passphrases must never flow through the LLM
// provider's API or be stored in conversation history.
//
// See Command::RotateKey in main.rs for the implementation.

// ── Service configs bundle ─────────────────────────────────────

/// All optional service configurations bundled into a single struct.
///
/// This prevents silent parameter reordering bugs when passing
/// multiple `Option<...Config>` arguments through function chains.
pub struct ServiceConfigs {
    pub email: Option<aivyx_actions::email::EmailConfig>,
    pub calendar: Option<aivyx_actions::calendar::CalendarConfig>,
    pub contacts: Option<aivyx_actions::contacts::ContactsConfig>,
    pub vault: Option<aivyx_actions::documents::VaultConfig>,
    pub telegram: Option<aivyx_actions::messaging::TelegramConfig>,
    pub matrix: Option<aivyx_actions::messaging::MatrixConfig>,
    pub devtools: Option<aivyx_actions::devtools::DevToolsConfig>,
    pub signal: Option<aivyx_actions::messaging::SignalConfig>,
    pub sms: Option<aivyx_actions::messaging::SmsConfig>,
}

// ── Agent construction ──────────────────────────────────────────

/// Build result — the agent plus optional shared brain store for the loop.
pub struct BuiltAgent {
    pub agent: Agent,
    pub brain_store: Option<Arc<BrainStore>>,
    /// Mission tool context for task-engine integration (future TUI views).
    pub mission_ctx: Option<MissionToolContext>,
    /// Memory manager for sharing with the agent loop (heartbeat consolidation).
    pub memory_manager: Option<Arc<Mutex<MemoryManager>>>,
    /// Derived workflow domain key for the trigger engine.
    pub workflow_key: MasterKey,
    /// MCP server pool for graceful shutdown of plugin connections.
    pub mcp_pool: Option<Arc<McpServerPool>>,
    /// Plugin state for runtime plugin management.
    pub plugin_state: Option<PluginState>,
    /// IMAP connection pool for sharing between agent tools and the loop.
    pub imap_pool: Option<Arc<aivyx_actions::email::ImapPool>>,
    /// True when this is the agent's very first launch (brain was empty).
    /// Used to trigger onboarding: the agent introduces itself and learns
    /// about the user.
    pub is_first_launch: bool,
}

/// Build an Agent from the unlocked key + provider. Shared by TUI and one-shot chat.
pub async fn build_agent(
    dirs: &AivyxDirs,
    config: &AivyxConfig,
    pa_config: &PaConfig,
    services: ServiceConfigs,
    store: Arc<EncryptedStore>,
    master_key: MasterKey,
    provider: Box<dyn LlmProvider>,
    audit_log: Option<AuditLog>,
) -> anyhow::Result<BuiltAgent> {
    let ServiceConfigs {
        email: email_config,
        calendar: calendar_config,
        contacts: contacts_config,
        vault: vault_config,
        telegram: telegram_config,
        matrix: matrix_config,
        devtools: devtools_config,
        signal: signal_config,
        sms: sms_config,
    } = services;
    let agent_cfg = pa_config.agent_config();
    let tier = config.autonomy.default_tier;
    let mut system_prompt = pa_config.effective_system_prompt();

    // Inject autonomy tier awareness — tells the agent how independently it can act.
    system_prompt.push_str(crate::config::pa_prompt_autonomy(tier));

    // Wrap in Arc so the key can be shared without copying raw bytes.
    // Only extract raw bytes when we must interface with upstream types that
    // don't support Arc<MasterKey> (e.g. MissionToolContext, AgentSession).
    let master_key = Arc::new(master_key);

    // Derive workflow domain key early
    let workflow_key = aivyx_crypto::derive_domain_key(&master_key, b"workflow");

    // Register action tools (files, shell, web)
    let mut registry = ToolRegistry::new();
    register_default_actions(&mut registry);

    // Register reminder tools (persisted to encrypted store)
    aivyx_actions::bridge::register_reminder_actions(
        &mut registry,
        Arc::clone(&store),
        &master_key,
    );

    // Create IMAP connection pool if email is configured.
    let imap_pool = email_config
        .as_ref()
        .map(|ec| aivyx_actions::email::ImapPool::new(ec.clone()));

    // Register email tools if email is configured.
    // Append email-specific prompt only when tools are actually available.
    if let Some(ref ec) = email_config {
        aivyx_actions::bridge::register_email_actions(&mut registry, ec.clone(), imap_pool.clone());
        system_prompt.push_str(&crate::config::pa_prompt_email(tier));
    }

    // Register calendar tools if calendar is configured.
    // Append calendar-specific prompt only when tools are actually available.
    if let Some(ref cc) = calendar_config {
        aivyx_actions::bridge::register_calendar_actions(&mut registry, cc.clone());
        system_prompt.push_str(&crate::config::pa_prompt_calendar(tier));
    }

    // Register Telegram tools if configured.
    if let Some(ref tc) = telegram_config {
        aivyx_actions::bridge::register_telegram_actions(&mut registry, tc.clone());
        system_prompt.push_str(&crate::config::pa_prompt_telegram(tier));
    }

    // Register Matrix tools if configured.
    if let Some(ref mc) = matrix_config {
        aivyx_actions::bridge::register_matrix_actions(&mut registry, mc.clone());
        system_prompt.push_str(&crate::config::pa_prompt_matrix(tier));
    }

    // Register Signal tools if configured.
    if let Some(ref sc) = signal_config {
        aivyx_actions::bridge::register_signal_actions(&mut registry, sc.clone());
        system_prompt.push_str(&crate::config::pa_prompt_signal(tier));
    }

    // Register SMS tools if configured.
    if let Some(ref sc) = sms_config {
        aivyx_actions::bridge::register_sms_actions(&mut registry, sc.clone());
        system_prompt.push_str(&crate::config::pa_prompt_sms(tier));
    }

    // Register dev tools (git + CI/CD) if configured.
    if let Some(ref dc) = devtools_config {
        aivyx_actions::bridge::register_devtools_actions(&mut registry, dc.clone());
        system_prompt.push_str(crate::config::PA_PROMPT_DEVTOOLS);
        if dc.forge.is_some() {
            system_prompt.push_str(crate::config::PA_PROMPT_CI);
            system_prompt.push_str(&crate::config::pa_prompt_issues(tier));
            system_prompt.push_str(&crate::config::pa_prompt_prs(tier));
        }
    }

    // Register contact tools (search/list always, sync only with CardDAV).
    // Contact tools are always registered since contacts may be populated
    // via email enrichment even without a CardDAV server.
    aivyx_actions::bridge::register_contact_actions(
        &mut registry,
        Arc::clone(&store),
        &master_key,
        contacts_config,
    )?;
    system_prompt.push_str(crate::config::PA_PROMPT_CONTACTS);

    // Register desktop interaction tools if configured.
    if let Some(ref dc) = pa_config.desktop {
        aivyx_actions::bridge::register_desktop_actions(&mut registry, dc.clone());
        system_prompt.push_str(crate::config::PA_PROMPT_DESKTOP);

        // Deep interaction tools (AT-SPI2/UIA, CDP, MPRIS/SMTC, ydotool/SendInput) if configured.
        if dc.interaction.as_ref().is_some_and(|ic| ic.enabled) {
            system_prompt.push_str(crate::config::pa_prompt_interaction());
        }
    }

    let agent_id = AgentId::new();

    // Wire memory if an embedding provider is available
    let memory_manager =
        match wire_memory(dirs, config, &store, &master_key, agent_id, &agent_cfg.name) {
            Ok(mgr) => Some(mgr),
            Err(e) => {
                tracing::warn!("Memory system unavailable: {e}");
                None
            }
        };

    // Register memory tools into the tool registry
    if let Some(ref mgr) = memory_manager {
        aivyx_memory::register_memory_tools(&mut registry, Arc::clone(mgr), agent_id);
        // Knowledge graph query tools (traverse, find paths, search, stats)
        aivyx_actions::bridge::register_knowledge_actions(&mut registry, Arc::clone(mgr));
        // Pattern mining results (discovered workflow patterns)
        registry.register(Box::new(ListDiscoveredPatternsTool::new(Arc::clone(mgr))));
        system_prompt.push_str(crate::config::PA_PROMPT_KNOWLEDGE_GRAPH);
    }

    // Save vault path before vault_config is moved into document tools.
    let vault_path_for_receipts = vault_config.as_ref().map(|vc| vc.path.clone());

    // Register document vault tools if vault is configured AND memory is available.
    // Document tools need the memory system for semantic search and indexing.
    if let (Some(vc), Some(mgr)) = (vault_config, &memory_manager) {
        let vault_key = aivyx_crypto::derive_domain_key(&master_key, b"vault");
        aivyx_actions::bridge::register_document_actions(
            &mut registry,
            vc,
            Arc::clone(mgr),
            Arc::clone(&store),
            vault_key,
        );
        system_prompt.push_str(crate::config::PA_PROMPT_DOCUMENTS);
    }

    // Register finance tracking tools if finance is enabled.
    if pa_config.finance.as_ref().is_some_and(|f| f.enabled) {
        let receipt_folder = pa_config
            .finance
            .as_ref()
            .map(|f| f.receipt_folder.as_str())
            .unwrap_or("receipts");
        let vault_path = vault_path_for_receipts.as_deref();
        aivyx_actions::bridge::register_finance_actions(
            &mut registry,
            Arc::clone(&store),
            &master_key,
            email_config.as_ref(),
            vault_path,
            receipt_folder,
        );
        system_prompt.push_str(crate::config::PA_PROMPT_FINANCE);
    }

    // Context fusion: append cross-source reasoning instructions when 2+ sources are active
    {
        let source_count = [
            email_config.is_some(),
            calendar_config.is_some(),
            true, // contacts always registered
            vault_path_for_receipts.is_some(),
            pa_config.finance.as_ref().is_some_and(|f| f.enabled),
            telegram_config.is_some(),
            matrix_config.is_some(),
        ]
        .iter()
        .filter(|&&x| x)
        .count();
        if source_count >= 2 {
            system_prompt.push_str(crate::config::PA_PROMPT_CONTEXT_FUSION);
        }
    }

    // Triage prompt + tools when autonomous email triage is enabled
    if pa_config.triage.as_ref().is_some_and(|t| t.enabled) && email_config.is_some() {
        aivyx_actions::bridge::register_triage_actions(
            &mut registry,
            Arc::clone(&store),
            &master_key,
        );
        system_prompt.push_str(crate::config::PA_PROMPT_TRIAGE);
    }

    // Workflow management tools (create, list, run, inspect, delete, library)
    aivyx_actions::bridge::register_workflow_actions(
        &mut registry,
        Arc::clone(&store),
        &master_key,
    );

    // Seed built-in workflow library (skip templates that already exist)
    {
        use aivyx_actions::workflow::WorkflowContext;
        let seed_ctx = WorkflowContext::new(
            Arc::clone(&store),
            &aivyx_crypto::derive_domain_key(&master_key, b"workflow"),
        );
        match aivyx_actions::workflow::library::seed_library(&seed_ctx) {
            Ok(n) if n > 0 => tracing::info!("Installed {n} workflow library templates"),
            Ok(_) => {} // all templates already exist
            Err(e) => tracing::warn!("Failed to seed workflow library: {e}"),
        }
    }
    system_prompt.push_str(crate::config::PA_PROMPT_WORKFLOW_LIBRARY);

    // Undo system — record, list, and reverse destructive actions
    aivyx_actions::bridge::register_undo_actions(&mut registry, Arc::clone(&store), &master_key);
    system_prompt.push_str(&crate::config::pa_prompt_undo(tier));

    // ── MCP Plugin Integration ──────────────────────────────────────
    //
    // Connect to configured MCP servers, discover their tools, and register
    // proxy tools alongside built-in actions. The agent sees MCP tools
    // identically to native tools — no special handling needed.
    let plugin_key = aivyx_crypto::derive_domain_key(&master_key, b"plugins");
    let plugin_state = PluginState::new(config, Arc::clone(&store), plugin_key);
    let enabled_plugins = plugin_state.enabled_plugins().await;

    let has_dynamic_plugins = !enabled_plugins.is_empty();
    let has_static_servers = !pa_config.mcp_servers.is_empty();

    let (mcp_pool, _mcp_tool_count) = if !has_dynamic_plugins && !has_static_servers {
        (None, 0usize)
    } else {
        let pool = Arc::new(McpServerPool::new());
        let cache = Arc::new(ToolResultCache::new(std::time::Duration::from_secs(300)));
        let mut total_tools = 0usize;

        // Connect dynamic plugins (installed via install_plugin)
        for plugin in &enabled_plugins {
            match connect_mcp_plugin(&plugin.mcp_config, &pool, &cache).await {
                Ok(tool_count) => {
                    tracing::info!(
                        "Plugin '{}': connected, {} tools discovered",
                        plugin.name,
                        tool_count,
                    );
                    total_tools += tool_count;
                }
                Err(e) => {
                    tracing::warn!("Plugin '{}' failed to connect: {e}", plugin.name);
                }
            }
        }

        // Connect static [[mcp_servers]] from config.toml
        for server_config in &pa_config.mcp_servers {
            match connect_mcp_plugin(server_config, &pool, &cache).await {
                Ok(tool_count) => {
                    tracing::info!(
                        "MCP server '{}': connected, {} tools discovered",
                        server_config.name,
                        tool_count,
                    );
                    total_tools += tool_count;
                }
                Err(e) => {
                    tracing::warn!("MCP server '{}' failed to connect: {e}", server_config.name);
                }
            }
        }

        // Collect all MCP configs for proxy tool creation
        let all_mcp_configs: Vec<&McpServerConfig> = enabled_plugins
            .iter()
            .map(|p| &p.mcp_config)
            .chain(pa_config.mcp_servers.iter())
            .collect();

        // Register proxy tools from all connected servers into the agent registry.
        for server_name in pool.server_names().await {
            if let Some(client) = pool.get(&server_name).await {
                let config_ref = all_mcp_configs
                    .iter()
                    .find(|c| c.name == server_name)
                    .copied();

                match client.list_tools().await {
                    Ok(tools) => {
                        for tool_def in tools {
                            let proxy = if let Some(cfg) = config_ref {
                                McpProxyTool::with_config(
                                    tool_def,
                                    Arc::clone(&pool),
                                    &server_name,
                                    Some(Arc::clone(&cache)),
                                    cfg,
                                )
                            } else {
                                McpProxyTool::new(
                                    tool_def,
                                    Arc::clone(&pool),
                                    &server_name,
                                    Some(Arc::clone(&cache)),
                                )
                            };
                            registry.register(Box::new(proxy));
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to list tools from '{server_name}': {e}");
                    }
                }
            }
        }

        // Register MCP prompts discovery tool
        registry.register(Box::new(McpListPromptsTool::new(Arc::clone(&pool))));

        (Some(pool), total_tools)
    };

    // Register plugin management tools (list, install, enable/disable, search).
    aivyx_actions::bridge::register_plugin_actions(&mut registry, plugin_state.clone());

    // Append plugin prompt — management tools (list, install, enable/disable) are
    // always registered, so the agent should always know about plugin capabilities.
    system_prompt.push_str(crate::config::PA_PROMPT_PLUGINS);

    // Webhook prompt when webhook receiver is configured
    if pa_config.webhook.as_ref().is_some_and(|w| w.enabled) {
        system_prompt.push_str(crate::config::PA_PROMPT_WEBHOOKS);
    }

    // Style adaptation: append instructions when memory is available (profile-backed)
    // or when explicit style preferences are configured.
    if memory_manager.is_some() || pa_config.style.is_some() {
        system_prompt.push_str(crate::config::PA_PROMPT_STYLE_ADAPTATION);
    }

    // Environment awareness: inject dedicated-hardware prompt when configured.
    if let Some(ref env) = pa_config.environment {
        if env.mode == "dedicated" {
            system_prompt.push_str(crate::config::PA_PROMPT_ENVIRONMENT_DEDICATED);
        }
        // Append custom description if provided (works for any mode).
        if let Some(ref desc) = env.description {
            system_prompt.push_str(&format!("\n\nEnvironment note: {desc}"));
        }
    }

    // Behavioral intelligence: error recovery strategies and multi-tool workflow patterns.
    // Desktop-specific examples are only included when desktop tools are registered.
    let has_desktop = pa_config.desktop.is_some();
    system_prompt.push_str(&crate::config::pa_prompt_error_recovery(has_desktop));
    system_prompt.push_str(&crate::config::pa_prompt_tool_chaining(has_desktop));

    // Wire brain (goals, self-model, working memory)
    let (brain, shared_brain_store, is_first_launch) = match wire_brain(
        dirs,
        &master_key,
        &agent_cfg.name,
        &agent_cfg.persona,
        &pa_config.initial_goals,
    ) {
        Ok((brain, brain_store, first_launch)) => {
            // Register brain tools so the LLM can manage goals.
            // Each tool needs its own derived key (MasterKey is not Clone).
            registry.register(Box::new(BrainSetGoalTool::new(
                Arc::clone(&brain_store),
                derive_brain_key(&master_key),
            )));
            registry.register(Box::new(BrainListGoalsTool::new(
                Arc::clone(&brain_store),
                derive_brain_key(&master_key),
            )));
            registry.register(Box::new(BrainUpdateGoalTool::new(
                Arc::clone(&brain_store),
                derive_brain_key(&master_key),
            )));
            registry.register(Box::new(BrainReflectTool::new(
                Arc::clone(&brain_store),
                derive_brain_key(&master_key),
            )));
            // Self-model update tool — closes the self-learning feedback loop
            registry.register(Box::new(BrainUpdateSelfModelTool::new(
                Arc::clone(&brain_store),
                derive_brain_key(&master_key),
            )));
            (Some(brain), Some(brain_store), first_launch)
        }
        Err(e) => {
            tracing::warn!("Brain system unavailable: {e}");
            (None, None, false)
        }
    };

    // Build AgentSession for the task engine.
    // Extract raw bytes only here — for upstream types that can't use Arc<MasterKey>.
    let master_key_bytes_for_upstream = Zeroizing::new(master_key.expose_secret().to_vec());
    let session_master_key = {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&master_key_bytes_for_upstream);
        MasterKey::from_bytes(bytes)
    };
    let session = Arc::new(aivyx_agent::AgentSession::new(
        dirs.clone(),
        config.clone(),
        session_master_key,
    ));

    // Build MissionToolContext and register mission tools.
    // When brain is available, replace mission_create with a PA-local version
    // that bridges mission outcomes back into brain goals + self-model.
    let mission_ctx = MissionToolContext {
        session: Arc::clone(&session),
        dirs: dirs.clone(),
        master_key_bytes: master_key_bytes_for_upstream,
        memory_manager: memory_manager.as_ref().map(Arc::clone),
    };

    // Resolve mission config defaults.
    let mission_cfg = pa_config.missions.clone().unwrap_or_default();
    let default_mode = match mission_cfg.default_mode.as_str() {
        "dag" => ExecutionMode::Dag,
        _ => ExecutionMode::Sequential,
    };
    let experiment_tracker = if mission_cfg.experiment_tracking {
        Some(Arc::new(tokio::sync::Mutex::new(ExperimentTracker::new())))
    } else {
        None
    };

    if let Some(ref brain_store) = shared_brain_store {
        // Register PA-local mission_create with brain bridge
        registry.register(Box::new(PaMissionCreateTool::new(
            mission_ctx.clone(),
            agent_cfg.name.clone(),
            Arc::clone(brain_store),
            Arc::clone(&master_key),
            default_mode,
            experiment_tracker,
        )));
        // Register the other 3 mission tools from the standard set
        // (list, status, control don't need the bridge)
        for tool in create_mission_tools(mission_ctx.clone(), &agent_cfg.name) {
            if tool.name() != "mission_create" {
                registry.register(tool);
            }
        }
    } else {
        // No brain — use standard mission tools (no bridge)
        for tool in create_mission_tools(mission_ctx.clone(), &agent_cfg.name) {
            registry.register(tool);
        }
    }

    // Register recipe-based mission creation tool.
    let recipe_dir = mission_cfg
        .recipe_dir
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| dirs.recipes_dir());
    registry.register(Box::new(MissionFromRecipeTool::new(
        mission_ctx.clone(),
        agent_cfg.name.clone(),
        recipe_dir,
    )));

    // Seed Nonagon team profiles and register delegation tool.
    // This gives the PA agent the ability to delegate complex tasks to a
    // team of 9 specialist sub-agents (coordinator, researcher, analyst, etc.).
    match seed_nonagon_team(dirs) {
        Ok(n) if n > 0 => tracing::info!("Seeded {n} Nonagon team files (profiles + config)"),
        Ok(_) => {}
        Err(e) => tracing::warn!("Failed to seed Nonagon team: {e}"),
    }
    registry.register(Box::new(aivyx_team::TeamDelegateTool::new(
        &session,
        dirs.clone(),
    )));
    system_prompt.push_str(crate::config::PA_PROMPT_DELEGATION);

    // Key rotation is handled via CLI (`aivyx rotate-key`), not as an LLM tool.
    // Passphrases must never flow through the LLM provider's API.

    // Register schedule management tools (create/edit/delete [[schedules]] in config.toml).
    let config_path = Arc::new(dirs.config_path());
    registry.register(Box::new(crate::schedule_tools::ScheduleCreateTool::new(
        Arc::clone(&config_path),
    )));
    registry.register(Box::new(crate::schedule_tools::ScheduleEditTool::new(
        Arc::clone(&config_path),
    )));
    registry.register(Box::new(crate::schedule_tools::ScheduleDeleteTool::new(
        config_path,
    )));
    system_prompt.push_str(&crate::config::pa_prompt_schedules(tier));

    // Register background task execution tools.
    // Lets the agent spawn long-running shell commands asynchronously and
    // report back when they finish, without blocking the conversation.
    //
    // Tasks are persisted to tasks.json so completed task history survives
    // process restarts. Running tasks are marked TimedOut on reload.
    let tasks_path = dirs.root().join("tasks.json");
    let task_persist_path = std::sync::Arc::new(tasks_path.clone());
    let task_registry = aivyx_actions::tasks::new_registry_from_path(&tasks_path);
    aivyx_actions::bridge::register_task_actions(
        &mut registry,
        task_registry,
        Some(task_persist_path),
    );
    system_prompt.push_str(crate::config::PA_PROMPT_TASKS);

    // Register system monitoring tools.
    // Proactive diagnostics: disk space, process health, log tailing,
    // URL health checks, and system stats snapshot.
    aivyx_actions::bridge::register_monitor_actions(&mut registry);
    system_prompt.push_str(crate::config::PA_PROMPT_MONITOR);

    // Build capability set — grant all scopes required by registered tools.
    let principal = Principal::Agent(agent_id);
    let mut caps = CapabilitySet::new();

    let grant = |scope: CapabilityScope, principal: &Principal| -> Capability {
        Capability {
            id: CapabilityId::new(),
            scope,
            pattern: ActionPattern::new("*").unwrap(),
            granted_to: vec![principal.clone()],
            granted_by: Principal::System,
            created_at: chrono::Utc::now(),
            expires_at: None,
            revoked: false,
            parent_id: None,
        }
    };

    caps.grant(grant(
        CapabilityScope::Custom("self-improvement".into()),
        &principal,
    ));
    caps.grant(grant(CapabilityScope::Custom("memory".into()), &principal));
    caps.grant(grant(
        CapabilityScope::Custom("missions".into()),
        &principal,
    ));
    caps.grant(grant(
        CapabilityScope::Custom("coordination".into()),
        &principal,
    ));
    caps.grant(grant(
        CapabilityScope::Shell {
            allowed_commands: vec![],
        },
        &principal,
    ));
    caps.grant(grant(
        CapabilityScope::Filesystem {
            root: std::path::PathBuf::from("/"),
        },
        &principal,
    ));
    caps.grant(grant(
        CapabilityScope::Network {
            hosts: vec![],
            ports: vec![],
        },
        &principal,
    ));
    caps.grant(grant(CapabilityScope::Custom("admin".into()), &principal));

    // Grant the "desktop" capability only when desktop tools are actually registered.
    // All desktop and interaction tools (open_application, clipboard, ui_inspect,
    // browser_navigate, etc.) require this scope. Without it every call fails at the
    // capability checker even though the tools are registered in the registry.
    if pa_config.desktop.is_some() {
        caps.grant(grant(CapabilityScope::Custom("desktop".into()), &principal));
        tracing::debug!("Desktop capability scope granted — desktop tools are active");
    }

    let mut agent = Agent::new(
        agent_id,
        agent_cfg.name.clone(),
        system_prompt,
        agent_cfg.max_tokens,
        config.autonomy.default_tier,
        provider,
        registry,
        caps,
        RateLimiter::new(config.autonomy.max_tool_calls_per_minute),
        CostTracker::new(config.autonomy.max_cost_per_session_usd, 0.0, 0.0),
        audit_log,
        config.autonomy.max_retries,
        config.autonomy.retry_base_delay_ms,
    );
    agent.set_require_approval_for_destructive(config.autonomy.require_approval_for_destructive);
    agent.set_scope_overrides(config.autonomy.scope_overrides.clone());
    agent.set_escalation_confidence_threshold(config.autonomy.escalation_confidence_threshold);

    // ── Abuse Detection ─────────────────────────────────────────
    // Wire sliding-window anomaly detector if configured.
    if let Some(ref abuse_cfg) = pa_config.abuse_detection
        && abuse_cfg.enabled
    {
        let detector = aivyx_audit::abuse::AbuseDetector::new(abuse_cfg.to_detector_config());
        agent.set_abuse_detector(Arc::new(detector));
        tracing::info!(
            window_secs = abuse_cfg.window_secs,
            max_calls = abuse_cfg.max_calls_per_window,
            "Tool abuse detection enabled"
        );
    }

    // ── Tool Discovery ──────────────────────────────────────────
    // When tool_discovery is configured, embed all tool descriptions and
    // attach the index so the agent only sends the most relevant tools per turn.
    if let Some(ref td_config) = pa_config.tool_discovery
        && td_config.is_enabled()
    {
        if let Some(ref embedding_config) = config.embedding {
            match create_embedding_provider(embedding_config, &store, &master_key) {
                Ok(emb_provider) => {
                    let emb_arc: Arc<dyn aivyx_llm::EmbeddingProvider> = Arc::from(emb_provider);

                    // Batch-embed all registered tool descriptions.
                    let tool_list = agent.tool_list();
                    let texts: Vec<String> = tool_list
                        .iter()
                        .map(|t| format!("{}: {}", t.name(), t.description()))
                        .collect();
                    let text_refs: Vec<&str> = texts.iter().map(|s| s.as_str()).collect();

                    match emb_arc.embed_batch(&text_refs).await {
                        Ok(embeddings) => {
                            let mut index = aivyx_core::ToolEmbeddingIndex::new();
                            for (tool, (text, emb)) in tool_list
                                .iter()
                                .zip(texts.into_iter().zip(embeddings.into_iter()))
                            {
                                index.upsert(tool.id(), emb.vector, text);
                            }
                            let mut engine_config = td_config.to_engine_config();

                            // When desktop tools are registered, ensure the most
                            // commonly needed ones are always visible to the LLM,
                            // regardless of how low top_k is set. Without this,
                            // a message like "open Firefox" may score below the
                            // threshold and the agent will never see open_application.
                            if has_desktop {
                                let desktop_essentials: &[&str] = &[
                                    "open_application",
                                    "clipboard_read",
                                    "clipboard_write",
                                    "list_windows",
                                    "send_notification",
                                    "ui_inspect",
                                    "ui_click",
                                    "ui_find_element",
                                    "browser_navigate",
                                    "browser_list_tabs",
                                    "window_screenshot",
                                ];
                                for &tool in desktop_essentials {
                                    let name = tool.to_string();
                                    if !engine_config.always_include.contains(&name) {
                                        engine_config.always_include.push(name);
                                    }
                                }
                            }

                            tracing::info!(
                                mode = ?engine_config.mode,
                                top_k = engine_config.top_k,
                                threshold = engine_config.threshold,
                                always_include = ?engine_config.always_include,
                                tools_indexed = tool_list.len(),
                                "Tool discovery enabled"
                            );
                            agent.enable_tool_discovery(index);
                            agent.set_tool_discovery(engine_config, emb_arc);
                        }
                        Err(e) => {
                            tracing::warn!("Failed to embed tools for discovery: {e}");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Tool discovery requires embedding provider: {e}");
                }
            }
        } else {
            tracing::warn!("Tool discovery enabled but no [embedding] configured — skipping");
        }
    }

    // Seed config-declared style preferences into the UserProfile so the LLM
    // has them from the first turn (before profile extraction runs).
    // Uses try_lock() since build_agent is sync — no contention at startup.
    let config_style_prefs = pa_config.style_preferences();
    if !config_style_prefs.is_empty()
        && let Some(ref mgr) = memory_manager
        && let Ok(mgr_lock) = mgr.try_lock()
    {
        match mgr_lock.get_profile() {
            Ok(mut profile) => {
                let mut changed = false;
                for pref in &config_style_prefs {
                    let already = profile
                        .style_preferences
                        .iter()
                        .any(|existing: &String| existing.eq_ignore_ascii_case(pref));
                    if !already {
                        profile.style_preferences.push(pref.clone());
                        changed = true;
                    }
                }
                if changed && let Err(e) = mgr_lock.update_profile(profile) {
                    tracing::warn!("Failed to seed style preferences: {e}");
                }
            }
            Err(e) => tracing::warn!("Failed to read profile for style seeding: {e}"),
        }
    }

    // Clone memory manager for the loop before agent consumes it
    let loop_memory_manager = memory_manager.as_ref().map(Arc::clone);

    // Attach memory manager to the agent for memory-augmented turns
    if let Some(mgr) = memory_manager {
        agent.set_memory_manager(mgr);
        // Enable RetrievalRouter if configured
        agent.use_retrieval_router = pa_config
            .consolidation
            .as_ref()
            .map(|c| c.retrieval_router)
            .unwrap_or(false);
    }

    // Attach brain to the agent for goal-aware turns
    if let Some(brain) = brain {
        agent.set_brain(brain);
    }

    Ok(BuiltAgent {
        agent,
        brain_store: shared_brain_store,
        mission_ctx: Some(mission_ctx),
        memory_manager: loop_memory_manager,
        workflow_key,
        mcp_pool,
        plugin_state: Some(plugin_state),
        imap_pool,
        is_first_launch,
    })
}

/// Connect a single MCP plugin: spawn/connect client, initialize, insert into pool.
///
/// Returns the number of tools discovered from this server.
async fn connect_mcp_plugin(
    config: &aivyx_config::McpServerConfig,
    pool: &Arc<McpServerPool>,
    _cache: &Arc<ToolResultCache>,
) -> anyhow::Result<usize> {
    let client = Arc::new(McpClient::connect(config).await?);
    client.initialize().await?;
    let tools = client.list_tools().await?;
    let tool_count = tools.len();
    pool.insert(config.name.clone(), client, config.clone())
        .await;
    Ok(tool_count)
}
/// Create embedding provider + MemoryStore + MemoryManager.
fn wire_memory(
    dirs: &AivyxDirs,
    config: &AivyxConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
    _agent_id: AgentId,
    agent_name: &str,
) -> anyhow::Result<Arc<Mutex<MemoryManager>>> {
    let embed_config = config.embedding.clone().unwrap_or_default();
    let embedding_provider = create_embedding_provider(&embed_config, store, master_key)?;

    let memory_key = aivyx_crypto::derive_memory_key(master_key);
    let agent_mem_dir = dirs.agent_memory_dir(agent_name);
    std::fs::create_dir_all(&agent_mem_dir)?;
    let memory_store = MemoryStore::open(agent_mem_dir.join("memory.db"))?;
    let manager = MemoryManager::new(
        memory_store,
        Arc::from(embedding_provider),
        memory_key,
        0, // unlimited memories
    )?;

    Ok(Arc::new(Mutex::new(manager)))
}

/// Open the Brain (goals, self-model) with its own derived key.
/// Seeds starter goals on first run if the brain is empty.
fn wire_brain(
    dirs: &AivyxDirs,
    master_key: &MasterKey,
    agent_name: &str,
    persona: &str,
    config_goals: &[crate::config::PaInitialGoal],
) -> anyhow::Result<(Brain, Arc<BrainStore>, bool)> {
    let brain_key = derive_brain_key(master_key);
    let brain_path = dirs.agent_brain_path(agent_name);

    // Ensure the directory exists
    if let Some(parent) = brain_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let store = Arc::new(BrainStore::open(&brain_path)?);
    let brain_key_for_model = aivyx_crypto::derive_brain_key(master_key);
    let brain = Brain::from_store(Arc::clone(&store), agent_name, brain_key)?;

    // Detect first PA launch by checking if the self-model exists.
    // We use the self-model (not goal count) because external tools like
    // `aivyx init` may seed goals before the TUI ever runs. The self-model
    // is only created by our seed_initial_self_model(), making it a reliable
    // "has the PA awakened?" signal.
    let has_self_model = store
        .load_self_model(&brain_key_for_model)
        .ok()
        .flatten()
        .is_some();
    let is_first_launch = !has_self_model;

    let goal_count = brain.active_goals().map(|g| g.len()).unwrap_or(0);

    // Seed initial goals if brain has no goals yet.
    // Config-provided goals take priority; fall back to persona defaults.
    if goal_count == 0 {
        if config_goals.is_empty() {
            seed_starter_goals(&brain, persona);
        } else {
            seed_config_goals(&brain, config_goals);
        }
    }

    // On first PA launch, seed growth goals and self-model
    if is_first_launch {
        seed_agent_growth_goals(&brain, persona);
        seed_initial_self_model(&store, &brain_key_for_model, persona);
    }

    tracing::info!(
        "Brain loaded with {} active goals{}",
        brain.active_goals().map(|g| g.len()).unwrap_or(0),
        if is_first_launch {
            " (first launch — seeded)"
        } else {
            ""
        },
    );

    Ok((brain, store, is_first_launch))
}

/// Seed persona-appropriate starter goals into an empty brain.
fn seed_starter_goals(brain: &Brain, persona: &str) {
    use aivyx_brain::Priority;

    let goals: &[(&str, &str, Priority)] = match persona {
        "coder" => &[
            (
                "Learn the user's tech stack and coding preferences",
                "Can accurately suggest code in the user's preferred language and style",
                Priority::Medium,
            ),
            (
                "Track and remember ongoing projects",
                "Maintains awareness of active project names, paths, and current tasks",
                Priority::Low,
            ),
        ],
        "researcher" => &[
            (
                "Build a knowledge base of the user's research interests",
                "Can name the user's top 3 research topics and recent papers/sources",
                Priority::Medium,
            ),
            (
                "Monitor key sources for new developments",
                "Proactively alerts user to relevant new publications or news",
                Priority::Low,
            ),
        ],
        "writer" => &[
            (
                "Learn the user's writing style and voice",
                "Can produce drafts that match the user's tone without correction",
                Priority::Medium,
            ),
            (
                "Track ongoing writing projects and deadlines",
                "Maintains awareness of current documents and their status",
                Priority::Low,
            ),
        ],
        "coach" => &[
            (
                "Learn the user's personal and professional goals",
                "Can name the user's top 3 goals and current progress on each",
                Priority::High,
            ),
            (
                "Check in on goal progress weekly",
                "Proactively asks about progress and offers encouragement",
                Priority::Medium,
            ),
        ],
        "companion" => &[
            (
                "Learn what matters to the user",
                "Remembers important people, interests, and preferences",
                Priority::Medium,
            ),
            (
                "Be attentive to the user's mood and energy",
                "Adjusts tone and suggestions based on context clues",
                Priority::Low,
            ),
        ],
        "ops" => &[
            (
                "Learn the user's infrastructure and systems",
                "Can name servers, services, and typical maintenance tasks",
                Priority::Medium,
            ),
            (
                "Monitor for system alerts and issues",
                "Proactively checks email for alerts and flags urgent items",
                Priority::High,
            ),
        ],
        "analyst" => &[
            (
                "Learn the user's data sources and key metrics",
                "Can name the user's primary dashboards and KPIs",
                Priority::Medium,
            ),
            (
                "Track recurring analysis tasks",
                "Maintains awareness of regular reporting cycles",
                Priority::Low,
            ),
        ],
        // "assistant" and anything else
        _ => &[
            (
                "Learn the user's daily routine and preferences",
                "Can anticipate common requests and suggest proactively",
                Priority::Medium,
            ),
            (
                "Monitor inbox for important messages",
                "Alerts user to emails that need a response",
                Priority::Low,
            ),
        ],
    };

    for (desc, criteria, priority) in goals {
        match brain.set_goal(desc.to_string(), criteria.to_string(), *priority) {
            Ok(goal) => tracing::info!("Seeded starter goal: {}", goal.description),
            Err(e) => tracing::warn!("Failed to seed goal: {e}"),
        }
    }
}

/// Seed goals from config.toml `[[initial_goals]]` entries.
fn seed_config_goals(brain: &Brain, goals: &[crate::config::PaInitialGoal]) {
    use aivyx_brain::Priority;

    for g in goals {
        let priority = match g.priority.as_str() {
            "high" => Priority::High,
            "low" => Priority::Low,
            _ => Priority::Medium,
        };
        match brain.set_goal(g.description.clone(), g.success_criteria.clone(), priority) {
            Ok(goal) => tracing::info!("Seeded config goal: {}", goal.description),
            Err(e) => tracing::warn!("Failed to seed config goal: {e}"),
        }
    }
}

/// Seed self-development goals the agent works toward on its own.
///
/// These are meta-goals about the agent becoming better at its role — they
/// complement the user-facing starter goals seeded by `seed_starter_goals`.
/// Tagged `[self-development]` so the heartbeat can distinguish them from
/// user-facing goals when deciding what to reflect on.
fn seed_agent_growth_goals(brain: &Brain, persona: &str) {
    use aivyx_brain::Priority;

    // Universal growth goals every agent gets
    let mut goals: Vec<(&str, &str, Priority)> = vec![
        (
            "Develop proficiency with all registered tools",
            "Successfully used each registered tool at least once with no errors",
            Priority::Medium,
        ),
        (
            "Build comprehensive understanding of my user",
            "Can name the user's top priorities, communication style, and preferences",
            Priority::High,
        ),
        (
            "Improve accuracy through self-reflection",
            "Reflection logs show measurable reduction in repeated mistakes over 30 days",
            Priority::Medium,
        ),
    ];

    // Persona-specific growth goals
    match persona {
        "coder" => goals.extend([
            (
                "Develop expertise in the user's primary language and frameworks",
                "Can generate idiomatic code in the user's stack without correction",
                Priority::High,
            ),
            (
                "Learn to anticipate debugging needs",
                "Proactively identifies potential issues before the user encounters them",
                Priority::Low,
            ),
        ]),
        "researcher" => goals.extend([
            (
                "Build knowledge graph connections across domains",
                "Can identify non-obvious links between research topics the user studies",
                Priority::Medium,
            ),
            (
                "Improve source evaluation skills",
                "Consistently distinguishes high-quality sources from low-quality ones",
                Priority::Low,
            ),
        ]),
        "writer" => goals.extend([
            (
                "Internalize the user's voice and style patterns",
                "Drafts require fewer than 3 style corrections per document",
                Priority::High,
            ),
            (
                "Develop editorial judgment for different contexts",
                "Can adjust tone appropriately for casual, professional, and academic writing",
                Priority::Low,
            ),
        ]),
        "coach" => goals.extend([
            (
                "Develop empathetic communication patterns",
                "User reports feeling understood and supported in check-ins",
                Priority::High,
            ),
            (
                "Learn to calibrate challenge vs support",
                "Can sense when to push harder and when to ease off based on context",
                Priority::Medium,
            ),
        ]),
        "companion" => goals.extend([
            (
                "Learn to read emotional context from messages",
                "Adjusts tone appropriately when user seems stressed, excited, or tired",
                Priority::High,
            ),
            (
                "Build a rich model of the user's interests and relationships",
                "Can recall important people, events, and preferences without prompting",
                Priority::Medium,
            ),
        ]),
        "ops" => goals.extend([
            (
                "Build expertise in the user's infrastructure patterns",
                "Can predict common failure modes for the user's systems",
                Priority::High,
            ),
            (
                "Develop proactive monitoring instincts",
                "Identifies potential issues before they escalate to incidents",
                Priority::Medium,
            ),
        ]),
        "analyst" => goals.extend([
            (
                "Develop fluency with the user's data domains",
                "Can interpret metrics and trends without needing context explained",
                Priority::High,
            ),
            (
                "Build pattern recognition across data sources",
                "Identifies cross-dataset correlations the user hasn't noticed",
                Priority::Low,
            ),
        ]),
        _ => goals.extend([
            (
                "Adapt communication style to the user's preferences",
                "User rarely needs to ask for format or tone changes",
                Priority::Medium,
            ),
            (
                "Develop anticipatory assistance habits",
                "Proactively suggests relevant actions before the user asks",
                Priority::Low,
            ),
        ]),
    }

    let tags = vec!["self-development".to_string()];
    for (desc, criteria, priority) in goals {
        let goal = aivyx_brain::Goal::new(desc, criteria)
            .with_priority(priority)
            .with_tags(tags.clone());
        match brain.store().upsert_goal(&goal, brain.key()) {
            Ok(()) => tracing::info!("Seeded growth goal: {}", goal.description),
            Err(e) => tracing::warn!("Failed to seed growth goal: {e}"),
        }
    }
}

/// Seed an initial self-model with persona-appropriate confidence values.
///
/// The self-model is what the agent "knows about itself" — domain expertise,
/// tool skill, strengths, and weaknesses. On first boot we prime it with
/// sensible defaults so the agent starts with appropriate self-awareness
/// rather than a blank slate. The heartbeat's self-reflection loop will
/// refine these values over time based on actual outcomes.
fn seed_initial_self_model(store: &BrainStore, key: &MasterKey, persona: &str) {
    use std::collections::HashMap;

    let (domain_confidence, strengths, weaknesses) = match persona {
        "coder" => {
            let mut dc = HashMap::new();
            dc.insert("programming".into(), 0.7);
            dc.insert("debugging".into(), 0.6);
            dc.insert("architecture".into(), 0.5);
            dc.insert("documentation".into(), 0.5);
            dc.insert("communication".into(), 0.3);
            (
                dc,
                vec![
                    "Technical problem solving".into(),
                    "Code generation and review".into(),
                ],
                vec![
                    "Understanding user's specific codebase (learning)".into(),
                    "Non-technical communication".into(),
                ],
            )
        }
        "researcher" => {
            let mut dc = HashMap::new();
            dc.insert("research".into(), 0.7);
            dc.insert("analysis".into(), 0.6);
            dc.insert("writing".into(), 0.5);
            dc.insert("source-evaluation".into(), 0.5);
            dc.insert("communication".into(), 0.4);
            (
                dc,
                vec![
                    "Information synthesis".into(),
                    "Source discovery and evaluation".into(),
                ],
                vec![
                    "Understanding user's specific domain (learning)".into(),
                    "Practical application of findings".into(),
                ],
            )
        }
        "writer" => {
            let mut dc = HashMap::new();
            dc.insert("writing".into(), 0.7);
            dc.insert("editing".into(), 0.6);
            dc.insert("communication".into(), 0.6);
            dc.insert("research".into(), 0.4);
            dc.insert("technical".into(), 0.2);
            (
                dc,
                vec![
                    "Clear and structured writing".into(),
                    "Adapting tone for different audiences".into(),
                ],
                vec![
                    "Matching user's unique voice (learning)".into(),
                    "Deep domain-specific terminology".into(),
                ],
            )
        }
        "coach" => {
            let mut dc = HashMap::new();
            dc.insert("communication".into(), 0.7);
            dc.insert("goal-setting".into(), 0.6);
            dc.insert("motivation".into(), 0.6);
            dc.insert("empathy".into(), 0.5);
            dc.insert("technical".into(), 0.2);
            (
                dc,
                vec![
                    "Structured goal tracking".into(),
                    "Encouraging accountability".into(),
                ],
                vec![
                    "Understanding user's specific challenges (learning)".into(),
                    "Knowing when to push vs support".into(),
                ],
            )
        }
        "companion" => {
            let mut dc = HashMap::new();
            dc.insert("communication".into(), 0.7);
            dc.insert("empathy".into(), 0.6);
            dc.insert("memory-recall".into(), 0.5);
            dc.insert("emotional-awareness".into(), 0.5);
            dc.insert("technical".into(), 0.2);
            (
                dc,
                vec![
                    "Active listening and engagement".into(),
                    "Remembering personal details".into(),
                ],
                vec![
                    "Understanding user's social world (learning)".into(),
                    "Reading subtle emotional cues from text".into(),
                ],
            )
        }
        "ops" => {
            let mut dc = HashMap::new();
            dc.insert("infrastructure".into(), 0.6);
            dc.insert("monitoring".into(), 0.6);
            dc.insert("troubleshooting".into(), 0.5);
            dc.insert("automation".into(), 0.5);
            dc.insert("communication".into(), 0.3);
            (
                dc,
                vec![
                    "Systematic troubleshooting".into(),
                    "Alert triage and prioritization".into(),
                ],
                vec![
                    "User's specific infrastructure (learning)".into(),
                    "Non-technical communication".into(),
                ],
            )
        }
        "analyst" => {
            let mut dc = HashMap::new();
            dc.insert("data-analysis".into(), 0.7);
            dc.insert("visualization".into(), 0.5);
            dc.insert("statistics".into(), 0.5);
            dc.insert("communication".into(), 0.4);
            dc.insert("domain-knowledge".into(), 0.3);
            (
                dc,
                vec![
                    "Pattern recognition in data".into(),
                    "Clear data presentation".into(),
                ],
                vec![
                    "User's specific data domains (learning)".into(),
                    "Business context for metrics".into(),
                ],
            )
        }
        // "assistant" and all others
        _ => {
            let mut dc = HashMap::new();
            dc.insert("communication".into(), 0.5);
            dc.insert("organization".into(), 0.5);
            dc.insert("research".into(), 0.4);
            dc.insert("writing".into(), 0.4);
            dc.insert("technical".into(), 0.3);
            (
                dc,
                vec![
                    "Versatile task handling".into(),
                    "Clear communication".into(),
                ],
                vec![
                    "Specialized domain expertise (learning)".into(),
                    "Anticipating user needs (learning)".into(),
                ],
            )
        }
    };

    let model = aivyx_brain::SelfModel {
        domain_confidence,
        tool_proficiency: HashMap::new(), // starts empty — learned from usage
        strengths,
        weaknesses,
        social: aivyx_brain::SocialMetrics::default(),
        updated_at: chrono::Utc::now(),
    };

    match store.save_self_model(&model, key) {
        Ok(()) => tracing::info!("Seeded initial self-model for persona '{persona}'"),
        Err(e) => tracing::warn!("Failed to seed self-model: {e}"),
    }
}

/// Build a `TaskEngine` from a `MissionToolContext` for read-only listing.
/// Shared between `PaMissionCreateTool::build_engine()` and the Missions view.
pub fn build_task_engine(ctx: &MissionToolContext) -> aivyx_core::Result<TaskEngine> {
    let master_key = {
        let bytes: [u8; 32] = ctx
            .master_key_bytes
            .as_slice()
            .try_into()
            .map_err(|_| aivyx_core::AivyxError::Crypto("invalid master key length".into()))?;
        MasterKey::from_bytes(bytes)
    };
    let task_key = aivyx_crypto::derive_task_key(&master_key);
    let store = TaskStore::open(ctx.dirs.tasks_dir().join("tasks.db"))?;
    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(ctx.dirs.audit_path(), &audit_key);
    let mut engine = TaskEngine::new(ctx.session.clone(), store, task_key, Some(audit_log));
    if let Some(ref mm) = ctx.memory_manager {
        engine = engine.with_memory_manager(mm.clone());
    }
    Ok(engine)
}
// ── Nonagon Team Seeding ──────────────────────────────────────

/// Seed the Nonagon team config and specialist agent profiles into the
/// aivyx directories. Idempotent — skips files that already exist.
///
/// Returns the number of files written (0 if everything was already seeded).
fn seed_nonagon_team(dirs: &AivyxDirs) -> anyhow::Result<usize> {
    let mut written = 0;

    // Ensure directories exist
    let agents_dir = dirs.agents_dir();
    let teams_dir = dirs.teams_dir();
    std::fs::create_dir_all(&agents_dir)?;
    std::fs::create_dir_all(&teams_dir)?;

    // 1. Seed agent profiles for all 9 Nonagon specialists.
    // Always overwrite: capabilities and soul text evolve with code updates.
    // Nonagon profiles are auto-generated, not user-customized, so overwriting
    // is safe and ensures capability fixes (e.g. coordinator gaining Shell scope
    // for proper attenuation) take effect immediately.
    let profiles = aivyx_team::nonagon::all_nonagon_profiles();
    for profile in &profiles {
        let path = agents_dir.join(format!("{}.toml", profile.name));
        let is_new = !path.exists();
        profile.save(&path)?;
        if is_new {
            tracing::debug!("Seeded Nonagon agent profile: {}", profile.name);
        }
        written += 1;
    }

    // 2. Seed the nonagon team config (always overwrite for same reason as profiles).
    let team_path = teams_dir.join("nonagon.toml");
    let members: Vec<aivyx_team::TeamMemberConfig> = aivyx_team::nonagon::NONAGON_ROLES
        .iter()
        .map(|r| aivyx_team::TeamMemberConfig {
            name: r.name.to_string(),
            role: r.role.to_string(),
        })
        .collect();

    let config = aivyx_team::TeamConfig {
        name: "nonagon".to_string(),
        description: "The Nonagon — 9 specialist sub-agents for complex multi-agent tasks"
            .to_string(),
        orchestration: aivyx_team::OrchestrationMode::LeadAgent {
            lead: "coordinator".to_string(),
        },
        members,
        dialogue: aivyx_team::DialogueConfig::default(),
    };
    config.save(&team_path)?;
    written += 1;

    Ok(written)
}
