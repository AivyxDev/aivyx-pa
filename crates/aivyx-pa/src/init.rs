//! Genesis wizard — 10 focused steps to create a complete agent.
//!
//! 1.  Set passphrase (encrypts all local data)
//! 2.  Choose provider + model (auto-detects Ollama models)
//! 3.  Name your assistant + choose persona (preview bundle)
//! 4.  Agent soul (personality narrative — accept/edit/skip)
//! 5.  Skills selection (accept defaults + add custom)
//! 6.  Configure schedule (briefing hour, check interval)
//! 7.  Initial goals (persona-specific starter goals)
//! 8.  Connect email (optional — IMAP/SMTP)
//! 9.  Additional integrations (Calendar, Contacts, Telegram, Matrix, Signal)
//! 10. Heartbeat intelligence (toggle 10 autonomous features)

use aivyx_config::AivyxDirs;
use aivyx_crypto::{EncryptedStore, MasterKey};
use std::io::{self, BufRead, Write};
use std::path::Path;
use zeroize::Zeroizing;

use crate::persona_defaults;
use crate::persona_defaults::{GoalTemplate, HeartbeatBundle, ScheduleTemplate};

struct EmailSettings {
    address: String,
    imap_host: String,
    imap_port: u16,
    smtp_host: String,
    smtp_port: u16,
    username: String,
    password: String,
}

/// PA-relevant persona presets (subset of aivyx-core's 28).
const PA_PERSONAS: &[(&str, &str)] = &[
    ("assistant", "Friendly and proactive general assistant"),
    ("coder", "Technical and precise — for developers"),
    ("researcher", "Methodical and thorough — deep analysis"),
    ("writer", "Clear and engaging — content creation"),
    ("coach", "Motivational and goal-oriented"),
    ("companion", "Warm and conversational"),
    ("ops", "Reliable and security-conscious — sysadmin"),
    ("analyst", "Data-driven and structured"),
];

/// Guess IMAP host from email domain (e.g., user@gmail.com → imap.gmail.com).
fn guess_imap_host(email: &str) -> String {
    if let Some(domain) = email.split('@').nth(1) {
        format!("imap.{domain}")
    } else {
        String::new()
    }
}

/// Guess SMTP host from email domain (e.g., user@gmail.com → smtp.gmail.com).
fn guess_smtp_host(email: &str) -> String {
    if let Some(domain) = email.split('@').nth(1) {
        format!("smtp.{domain}")
    } else {
        String::new()
    }
}

const PROVIDERS: &[(&str, &str, &[&str])] = &[
    (
        "Ollama",
        "ollama",
        &[
            "qwen3:14b",
            "qwen2.5-coder:14b",
            "llama3.1:8b",
            "mistral:7b",
        ],
    ),
    ("OpenAI", "openai", &["gpt-4o", "gpt-4o-mini", "o1-mini"]),
    (
        "Claude",
        "anthropic",
        &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"],
    ),
    (
        "OpenAICompatible",
        "openrouter",
        &[
            "anthropic/claude-sonnet-4-20250514",
            "openai/gpt-4o",
            "google/gemini-2.0-flash-001",
        ],
    ),
];

/// Sanitize a string for safe interpolation into a TOML double-quoted value.
///
/// Escapes characters that could break TOML syntax or inject new sections:
/// - `"` → `\"`  (prevents closing the quoted value)
/// - `\` → `\\`  (prevents escape sequences)
/// - Newlines stripped (prevents multi-line injection of `[section]` headers)
/// - Control characters stripped
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

fn prompt(msg: &str) -> anyhow::Result<String> {
    print!("{msg}");
    let _ = io::stdout().flush();
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|e| anyhow::anyhow!("Error reading input: {e}"))?;
    Ok(input.trim().to_string())
}

/// Prompt for a secret value with echo suppressed.
fn prompt_secret(msg: &str) -> anyhow::Result<Zeroizing<String>> {
    print!("{msg}");
    let _ = io::stdout().flush();
    match rpassword::read_password() {
        Ok(p) => Ok(Zeroizing::new(p)),
        Err(_) => {
            let mut input = String::new();
            io::stdin()
                .lock()
                .read_line(&mut input)
                .map_err(|e| anyhow::anyhow!("Error reading input: {e}"))?;
            Ok(Zeroizing::new(input.trim().to_string()))
        }
    }
}

fn prompt_default(msg: &str, default: &str) -> anyhow::Result<String> {
    let input = prompt(&format!("{msg} [{default}]: "))?;
    Ok(if input.is_empty() {
        default.to_string()
    } else {
        input
    })
}

// ── Config Section Formatters ──────────────────────────────────────
//
// Each function returns a valid TOML fragment ready to append to the
// config string. Used by the wizard to incrementally build config.toml.

/// Format the `[heartbeat]` section from a HeartbeatBundle.
fn format_heartbeat_section(bundle: &HeartbeatBundle) -> String {
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
        bundle.can_reflect,
        bundle.can_consolidate_memory,
        bundle.can_analyze_failures,
        bundle.can_extract_knowledge,
        bundle.can_plan_review,
        bundle.can_strategy_review,
        bundle.can_track_mood,
        bundle.can_encourage,
        bundle.can_track_milestones,
        bundle.notification_pacing,
    )
}

/// Format a skills array as an inline TOML value.
fn format_skills_line(skills: &[&str]) -> String {
    let items: Vec<String> = skills
        .iter()
        .map(|s| format!("\"{}\"", sanitize_toml_value(s)))
        .collect();
    format!("skills = [{}]", items.join(", "))
}

/// Format `[[initial_goals]]` sections from goal templates.
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

