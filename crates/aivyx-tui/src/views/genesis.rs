#![allow(clippy::if_same_then_else)]

//! Native Genesis Wizard — 8-step TUI agent creation and onboarding.
//!
//! Runs its own event loop in the alternate screen before the main TUI
//! app launches. Collects all configuration, writes config.toml and
//! encrypted secrets, then returns the passphrase for seamless unlock.
//!
//! Steps:
//! 1. Passphrase — encryption key setup
//! 2. Provider   — LLM selection (Ollama auto-discovery, cloud API keys)
//! 3. Identity   — agent name + persona (8 options with preview)
//! 4. Soul+Skills — personality narrative + skill customization
//! 5. Schedule   — briefing hour, check interval, initial goals
//! 6. Email      — optional IMAP/SMTP with smart defaults
//! 7. Integrations — Calendar, Contacts, Telegram, Matrix, Signal
//! 8. Ignition   — summary review + heartbeat toggles + confirm

use std::io::Stdout;
use std::time::Duration;

use aivyx_config::AivyxDirs;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_pa::persona_defaults;
use aivyx_pa::persona_defaults::{GoalTemplate, ScheduleTemplate};
use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::prelude::*;
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};
use zeroize::Zeroizing;

use crate::theme;

// ── Step Machine ──────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Passphrase = 0,
    Provider = 1,
    Identity = 2,
    SoulSkills = 3,
    Schedule = 4,
    Email = 5,
    Integrations = 6,
    Ignition = 7,
}

impl Step {
    fn label(self) -> &'static str {
        match self {
            Step::Passphrase => "SEC",
            Step::Provider => "LLM",
            Step::Identity => "IDN",
            Step::SoulSkills => "SOL",
            Step::Schedule => "SCH",
            Step::Email => "EML",
            Step::Integrations => "INT",
            Step::Ignition => "IGN",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Step::Passphrase => "// DIRECTIVE: ESTABLISH ENCRYPTION",
            Step::Provider => "// DIRECTIVE: CHOOSE COMPUTE HOST",
            Step::Identity => "// DIRECTIVE: AGENT DESIGNATION",
            Step::SoulSkills => "// DIRECTIVE: LOAD NEURAL WEIGHTS",
            Step::Schedule => "// DIRECTIVE: INJECT GOAL STATE",
            Step::Email => "// DIRECTIVE: BIND MAIL PROTOCOLS",
            Step::Integrations => "// DIRECTIVE: ATTACH SYNC PIPELINES",
            Step::Ignition => "// DIRECTIVE: INITIATE HEARTBEAT",
        }
    }

    fn next(self) -> Option<Step> {
        match self {
            Step::Passphrase => Some(Step::Provider),
            Step::Provider => Some(Step::Identity),
            Step::Identity => Some(Step::SoulSkills),
            Step::SoulSkills => Some(Step::Schedule),
            Step::Schedule => Some(Step::Email),
            Step::Email => Some(Step::Integrations),
            Step::Integrations => Some(Step::Ignition),
            Step::Ignition => None,
        }
    }

    fn prev(self) -> Option<Step> {
        match self {
            Step::Passphrase => None,
            Step::Provider => Some(Step::Passphrase),
            Step::Identity => Some(Step::Provider),
            Step::SoulSkills => Some(Step::Identity),
            Step::Schedule => Some(Step::SoulSkills),
            Step::Email => Some(Step::Schedule),
            Step::Integrations => Some(Step::Email),
            Step::Ignition => Some(Step::Integrations),
        }
    }
}

// ── Constants ─────────────────────────────────────────────────────

/// Base URL for the OpenRouter "OpenAI-compatible" endpoint.
///
/// Hoisted to a constant so the URL is discoverable in one place and
/// doesn't get copy-pasted across the config writer.
const OPENROUTER_BASE_URL: &str = "https://openrouter.ai/api";

const PROVIDERS: &[(&str, &str, &[&str])] = &[
    (
        "Ollama",
        "Ollama",
        &[
            "qwen3:14b",
            "qwen2.5-coder:14b",
            "llama3.1:8b",
            "mistral:7b",
        ],
    ),
    ("OpenAI", "OpenAI", &["gpt-4o", "gpt-4o-mini", "o1-mini"]),
    (
        "Claude",
        "Claude",
        &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"],
    ),
    (
        "OpenRouter",
        "OpenAICompatible",
        &[
            "anthropic/claude-sonnet-4-20250514",
            "openai/gpt-4o",
            "google/gemini-2.0-flash-001",
        ],
    ),
];

const PERSONAS: &[(&str, &str)] = &[
    ("assistant", "Friendly and proactive general assistant"),
    ("coder", "Technical and precise — for developers"),
    ("researcher", "Methodical and thorough — deep analysis"),
    ("writer", "Clear and engaging — content creation"),
    ("coach", "Motivational and goal-oriented"),
    ("companion", "Warm and conversational"),
    ("ops", "Reliable and security-conscious — sysadmin"),
    ("analyst", "Data-driven and structured"),
];

const HEARTBEAT_LABELS: &[&str] = &[
    "Self-reflection (update self-model)",
    "Memory consolidation",
    "Failure analysis (learn from errors)",
    "Knowledge extraction (store facts)",
    "Plan review (organize goals)",
    "Strategy review (weekly analysis)",
    "Mood tracking (detect stress)",
    "Encouragement (celebrate wins)",
    "Milestone tracking (anniversaries)",
    "Notification pacing (throttle)",
];

const INTEGRATION_LABELS: &[&str] = &[
    "Calendar (CalDAV)",
    "Contacts (CardDAV)",
    "Telegram",
    "Matrix",
    "Signal",
    "SMS (Twilio/Vonage)",
    "Document Vault",
    "Finance Tracking",
    "Dev Tools (git + forge)",
    "Desktop Interaction (AT-SPI2)",
];

// ── Wizard State ──────────────────────────────────────────────────

struct GenesisState {
    step: Step,
    frame_count: u64,

    // Step 1: Passphrase
    passphrase: Zeroizing<String>,
    confirm: Zeroizing<String>,

    // Step 2: Provider
    provider_idx: usize,
    model_input: String,
    model_list_idx: usize,
    api_key: Zeroizing<String>,
    ollama_url: String,
    ollama_models: Vec<String>,
    ollama_probed: bool,

    // Step 3: Identity
    agent_name: String,
    persona_idx: usize,

    // Step 4: Soul + Skills
    soul_accepted: bool,
    skills_accepted: bool,
    custom_skills_input: String,

    // Step 5: Schedule + Goals
    briefing_hour: String,
    check_interval: String,
    include_goals: bool,

    // Step 6: Email
    setup_email: bool,
    email_address: String,
    imap_host: String,
    imap_port: String,
    smtp_host: String,
    smtp_port: String,
    email_username: String,
    email_password: Zeroizing<String>,

    // Step 7: Integrations (10 toggles)
    integration_flags: [bool; 10],
    // Calendar (0)
    calendar_url: String,
    calendar_user: String,
    calendar_pass: Zeroizing<String>,
    // Contacts (1)
    contacts_url: String,
    contacts_user: String,
    contacts_pass: Zeroizing<String>,
    // Telegram (2)
    telegram_token: Zeroizing<String>,
    telegram_chat_id: String,
    // Matrix (3)
    matrix_homeserver: String,
    matrix_token: Zeroizing<String>,
    matrix_room_id: String,
    // Signal (4)
    signal_account: String,
    signal_socket: String,
    // SMS (5)
    sms_provider: String, // "twilio" or "vonage"
    sms_account_id: String,
    sms_auth_token: Zeroizing<String>,
    sms_from_number: String,
    // Vault (6)
    vault_path: String,
    // Finance (7)
    finance_receipt_folder: String,
    // DevTools (8)
    devtools_repo_path: String,
    devtools_forge: String, // "" = none, "github", "gitea"
    devtools_forge_api_url: String,
    devtools_repo_slug: String, // "owner/name"
    devtools_forge_token: Zeroizing<String>,
    // Desktop (9)
    desktop_deep_interaction: bool,

    // Step 8: Ignition
    heartbeat_flags: [bool; 10],

    // UI state
    focused_field: usize,
    error_message: Option<String>,
    needs_probe: bool,
}

impl GenesisState {
    fn new() -> Self {
        Self {
            step: Step::Passphrase,
            frame_count: 0,
            passphrase: Zeroizing::new(String::new()),
            confirm: Zeroizing::new(String::new()),
            provider_idx: 0,
            model_input: PROVIDERS[0].2[0].to_string(),
            model_list_idx: 0,
            api_key: Zeroizing::new(String::new()),
            ollama_url: "http://localhost:11434".into(),
            ollama_models: Vec::new(),
            ollama_probed: false,
            agent_name: String::new(),
            persona_idx: 0,
            soul_accepted: true,
            skills_accepted: true,
            custom_skills_input: String::new(),
            briefing_hour: "8".into(),
            check_interval: "15".into(),
            include_goals: true,
            setup_email: false,
            email_address: String::new(),
            imap_host: String::new(),
            imap_port: "993".into(),
            smtp_host: String::new(),
            smtp_port: "587".into(),
            email_username: String::new(),
            email_password: Zeroizing::new(String::new()),
            integration_flags: [false; 10],
            calendar_url: String::new(),
            calendar_user: String::new(),
            calendar_pass: Zeroizing::new(String::new()),
            contacts_url: String::new(),
            contacts_user: String::new(),
            contacts_pass: Zeroizing::new(String::new()),
            telegram_token: Zeroizing::new(String::new()),
            telegram_chat_id: String::new(),
            matrix_homeserver: String::new(),
            matrix_token: Zeroizing::new(String::new()),
            matrix_room_id: String::new(),
            signal_account: String::new(),
            signal_socket: "/var/run/signal-cli/socket".into(),
            sms_provider: "twilio".into(),
            sms_account_id: String::new(),
            sms_auth_token: Zeroizing::new(String::new()),
            sms_from_number: String::new(),
            vault_path: "~/Documents/vault".into(),
            finance_receipt_folder: "receipts".into(),
            devtools_repo_path: String::new(),
            devtools_forge: String::new(),
            devtools_forge_api_url: String::new(),
            devtools_repo_slug: String::new(),
            devtools_forge_token: Zeroizing::new(String::new()),
            desktop_deep_interaction: false,
            heartbeat_flags: [false; 10],
            focused_field: 0,
            error_message: None,
            needs_probe: false,
        }
    }

