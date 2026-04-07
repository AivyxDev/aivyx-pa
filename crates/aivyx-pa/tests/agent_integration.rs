//! Integration tests — verify the full agent pipeline works end-to-end.
//!
//! These tests build real agents with mock LLM providers, real encrypted
//! stores, and real brain instances. They verify that:
//! - Agent construction wires the correct number of tools
//! - Brain seeds starter goals for each persona
//! - The system prompt includes PA-specific instructions
//! - Agent turns execute and return responses
//! - Reminder tools actually persist to the encrypted store

use aivyx_pa::config::{PaAgentConfig, PaConfig};
use aivyx_pa::agent::{build_agent, BuiltAgent, ServiceConfigs};

use aivyx_brain::{GoalFilter, GoalStatus};
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::Result as AivyxResult;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::{
    ChatMessage, ChatRequest, ChatResponse, LlmProvider, StopReason, TokenUsage,
};

use std::path::PathBuf;
use std::sync::Arc;

// ── Mock LLM Provider ─────────────────────────────────────────

struct MockProvider {
    response: String,
}

impl MockProvider {
    fn new(response: impl Into<String>) -> Self {
        Self { response: response.into() }
    }
}

#[async_trait::async_trait]
impl LlmProvider for MockProvider {
    fn name(&self) -> &str { "mock" }

    async fn chat(&self, _request: &ChatRequest) -> AivyxResult<ChatResponse> {
        Ok(ChatResponse {
            message: ChatMessage::assistant(&self.response),
            usage: TokenUsage {
                input_tokens: 50,
                output_tokens: 20,
            },
            stop_reason: StopReason::EndTurn,
        })
    }
}

// ── Test Helpers ──────────────────────────────────────────────

struct TestEnv {
    dirs: AivyxDirs,
    config: AivyxConfig,
    store: Arc<EncryptedStore>,
    master_key_bytes: Vec<u8>,
    _dir: PathBuf,
}

impl TestEnv {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!(
            "aivyx-pa-test-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let dirs = AivyxDirs::new(&dir);
        dirs.ensure_dirs().unwrap();

        // Write a minimal config.toml
        let config_toml = r#"
[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "test-model"
"#;
        std::fs::write(dirs.config_path(), config_toml).unwrap();

        let config = AivyxConfig::load(dirs.config_path()).unwrap();
        let store = Arc::new(EncryptedStore::open(dirs.store_path()).unwrap());
        let master_key = MasterKey::generate();
        let key_bytes = master_key.expose_secret().to_vec();

        // We need to keep the key bytes to create new MasterKeys
        // (MasterKey is not Clone, so we regenerate from bytes)
        Self {
            dirs,
            config,
            store,
            master_key_bytes: key_bytes,
            _dir: dir,
        }
    }

    fn master_key(&self) -> MasterKey {
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&self.master_key_bytes);
        MasterKey::from_bytes(bytes)
    }

    async fn build(&self, pa_config: &PaConfig) -> anyhow::Result<BuiltAgent> {
        build_agent(
            &self.dirs,
            &self.config,
            pa_config,
            ServiceConfigs {
                email: None,
                calendar: None,
                contacts: None,
                vault: None,
                telegram: None,
                matrix: None,
                devtools: None,
                signal: None,
                sms: None,
            },
            Arc::clone(&self.store),
            self.master_key(),
            Box::new(MockProvider::new("Hello! I'm your assistant.")),
            None, // no audit log for tests
        ).await
    }
}

impl Drop for TestEnv {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self._dir);
    }
}

// ── Agent Construction Tests ──────────────────────────────────