/// Format `[[schedules]]` sections from schedule templates.
///
/// The `agent` field is required by `ScheduleEntry`, so we set it to
/// the agent name. `description` from the template becomes a TOML
/// comment (it's not a field on `ScheduleEntry`).
fn format_schedules_section(
    schedules: &[ScheduleTemplate],
    agent_name: &str,
    configured: &[&str],
) -> String {
    let safe_agent = sanitize_toml_value(agent_name);
    let mut out = String::new();
    let mut skipped = 0usize;
    for s in schedules {
        // Skip schedules that require an integration not configured.
        if !s.requires.is_empty() && !configured.contains(&s.requires) {
            skipped += 1;
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
    if skipped > 0 {
        println!("    ({skipped} schedule(s) skipped — require unconfigured integrations)");
    }
    out
}

/// Format a soul narrative as a triple-quoted TOML string.
fn format_soul_line(soul: &str) -> String {
    // Triple-quoted strings don't need escaping for most content.
    // Just ensure no sequence of 3+ consecutive quotes exists.
    let safe = soul.replace("\"\"\"", "\"\"\\\"");
    format!("soul = \"\"\"\n{safe}\"\"\"")
}

/// Format the `[persona]` section with preset dimensions.
///
/// Fields match `PaPersonaConfig`: formality, verbosity, warmth, humor,
/// confidence, curiosity. Values tuned per persona role.
fn format_persona_section(persona_name: &str) -> String {
    // (warmth, formality, verbosity, humor, confidence, curiosity)
    let (warmth, formality, verbosity, humor, confidence, curiosity) = match persona_name {
        "assistant" => (0.7, 0.4, 0.5, 0.3, 0.6, 0.7),
        "coder" => (0.4, 0.3, 0.4, 0.2, 0.9, 0.5),
        "researcher" => (0.4, 0.7, 0.7, 0.1, 0.7, 0.9),
        "writer" => (0.6, 0.5, 0.6, 0.4, 0.7, 0.6),
        "coach" => (0.8, 0.3, 0.6, 0.4, 0.7, 0.6),
        "companion" => (0.9, 0.2, 0.6, 0.6, 0.5, 0.8),
        "ops" => (0.3, 0.6, 0.3, 0.1, 0.9, 0.4),
        "analyst" => (0.3, 0.7, 0.5, 0.1, 0.8, 0.8),
        _ => (0.7, 0.4, 0.5, 0.3, 0.6, 0.7), // assistant fallback
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

/// Run the Genesis wizard. Returns the passphrase on success so the
/// caller can unlock the master key without re-prompting.
pub async fn run(home: &Path) -> anyhow::Result<Zeroizing<String>> {
    println!();
    println!("  Welcome to Aivyx");
    println!("  Your private AI personal assistant.");
    println!();

    let dirs = AivyxDirs::new(home);

    // Check if already initialized
    if dirs.is_initialized() {
        let answer = prompt("  Already initialized. Re-run setup? (y/N): ")?;
        if !answer.to_lowercase().starts_with('y') {
            println!("  Setup cancelled.");
            anyhow::bail!("Setup cancelled by user.");
        }
    }

    // ── Step 1: Passphrase ──────────────────────────────────────
    println!("  Step 1/10: Set a passphrase");
    println!("  This encrypts all your local data. Choose something memorable.");
    println!();

    let passphrase = prompt_secret("  Passphrase: ")?;
    if passphrase.len() < 8 {
        anyhow::bail!("Passphrase too short (minimum 8 characters).");
    }

    let confirm = prompt_secret("  Confirm: ")?;
    if *passphrase != *confirm {
        anyhow::bail!("Passphrases don't match.");
    }

    // ── Step 2: Provider + Model ────────────────────────────────
    println!();
    println!("  Step 2/10: Choose your LLM provider");
    println!();
    for (i, (name, _, _)) in PROVIDERS.iter().enumerate() {
        println!("    [{i}] {name}");
    }
    println!();

    let provider_idx: usize = loop {
        let input = prompt("  Provider [0]: ")?;
        let val = if input.is_empty() {
            0
        } else {
            input.parse().unwrap_or(99)
        };
        if val < PROVIDERS.len() {
            break val;
        }
        println!("  Invalid selection.");
    };

    let (provider_type, _label, suggestions) = PROVIDERS[provider_idx];

    // For Ollama, try to discover locally installed models
    let mut ollama_models: Vec<String> = Vec::new();
    let base_url = if provider_type == "Ollama" {
        let url = prompt_default("  Ollama URL", "http://localhost:11434")?;
        // Probe the Ollama API for installed models
        let tag_url = format!("{url}/api/tags");
        match reqwest::Client::new()
            .get(&tag_url)
            .timeout(std::time::Duration::from_secs(3))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(json) = resp.json::<serde_json::Value>().await
                    && let Some(models) = json["models"].as_array()
                {
                    for m in models {
                        if let Some(name) = m["name"].as_str() {
                            ollama_models.push(name.to_string());
                        }
                    }
                }
                if ollama_models.is_empty() {
                    println!("  \u{26a0} Ollama is running but no models are installed.");
                    println!("    Run: ollama pull <model> to install one.");
                } else {
                    println!();
                    println!("  Detected {} installed model(s):", ollama_models.len());
                    println!();
                    for (i, m) in ollama_models.iter().enumerate() {
                        println!("    [{i}] {m}");
                    }
                }
            }
            _ => {
                println!("  \u{26a0} Ollama doesn't seem to be running at {url}");
                println!("    Make sure Ollama is started before launching the agent.");
            }
        }
        Some(url)
    } else {
        None
    };

    // Model selection — prefer Ollama-detected models when available
    println!();
    let effective_suggestions: Vec<&str> = if !ollama_models.is_empty() {
        ollama_models.iter().map(|s| s.as_str()).collect()
    } else if !suggestions.is_empty() {
        suggestions.to_vec()
    } else {
        Vec::new()
    };

    if !effective_suggestions.is_empty() && ollama_models.is_empty() {
        println!("  Choose a model:");
        println!();
        for (i, model) in effective_suggestions.iter().enumerate() {
            println!("    [{i}] {model}");
        }
    }
    println!();

    let model = if !effective_suggestions.is_empty() {
        let input = prompt_default("  Model", effective_suggestions[0])?;
        if let Ok(idx) = input.parse::<usize>() {
            if idx < effective_suggestions.len() {
                effective_suggestions[idx].to_string()
            } else {
                input
            }
        } else {
            input
        }
    } else {
        prompt("  Model name: ")?
    };

    // API key for cloud providers (echo suppressed for security)
    let api_key = if provider_type != "Ollama" {
        let key = prompt_secret("  API Key: ")?;
        if key.is_empty() {
            None
        } else {
            Some((*key).clone())
        }
    } else {
        None
    };

    // ── Step 3: Name + Persona ──────────────────────────────────
    println!();
    println!("  Step 3/10: Name your assistant and choose a personality");
    println!();

    let agent_name = prompt_default("  Assistant name", "assistant")?;

    println!();
    for (i, (name, desc)) in PA_PERSONAS.iter().enumerate() {
        println!("    [{i}] {name:<12} {desc}");
    }
    println!();

    let persona_idx: usize = loop {
        let input = prompt("  Persona [0]: ")?;
        let val = if input.is_empty() {
            0
        } else {
            input.parse().unwrap_or(99)
        };
        if val < PA_PERSONAS.len() {
            break val;
        }
        println!("  Invalid selection.");
    };

    let (persona_name, _persona_desc) = PA_PERSONAS[persona_idx];

    // Show persona preview using PersonaBundle
    let bundle = persona_defaults::for_persona(persona_name);
    println!();
    println!("  Persona: {persona_name}");
    println!("    Skills:    {}", bundle.skills.join(", "));
    let hb_features: Vec<&str> = [
        (bundle.heartbeat.can_reflect, "reflect"),
        (bundle.heartbeat.can_consolidate_memory, "consolidate"),
        (bundle.heartbeat.can_analyze_failures, "analyze failures"),
        (bundle.heartbeat.can_extract_knowledge, "extract knowledge"),
        (bundle.heartbeat.can_plan_review, "plan review"),
        (bundle.heartbeat.can_strategy_review, "strategy review"),
        (bundle.heartbeat.can_track_mood, "mood"),
        (bundle.heartbeat.can_encourage, "encourage"),
        (bundle.heartbeat.can_track_milestones, "milestones"),
    ]
    .iter()
    .filter(|(enabled, _)| *enabled)
    .map(|(_, name)| *name)
    .collect();
    println!("    Heartbeat: {}", hb_features.join(", "));
    println!(
        "    Schedules: {}",
        bundle
            .schedules
            .iter()
            .map(|s| s.name)
            .collect::<Vec<_>>()
            .join(", ")
    );

    // ── Step 4: Soul ───────────────────────────────────────────
    println!();
    println!("  Step 4/10: Agent Soul");
    println!();
    println!("  Your agent's soul defines its personality and operating principles:");
    println!();
    // Show first 3 lines of the soul template as a preview
    for line in bundle.soul_template.lines().take(3) {
        println!("    \"{line}\"");
    }
    println!("    ...");
    println!();

    let soul_choice = prompt("  Accept this soul? (Y/n/edit): ")?;
    let soul: Option<String> = match soul_choice.to_lowercase().as_str() {
        "n" | "no" => None, // skip — auto-generated from persona
        "edit" | "e" => {
            println!("  Enter your custom soul (blank line to finish):");
            let mut lines = Vec::new();
            loop {
                let line = prompt("  > ")?;
                if line.is_empty() {
                    break;
                }
                lines.push(line);
            }
            if lines.is_empty() {
                None
            } else {
                Some(lines.join("\n"))
            }
        }
        _ => Some(bundle.soul_template.to_string()), // Y / Enter
    };

    // ── Step 5: Skills ─────────────────────────────────────────
    println!();
    println!("  Step 5/10: Skills");
    println!();
    println!("  Suggested skills for {persona_name}:");
    for (i, skill) in bundle.skills.iter().enumerate() {
        println!("    [{i}] {skill}");
    }
    println!();

    let skills_choice = prompt("  Accept? (Y/n): ")?;
    let mut skills: Vec<String> = if skills_choice.to_lowercase().starts_with('n') {
        Vec::new()
    } else {
        bundle.skills.iter().map(|s| s.to_string()).collect()
    };

    let extra = prompt("  Add more? (comma-separated, or Enter to skip): ")?;
    if !extra.is_empty() {
        for s in extra.split(',') {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                skills.push(trimmed);
            }
        }
    }

    // ── Step 6: Schedule ────────────────────────────────────────
    println!();
    println!("  Step 6/10: Configure your schedule");
    println!("  Your assistant checks for new email, evaluates goals, and runs");
    println!("  scheduled tasks on a timer.");
    println!();

    let briefing_hour = prompt_default("  Morning briefing hour (0-23)", "8")?;
    let briefing_hour: u8 = briefing_hour.parse().unwrap_or(8).min(23);

    let check_interval = prompt_default("  Check interval (minutes)", "15")?;
    let check_interval: u32 = check_interval.parse().unwrap_or(15).max(1);

    // ── Step 7: Initial Goals ──────────────────────────────────
    println!();
    println!("  Step 7/10: Initial Goals");
    println!();
    if bundle.goals.is_empty() {
        println!("  No starter goals for this persona.");
    } else {
        println!("  Suggested starter goals:");
        for (i, g) in bundle.goals.iter().enumerate() {
            println!("    [{}] {} ({})", i + 1, g.description, g.priority);
        }
        println!();
    }

    let include_goals = if !bundle.goals.is_empty() {
        let choice = prompt("  Include these goals? (Y/n): ")?;
        !choice.to_lowercase().starts_with('n')
    } else {
        false
    };

    // ── Step 8: Optional Email ──────────────────────────────────
    println!();
    println!("  Step 8/10: Connect your email (optional)");
    println!("  This lets your assistant read and send email on your behalf.");
    println!();

    let setup_email = prompt("  Set up email? (y/N): ")?;
    let email_settings = if setup_email.to_lowercase().starts_with('y') {
        let address = prompt("  Email address: ")?;
        if address.is_empty() {
            None
        } else {
            let imap_host = prompt_default("  IMAP host", &guess_imap_host(&address))?;
            let imap_port = prompt_default("  IMAP port", "993")?;
            let smtp_host = prompt_default("  SMTP host", &guess_smtp_host(&address))?;
            let smtp_port = prompt_default("  SMTP port", "587")?;
            let username = prompt_default("  Username", &address)?;
            let password = prompt_secret("  Password: ")?.to_string();

            Some(EmailSettings {
                address,
                imap_host,
                imap_port: imap_port.parse().unwrap_or(993),
                smtp_host,
                smtp_port: smtp_port.parse().unwrap_or(587),
                username,
                password,
            })
        }
    } else {
        None
    };

    // ── Step 9: Additional Integrations ─────────────────────────
    println!();
    println!("  Step 9/10: Integrations (optional)");
    println!();
    println!("  Available integrations:");
    println!("    [1]  Calendar (CalDAV)");
    println!("    [2]  Contacts (CardDAV)");
    println!("    [3]  Telegram");
    println!("    [4]  Matrix");
    println!("    [5]  Signal");
    println!("    [6]  SMS (Twilio/Vonage)");
    println!("    [7]  Document Vault");
    println!("    [8]  Finance Tracking");
    println!("    [9]  Dev Tools (git + forge)");
    println!("    [10] Desktop Interaction (AT-SPI2/CDP)");
    println!();

    let integrations_input = prompt("  Set up any? (comma-separated numbers, or Enter to skip): ")?;

    struct CalendarSetup {
        url: String,
        username: String,
        password: String,
    }
    struct ContactsSetup {
        url: String,
        username: String,
        password: String,
    }
    struct TelegramSetup {
        default_chat_id: Option<String>,
    }
    struct MatrixSetup {
        homeserver: String,
        default_room_id: Option<String>,
    }
    struct SignalSetup {
        account: String,
        socket_path: String,
    }
    struct SmsSetup {
        provider: String,
        account_id: String,
        from_number: String,
    }
    struct VaultSetup {
        path: String,
    }
    struct FinanceSetup {
        receipt_folder: String,
    }
    struct DevToolsSetup {
        repo_path: String,
        forge: Option<String>,
        forge_api_url: Option<String>,
        repo: Option<String>,
    }
    struct DesktopInteractionSetup {
        enabled: bool,
    }

    let mut integration_secrets: Vec<(&str, String)> = Vec::new();
    let mut calendar_setup: Option<CalendarSetup> = None;
    let mut contacts_setup: Option<ContactsSetup> = None;
    let mut telegram_setup: Option<TelegramSetup> = None;
    let mut matrix_setup: Option<MatrixSetup> = None;
    let mut signal_setup: Option<SignalSetup> = None;
    let mut sms_setup: Option<SmsSetup> = None;
    let mut vault_setup: Option<VaultSetup> = None;
    let mut finance_setup: Option<FinanceSetup> = None;
    let mut devtools_setup: Option<DevToolsSetup> = None;
    let mut desktop_interaction_setup: Option<DesktopInteractionSetup> = None;

    // Parse comma-or-space-separated numbers (supports two-digit "10").
    let selected: Vec<u8> = integrations_input
        .replace(',', " ")
        .split_whitespace()
        .filter_map(|s| s.parse::<u8>().ok())
        .collect();

    for choice in &selected {
        match choice {
            1 => {
                println!();
                println!("  Calendar (CalDAV):");
                let url = prompt("    Server URL: ")?;
                let username = prompt("    Username: ")?;
                let password = prompt_secret("    Password: ")?.to_string();
                calendar_setup = Some(CalendarSetup {
                    url,
                    username,
                    password,
                });
            }
            2 => {
                println!();
                println!("  Contacts (CardDAV):");
                let url = prompt("    Server URL: ")?;
                let username = prompt("    Username: ")?;
                let password = prompt_secret("    Password: ")?.to_string();
                contacts_setup = Some(ContactsSetup {
                    url,
                    username,
                    password,
                });
            }
            3 => {
                println!();
                println!("  Telegram:");
                let token = prompt_secret("    Bot token: ")?.to_string();
                let chat_id = prompt("    Default chat ID (optional): ")?;
                telegram_setup = Some(TelegramSetup {
                    default_chat_id: if chat_id.is_empty() {
                        None
                    } else {
                        Some(chat_id)
                    },
                });
                integration_secrets.push(("TELEGRAM_BOT_TOKEN", token));
            }
            4 => {
                println!();
                println!("  Matrix:");
                let homeserver = prompt("    Homeserver URL: ")?;
                let token = prompt_secret("    Access token: ")?.to_string();
                let room_id = prompt("    Default room ID (optional): ")?;
                matrix_setup = Some(MatrixSetup {
                    homeserver,
                    default_room_id: if room_id.is_empty() {
                        None
                    } else {
                        Some(room_id)
                    },
                });
                integration_secrets.push(("MATRIX_ACCESS_TOKEN", token));
            }
            5 => {
                println!();
                println!("  Signal:");
                let account = prompt("    Phone number: ")?;
                let socket_path = prompt_default("    Socket path", "/var/run/signal-cli/socket")?;
                signal_setup = Some(SignalSetup {
                    account,
                    socket_path,
                });
            }
            6 => {
                println!();
                println!("  SMS:");
                println!("    Providers: [1] Twilio  [2] Vonage");
                let provider_choice = prompt_default("    Provider", "1")?;
                let provider = if provider_choice == "2" {
                    "vonage"
                } else {
                    "twilio"
                };
                let account_id = prompt("    Account ID / API Key: ")?;
                let auth_token = prompt_secret("    Auth Token: ")?.to_string();
                let from_number = prompt("    From number (E.164, e.g. +15551234567): ")?;
                sms_setup = Some(SmsSetup {
                    provider: provider.into(),
                    account_id,
                    from_number,
                });
                integration_secrets.push(("SMS_AUTH_TOKEN", auth_token));
            }
            7 => {
                println!();
                println!("  Document Vault:");
                println!("    A local directory of markdown, text, and PDF files for");
                println!("    semantic search and knowledge extraction.");
                let path = prompt_default("    Vault path", "~/Documents/vault")?;
                vault_setup = Some(VaultSetup { path });
            }
            8 => {
                println!();
                println!("  Finance Tracking:");
                println!("    Track expenses, bills, and budgets. Detects transactions");
                println!("    from emails automatically when email is configured.");
                let receipt_folder =
                    prompt_default("    Receipt subfolder (in vault)", "receipts")?;
                finance_setup = Some(FinanceSetup { receipt_folder });
            }
            9 => {
                println!();
                println!("  Dev Tools:");
                let repo_path = prompt("    Default repository path: ")?;
                let setup_forge = prompt("    Connect a forge? (y/N): ")?;
                let (forge, forge_api_url, repo) = if setup_forge.to_lowercase().starts_with('y') {
                    println!("    Forge: [1] GitHub  [2] Gitea");
                    let forge_choice = prompt_default("    Forge", "1")?;
                    let forge_name = if forge_choice == "2" {
                        "gitea"
                    } else {
                        "github"
                    };
                    let api_url = if forge_name == "gitea" {
                        Some(prompt(
                            "    Gitea API URL (e.g. https://gitea.example.com/api/v1): ",
                        )?)
                    } else {
                        None // GitHub uses default
                    };
                    let repo_slug = prompt("    Repository (owner/name): ")?;
                    let token = prompt_secret("    API Token: ")?.to_string();
                    integration_secrets.push(("FORGE_TOKEN", token));
                    (
                        Some(forge_name.to_string()),
                        api_url,
                        if repo_slug.is_empty() {
                            None
                        } else {
                            Some(repo_slug)
                        },
                    )
                } else {
                    (None, None, None)
                };
                devtools_setup = Some(DevToolsSetup {
                    repo_path,
                    forge,
                    forge_api_url,
                    repo,
                });
            }
            10 => {
                println!();
                println!("  Desktop Interaction:");
                println!("    Enables deep control of GUI applications via AT-SPI2");
                println!("    (accessibility), Chrome DevTools Protocol, and input");
                println!("    injection. Requires: at-spi2-core, ydotool (optional).");
                println!();
                println!("    Note: Basic desktop tools (app launching, clipboard,");
                println!("    windows, notifications) are auto-enabled when a display");
                println!("    server is detected. This adds the advanced interaction layer.");
                let confirm = prompt("    Enable deep interaction? (Y/n): ")?;
                desktop_interaction_setup = Some(DesktopInteractionSetup {
                    enabled: !confirm.to_lowercase().starts_with('n'),
                });
            }
            _ => {}
        }
    }

    // ── Step 10: Intelligence (Heartbeat Tuning) ───────────────
    println!();
    println!("  Step 10/10: Agent Intelligence");
    println!();
    println!("  Recommended heartbeat features for {persona_name}:");

    let flag_names = [
        "Self-reflection (update self-model)",
        "Memory consolidation",
        "Failure analysis (learn from errors)",
        "Knowledge extraction (store learned facts)",
        "Plan review (organize goals by horizon)",
        "Strategy review (weekly goal analysis)",
        "Mood tracking (detect user stress)",
        "Encouragement (celebrate achievements)",
        "Milestone tracking (goal anniversaries)",
        "Notification pacing (throttle delivery)",
    ];
    let default_flags = [
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
    ];

    for (i, (name, enabled)) in flag_names.iter().zip(default_flags.iter()).enumerate() {
        let mark = if *enabled { "x" } else { " " };
        println!("    [{mark}] {}: {name}", i + 1);
    }
    println!();

    let hb_choice = prompt("  Accept recommended? (Y/n/custom): ")?;
    let heartbeat_flags: [bool; 10] = match hb_choice.to_lowercase().as_str() {
        "n" | "no" => [false; 10], // minimal — all off
        "custom" | "c" => {
            println!("  Toggle by number (comma-separated, or Enter to finish):");
            let toggles = prompt("  > ")?;
            let mut flags = default_flags;
            for num in toggles.split(',') {
                if let Ok(idx) = num.trim().parse::<usize>()
                    && (1..=10).contains(&idx)
                {
                    flags[idx - 1] = !flags[idx - 1];
                }
            }
            flags
        }
        _ => default_flags, // Y / Enter — accept recommended
    };

    // Override the bundle heartbeat with user's choices
    let final_heartbeat = HeartbeatBundle {
        can_reflect: heartbeat_flags[0],
        can_consolidate_memory: heartbeat_flags[1],
        can_analyze_failures: heartbeat_flags[2],
        can_extract_knowledge: heartbeat_flags[3],
        can_plan_review: heartbeat_flags[4],
        can_strategy_review: heartbeat_flags[5],
        can_track_mood: heartbeat_flags[6],
        can_encourage: heartbeat_flags[7],
        can_track_milestones: heartbeat_flags[8],
        notification_pacing: heartbeat_flags[9],
    };

    // ── Create everything ───────────────────────────────────────
    println!();
    print!("  Setting up...");
    let _ = io::stdout().flush();

    // Create directory structure (0o700 permissions)
    dirs.ensure_dirs()?;

    // Generate master key and encrypt under passphrase
    let master_key = MasterKey::generate();
    let envelope = master_key.encrypt_to_envelope(passphrase.as_bytes())?;
    let envelope_json = serde_json::to_string_pretty(&envelope)?;
    std::fs::write(dirs.master_key_path(), envelope_json)?;

    // Store secrets in encrypted keystore
    let store = EncryptedStore::open(dirs.store_path())?;

    if let Some(ref key) = api_key {
        let key_name = match provider_type {
            "OpenAI" => "OPENAI_API_KEY",
            "Claude" => "ANTHROPIC_API_KEY",
            "OpenAICompatible" => "OPENROUTER_API_KEY",
            _ => "API_KEY",
        };
        store.put(key_name, key.as_bytes(), &master_key)?;
    }

    if let Some(ref email) = email_settings {
        store.put("EMAIL_PASSWORD", email.password.as_bytes(), &master_key)?;
    }

    // Store integration secrets
    if let Some(ref cal) = calendar_setup {
        store.put("CALENDAR_PASSWORD", cal.password.as_bytes(), &master_key)?;
    }
    if let Some(ref con) = contacts_setup {
        store.put("CONTACTS_PASSWORD", con.password.as_bytes(), &master_key)?;
    }
    for (key, secret) in &integration_secrets {
        store.put(key, secret.as_bytes(), &master_key)?;
    }

    // Write config.toml
    // Sanitize user-provided values to prevent TOML injection
    // (e.g., a model name containing `"\n[malicious]` would inject config sections).
    let safe_model = sanitize_toml_value(&model);
    let safe_agent_name = sanitize_toml_value(&agent_name);

    let provider_section = match provider_type {
        "Ollama" => {
            let url = base_url.as_deref().unwrap_or("http://localhost:11434");
            let safe_url = sanitize_toml_value(url);
            format!(
                r#"[provider]
type = "Ollama"
base_url = "{safe_url}"
model = "{safe_model}""#
            )
        }
        "OpenAI" => format!(
            r#"[provider]
type = "OpenAI"
api_key_ref = "OPENAI_API_KEY"
model = "{safe_model}""#
        ),
        "Claude" => format!(
            r#"[provider]
type = "Claude"
api_key_ref = "ANTHROPIC_API_KEY"
model = "{safe_model}""#
        ),
        "OpenAICompatible" => format!(
            r#"[provider]
type = "OpenAICompatible"
api_key_ref = "OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api"
model = "{safe_model}""#
        ),
        _ => unreachable!(),
    };

    // ── Build config.toml incrementally ───────────────────────
    let mut config_toml =
        format!("# Aivyx Personal Assistant Configuration\n\n{provider_section}\n");

    config_toml.push_str("\n[autonomy]\ndefault_tier = \"Trust\"\n");

    // Agent section with optional soul and skills
    config_toml.push_str(&format!(
        "\n[agent]\nname = \"{safe_agent_name}\"\npersona = \"{persona_name}\"\n"
    ));
    if let Some(ref s) = soul {
        config_toml.push_str(&format_soul_line(s));
        config_toml.push('\n');
    }
    if !skills.is_empty() {
        let skill_refs: Vec<&str> = skills.iter().map(|s| s.as_str()).collect();
        config_toml.push_str(&format_skills_line(&skill_refs));
        config_toml.push('\n');
    }

    // Loop config
    config_toml.push_str(&format!(
        "\n[loop]\ncheck_interval_minutes = {check_interval}\nmorning_briefing = true\nbriefing_hour = {briefing_hour}\n"
    ));

    // Heartbeat — use user's tuned flags
    config_toml.push_str(&format_heartbeat_section(&final_heartbeat));

    // Persona dimensions
    config_toml.push_str(&format_persona_section(persona_name));

    // Initial goals
    if include_goals {
        config_toml.push_str(&format_goals_section(bundle.goals));
    }

    // Schedules from persona defaults — skip schedules that require unconfigured integrations.
    if !bundle.schedules.is_empty() {
        let mut configured_integrations: Vec<&str> = Vec::new();
        if email_settings.is_some() {
            configured_integrations.push("email");
        }
        if calendar_setup.is_some() {
            configured_integrations.push("calendar");
        }
        if contacts_setup.is_some() {
            configured_integrations.push("contacts");
        }
        if telegram_setup.is_some() {
            configured_integrations.push("telegram");
        }
        if matrix_setup.is_some() {
            configured_integrations.push("matrix");
        }
        if signal_setup.is_some() {
            configured_integrations.push("signal");
        }
        if sms_setup.is_some() {
            configured_integrations.push("sms");
        }
        if vault_setup.is_some() {
            configured_integrations.push("vault");
        }
        if finance_setup.is_some() {
            configured_integrations.push("finance");
        }
        if devtools_setup.is_some() {
            configured_integrations.push("devtools");
        }
        config_toml.push_str(&format_schedules_section(
            bundle.schedules,
            &agent_name,
            &configured_integrations,
        ));
    }

    // Email
    if let Some(ref email) = email_settings {
        config_toml.push_str(&format!(
            "\n[email]\nimap_host = \"{}\"\nimap_port = {}\nsmtp_host = \"{}\"\nsmtp_port = {}\naddress = \"{}\"\nusername = \"{}\"\n# Password stored encrypted as EMAIL_PASSWORD\n",
            sanitize_toml_value(&email.imap_host), email.imap_port,
            sanitize_toml_value(&email.smtp_host), email.smtp_port,
            sanitize_toml_value(&email.address), sanitize_toml_value(&email.username),
        ));
    }

    // Integration sections
    if let Some(ref cal) = calendar_setup {
        config_toml.push_str(&format!(
            "\n[calendar]\nurl = \"{}\"\nusername = \"{}\"\n# Password stored encrypted as CALENDAR_PASSWORD\n",
            sanitize_toml_value(&cal.url),
            sanitize_toml_value(&cal.username),
        ));
    }
    if let Some(ref con) = contacts_setup {
        config_toml.push_str(&format!(
            "\n[contacts]\nurl = \"{}\"\nusername = \"{}\"\n# Password stored encrypted as CONTACTS_PASSWORD\n",
            sanitize_toml_value(&con.url),
            sanitize_toml_value(&con.username),
        ));
    }
    if let Some(ref tg) = telegram_setup {
        config_toml.push_str("\n[telegram]\n# Bot token stored encrypted as TELEGRAM_BOT_TOKEN\n");
        if let Some(ref chat_id) = tg.default_chat_id {
            config_toml.push_str(&format!(
                "default_chat_id = \"{}\"\n",
                sanitize_toml_value(chat_id),
            ));
        }
    }
    if let Some(ref mx) = matrix_setup {
        config_toml.push_str(&format!(
            "\n[matrix]\nhomeserver = \"{}\"\n# Access token stored encrypted as MATRIX_ACCESS_TOKEN\n",
            sanitize_toml_value(&mx.homeserver),
        ));
        if let Some(ref room_id) = mx.default_room_id {
            config_toml.push_str(&format!(
                "default_room_id = \"{}\"\n",
                sanitize_toml_value(room_id),
            ));
        }
    }
    if let Some(ref sig) = signal_setup {
        config_toml.push_str(&format!(
            "\n[signal]\naccount = \"{}\"\nsocket_path = \"{}\"\n",
            sanitize_toml_value(&sig.account),
            sanitize_toml_value(&sig.socket_path),
        ));
    }
    if let Some(ref sms) = sms_setup {
        config_toml.push_str(&format!(
            "\n[sms]\nprovider = \"{}\"\naccount_id = \"{}\"\nfrom_number = \"{}\"\n# Auth token stored encrypted as SMS_AUTH_TOKEN\n",
            sanitize_toml_value(&sms.provider),
            sanitize_toml_value(&sms.account_id),
            sanitize_toml_value(&sms.from_number),
        ));
    }
    if let Some(ref v) = vault_setup {
        config_toml.push_str(&format!(
            "\n[vault]\npath = \"{}\"\n# extensions = [\"md\", \"txt\", \"pdf\"]\n",
            sanitize_toml_value(&v.path),
        ));
    }
    if let Some(ref f) = finance_setup {
        config_toml.push_str(&format!(
            "\n[finance]\nenabled = true\nreceipt_folder = \"{}\"\n",
            sanitize_toml_value(&f.receipt_folder),
        ));
    }
    if let Some(ref dt) = devtools_setup {
        config_toml.push_str(&format!(
            "\n[devtools]\nrepo_path = \"{}\"\n",
            sanitize_toml_value(&dt.repo_path),
        ));
        if let Some(ref forge) = dt.forge {
            config_toml.push_str(&format!("forge = \"{}\"\n", sanitize_toml_value(forge),));
        }
        if let Some(ref url) = dt.forge_api_url {
            config_toml.push_str(&format!(
                "forge_api_url = \"{}\"\n",
                sanitize_toml_value(url),
            ));
        }
        if let Some(ref repo) = dt.repo {
            config_toml.push_str(&format!(
                "repo = \"{}\"\n# Forge token stored encrypted as FORGE_TOKEN\n",
                sanitize_toml_value(repo),
            ));
        }
    }

    // Desktop section — always write when a display server is present,
    // even without deep interaction, so users can see and modify it.
    let has_display = std::env::var("DISPLAY").is_ok() || std::env::var("WAYLAND_DISPLAY").is_ok();
    if has_display || desktop_interaction_setup.is_some() {
        config_toml.push_str("\n[desktop]\n# Auto-enabled: display server detected\nclipboard = true\nwindows = true\nnotifications = true\n");

        // Determine whether to enable deep interaction:
        // - User explicitly chose option 10 → respect their choice (yes or no)
        // - User skipped option 10, but display server is present → auto-enable
        //   (AT-SPI2/CDP/ydotool are now compiled in; runtime deps may still be needed)
        let enable_interaction = match &desktop_interaction_setup {
            Some(di) => di.enabled, // explicit user choice always wins
            None => has_display,    // auto-enable when display detected
        };
        if enable_interaction {
            config_toml.push_str(concat!(
                "\n[desktop.interaction]\n",
                "enabled = true\n",
                "# Backends (all enabled by default — disable if not installed):\n",
                "# [desktop.interaction.accessibility]  # AT-SPI2: requires at-spi2-core\n",
                "# enabled = true\n",
                "# [desktop.interaction.browser]        # CDP: requires Chrome --remote-debugging-port=9222\n",
                "# enabled = true\n",
                "# debug_port = 9222\n",
                "# [desktop.interaction.media]          # MPRIS: requires D-Bus + media player\n",
                "# enabled = true\n",
                "# [desktop.interaction.input]          # ydotool: requires ydotoold daemon\n",
                "# enabled = true\n",
            ));
        }
    }

    std::fs::write(dirs.config_path(), config_toml)?;

    println!(" Done!");
    println!();
    println!("  Name:      {agent_name}");
    println!("  Persona:   {persona_name}");
    println!("  Provider:  {provider_type}");
    println!("  Model:     {model}");
    println!("  Briefing:  {briefing_hour}:00");
    println!("  Interval:  every {check_interval} min");
    if !skills.is_empty() {
        println!("  Skills:    {} total", skills.len());
    }
    if include_goals {
        println!("  Goals:     {} seeded", bundle.goals.len());
    }
    if !bundle.schedules.is_empty() {
        println!("  Schedules: {} configured", bundle.schedules.len());
    }
    if let Some(ref email) = email_settings {
        println!("  Email:     {}", email.address);
    }
    let integration_count = [
        calendar_setup.is_some(),
        contacts_setup.is_some(),
        telegram_setup.is_some(),
        matrix_setup.is_some(),
        signal_setup.is_some(),
        sms_setup.is_some(),
        vault_setup.is_some(),
        finance_setup.is_some(),
        devtools_setup.is_some(),
        desktop_interaction_setup.is_some(),
    ]
    .iter()
    .filter(|&&x| x)
    .count();
    if integration_count > 0 {
        println!("  Integrations: {integration_count} configured");
    }
    if has_display {
        println!("  Desktop:   auto-enabled (display server detected)");
    }
    let enabled_hb = heartbeat_flags.iter().filter(|&&x| x).count();
    println!("  Heartbeat: {enabled_hb}/10 features enabled");
    println!("  Data:      {}", home.display());
    println!();

    Ok(passphrase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PaConfig;
    use crate::persona_defaults;

    /// Build a minimal valid config TOML, append a fragment, and parse as PaConfig.
    fn parse_with_fragment(fragment: &str) -> PaConfig {
        let toml = format!(
            r#"
[agent]
name = "test"
persona = "assistant"
{fragment}
"#
        );
        toml::from_str::<PaConfig>(&toml)
            .unwrap_or_else(|e| panic!("Failed to parse TOML:\n{toml}\nError: {e}"))
    }

    #[test]
    fn heartbeat_section_roundtrips() {
        let bundle = persona_defaults::for_persona("coder");
        let fragment = format_heartbeat_section(&bundle.heartbeat);
        let config = parse_with_fragment(&fragment);
        let hb = config.heartbeat.expect("heartbeat section missing");
        assert!(hb.enabled);
        assert_eq!(hb.can_reflect, bundle.heartbeat.can_reflect);
        assert_eq!(
            hb.can_analyze_failures,
            bundle.heartbeat.can_analyze_failures
        );
        assert_eq!(hb.can_track_mood, bundle.heartbeat.can_track_mood);
        assert_eq!(hb.notification_pacing, bundle.heartbeat.notification_pacing);
    }

    #[test]
    fn goals_section_roundtrips() {
        let bundle = persona_defaults::for_persona("coach");
        let fragment = format_goals_section(bundle.goals);
        let config = parse_with_fragment(&fragment);
        assert_eq!(config.initial_goals.len(), bundle.goals.len());
        assert_eq!(
            config.initial_goals[0].description,
            bundle.goals[0].description
        );
        assert_eq!(config.initial_goals[0].priority, bundle.goals[0].priority);
    }

    #[test]
    fn schedules_section_produces_valid_toml() {
        let bundle = persona_defaults::for_persona("ops");
        // Pass all integrations as configured so no schedules are filtered.
        let all = &["email", "calendar", "contacts", "devtools", "vault"];
        let fragment = format_schedules_section(bundle.schedules, "sentinel", all);
        let config = parse_with_fragment(&fragment);
        assert_eq!(config.schedules.len(), bundle.schedules.len());
        // Schedule names are prefixed with agent name
        assert!(config.schedules[0].name.starts_with("sentinel-"));
    }

    #[test]
    fn schedules_filtered_when_integration_missing() {
        let bundle = persona_defaults::for_persona("coder");
        // Coder has 3 schedules, all requiring "devtools".
        // With no devtools configured, all should be filtered out.
        let none: &[&str] = &[];
        let fragment = format_schedules_section(bundle.schedules, "test", none);
        let config = parse_with_fragment(&fragment);
        assert_eq!(
            config.schedules.len(),
            0,
            "All coder schedules need devtools"
        );

        // With devtools configured, all should be included.
        let with_devtools = &["devtools"];
        let fragment = format_schedules_section(bundle.schedules, "test", with_devtools);
        let config = parse_with_fragment(&fragment);
        assert_eq!(
            config.schedules.len(),
            3,
            "All coder schedules should be included with devtools"
        );
    }

    #[test]
    fn sms_section_roundtrips() {
        let fragment = r#"
[sms]
provider = "twilio"
account_id = "AC1234"
from_number = "+15551234567"
"#;
        let config = parse_with_fragment(fragment);
        let sms = config.sms.expect("sms section missing");
        assert_eq!(sms.account_id, "AC1234");
        assert_eq!(sms.from_number, "+15551234567");
    }

    #[test]
    fn vault_section_roundtrips() {
        let fragment = r#"
[vault]
path = "~/Documents/vault"
"#;
        let config = parse_with_fragment(fragment);
        let vault = config.vault.expect("vault section missing");
        assert_eq!(vault.path, "~/Documents/vault");
    }

    #[test]
    fn finance_section_roundtrips() {
        let fragment = r#"
[finance]
enabled = true
receipt_folder = "receipts"
"#;
        let config = parse_with_fragment(fragment);
        let finance = config.finance.expect("finance section missing");
        assert!(finance.enabled);
        assert_eq!(finance.receipt_folder, "receipts");
    }

    #[test]
    fn devtools_section_roundtrips() {
        let fragment = r#"
[devtools]
repo_path = "/home/user/project"
forge = "github"
repo = "owner/repo"
"#;
        let config = parse_with_fragment(fragment);
        let dt = config.devtools.expect("devtools section missing");
        assert_eq!(dt.repo_path, "/home/user/project");
        assert!(dt.forge.is_some());
        assert_eq!(dt.repo.as_deref(), Some("owner/repo"));
    }

    #[test]
    fn desktop_section_roundtrips() {
        let fragment = r#"
[desktop]
clipboard = true
windows = true
notifications = true
"#;
        let config = parse_with_fragment(fragment);
        let desktop = config.desktop.expect("desktop section missing");
        assert!(desktop.clipboard);
        assert!(desktop.windows);
        assert!(desktop.notifications);
    }

    #[test]
    fn desktop_interaction_section_roundtrips() {
        let fragment = r#"
[desktop]
clipboard = true
windows = true
notifications = true

[desktop.interaction]
enabled = true
"#;
        let config = parse_with_fragment(fragment);
        let desktop = config.desktop.expect("desktop section missing");
        let interaction = desktop.interaction.expect("interaction section missing");
        assert!(interaction.enabled);
    }

    #[test]
    fn lint_detects_unknown_section() {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "[agent]\nname = \"test\"\n\n[emal]\nimap_host = \"x\"\n").unwrap();
        let warnings = PaConfig::lint(f.path(), None, None);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("Unknown section [emal]"))
        );
        assert!(warnings.iter().any(|w| w.contains("did you mean [email]?")));
    }

    #[test]
    fn lint_warns_nonstandard_port() {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "[email]\nimap_port = 1234\nsmtp_port = 9999\n").unwrap();
        let warnings = PaConfig::lint(f.path(), None, None);
        assert!(warnings.iter().any(|w| w.contains("imap_port = 1234")));
        assert!(warnings.iter().any(|w| w.contains("smtp_port = 9999")));
    }

    #[test]
    fn lint_warns_fast_heartbeat() {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "[heartbeat]\ninterval_minutes = 2\n").unwrap();
        let warnings = PaConfig::lint(f.path(), None, None);
        assert!(warnings.iter().any(|w| w.contains("interval_minutes = 2")));
    }

    #[test]
    fn lint_clean_config_no_warnings() {
        use std::io::Write as _;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        write!(f, "[agent]\nname = \"test\"\npersona = \"assistant\"\n\n[heartbeat]\nenabled = true\ninterval_minutes = 30\n").unwrap();
        let warnings = PaConfig::lint(f.path(), None, None);
        assert!(
            warnings.is_empty(),
            "Expected no warnings, got: {:?}",
            warnings
        );
    }

    /// End-to-end test: build a full genesis config for every persona with
    /// all integrations enabled, then parse through both AivyxConfig and
    /// PaConfig and verify the SettingsSnapshot.
    #[test]
    fn genesis_config_end_to_end_all_personas() {
        use crate::settings::reload_settings_snapshot;
        use aivyx_config::AivyxConfig;

        let personas = [
            "assistant",
            "coder",
            "researcher",
            "writer",
            "coach",
            "companion",
            "ops",
            "analyst",
        ];
        let providers = [
            (
                "Ollama",
                r#"type = "Ollama"
base_url = "http://localhost:11434"
model = "qwen3:8b""#,
            ),
            (
                "Claude",
                r#"type = "Claude"
api_key_ref = "ANTHROPIC_API_KEY"
model = "claude-sonnet-4-20250514""#,
            ),
            (
                "OpenAI",
                r#"type = "OpenAI"
api_key_ref = "OPENAI_API_KEY"
model = "gpt-4o""#,
            ),
            (
                "OpenAICompatible",
                r#"type = "OpenAICompatible"
api_key_ref = "OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api"
model = "deepseek/deepseek-r1""#,
            ),
        ];

        for persona in &personas {
            for (provider_label, provider_toml) in &providers {
                let bundle = persona_defaults::for_persona(persona);
                let safe_agent = "TestAgent";

                // Build config.toml exactly as genesis does
                let mut config = format!(
                    "# Genesis E2E test: {persona} + {provider_label}\n\n[provider]\n{provider_toml}\n"
                );

                config.push_str("\n[autonomy]\ndefault_tier = \"Trust\"\n");

                // Agent
                config.push_str(&format!(
                    "\n[agent]\nname = \"{safe_agent}\"\npersona = \"{persona}\"\n"
                ));
                config.push_str(&format_soul_line(bundle.soul_template));
                config.push('\n');
                let skills: Vec<&str> = bundle.skills.to_vec();
                config.push_str(&format_skills_line(&skills));
                config.push('\n');

                // Loop
                config.push_str("\n[loop]\ncheck_interval_minutes = 15\nmorning_briefing = true\nbriefing_hour = 8\n");

                // Heartbeat
                config.push_str(&format_heartbeat_section(&bundle.heartbeat));

                // Persona dimensions
                config.push_str(&format_persona_section(persona));

                // Goals
                config.push_str(&format_goals_section(bundle.goals));

                // Schedules — all integrations "configured" so nothing is skipped
                let all_integrations = vec![
                    "email", "calendar", "contacts", "telegram", "matrix", "signal", "sms",
                    "vault", "finance", "devtools",
                ];
                config.push_str(&format_schedules_section(
                    bundle.schedules,
                    safe_agent,
                    &all_integrations,
                ));

                // All integration sections
                config.push_str(
                    r#"
[email]
imap_host = "imap.example.com"
imap_port = 993
smtp_host = "smtp.example.com"
smtp_port = 587
address = "user@example.com"
username = "user@example.com"
# Password stored encrypted as EMAIL_PASSWORD

[calendar]
url = "https://dav.example.com/calendars/user"
username = "user"
# Password stored encrypted as CALENDAR_PASSWORD

[contacts]
url = "https://dav.example.com/contacts/user"
username = "user"
# Password stored encrypted as CONTACTS_PASSWORD

[telegram]
# Bot token stored encrypted as TELEGRAM_BOT_TOKEN
default_chat_id = "123456789"

[matrix]
homeserver = "https://matrix.example.com"
# Access token stored encrypted as MATRIX_ACCESS_TOKEN
default_room_id = "!abc:example.com"

[signal]
account = "+1234567890"
socket_path = "/run/signald/signald.sock"

[sms]
provider = "twilio"
account_id = "AC1234"
from_number = "+15555555555"
# Auth token stored encrypted as SMS_AUTH_TOKEN

[vault]
path = "~/Documents/vault"
# extensions = ["md", "txt", "pdf"]

[finance]
enabled = true
receipt_folder = "~/receipts"

[devtools]
repo_path = "/home/user/project"
forge = "github"
repo = "user/project"
# Forge token stored encrypted as FORGE_TOKEN

[desktop]
# Auto-enabled: display server detected
clipboard = true
windows = true
notifications = true
"#,
                );

                // ── Step 1: Verify raw TOML parses ──────────────
                let core_result = toml::from_str::<AivyxConfig>(&config);
                assert!(
                    core_result.is_ok(),
                    "AivyxConfig parse FAILED for {persona}+{provider_label}:\n{}\nConfig:\n{config}",
                    core_result.unwrap_err()
                );

                let pa_result = toml::from_str::<PaConfig>(&config);
                assert!(
                    pa_result.is_ok(),
                    "PaConfig parse FAILED for {persona}+{provider_label}:\n{}\nConfig:\n{config}",
                    pa_result.unwrap_err()
                );

                // ── Step 2: Verify via reload_settings_snapshot ─
                let mut f = tempfile::NamedTempFile::new().unwrap();
                std::io::Write::write_all(&mut f, config.as_bytes()).unwrap();
                let snapshot = reload_settings_snapshot(f.path());
                assert!(
                    snapshot.is_ok(),
                    "reload_settings_snapshot FAILED for {persona}+{provider_label}: {}",
                    snapshot.unwrap_err()
                );
                let s = snapshot.unwrap();

                // ── Step 3: Verify snapshot fields ──────────────
                assert_eq!(
                    s.agent_name, "TestAgent",
                    "{persona}+{provider_label}: agent_name"
                );
                assert_eq!(
                    s.agent_persona, *persona,
                    "{persona}+{provider_label}: persona"
                );
                assert_eq!(
                    s.provider_label,
                    match *provider_label {
                        "Ollama" => "Ollama",
                        "Claude" => "Claude",
                        "OpenAI" => "OpenAI",
                        "OpenAICompatible" => "OpenAI-Compatible",
                        _ => panic!("unknown provider"),
                    },
                    "{persona}+{provider_label}: provider_label"
                );
                assert!(
                    !s.agent_skills.is_empty(),
                    "{persona}+{provider_label}: skills should not be empty"
                );
                assert!(
                    s.has_custom_soul,
                    "{persona}+{provider_label}: should have custom soul"
                );
                assert_eq!(
                    s.autonomy_tier, "Trust",
                    "{persona}+{provider_label}: autonomy tier"
                );

                // Heartbeat
                assert!(
                    s.heartbeat_enabled,
                    "{persona}+{provider_label}: heartbeat should be enabled"
                );
                assert_eq!(s.heartbeat_interval, 30);

                // Persona dimensions present
                assert!(
                    s.persona_dimensions.is_some(),
                    "{persona}+{provider_label}: persona dimensions"
                );

                // Loop
                assert!(s.morning_briefing);
                assert_eq!(s.briefing_hour, 8);
                assert_eq!(s.loop_check_interval, 15);

                // Schedules (may vary by persona but should have at least one)
                // Some personas have no schedules with requires="" so count may vary

                // Integrations — all should be configured
                assert!(s.email_configured, "{persona}+{provider_label}: email");
                assert_eq!(s.email_address.as_deref(), Some("user@example.com"));
                assert!(
                    s.calendar_configured,
                    "{persona}+{provider_label}: calendar"
                );
                assert!(
                    s.contacts_configured,
                    "{persona}+{provider_label}: contacts"
                );
                assert!(
                    s.telegram_configured,
                    "{persona}+{provider_label}: telegram"
                );
                assert!(s.matrix_configured, "{persona}+{provider_label}: matrix");
                assert!(s.matrix_homeserver.as_deref() == Some("https://matrix.example.com"));
                assert!(s.signal_configured, "{persona}+{provider_label}: signal");
                assert!(s.sms_configured, "{persona}+{provider_label}: sms");
                assert!(s.vault_configured, "{persona}+{provider_label}: vault");
                assert!(s.finance_configured, "{persona}+{provider_label}: finance");
                assert!(
                    s.devtools_configured,
                    "{persona}+{provider_label}: devtools"
                );
                assert!(s.desktop_configured, "{persona}+{provider_label}: desktop");
            }
        }
    }

    /// Verify that a minimal genesis config (no integrations) also parses.
    #[test]
    fn genesis_config_minimal_no_integrations() {
        use aivyx_config::AivyxConfig;

        let config = r#"# Minimal genesis
[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "qwen3:8b"

[autonomy]
default_tier = "Trust"

[agent]
name = "Aria"
persona = "assistant"
"#;
        let core: AivyxConfig =
            toml::from_str(config).unwrap_or_else(|e| panic!("AivyxConfig failed: {e}"));
        let pa: PaConfig =
            toml::from_str(config).unwrap_or_else(|e| panic!("PaConfig failed: {e}"));

        assert!(matches!(
            core.provider,
            aivyx_config::ProviderConfig::Ollama { .. }
        ));
        assert_eq!(pa.agent.unwrap().name, "Aria");
        assert!(pa.email.is_none());
        assert!(pa.desktop.is_none());
    }
}