    /// Load persona defaults into heartbeat flags when persona changes.
    fn sync_heartbeat_from_persona(&mut self) {
        let bundle = persona_defaults::for_persona(self.current_persona());
        let hb = &bundle.heartbeat;
        self.heartbeat_flags = [
            hb.can_reflect,
            hb.can_consolidate_memory,
            hb.can_analyze_failures,
            hb.can_extract_knowledge,
            hb.can_plan_review,
            hb.can_strategy_review,
            hb.can_track_mood,
            hb.can_encourage,
            hb.can_track_milestones,
            hb.notification_pacing,
        ];
    }

    fn current_persona(&self) -> &str {
        PERSONAS[self.persona_idx].0
    }

    fn current_provider_label(&self) -> &str {
        PROVIDERS[self.provider_idx].0
    }

    fn current_provider_type(&self) -> &str {
        PROVIDERS[self.provider_idx].1
    }

    fn is_ollama(&self) -> bool {
        self.provider_idx == 0
    }

    fn needs_api_key(&self) -> bool {
        !self.is_ollama()
    }

    fn effective_model_list(&self) -> Vec<&str> {
        if !self.ollama_models.is_empty() {
            self.ollama_models.iter().map(|s| s.as_str()).collect()
        } else {
            PROVIDERS[self.provider_idx].2.to_vec()
        }
    }

    fn effective_agent_name(&self) -> &str {
        if self.agent_name.is_empty() {
            "assistant"
        } else {
            &self.agent_name
        }
    }

    /// Blinking cursor character (500ms on/off).
    fn cursor(&self) -> &str {
        if self.frame_count % 60 < 30 { "_" } else { " " }
    }
}

// ── Public Entry Point ────────────────────────────────────────────

pub async fn run_wizard(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    dirs: &AivyxDirs,
) -> anyhow::Result<Option<Zeroizing<String>>> {
    let mut state = GenesisState::new();

    loop {
        state.frame_count = state.frame_count.wrapping_add(1);
        terminal.draw(|f| render(&state, f))?;

        // Run deferred Ollama probe (triggered by provider switch)
        if state.needs_probe {
            state.needs_probe = false;
            probe_ollama(&mut state).await;
        }

        if event::poll(Duration::from_millis(16))?
            && let Event::Key(key) = event::read()?
        {
            // Ctrl+C always aborts
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
                return Ok(None);
            }

            // Clear error on any keystroke
            state.error_message = None;

            match handle_key(&mut state, key) {
                Action::Continue => {}
                Action::Advance => {
                    if let Err(msg) = validate_step(&state) {
                        state.error_message = Some(msg);
                    } else if let Some(next) = state.step.next() {
                        // Side effects on step transition
                        on_leave_step(&mut state);
                        state.step = next;
                        state.focused_field = 0;
                        on_enter_step(&mut state).await;
                    } else {
                        // Ignition — finalize
                        finalize(&state, dirs)?;
                        return Ok(Some(state.passphrase.clone()));
                    }
                }
                Action::Back => {
                    if let Some(prev) = state.step.prev() {
                        state.step = prev;
                        state.focused_field = 0;
                        on_enter_step(&mut state).await;
                    } else {
                        return Ok(None); // Esc on first step = abort
                    }
                }
            }
        }
    }
}

enum Action {
    Continue,
    Advance,
    Back,
}

/// Side effects when leaving a step (before advancing).
fn on_leave_step(state: &mut GenesisState) {
    match state.step {
        Step::Identity => {
            // Sync heartbeat defaults when persona is chosen
            state.sync_heartbeat_from_persona();
        }
        Step::Provider => {
            // Sync model_input from list selection if user didn't type
            let list = state.effective_model_list();
            if state.model_list_idx < list.len() && state.focused_field == 0 {
                state.model_input = list[state.model_list_idx].to_string();
            }
        }
        _ => {}
    }
}

/// Side effects when entering a step.
async fn on_enter_step(state: &mut GenesisState) {
    if state.step == Step::Provider {
        // Schedule Ollama probe if not yet probed
        if state.is_ollama() && !state.ollama_probed {
            state.needs_probe = true;
        }
    }
}

/// Probe Ollama for installed models.
async fn probe_ollama(state: &mut GenesisState) {
    state.ollama_probed = true;
    let url = format!("{}/api/tags", state.ollama_url);
    let client = reqwest::Client::new();
    match client
        .get(&url)
        .timeout(Duration::from_secs(3))
        .send()
        .await
    {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(json) = resp.json::<serde_json::Value>().await
                && let Some(models) = json["models"].as_array()
            {
                state.ollama_models = models
                    .iter()
                    .filter_map(|m| m["name"].as_str().map(String::from))
                    .collect();
            }
            if !state.ollama_models.is_empty() {
                state.model_input = state.ollama_models[0].clone();
                state.model_list_idx = 0;
            }
        }
        _ => {}
    }
}

// ── Validation ────────────────────────────────────────────────────

fn validate_step(state: &GenesisState) -> Result<(), String> {
    match state.step {
        Step::Passphrase => {
            if state.passphrase.len() < 8 {
                return Err("Passphrase must be at least 8 characters.".into());
            }
            if *state.passphrase != *state.confirm {
                return Err("Passphrases do not match.".into());
            }
        }
        Step::Provider => {
            if state.model_input.is_empty() {
                return Err("Model name is required.".into());
            }
            if state.needs_api_key() && state.api_key.is_empty() {
                return Err("API key is required for cloud providers.".into());
            }
        }
        Step::Identity => {
            // agent_name can be empty (defaults to "assistant")
        }
        Step::Email => {
            if state.setup_email && state.email_address.is_empty() {
                return Err("Email address is required.".into());
            }
        }
        _ => {}
    }
    Ok(())
}

// ── Key Handling ──────────────────────────────────────────────────

fn handle_key(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    match state.step {
        Step::Passphrase => handle_passphrase(state, key),
        Step::Provider => handle_provider(state, key),
        Step::Identity => handle_identity(state, key),
        Step::SoulSkills => handle_soul_skills(state, key),
        Step::Schedule => handle_schedule(state, key),
        Step::Email => handle_email(state, key),
        Step::Integrations => handle_integrations(state, key),
        Step::Ignition => handle_ignition(state, key),
    }
}