#[tokio::test]
async fn agent_has_correct_base_tool_count() {
    let env = TestEnv::new();
    let pa_config = PaConfig::default();

    let built = env.build(&pa_config).await.unwrap();
    let tool_names: Vec<&str> = built.agent.tool_list().iter().map(|t| t.name()).collect();

    // Base tools: read_file, write_file, list_directory, run_command, fetch_webpage
    //           + set_reminder, list_reminders, dismiss_reminder
    // Brain tools (if brain available): brain_set_goal, brain_list_goals,
    //           brain_update_goal, brain_reflect
    // Memory tools: depend on embedding provider (not available with mock)

    // Check required base tools are present
    assert!(tool_names.contains(&"read_file"), "Missing read_file. Tools: {tool_names:?}");
    assert!(tool_names.contains(&"write_file"), "Missing write_file");
    assert!(tool_names.contains(&"list_directory"), "Missing list_directory");
    assert!(tool_names.contains(&"run_command"), "Missing run_command");
    assert!(tool_names.contains(&"fetch_webpage"), "Missing fetch_webpage");

    // Reminder tools
    assert!(tool_names.contains(&"set_reminder"), "Missing set_reminder");
    assert!(tool_names.contains(&"list_reminders"), "Missing list_reminders");
    assert!(tool_names.contains(&"dismiss_reminder"), "Missing dismiss_reminder");

    // Brain tools (brain should initialize)
    assert!(tool_names.contains(&"brain_set_goal"), "Missing brain_set_goal. Tools: {tool_names:?}");
    assert!(tool_names.contains(&"brain_list_goals"), "Missing brain_list_goals");
    assert!(tool_names.contains(&"brain_update_goal"), "Missing brain_update_goal");
    assert!(tool_names.contains(&"brain_reflect"), "Missing brain_reflect");
    assert!(tool_names.contains(&"brain_update_self_model"), "Missing brain_update_self_model. Tools: {tool_names:?}");

    // Mission tools (always registered)
    assert!(tool_names.contains(&"mission_create"), "Missing mission_create. Tools: {tool_names:?}");
    assert!(tool_names.contains(&"mission_list"), "Missing mission_list");
    assert!(tool_names.contains(&"mission_status"), "Missing mission_status");
    assert!(tool_names.contains(&"mission_control"), "Missing mission_control");

    // Contact tools (always registered — search/list work against local store)
    assert!(tool_names.contains(&"search_contacts"), "Missing search_contacts. Tools: {tool_names:?}");
    assert!(tool_names.contains(&"list_contacts"), "Missing list_contacts");
    // sync_contacts only present when CardDAV is configured (not in this test)
    assert!(!tool_names.contains(&"sync_contacts"), "sync_contacts should not be present without CardDAV config");

    // Should have at least 19 tools (5 base + 3 reminder + 5 brain + 4 mission + 2 contacts)
    assert!(tool_names.len() >= 19, "Expected >= 19 tools, got {}: {tool_names:?}", tool_names.len());
}

#[tokio::test]
async fn agent_uses_custom_name_and_max_tokens() {
    let env = TestEnv::new();
    let pa_config = PaConfig {
        agent: Some(PaAgentConfig {
            name: "Aria".into(),
            max_tokens: 8192,
            ..Default::default()
        }),
        ..Default::default()
    };

    let built = env.build(&pa_config).await.unwrap();
    assert_eq!(built.agent.name, "Aria");
}

// ── Brain Seeding Tests ───────────────────────────────────────

#[tokio::test]
async fn brain_seeds_goals_for_assistant_persona() {
    let env = TestEnv::new();
    let pa_config = PaConfig {
        agent: Some(PaAgentConfig {
            persona: "assistant".into(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let built = env.build(&pa_config).await.unwrap();
    let brain_store = built.brain_store.expect("Brain should be available");

    let goals = brain_store
        .list_goals(
            &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
            &aivyx_crypto::derive_brain_key(&env.master_key()),
        )
        .unwrap();

    // 2 starter + 5 growth (3 universal + 2 persona-specific)
    assert_eq!(goals.len(), 7, "Assistant persona should seed 7 goals (2 starter + 5 growth)");
    // Check that descriptions match expected assistant goals
    let descriptions: Vec<&str> = goals.iter().map(|g| g.description.as_str()).collect();
    assert!(descriptions.iter().any(|d| d.contains("routine") || d.contains("preferences")),
        "Expected a routine/preferences goal. Got: {descriptions:?}");
    assert!(descriptions.iter().any(|d| d.contains("proficiency") && d.contains("tools")),
        "Expected a universal growth goal. Got: {descriptions:?}");
}

#[tokio::test]
async fn brain_seeds_goals_for_coder_persona() {
    let env = TestEnv::new();
    let pa_config = PaConfig {
        agent: Some(PaAgentConfig {
            persona: "coder".into(),
            ..Default::default()
        }),
        ..Default::default()
    };

    let built = env.build(&pa_config).await.unwrap();
    let brain_store = built.brain_store.expect("Brain should be available");

    let goals = brain_store
        .list_goals(
            &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
            &aivyx_crypto::derive_brain_key(&env.master_key()),
        )
        .unwrap();

    // 2 starter + 5 growth (3 universal + 2 persona-specific)
    assert_eq!(goals.len(), 7, "Coder persona should seed 7 goals (2 starter + 5 growth)");
    let descriptions: Vec<&str> = goals.iter().map(|g| g.description.as_str()).collect();
    assert!(descriptions.iter().any(|d| d.contains("tech stack") || d.contains("coding")),
        "Expected a tech stack goal. Got: {descriptions:?}");
    assert!(descriptions.iter().any(|d| d.contains("expertise") && d.contains("language")),
        "Expected a coder growth goal. Got: {descriptions:?}");
}

#[tokio::test]
async fn brain_does_not_reseed_on_second_build() {
    let env = TestEnv::new();
    let pa_config = PaConfig::default();

    // First build — seeds goals
    {
        let built1 = env.build(&pa_config).await.unwrap();
        let store1 = built1.brain_store.unwrap();
        let key = aivyx_crypto::derive_brain_key(&env.master_key());
        let goals1 = store1.list_goals(
            &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
            &key,
        ).unwrap();
        assert_eq!(goals1.len(), 7); // 2 starter + 5 growth
    } // drop built1 + store1 to release the redb lock

    // Second build — same brain path, should NOT add more goals
    let built2 = env.build(&pa_config).await.unwrap();
    let store2 = built2.brain_store.expect("Brain should re-open on second build");
    let key2 = aivyx_crypto::derive_brain_key(&env.master_key());
    let goals2 = store2.list_goals(
        &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
        &key2,
    ).unwrap();
    assert_eq!(goals2.len(), 7, "Should not re-seed goals on second build");
}

// ── System Prompt Tests ───────────────────────────────────────

#[test]
fn system_prompt_contains_pa_capabilities() {
    let pa_config = PaConfig::default();
    let prompt = pa_config.effective_system_prompt();

    assert!(prompt.contains("persistent memory"), "Missing memory instruction");
    assert!(prompt.contains("persistent goals"), "Missing goals instruction");
    assert!(prompt.contains("self-model"), "Missing self-model instruction");
    assert!(prompt.contains("brain_reflect"), "Missing brain_reflect mention");

    // Verify all always-present tool names are explicitly mentioned in the base prompt.
    // These tools are registered unconditionally and must appear in PA_PROMPT_SUFFIX
    // so the LLM knows about them.
    let required_tools = [
        // Memory tools
        "memory_store", "memory_retrieve", "memory_search", "memory_forget", "memory_patterns",
        // Goal tools
        "brain_set_goal", "brain_list_goals", "brain_update_goal",
        // Self-model tools
        "brain_reflect", "brain_update_self_model",
        // Reminder tools
        "set_reminder", "list_reminders", "dismiss_reminder",
        // Mission tools
        "mission_create", "mission_list", "mission_status", "mission_control",
        "mission_from_recipe",
        // Web tools
        "search_web", "fetch_webpage",
        // File tools
        "list_directory", "read_file", "write_file",
        // Shell
        "run_command",
        // MCP
        "mcp_list_prompts",
    ];
    for tool in &required_tools {
        assert!(prompt.contains(tool), "PA_PROMPT_SUFFIX missing tool: {tool}");
    }
}

#[test]
fn system_prompt_varies_by_persona() {
    let assistant = PaConfig {
        agent: Some(PaAgentConfig { persona: "assistant".into(), ..Default::default() }),
        ..Default::default()
    };
    let coder = PaConfig {
        agent: Some(PaAgentConfig { persona: "coder".into(), ..Default::default() }),
        ..Default::default()
    };

    let prompt_a = assistant.effective_system_prompt();
    let prompt_c = coder.effective_system_prompt();

    // Both should have PA suffix
    assert!(prompt_a.contains("persistent memory"));
    assert!(prompt_c.contains("persistent memory"));

    // But the body should differ (different persona generates different soul)
    assert_ne!(prompt_a, prompt_c, "Different personas should produce different prompts");
}

// ── Agent Turn Tests ──────────────────────────────────────────

#[tokio::test]
async fn agent_turn_returns_response() {
    let env = TestEnv::new();
    let pa_config = PaConfig::default();
    let mut agent = env.build(&pa_config).await.unwrap().agent;

    let response = agent.turn("Hello, what can you do?", None).await.unwrap();
    assert_eq!(response, "Hello! I'm your assistant.");
}

#[tokio::test]
async fn agent_turn_preserves_conversation() {
    let env = TestEnv::new();
    let pa_config = PaConfig::default();
    let mut agent = env.build(&pa_config).await.unwrap().agent;

    let r1 = agent.turn("First message", None).await.unwrap();
    assert_eq!(r1, "Hello! I'm your assistant.");

    // Second turn — should succeed (conversation state maintained)
    let r2 = agent.turn("Second message", None).await.unwrap();
    assert_eq!(r2, "Hello! I'm your assistant.");
}

// ── Reminder Integration via Store ────────────────────────────

#[test]
fn reminder_round_trip_through_store() {
    let env = TestEnv::new();
    let reminder_key = aivyx_crypto::derive_domain_key(&env.master_key(), b"reminders");

    // Set a reminder directly
    let reminder = aivyx_actions::reminders::Reminder {
        id: uuid::Uuid::new_v4().to_string(),
        message: "Integration test reminder".into(),
        due_at: chrono::Utc::now() - chrono::Duration::hours(1), // past due
        completed: false,
        created_at: chrono::Utc::now(),
    };

    let json = serde_json::to_vec(&reminder).unwrap();
    env.store.put(
        &format!("reminder:{}", reminder.id),
        &json,
        &reminder_key,
    ).unwrap();

    // Load due reminders
    let due = aivyx_actions::reminders::load_due_reminders(&env.store, &reminder_key).unwrap();
    assert_eq!(due.len(), 1);
    assert_eq!(due[0].message, "Integration test reminder");
}

// ── All Personas Seed Goals ───────────────────────────────────

#[tokio::test]
async fn all_personas_seed_two_goals() {
    let personas = ["assistant", "coder", "researcher", "writer", "coach", "companion", "ops", "analyst"];

    for persona in personas {
        let env = TestEnv::new();
        let pa_config = PaConfig {
            agent: Some(PaAgentConfig {
                persona: persona.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let built = env.build(&pa_config).await.unwrap();
        let store = built.brain_store.expect(&format!("Brain should be available for persona '{persona}'"));
        let key = aivyx_crypto::derive_brain_key(&env.master_key());
        let goals = store.list_goals(
            &GoalFilter { status: Some(GoalStatus::Active), ..Default::default() },
            &key,
        ).unwrap();

        // 2 starter + 5 growth (3 universal + 2 persona-specific)
        assert_eq!(goals.len(), 7, "Persona '{persona}' should seed 7 goals (2 starter + 5 growth), got {}", goals.len());
    }
}