fn handle_passphrase(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    match key.code {
        KeyCode::Tab => {
            state.focused_field = 1 - state.focused_field;
        }
        KeyCode::Enter => {
            if state.focused_field == 0 {
                state.focused_field = 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        KeyCode::Backspace => {
            if state.focused_field == 0 {
                state.passphrase.pop();
            } else {
                state.confirm.pop();
            }
        }
        KeyCode::Char(c) => {
            if state.focused_field == 0 {
                state.passphrase.push(c);
            } else {
                state.confirm.push(c);
            }
        }
        _ => {}
    }
    Action::Continue
}

fn handle_provider(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    // Fields: 0 = provider list, 1 = model input, 2 = API key (if cloud)
    let max_field = if state.needs_api_key() { 2 } else { 1 };
    match key.code {
        KeyCode::Tab => {
            state.focused_field = (state.focused_field + 1) % (max_field + 1);
        }
        KeyCode::Up => {
            if state.focused_field == 0 && state.provider_idx > 0 {
                state.provider_idx -= 1;
                state.model_input = PROVIDERS[state.provider_idx].2[0].to_string();
                state.model_list_idx = 0;
                state.ollama_models.clear();
                state.ollama_probed = false;
                if state.is_ollama() {
                    state.needs_probe = true;
                }
            } else if state.focused_field == 1 && state.model_list_idx > 0 {
                state.model_list_idx -= 1;
                let list = state.effective_model_list();
                state.model_input = list[state.model_list_idx].to_string();
            }
        }
        KeyCode::Down => {
            if state.focused_field == 0 && state.provider_idx < PROVIDERS.len() - 1 {
                state.provider_idx += 1;
                state.model_input = PROVIDERS[state.provider_idx].2[0].to_string();
                state.model_list_idx = 0;
                state.ollama_models.clear();
                state.ollama_probed = false;
                if state.is_ollama() {
                    state.needs_probe = true;
                }
            } else if state.focused_field == 1 {
                let list_len = state.effective_model_list().len();
                if state.model_list_idx + 1 < list_len {
                    state.model_list_idx += 1;
                    let list = state.effective_model_list();
                    state.model_input = list[state.model_list_idx].to_string();
                }
            }
        }
        KeyCode::Enter => {
            if state.focused_field < max_field {
                state.focused_field += 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        KeyCode::Backspace => {
            if state.focused_field == 1 {
                state.model_input.pop();
            } else if state.focused_field == 2 {
                state.api_key.pop();
            }
        }
        KeyCode::Char(c) => {
            if state.focused_field == 1 {
                state.model_input.push(c);
            } else if state.focused_field == 2 {
                state.api_key.push(c);
            }
        }
        _ => {}
    }
    Action::Continue
}

fn handle_identity(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    // Fields: 0 = agent name, 1 = persona list
    match key.code {
        KeyCode::Tab => {
            state.focused_field = 1 - state.focused_field;
        }
        KeyCode::Up if state.focused_field == 1 => {
            if state.persona_idx > 0 {
                state.persona_idx -= 1;
            }
        }
        KeyCode::Down if state.focused_field == 1 => {
            if state.persona_idx < PERSONAS.len() - 1 {
                state.persona_idx += 1;
            }
        }
        KeyCode::Enter => {
            if state.focused_field == 0 {
                state.focused_field = 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        KeyCode::Backspace if state.focused_field == 0 => {
            state.agent_name.pop();
        }
        KeyCode::Char(c) if state.focused_field == 0 => state.agent_name.push(c),
        _ => {}
    }
    Action::Continue
}

fn handle_soul_skills(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    // Fields: 0 = soul accept toggle, 1 = skills accept toggle, 2 = custom skills input
    match key.code {
        KeyCode::Tab => {
            state.focused_field = (state.focused_field + 1) % 3;
        }
        KeyCode::Char(' ') if state.focused_field == 0 => {
            state.soul_accepted = !state.soul_accepted;
        }
        KeyCode::Char(' ') if state.focused_field == 1 => {
            state.skills_accepted = !state.skills_accepted;
        }
        KeyCode::Backspace if state.focused_field == 2 => {
            state.custom_skills_input.pop();
        }
        KeyCode::Char(c) if state.focused_field == 2 => state.custom_skills_input.push(c),
        KeyCode::Enter => {
            if state.focused_field < 2 {
                state.focused_field += 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        _ => {}
    }
    Action::Continue
}

fn handle_schedule(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    // Fields: 0 = briefing hour, 1 = check interval, 2 = goals toggle
    match key.code {
        KeyCode::Tab => {
            state.focused_field = (state.focused_field + 1) % 3;
        }
        KeyCode::Char(' ') if state.focused_field == 2 => {
            state.include_goals = !state.include_goals;
        }
        KeyCode::Backspace => {
            if state.focused_field == 0 {
                state.briefing_hour.pop();
            } else if state.focused_field == 1 {
                state.check_interval.pop();
            }
        }
        KeyCode::Char(c) if c.is_ascii_digit() => {
            // Eagerly reject out-of-range keystrokes so the user sees the
            // constraint immediately rather than silently getting their
            // value clamped at config-write time.
            if state.focused_field == 0 {
                // Briefing hour: 0..=23 (max 2 digits).
                let mut candidate = state.briefing_hour.clone();
                candidate.push(c);
                if candidate.len() <= 2
                    && let Ok(v) = candidate.parse::<u8>()
                    && v <= 23
                {
                    state.briefing_hour = candidate;
                }
            } else if state.focused_field == 1 {
                // Poll interval: 1..=10_080 minutes (one week cap — anything
                // larger would make the loop effectively off and is almost
                // certainly a typo). Max 5 digits.
                let mut candidate = state.check_interval.clone();
                candidate.push(c);
                if candidate.len() <= 5
                    && let Ok(v) = candidate.parse::<u32>()
                    && v <= 10_080
                {
                    state.check_interval = candidate;
                }
            }
        }
        KeyCode::Enter => {
            if state.focused_field < 2 {
                state.focused_field += 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        _ => {}
    }
    Action::Continue
}

fn handle_email(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    if !state.setup_email {
        // Only toggle and advance/back
        match key.code {
            KeyCode::Char(' ') | KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.setup_email = true;
                state.focused_field = 0;
            }
            KeyCode::Enter => return Action::Advance,
            KeyCode::Esc => return Action::Back,
            _ => {}
        }
        return Action::Continue;
    }

    // Email fields: 0=address, 1=imap_host, 2=imap_port, 3=smtp_host, 4=smtp_port, 5=username, 6=password
    let field_count = 7;
    match key.code {
        KeyCode::Tab => {
            state.focused_field = (state.focused_field + 1) % field_count;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.setup_email = false;
        }
        KeyCode::Enter => {
            if state.focused_field == 0 && !state.email_address.is_empty() {
                // Auto-fill defaults from domain
                if state.imap_host.is_empty() {
                    state.imap_host = guess_imap_host(&state.email_address);
                }
                if state.smtp_host.is_empty() {
                    state.smtp_host = guess_smtp_host(&state.email_address);
                }
                if state.email_username.is_empty() {
                    state.email_username = state.email_address.clone();
                }
                state.focused_field = 1;
            } else if state.focused_field < field_count - 1 {
                state.focused_field += 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        KeyCode::Backspace => match state.focused_field {
            0 => {
                state.email_address.pop();
            }
            1 => {
                state.imap_host.pop();
            }
            2 => {
                state.imap_port.pop();
            }
            3 => {
                state.smtp_host.pop();
            }
            4 => {
                state.smtp_port.pop();
            }
            5 => {
                state.email_username.pop();
            }
            6 => {
                state.email_password.pop();
            }
            _ => {}
        },
        KeyCode::Char(c) => match state.focused_field {
            0 => state.email_address.push(c),
            1 => state.imap_host.push(c),
            2 if c.is_ascii_digit() => state.imap_port.push(c),
            3 => state.smtp_host.push(c),
            4 if c.is_ascii_digit() => state.smtp_port.push(c),
            5 => state.email_username.push(c),
            6 => state.email_password.push(c),
            _ => {}
        },
        _ => {}
    }
    Action::Continue
}

fn handle_integrations(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    let max = integration_field_count(state);
    match key.code {
        KeyCode::Up => {
            if state.focused_field > 0 {
                state.focused_field -= 1;
            }
        }
        KeyCode::Down => {
            state.focused_field += 1;
            if state.focused_field >= max {
                state.focused_field = max.saturating_sub(1);
            }
        }
        KeyCode::Char(' ') => match integration_focus(state) {
            IntegrationFocus::Toggle(i) => {
                state.integration_flags[i] = !state.integration_flags[i];
            }
            IntegrationFocus::SubField("desktop", 0) => {
                state.desktop_deep_interaction = !state.desktop_deep_interaction;
            }
            _ => {}
        },
        KeyCode::Enter => {
            if state.focused_field + 1 < max {
                state.focused_field += 1;
            } else {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        KeyCode::Backspace => {
            integration_backspace(state);
        }
        KeyCode::Char(c) => {
            integration_char(state, c);
        }
        _ => {}
    }
    Action::Continue
}

/// Sub-field counts for each integration when enabled.
const INTEGRATION_SUB_COUNTS: &[(&str, usize)] = &[
    ("calendar", 3), // url, user, pass
    ("contacts", 3), // url, user, pass
    ("telegram", 2), // token, chat_id
    ("matrix", 3),   // homeserver, token, room_id
    ("signal", 2),   // account, socket
    ("sms", 4),      // provider, account_id, auth_token, from_number
    ("vault", 1),    // path
    ("finance", 1),  // receipt_folder
    ("devtools", 5), // repo_path, forge, forge_api_url, repo_slug, forge_token
    ("desktop", 1),  // deep_interaction toggle
];

fn integration_field_count(state: &GenesisState) -> usize {
    let mut count = 10; // base toggles (one per integration)
    for (i, &(_, sub_count)) in INTEGRATION_SUB_COUNTS.iter().enumerate() {
        if state.integration_flags[i] {
            count += sub_count;
        }
    }
    count
}

/// What kind of field is at `focused_field`?
/// Returns `Toggle(integration_index)` or `SubField(group_name, sub_index)`.
enum IntegrationFocus {
    Toggle(usize),
    SubField(&'static str, usize),
    None,
}

fn integration_focus(state: &GenesisState) -> IntegrationFocus {
    let mut idx = state.focused_field;
    for (i, &(name, sub_count)) in INTEGRATION_SUB_COUNTS.iter().enumerate() {
        // This position is the toggle row for integration i
        if idx == 0 {
            return IntegrationFocus::Toggle(i);
        }
        idx -= 1;
        // If this integration is enabled, its sub-fields follow the toggle
        if state.integration_flags[i] {
            if idx < sub_count {
                return IntegrationFocus::SubField(name, idx);
            }
            idx -= sub_count;
        }
    }
    IntegrationFocus::None
}

/// Legacy helper used by backspace/char handlers — returns (group, sub_index).
fn integration_sub_field(state: &GenesisState) -> Option<(&'static str, usize)> {
    match integration_focus(state) {
        IntegrationFocus::SubField(name, idx) => Some((name, idx)),
        _ => None,
    }
}

fn integration_backspace(state: &mut GenesisState) {
    if let Some((group, sub)) = integration_sub_field(state) {
        match (group, sub) {
            ("calendar", 0) => {
                state.calendar_url.pop();
            }
            ("calendar", 1) => {
                state.calendar_user.pop();
            }
            ("calendar", 2) => {
                state.calendar_pass.pop();
            }
            ("contacts", 0) => {
                state.contacts_url.pop();
            }
            ("contacts", 1) => {
                state.contacts_user.pop();
            }
            ("contacts", 2) => {
                state.contacts_pass.pop();
            }
            ("telegram", 0) => {
                state.telegram_token.pop();
            }
            ("telegram", 1) => {
                state.telegram_chat_id.pop();
            }
            ("matrix", 0) => {
                state.matrix_homeserver.pop();
            }
            ("matrix", 1) => {
                state.matrix_token.pop();
            }
            ("matrix", 2) => {
                state.matrix_room_id.pop();
            }
            ("signal", 0) => {
                state.signal_account.pop();
            }
            ("signal", 1) => {
                state.signal_socket.pop();
            }
            ("sms", 0) => {
                state.sms_provider.pop();
            }
            ("sms", 1) => {
                state.sms_account_id.pop();
            }
            ("sms", 2) => {
                state.sms_auth_token.pop();
            }
            ("sms", 3) => {
                state.sms_from_number.pop();
            }
            ("vault", 0) => {
                state.vault_path.pop();
            }
            ("finance", 0) => {
                state.finance_receipt_folder.pop();
            }
            ("devtools", 0) => {
                state.devtools_repo_path.pop();
            }
            ("devtools", 1) => {
                state.devtools_forge.pop();
            }
            ("devtools", 2) => {
                state.devtools_forge_api_url.pop();
            }
            ("devtools", 3) => {
                state.devtools_repo_slug.pop();
            }
            ("devtools", 4) => {
                state.devtools_forge_token.pop();
            }
            // desktop (0) is a toggle, handled via Space in handle_integrations
            _ => {}
        }
    }
}

fn integration_char(state: &mut GenesisState, c: char) {
    if let Some((group, sub)) = integration_sub_field(state) {
        match (group, sub) {
            ("calendar", 0) => state.calendar_url.push(c),
            ("calendar", 1) => state.calendar_user.push(c),
            ("calendar", 2) => state.calendar_pass.push(c),
            ("contacts", 0) => state.contacts_url.push(c),
            ("contacts", 1) => state.contacts_user.push(c),
            ("contacts", 2) => state.contacts_pass.push(c),
            ("telegram", 0) => state.telegram_token.push(c),
            ("telegram", 1) => state.telegram_chat_id.push(c),
            ("matrix", 0) => state.matrix_homeserver.push(c),
            ("matrix", 1) => state.matrix_token.push(c),
            ("matrix", 2) => state.matrix_room_id.push(c),
            ("signal", 0) => state.signal_account.push(c),
            ("signal", 1) => state.signal_socket.push(c),
            ("sms", 0) => state.sms_provider.push(c),
            ("sms", 1) => state.sms_account_id.push(c),
            ("sms", 2) => state.sms_auth_token.push(c),
            ("sms", 3) => state.sms_from_number.push(c),
            ("vault", 0) => state.vault_path.push(c),
            ("finance", 0) => state.finance_receipt_folder.push(c),
            ("devtools", 0) => state.devtools_repo_path.push(c),
            ("devtools", 1) => state.devtools_forge.push(c),
            ("devtools", 2) => state.devtools_forge_api_url.push(c),
            ("devtools", 3) => state.devtools_repo_slug.push(c),
            ("devtools", 4) => state.devtools_forge_token.push(c),
            // desktop (0) is a toggle, not text input
            _ => {}
        }
    }
}

fn handle_ignition(state: &mut GenesisState, key: event::KeyEvent) -> Action {
    match key.code {
        KeyCode::Up => {
            if state.focused_field > 0 {
                state.focused_field -= 1;
            }
        }
        KeyCode::Down => {
            if state.focused_field < 10 {
                state.focused_field += 1;
            }
        }
        KeyCode::Char(' ') => {
            if state.focused_field < 10 {
                state.heartbeat_flags[state.focused_field] =
                    !state.heartbeat_flags[state.focused_field];
            }
        }
        KeyCode::Enter => {
            // Only finalize when on the Ignite button (field 10)
            if state.focused_field == 10 {
                return Action::Advance;
            }
        }
        KeyCode::Esc => return Action::Back,
        _ => {}
    }
    Action::Continue
}

// ── Rendering ─────────────────────────────────────────────────────

fn render(state: &GenesisState, f: &mut Frame) {
    let size = f.area();

    // Full-screen background
    f.render_widget(Block::default().style(Style::default().bg(theme::BG)), size);

    // Vertical layout: header(2) + progress(1) + gap(1) + panel(flex) + gap(1) + hints(1)
    let [
        header_area,
        progress_area,
        _gap1,
        panel_area,
        _gap2,
        hints_area,
    ] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(14),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(size);

    render_header(header_area, f);
    render_progress(state, progress_area, f);
    render_hints(state, hints_area, f);

    // Center the wizard panel horizontally (60 cols, or screen width if smaller)
    let panel_width = 62u16.min(size.width.saturating_sub(4));
    let panel_x = (size.width.saturating_sub(panel_width)) / 2;
    let panel_height = panel_area.height.min(24);
    let panel_y = panel_area.y + (panel_area.height.saturating_sub(panel_height)) / 2;

    let wizard_rect = Rect {
        x: panel_x,
        y: panel_y,
        width: panel_width,
        height: panel_height,
    };

    // Panel border with step title
    let step_num = state.step as usize + 1;
    let title = format!(" Step {step_num}/8: {} ", state.step.title());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(theme::border())
        .title(Line::from(Span::styled(title, theme::primary_bold())));

    let inner = block.inner(wizard_rect);
    f.render_widget(Clear, wizard_rect);
    f.render_widget(block, wizard_rect);

    // Error message (render above panel if present)
    if let Some(ref err) = state.error_message {
        let err_y = wizard_rect.y.saturating_sub(1);
        let err_line = Line::from(Span::styled(
            format!("  [!!!] CRITICAL: {err}"),
            Style::default()
                .fg(theme::ERROR)
                .add_modifier(Modifier::BOLD),
        ));
        f.render_widget(
            Paragraph::new(err_line),
            Rect {
                x: wizard_rect.x,
                y: err_y,
                width: wizard_rect.width,
                height: 1,
            },
        );
    }

    // Dispatch to step renderer
    match state.step {
        Step::Passphrase => render_passphrase(state, inner, f),
        Step::Provider => render_provider(state, inner, f),
        Step::Identity => render_identity(state, inner, f),
        Step::SoulSkills => render_soul_skills(state, inner, f),
        Step::Schedule => render_schedule(state, inner, f),
        Step::Email => render_email(state, inner, f),
        Step::Integrations => render_integrations(state, inner, f),
        Step::Ignition => render_ignition(state, inner, f),
    }
}

fn render_header(area: Rect, f: &mut Frame) {
    let line = Line::from(vec![
        Span::styled(
            "  AIVYX.STUDIO ",
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("// GENESIS PROTOCOL v0.1.0", theme::secondary()),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn render_progress(state: &GenesisState, area: Rect, f: &mut Frame) {
    let current = state.step as usize;
    let labels = [
        Step::Passphrase,
        Step::Provider,
        Step::Identity,
        Step::SoulSkills,
        Step::Schedule,
        Step::Email,
        Step::Integrations,
        Step::Ignition,
    ];

    let mut spans = vec![Span::styled(" ", theme::dim())];
    for (i, step) in labels.iter().enumerate() {
        let active = i == current;
        let past = i < current;

        let style = if active {
            Style::default()
                .fg(theme::PRIMARY)
                .add_modifier(Modifier::BOLD)
        } else if past {
            theme::sage()
        } else {
            theme::dim()
        };

        let num_str = format!("{:02}", i + 1);
        let fmt = if active {
            format!("[{}_{}]", num_str, step.label())
        } else {
            format!(" {}_{} ", num_str, step.label())
        };

        spans.push(Span::styled(fmt, style));

        if i < labels.len() - 1 {
            let sep_style = if past { theme::sage() } else { theme::muted() };
            spans.push(Span::styled(" - ", sep_style));
        }
    }
    f.render_widget(
        Paragraph::new(Line::from(spans).alignment(Alignment::Center)),
        area,
    );
}

fn render_hints(state: &GenesisState, area: Rect, f: &mut Frame) {
    let back = if state.step.prev().is_some() {
        "Esc:Back  "
    } else {
        "Esc:Quit  "
    };
    let next = if state.step == Step::Ignition {
        if state.focused_field == 10 {
            "Enter:Ignite"
        } else {
            "Space:Toggle  ↓:Ignite button"
        }
    } else {
        "Enter:Next"
    };
    let extra = if state.step == Step::Ignition {
        ""
    } else {
        "Tab:Field  "
    };
    let line = Line::from(vec![
        Span::styled("  ", theme::dim()),
        Span::styled(next, theme::primary()),
        Span::styled("  ", theme::dim()),
        Span::styled(back, theme::muted()),
        Span::styled(extra, theme::muted()),
        Span::styled("Ctrl+C:Quit", theme::muted()),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

// ── Step Renderers ────────────────────────────────────────────────

fn render_passphrase(state: &GenesisState, area: Rect, f: &mut Frame) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "ESTABLISH ENCRYPTION GATEWAY",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "All local state is secured via ChaCha20-Poly1305 algorithms.",
            theme::dim(),
        )),
        Line::from(Span::styled(
            "Input a passphrase containing a minimum of 8 cryptographic characters.",
            theme::dim(),
        )),
        Line::from(""),
    ];

    // Passphrase field
    let p1_style = if state.focused_field == 0 {
        theme::text()
    } else {
        theme::muted()
    };
    let p1 = "*".repeat(state.passphrase.len());
    lines.push(Line::from(Span::styled(
        "  MASTER_PASSPHRASE_KEY:",
        theme::muted(),
    )));
    lines.push(Line::from(vec![
        Span::styled(
            if state.focused_field == 0 {
                "  ► [ "
            } else {
                "    [ "
            },
            theme::primary_bold(),
        ),
        Span::styled(p1, p1_style),
        Span::styled(
            if state.focused_field == 0 {
                state.cursor()
            } else {
                " "
            },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    lines.push(Line::from(""));

    // Confirm field
    let p2_style = if state.focused_field == 1 {
        theme::text()
    } else {
        theme::muted()
    };
    let p2 = "*".repeat(state.confirm.len());
    lines.push(Line::from(Span::styled("  CONFIRM_KEY:", theme::muted())));
    lines.push(Line::from(vec![
        Span::styled(
            if state.focused_field == 1 {
                "  ► [ "
            } else {
                "    [ "
            },
            theme::primary_bold(),
        ),
        Span::styled(p2, p2_style),
        Span::styled(
            if state.focused_field == 1 {
                state.cursor()
            } else {
                " "
            },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    // Match indicator
    if state.focused_field == 1 && !state.confirm.is_empty() {
        lines.push(Line::from(""));
        if *state.passphrase != *state.confirm {
            lines.push(Line::from(Span::styled(
                "    [!!!] HASH MISMATCH DETECTED.",
                theme::error(),
            )));
        } else if state.passphrase.len() >= 8 {
            lines.push(Line::from(Span::styled(
                "    [OK] SIGNATURES ALIGNED.",
                theme::sage(),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_provider(state: &GenesisState, area: Rect, f: &mut Frame) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "CHOOSE COMPUTE HOST",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Provider list
    for (i, (name, _, _)) in PROVIDERS.iter().enumerate() {
        let selected = i == state.provider_idx;
        let marker = if selected && state.focused_field == 0 {
            "▌ "
        } else {
            "  "
        };
        let style = if selected {
            theme::highlight()
        } else {
            theme::muted()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {marker}"), style),
            Span::styled(name.to_uppercase(), style),
        ]));
    }

    lines.push(Line::from(""));

    // Model
    let model_focused = state.focused_field == 1;
    lines.push(Line::from(Span::styled(
        "  MODEL_IDENTIFIER:",
        theme::muted(),
    )));

    let model_list = state.effective_model_list();
    if !model_list.is_empty() {
        for (i, m) in model_list.iter().enumerate() {
            let sel = i == state.model_list_idx;
            let marker = if sel && model_focused {
                "▌ "
            } else if sel {
                "■ "
            } else {
                "  "
            };
            let style = if sel && model_focused {
                theme::highlight()
            } else if sel {
                theme::primary()
            } else {
                theme::dim()
            };
            lines.push(Line::from(Span::styled(format!("    {marker}{m}"), style)));
        }
    } else {
        // No list — free-form text input
        let m_style = if model_focused {
            theme::text()
        } else {
            theme::muted()
        };
        lines.push(Line::from(vec![
            Span::styled(
                if model_focused { "  [ " } else { "  [ " },
                theme::primary_bold(),
            ),
            Span::styled(&state.model_input, m_style),
            Span::styled(
                if model_focused { state.cursor() } else { " " },
                theme::primary(),
            ),
            Span::styled(" ]", theme::primary_bold()),
        ]));
    }

    // API key (cloud providers only)
    if state.needs_api_key() {
        lines.push(Line::from(""));
        let key_focused = state.focused_field == 2;
        let k_style = if key_focused {
            theme::text()
        } else {
            theme::muted()
        };
        let k_display = "*".repeat(state.api_key.len());
        lines.push(Line::from(Span::styled("  API_TOKEN:", theme::muted())));
        lines.push(Line::from(vec![
            Span::styled(
                if key_focused { "  [ " } else { "  [ " },
                theme::primary_bold(),
            ),
            Span::styled(k_display, k_style),
            Span::styled(
                if key_focused { state.cursor() } else { " " },
                theme::primary(),
            ),
            Span::styled(" ]", theme::primary_bold()),
        ]));
    }

    // Ollama status
    if state.is_ollama() {
        lines.push(Line::from(""));
        if state.needs_probe || !state.ollama_probed {
            lines.push(Line::from(Span::styled(
                "  [!] DISCOVERING LOCAL BINARIES...",
                theme::muted(),
            )));
        } else if state.ollama_models.is_empty() {
            lines.push(Line::from(Span::styled(
                "  [!] NO BINARIES DETECTED ON LOCAL HOST",
                theme::warning(),
            )));
            lines.push(Line::from(Span::styled(
                "  EXECUTE: ollama pull <model>",
                theme::dim(),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                format!("  [OK] {} BINARIES INDEXED", state.ollama_models.len()),
                theme::sage(),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_identity(state: &GenesisState, area: Rect, f: &mut Frame) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "AGENT DESIGNATION",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Agent name
    let name_focused = state.focused_field == 0;
    lines.push(Line::from(Span::styled(
        "  DESIGNATION_IDENTIFIER (default: assistant):",
        theme::muted(),
    )));
    let n_style = if name_focused {
        theme::text()
    } else {
        theme::muted()
    };
    lines.push(Line::from(vec![
        Span::styled(
            if name_focused { "  [ " } else { "  [ " },
            theme::primary_bold(),
        ),
        Span::styled(
            if state.agent_name.is_empty() && !name_focused {
                "assistant"
            } else {
                &state.agent_name
            },
            n_style,
        ),
        Span::styled(
            if name_focused { state.cursor() } else { " " },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  PERSONA_PRESET:",
        theme::muted(),
    )));

    // Persona list (compact: 1 line each)
    let persona_focused = state.focused_field == 1;
    for (i, (name, desc)) in PERSONAS.iter().enumerate() {
        let selected = i == state.persona_idx;
        let marker = if selected && persona_focused {
            "▌ "
        } else if selected {
            "■ "
        } else {
            "  "
        };
        let name_style = if selected {
            theme::highlight()
        } else {
            theme::muted()
        };
        let desc_style = if selected { theme::dim() } else { theme::dim() };
        lines.push(Line::from(vec![
            Span::styled(format!("  {marker}"), name_style),
            Span::styled(format!("{:<15}", name.to_uppercase()), name_style),
            Span::styled(*desc, desc_style),
        ]));
    }

    // Persona preview
    if persona_focused {
        let bundle = persona_defaults::for_persona(PERSONAS[state.persona_idx].0);
        lines.push(Line::from(""));
        let skills_str = bundle.skills.join(", ");
        let sched_count = bundle.schedules.len();
        let hb_count = [
            bundle.heartbeat.can_reflect,
            bundle.heartbeat.can_consolidate_memory,
            bundle.heartbeat.can_analyze_failures,
            bundle.heartbeat.can_extract_knowledge,
            bundle.heartbeat.can_plan_review,
            bundle.heartbeat.can_strategy_review,
            bundle.heartbeat.can_track_mood,
            bundle.heartbeat.can_encourage,
            bundle.heartbeat.can_track_milestones,
            bundle.heartbeat.notification_pacing,
        ]
        .iter()
        .filter(|&&x| x)
        .count();
        lines.push(Line::from(Span::styled(
            format!("  SKILLS_ALLOCATED: {skills_str}"),
            theme::dim(),
        )));
        lines.push(Line::from(Span::styled(
            format!("  DIAGNOSTICS: {sched_count} crons, {hb_count}/10 parameters active"),
            theme::dim(),
        )));
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_soul_skills(state: &GenesisState, area: Rect, f: &mut Frame) {
    let bundle = persona_defaults::for_persona(state.current_persona());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "LOAD NEURAL WEIGHTS",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Soul toggle
    let soul_focused = state.focused_field == 0;
    let soul_mark = if state.soul_accepted {
        "[ ON ]"
    } else {
        "[ OFF]"
    };
    let soul_style = if soul_focused {
        theme::highlight()
    } else {
        theme::text()
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {soul_mark} "),
            if state.soul_accepted {
                theme::sage()
            } else {
                theme::muted()
            },
        ),
        Span::styled("INSTALL SOUL_TEMPLATE", soul_style),
    ]));

    // Soul preview (first 2 lines)
    if state.soul_accepted {
        for line in bundle.soul_template.lines().take(2) {
            let truncated: String = line
                .chars()
                .take((area.width as usize).saturating_sub(8))
                .collect();
            lines.push(Line::from(Span::styled(
                format!("      \"{truncated}\""),
                theme::dim(),
            )));
        }
        lines.push(Line::from(Span::styled("      ...", theme::dim())));
    }

    lines.push(Line::from(""));

    // Skills toggle
    let skills_focused = state.focused_field == 1;
    let skills_mark = if state.skills_accepted {
        "[ ON ]"
    } else {
        "[ OFF]"
    };
    let skills_style = if skills_focused {
        theme::highlight()
    } else {
        theme::text()
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {skills_mark} "),
            if state.skills_accepted {
                theme::sage()
            } else {
                theme::muted()
            },
        ),
        Span::styled("BOND DEFAULT_SKILLS", skills_style),
    ]));

    if state.skills_accepted {
        let skills_str = bundle.skills.join(", ");
        lines.push(Line::from(Span::styled(
            format!("      {skills_str}"),
            theme::dim(),
        )));
    }

    lines.push(Line::from(""));

    // Custom skills input
    let custom_focused = state.focused_field == 2;
    lines.push(Line::from(Span::styled(
        "  ADDITIONAL_SKILLS (comma-separated):",
        theme::muted(),
    )));
    let c_style = if custom_focused {
        theme::text()
    } else {
        theme::muted()
    };
    lines.push(Line::from(vec![
        Span::styled(
            if custom_focused { "  [ " } else { "  [ " },
            theme::primary_bold(),
        ),
        Span::styled(
            if state.custom_skills_input.is_empty() && !custom_focused {
                "(none)"
            } else {
                &state.custom_skills_input
            },
            c_style,
        ),
        Span::styled(
            if custom_focused { state.cursor() } else { " " },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    f.render_widget(Paragraph::new(lines), area);
}

fn render_schedule(state: &GenesisState, area: Rect, f: &mut Frame) {
    let bundle = persona_defaults::for_persona(state.current_persona());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "INJECT GOAL STATE",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Briefing hour
    let hour_focused = state.focused_field == 0;
    lines.push(Line::from(Span::styled(
        "  CRON_BRIEFING_HOUR (0-23):",
        theme::muted(),
    )));
    let h_style = if hour_focused {
        theme::text()
    } else {
        theme::muted()
    };
    lines.push(Line::from(vec![
        Span::styled(
            if hour_focused { "  [ " } else { "  [ " },
            theme::primary_bold(),
        ),
        Span::styled(&state.briefing_hour, h_style),
        Span::styled(
            if hour_focused { state.cursor() } else { " " },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    lines.push(Line::from(""));

    // Check interval
    let interval_focused = state.focused_field == 1;
    lines.push(Line::from(Span::styled(
        "  BACKGROUND_POLL_INTERVAL (minutes):",
        theme::muted(),
    )));
    let i_style = if interval_focused {
        theme::text()
    } else {
        theme::muted()
    };
    lines.push(Line::from(vec![
        Span::styled(
            if interval_focused { "  [ " } else { "  [ " },
            theme::primary_bold(),
        ),
        Span::styled(&state.check_interval, i_style),
        Span::styled(
            if interval_focused {
                state.cursor()
            } else {
                " "
            },
            theme::primary(),
        ),
        Span::styled(" ]", theme::primary_bold()),
    ]));

    lines.push(Line::from(""));

    // Default schedules preview
    if !bundle.schedules.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(
                "  SYSTEM QUEUED {} DEFAULT SCHEDULES:",
                bundle.schedules.len()
            ),
            theme::dim(),
        )));
        for s in bundle.schedules {
            lines.push(Line::from(Span::styled(
                format!("    {} ({})", s.name, s.cron),
                theme::dim(),
            )));
        }
        lines.push(Line::from(""));
    }

    // Goals toggle
    let goals_focused = state.focused_field == 2;
    let goals_mark = if state.include_goals {
        "[ ON ]"
    } else {
        "[ OFF]"
    };
    let goals_style = if goals_focused {
        theme::highlight()
    } else {
        theme::text()
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {goals_mark} "),
            if state.include_goals {
                theme::sage()
            } else {
                theme::muted()
            },
        ),
        Span::styled(
            format!("SEED {} INITIAL_GOAL_STATES", bundle.goals.len()),
            goals_style,
        ),
    ]));

    if state.include_goals && !bundle.goals.is_empty() {
        for g in bundle.goals {
            lines.push(Line::from(Span::styled(
                format!("      [{}] {}", g.priority, g.description),
                theme::dim(),
            )));
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_email(state: &GenesisState, area: Rect, f: &mut Frame) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "BIND MAIL PROTOCOLS",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "AUTHORIZE SMTP/IMAP FOR BACKGROUND INTELLIGENCE PIPELINES.",
            theme::dim(),
        )),
        Line::from(""),
    ];

    if !state.setup_email {
        lines.push(Line::from(Span::styled(
            "  [ Press Space/Y to mount pipelines, Enter to bypass ]",
            theme::muted(),
        )));
        f.render_widget(Paragraph::new(lines), area);
        return;
    }

    let fields: &[(&str, &str, bool)] = &[
        ("ADDRESS_ID", &state.email_address, false),
        ("IMAP_HOST", &state.imap_host, false),
        ("IMAP_PORT", &state.imap_port, false),
        ("SMTP_HOST", &state.smtp_host, false),
        ("SMTP_PORT", &state.smtp_port, false),
        ("AUTH_USER", &state.email_username, false),
        ("AUTH_PASS", "", true),
    ];

    for (i, (label, value, is_secret)) in fields.iter().enumerate() {
        let focused = state.focused_field == i;
        let display = if *is_secret {
            "*".repeat(state.email_password.len())
        } else {
            value.to_string()
        };
        let style = if focused {
            theme::text()
        } else {
            theme::muted()
        };
        lines.push(Line::from(vec![
            Span::styled(if focused { "  [ " } else { "  [ " }, theme::primary_bold()),
            Span::styled(format!("{label:<12} "), theme::muted()),
            Span::styled(display, style),
            Span::styled(if focused { state.cursor() } else { " " }, theme::primary()),
            Span::styled(" ]", theme::primary_bold()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Ctrl+N to disable email",
        theme::dim(),
    )));

    f.render_widget(Paragraph::new(lines), area);
}

fn render_integrations(state: &GenesisState, area: Rect, f: &mut Frame) {
    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "ATTACH SYNC PIPELINES",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "TOGGLE MOUNT POINTS (Space), MOUNT PARAMETERS (Tab/Down)",
            theme::dim(),
        )),
        Line::from(""),
    ];

    let mut field_idx = 0;
    for (i, label) in INTEGRATION_LABELS.iter().enumerate() {
        let focused = state.focused_field == field_idx;
        let enabled = state.integration_flags[i];
        let mark = if enabled { "[ ON ]" } else { "[ OFF]" };
        let mark_style = if enabled {
            theme::sage()
        } else {
            theme::muted()
        };
        let label_style = if focused {
            theme::highlight()
        } else {
            theme::text()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {mark} "), mark_style),
            Span::styled(label.to_uppercase(), label_style),
        ]));
        field_idx += 1;

        // Sub-fields for enabled integrations
        if enabled {
            // Each entry: (label, display_value, is_secret, is_toggle)
            let sub_fields: Vec<(&str, String, bool, bool)> = match i {
                0 => vec![
                    ("URL", state.calendar_url.clone(), false, false),
                    ("USER", state.calendar_user.clone(), false, false),
                    ("PASS", "*".repeat(state.calendar_pass.len()), true, false),
                ],
                1 => vec![
                    ("URL", state.contacts_url.clone(), false, false),
                    ("USER", state.contacts_user.clone(), false, false),
                    ("PASS", "*".repeat(state.contacts_pass.len()), true, false),
                ],
                2 => vec![
                    ("TOKEN", "*".repeat(state.telegram_token.len()), true, false),
                    ("CHAT_ID", state.telegram_chat_id.clone(), false, false),
                ],
                3 => vec![
                    ("HOST", state.matrix_homeserver.clone(), false, false),
                    ("TOKEN", "*".repeat(state.matrix_token.len()), true, false),
                    ("ROOM_ID", state.matrix_room_id.clone(), false, false),
                ],
                4 => vec![
                    ("PHONE", state.signal_account.clone(), false, false),
                    ("SOCKET", state.signal_socket.clone(), false, false),
                ],
                5 => vec![
                    ("PROVIDER", state.sms_provider.clone(), false, false),
                    ("ACCT_ID", state.sms_account_id.clone(), false, false),
                    ("TOKEN", "*".repeat(state.sms_auth_token.len()), true, false),
                    ("FROM_NUM", state.sms_from_number.clone(), false, false),
                ],
                6 => vec![("PATH", state.vault_path.clone(), false, false)],
                7 => vec![(
                    "RECEIPTS",
                    state.finance_receipt_folder.clone(),
                    false,
                    false,
                )],
                8 => vec![
                    ("REPO_PATH", state.devtools_repo_path.clone(), false, false),
                    ("FORGE", state.devtools_forge.clone(), false, false),
                    (
                        "API_URL",
                        state.devtools_forge_api_url.clone(),
                        false,
                        false,
                    ),
                    ("REPO_SLUG", state.devtools_repo_slug.clone(), false, false),
                    (
                        "TOKEN",
                        "*".repeat(state.devtools_forge_token.len()),
                        true,
                        false,
                    ),
                ],
                9 => {
                    let mark = if state.desktop_deep_interaction {
                        "ON"
                    } else {
                        "OFF"
                    };
                    vec![("DEEP_INT", mark.to_string(), false, true)]
                }
                _ => vec![],
            };

            for (label, display, _is_secret, is_toggle) in sub_fields {
                let sub_focused = state.focused_field == field_idx;
                let style = if sub_focused {
                    theme::text()
                } else {
                    theme::muted()
                };
                let cursor = if sub_focused && !is_toggle {
                    state.cursor()
                } else {
                    " "
                };
                let prefix = if is_toggle && sub_focused {
                    "  >>  [ "
                } else {
                    "      [ "
                };
                lines.push(Line::from(vec![
                    Span::styled(prefix, theme::primary_bold()),
                    Span::styled(format!("{label:<10}"), theme::dim()),
                    Span::styled(display, style),
                    Span::styled(cursor, theme::primary()),
                    Span::styled(" ]", theme::primary_bold()),
                ]));
                field_idx += 1;
            }
        }
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn render_ignition(state: &GenesisState, area: Rect, f: &mut Frame) {
    let bundle = persona_defaults::for_persona(state.current_persona());

    let mut lines = vec![
        Line::from(vec![
            Span::styled("// DIRECTIVE: ", theme::text_bold()),
            Span::styled(
                "INITIATE HEARTBEAT",
                Style::default()
                    .fg(theme::PRIMARY)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
    ];

    // Summary
    let model_display: String = state.model_input.chars().take(28).collect();
    lines.push(Line::from(vec![
        Span::styled("  DESCRIPTOR       ", theme::dim()),
        Span::styled(
            state.effective_agent_name().to_uppercase(),
            theme::text_bold(),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  RUNTIME_CORE     ", theme::dim()),
        Span::styled(
            format!(
                "{} : {}",
                state.current_provider_label().to_uppercase(),
                model_display.to_uppercase()
            ),
            theme::primary(),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  NEURAL_PERSONA   ", theme::dim()),
        Span::styled(state.current_persona().to_uppercase(), theme::secondary()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  ENCRYPTION       ", theme::dim()),
        Span::styled("CHACHA20-POLY1305", theme::sage()),
    ]));

    let skill_count = if state.skills_accepted {
        bundle.skills.len()
    } else {
        0
    } + state
        .custom_skills_input
        .split(',')
        .filter(|s| !s.trim().is_empty())
        .count();
    lines.push(Line::from(vec![
        Span::styled("  SKILLS_ALLOCATED ", theme::dim()),
        Span::styled(format!("{skill_count} REGISTERED"), theme::muted()),
    ]));

    if state.setup_email {
        lines.push(Line::from(vec![
            Span::styled("  IMAP_BINDING     ", theme::dim()),
            Span::styled(&state.email_address, theme::muted()),
        ]));
    }

    let int_count = state.integration_flags.iter().filter(|&&x| x).count();
    if int_count > 0 {
        lines.push(Line::from(vec![
            Span::styled("  SYNC_PIPELINES   ", theme::dim()),
            Span::styled(format!(" {int_count} MOUNTED"), theme::muted()),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  HEARTBEAT_DIAGNOSTIC_PARAMETERS:",
        theme::muted(),
    )));

    // Two-column heartbeat toggle layout (5 per column)
    // Left column: items 0-4  |  Right column: items 5-9
    let col_width = 28usize; // characters per column
    for row in 0..5usize {
        let left_i = row;
        let right_i = row + 5;

        let make_toggle = |i: usize| -> (String, String, Style, Style) {
            let focused = state.focused_field == i;
            let enabled = state.heartbeat_flags[i];
            let mark = if enabled { "[■]" } else { "[○]" };
            let mark_style = if enabled {
                theme::sage()
            } else {
                theme::muted()
            };
            let label_style = if focused {
                theme::highlight()
            } else {
                theme::dim()
            };
            let label_upper: String = HEARTBEAT_LABELS[i]
                .chars()
                .take(col_width - 5)
                .collect::<String>()
                .to_uppercase();
            (mark.to_string(), label_upper, mark_style, label_style)
        };

        let (lm_str, ll_str, lm, ll) = make_toggle(left_i);
        let (rm_str, rl_str, rm, rl) = make_toggle(right_i);

        lines.push(Line::from(vec![
            Span::styled("  ", theme::dim()),
            Span::styled(lm_str, lm),
            Span::styled(format!(" {ll_str:<width$}  ", width = col_width - 5), ll),
            Span::styled(rm_str, rm),
            Span::styled(format!(" {rl_str}"), rl),
        ]));
    }

    lines.push(Line::from(""));
    let on_ignite = state.focused_field == 10;
    let blink = state.frame_count % 60 < 30;
    let ignite_style = if on_ignite {
        Style::default()
            .fg(theme::PRIMARY)
            .add_modifier(Modifier::BOLD | Modifier::REVERSED)
    } else {
        theme::primary_bold()
    };
    lines.push(Line::from(Span::styled(
        if on_ignite || blink {
            "  [ EXECUTE PROTOCOL -> SYSTEM IGNITION ]"
        } else {
            " "
        },
        ignite_style,
    )));

    f.render_widget(Paragraph::new(lines), area);
}

// ── TOML Formatters ───────────────────────────────────────────────

fn sanitize_toml_value(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control())
        .map(|c| match c {
            '"' => "\\\"".to_string(),
            '\\' => "\\\\".to_string(),
            other => other.to_string(),
        })
        .collect()
}

fn format_heartbeat_section(flags: &[bool; 10]) -> String {
    format!(
        r#"
[heartbeat]
enabled = true
interval_minutes = 30
can_reflect = {}
can_consolidate_memory = {}
can_analyze_failures = {}
can_extract_knowledge = {}
can_plan_review = {}
can_strategy_review = {}
can_track_mood = {}
can_encourage = {}
can_track_milestones = {}
notification_pacing = {}
"#,
        flags[0],
        flags[1],
        flags[2],
        flags[3],
        flags[4],
        flags[5],
        flags[6],
        flags[7],
        flags[8],
        flags[9],
    )
}

fn format_skills_line(skills: &[&str]) -> String {
    let items: Vec<String> = skills
        .iter()
        .map(|s| format!("\"{}\"", sanitize_toml_value(s)))
        .collect();
    format!("skills = [{}]", items.join(", "))
}

fn format_goals_section(goals: &[GoalTemplate]) -> String {
    let mut out = String::new();
    for g in goals {
        out.push_str(&format!(
            r#"
[[initial_goals]]
description = "{}"
success_criteria = "{}"
priority = "{}"
"#,
            sanitize_toml_value(g.description),
            sanitize_toml_value(g.success_criteria),
            sanitize_toml_value(g.priority),
        ));
    }
    out
}

fn format_schedules_section(
    schedules: &[ScheduleTemplate],
    agent_name: &str,
    configured: &[&str],
) -> String {
    let safe_agent = sanitize_toml_value(agent_name);
    let mut out = String::new();
    for s in schedules {
        // Skip schedules that require an integration not configured.
        if !s.requires.is_empty() && !configured.contains(&s.requires) {
            continue;
        }
        let safe_name = sanitize_toml_value(s.name);
        let safe_prompt = sanitize_toml_value(s.prompt);
        out.push_str(&format!(
            r#"
# {desc}
[[schedules]]
name = "{safe_agent}-{safe_name}"
cron = "{cron}"
agent = "{safe_agent}"
prompt = "{safe_prompt}"
"#,
            desc = s.description,
            cron = s.cron,
        ));
    }
    out
}

fn format_soul_line(soul: &str) -> String {
    let safe = soul.replace("\"\"\"", "\"\"\\\"");
    format!("soul = \"\"\"\n{safe}\"\"\"")
}

fn format_persona_section(persona_name: &str) -> String {
    let (warmth, formality, verbosity, humor, confidence, curiosity) = match persona_name {
        "assistant" => (0.7, 0.4, 0.5, 0.3, 0.6, 0.7),
        "coder" => (0.4, 0.3, 0.4, 0.2, 0.9, 0.5),
        "researcher" => (0.4, 0.7, 0.7, 0.1, 0.7, 0.9),
        "writer" => (0.6, 0.5, 0.6, 0.4, 0.7, 0.6),
        "coach" => (0.8, 0.3, 0.6, 0.4, 0.7, 0.6),
        "companion" => (0.9, 0.2, 0.6, 0.6, 0.5, 0.8),
        "ops" => (0.3, 0.6, 0.3, 0.1, 0.9, 0.4),
        "analyst" => (0.3, 0.7, 0.5, 0.1, 0.8, 0.8),
        _ => (0.7, 0.4, 0.5, 0.3, 0.6, 0.7),
    };
    format!(
        r#"
[persona]
warmth = {warmth}
formality = {formality}
verbosity = {verbosity}
humor = {humor}
confidence = {confidence}
curiosity = {curiosity}
"#
    )
}

// ── Email Helpers ─────────────────────────────────────────────────

fn guess_imap_host(email: &str) -> String {
    email
        .split('@')
        .nth(1)
        .map(|d| format!("imap.{d}"))
        .unwrap_or_default()
}

fn guess_smtp_host(email: &str) -> String {
    email
        .split('@')
        .nth(1)
        .map(|d| format!("smtp.{d}"))
        .unwrap_or_default()
}

// ── Finalization ──────────────────────────────────────────────────

fn finalize(state: &GenesisState, dirs: &AivyxDirs) -> anyhow::Result<()> {
    // 1. Create directory structure
    dirs.ensure_dirs()?;

    // 2. Generate and encrypt master key
    let master_key = MasterKey::generate();
    let envelope = master_key.encrypt_to_envelope(state.passphrase.as_bytes())?;
    let envelope_json = serde_json::to_string_pretty(&envelope)?;
    std::fs::write(dirs.master_key_path(), envelope_json)?;

    // 3. Store secrets in encrypted keystore
    let store = EncryptedStore::open(dirs.store_path())?;

    if state.needs_api_key() && !state.api_key.is_empty() {
        let key_name = match state.current_provider_type() {
            "OpenAI" => "OPENAI_API_KEY",
            "Claude" => "ANTHROPIC_API_KEY",
            "OpenAICompatible" => "OPENROUTER_API_KEY",
            _ => "API_KEY",
        };
        store.put(key_name, state.api_key.as_bytes(), &master_key)?;
    }

    if state.setup_email && !state.email_password.is_empty() {
        store.put(
            "EMAIL_PASSWORD",
            state.email_password.as_bytes(),
            &master_key,
        )?;
    }

    // Integration secrets
    if state.integration_flags[0] && !state.calendar_pass.is_empty() {
        store.put(
            "CALENDAR_PASSWORD",
            state.calendar_pass.as_bytes(),
            &master_key,
        )?;
    }
    if state.integration_flags[1] && !state.contacts_pass.is_empty() {
        store.put(
            "CONTACTS_PASSWORD",
            state.contacts_pass.as_bytes(),
            &master_key,
        )?;
    }
    if state.integration_flags[2] && !state.telegram_token.is_empty() {
        store.put(
            "TELEGRAM_BOT_TOKEN",
            state.telegram_token.as_bytes(),
            &master_key,
        )?;
    }
    if state.integration_flags[3] && !state.matrix_token.is_empty() {
        store.put(
            "MATRIX_ACCESS_TOKEN",
            state.matrix_token.as_bytes(),
            &master_key,
        )?;
    }
    if state.integration_flags[5] && !state.sms_auth_token.is_empty() {
        store.put(
            "SMS_AUTH_TOKEN",
            state.sms_auth_token.as_bytes(),
            &master_key,
        )?;
    }
    if state.integration_flags[8] && !state.devtools_forge_token.is_empty() {
        store.put(
            "FORGE_TOKEN",
            state.devtools_forge_token.as_bytes(),
            &master_key,
        )?;
    }

    // 4. Build config.toml
    let safe_model = sanitize_toml_value(&state.model_input);
    let safe_agent = sanitize_toml_value(state.effective_agent_name());
    let persona_name = state.current_persona();

    let provider_section = match state.current_provider_type() {
        "Ollama" => {
            let safe_url = sanitize_toml_value(&state.ollama_url);
            format!(
                "[provider]\ntype = \"Ollama\"\nbase_url = \"{safe_url}\"\nmodel = \"{safe_model}\""
            )
        }
        "OpenAI" => format!(
            "[provider]\ntype = \"OpenAI\"\napi_key_ref = \"OPENAI_API_KEY\"\nmodel = \"{safe_model}\""
        ),
        "Claude" => format!(
            "[provider]\ntype = \"Claude\"\napi_key_ref = \"ANTHROPIC_API_KEY\"\nmodel = \"{safe_model}\""
        ),
        "OpenAICompatible" => format!(
            "[provider]\ntype = \"OpenAICompatible\"\napi_key_ref = \"OPENROUTER_API_KEY\"\nbase_url = \"{OPENROUTER_BASE_URL}\"\nmodel = \"{safe_model}\""
        ),
        // `current_provider_type()` indexes into PROVIDERS which only
        // contains the four strings above, so this branch is unreachable
        // by construction. We still handle it gracefully rather than
        // panicking — if PROVIDERS grows a fifth entry in the future and
        // someone forgets to update this match, the wizard will fail
        // with a clean error instead of crashing the TUI mid-genesis.
        other => {
            return Err(anyhow::anyhow!(
                "unsupported provider type {other:?} — config.toml not written"
            ));
        }
    };

    let mut config = format!("# Aivyx Personal Assistant Configuration\n\n{provider_section}\n");

    config.push_str("\n[autonomy]\ndefault_tier = \"Trust\"\n");

    // Agent section
    config.push_str(&format!(
        "\n[agent]\nname = \"{safe_agent}\"\npersona = \"{persona_name}\"\n"
    ));

    let bundle = persona_defaults::for_persona(persona_name);

    if state.soul_accepted {
        config.push_str(&format_soul_line(bundle.soul_template));
        config.push('\n');
    }

    // Skills
    let mut all_skills: Vec<&str> = Vec::new();
    if state.skills_accepted {
        all_skills.extend_from_slice(bundle.skills);
    }
    let custom: Vec<String> = state
        .custom_skills_input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    // We need to hold the custom skills in a stable vec for the &str references
    let custom_refs: Vec<&str> = custom.iter().map(|s| s.as_str()).collect();
    all_skills.extend_from_slice(&custom_refs);

    if !all_skills.is_empty() {
        config.push_str(&format_skills_line(&all_skills));
        config.push('\n');
    }

    // Loop config. The schedule step restricts keystrokes to valid ranges
    // (hour 0..=23, interval 1..=10_080), so these `.unwrap_or` defaults
    // only fire on a truly empty buffer — no silent clamping of user input.
    let briefing_hour: u8 = state.briefing_hour.parse().unwrap_or(8);
    let check_interval: u32 = state.check_interval.parse().unwrap_or(15).max(1);
    config.push_str(&format!(
        "\n[loop]\ncheck_interval_minutes = {check_interval}\nmorning_briefing = true\nbriefing_hour = {briefing_hour}\n"
    ));

    // Heartbeat
    config.push_str(&format_heartbeat_section(&state.heartbeat_flags));

    // Persona dimensions
    config.push_str(&format_persona_section(persona_name));

    // Initial goals
    if state.include_goals {
        config.push_str(&format_goals_section(bundle.goals));
    }

    // Schedules — filter by configured integrations
    if !bundle.schedules.is_empty() {
        let mut configured: Vec<&str> = Vec::new();
        if state.setup_email {
            configured.push("email");
        }
        if state.integration_flags[0] {
            configured.push("calendar");
        }
        if state.integration_flags[1] {
            configured.push("contacts");
        }
        if state.integration_flags[2] {
            configured.push("telegram");
        }
        if state.integration_flags[3] {
            configured.push("matrix");
        }
        if state.integration_flags[4] {
            configured.push("signal");
        }
        if state.integration_flags[5] {
            configured.push("sms");
        }
        if state.integration_flags[6] {
            configured.push("vault");
        }
        if state.integration_flags[7] {
            configured.push("finance");
        }
        if state.integration_flags[8] {
            configured.push("devtools");
        }
        config.push_str(&format_schedules_section(
            bundle.schedules,
            state.effective_agent_name(),
            &configured,
        ));
    }

    // Email
    if state.setup_email && !state.email_address.is_empty() {
        config.push_str(&format!(
            "\n[email]\nimap_host = \"{}\"\nimap_port = {}\nsmtp_host = \"{}\"\nsmtp_port = {}\naddress = \"{}\"\nusername = \"{}\"\n",
            sanitize_toml_value(&state.imap_host),
            state.imap_port.parse::<u16>().unwrap_or(993),
            sanitize_toml_value(&state.smtp_host),
            state.smtp_port.parse::<u16>().unwrap_or(587),
            sanitize_toml_value(&state.email_address),
            sanitize_toml_value(if state.email_username.is_empty() {
                &state.email_address
            } else {
                &state.email_username
            }),
        ));
    }

    // Integration sections
    if state.integration_flags[0] {
        config.push_str(&format!(
            "\n[calendar]\nurl = \"{}\"\nusername = \"{}\"\n",
            sanitize_toml_value(&state.calendar_url),
            sanitize_toml_value(&state.calendar_user),
        ));
    }
    if state.integration_flags[1] {
        config.push_str(&format!(
            "\n[contacts]\nurl = \"{}\"\nusername = \"{}\"\n",
            sanitize_toml_value(&state.contacts_url),
            sanitize_toml_value(&state.contacts_user),
        ));
    }
    if state.integration_flags[2] {
        config.push_str("\n[telegram]\n");
        if !state.telegram_chat_id.is_empty() {
            config.push_str(&format!(
                "default_chat_id = \"{}\"\n",
                sanitize_toml_value(&state.telegram_chat_id),
            ));
        }
    }
    if state.integration_flags[3] {
        config.push_str(&format!(
            "\n[matrix]\nhomeserver = \"{}\"\n",
            sanitize_toml_value(&state.matrix_homeserver),
        ));
        if !state.matrix_room_id.is_empty() {
            config.push_str(&format!(
                "default_room_id = \"{}\"\n",
                sanitize_toml_value(&state.matrix_room_id),
            ));
        }
    }
    if state.integration_flags[4] {
        config.push_str(&format!(
            "\n[signal]\naccount = \"{}\"\nsocket_path = \"{}\"\n",
            sanitize_toml_value(&state.signal_account),
            sanitize_toml_value(&state.signal_socket),
        ));
    }
    if state.integration_flags[5] {
        config.push_str(&format!(
            "\n[sms]\nprovider = \"{}\"\naccount_id = \"{}\"\nfrom_number = \"{}\"\n# Auth token stored encrypted as SMS_AUTH_TOKEN\n",
            sanitize_toml_value(&state.sms_provider),
            sanitize_toml_value(&state.sms_account_id),
            sanitize_toml_value(&state.sms_from_number),
        ));
    }
    if state.integration_flags[6] {
        config.push_str(&format!(
            "\n[vault]\npath = \"{}\"\n",
            sanitize_toml_value(&state.vault_path),
        ));
    }
    if state.integration_flags[7] {
        config.push_str(&format!(
            "\n[finance]\nenabled = true\nreceipt_folder = \"{}\"\n",
            sanitize_toml_value(&state.finance_receipt_folder),
        ));
    }
    if state.integration_flags[8] {
        config.push_str(&format!(
            "\n[devtools]\nrepo_path = \"{}\"\n",
            sanitize_toml_value(&state.devtools_repo_path),
        ));
        if !state.devtools_forge.is_empty() {
            config.push_str(&format!(
                "forge = \"{}\"\n",
                sanitize_toml_value(&state.devtools_forge),
            ));
        }
        if !state.devtools_forge_api_url.is_empty() {
            config.push_str(&format!(
                "forge_api_url = \"{}\"\n",
                sanitize_toml_value(&state.devtools_forge_api_url),
            ));
        }
        if !state.devtools_repo_slug.is_empty() {
            config.push_str(&format!(
                "repo = \"{}\"\n",
                sanitize_toml_value(&state.devtools_repo_slug),
            ));
        }
    }

    // Desktop section — auto-enabled when a display server is present,
    // deep interaction is opt-in via the integration toggle.
    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    if has_display || state.integration_flags[9] {
        config.push_str("\n[desktop]\nclipboard = true\nwindows = true\nnotifications = true\n");
        if state.integration_flags[9] && state.desktop_deep_interaction {
            config.push_str("\n[desktop.interaction]\nenabled = true\n");
        } else {
            config.push_str("\n[desktop.interaction]\nenabled = false\n");
        }
    }

    // 5. Write config
    std::fs::write(dirs.config_path(), config)?;

    Ok(())
}
