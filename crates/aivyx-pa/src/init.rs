//! Simplified first-time setup — 3 steps, not 8.
//!
//! 1. Set passphrase (encrypts all local data)
//! 2. Choose provider (Ollama / OpenAI / Anthropic / OpenRouter)
//! 3. Choose model
//!
//! That's it. No agent naming, no persona presets, no constellation,
//! no nexus, no autonomy tier selection. Sensible defaults for everything.

use aivyx_config::AivyxDirs;
use aivyx_crypto::{EncryptedStore, MasterKey};
use std::io::{self, BufRead, Write};
use std::path::Path;

const PROVIDERS: &[(&str, &str, &[&str])] = &[
    ("Ollama", "ollama", &["qwen3:14b", "qwen2.5-coder:14b", "llama3.1:8b", "mistral:7b"]),
    ("OpenAI", "openai", &["gpt-4o", "gpt-4o-mini", "o1-mini"]),
    ("Claude", "anthropic", &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"]),
    ("OpenAICompatible", "openrouter", &["anthropic/claude-sonnet-4-20250514", "openai/gpt-4o", "google/gemini-2.0-flash-001"]),
];

fn prompt(msg: &str) -> String {
    print!("{msg}");
    let _ = io::stdout().flush();
    let mut input = String::new();
    if io::stdin().lock().read_line(&mut input).is_err() {
        eprintln!("\nError reading input.");
        std::process::exit(1);
    }
    input.trim().to_string()
}

fn prompt_default(msg: &str, default: &str) -> String {
    let input = prompt(&format!("{msg} [{default}]: "));
    if input.is_empty() { default.to_string() } else { input }
}

pub async fn run(home: &Path) -> anyhow::Result<()> {
    println!();
    println!("  Welcome to Aivyx");
    println!("  Your private AI personal assistant.");
    println!();

    let dirs = AivyxDirs::new(home);

    // Check if already initialized
    if dirs.is_initialized() {
        let answer = prompt("  Already initialized. Re-run setup? (y/N): ");
        if !answer.to_lowercase().starts_with('y') {
            println!("  Setup cancelled.");
            return Ok(());
        }
    }

    // ── Step 1: Passphrase ──────────────────────────────────────
    println!("  Step 1/3: Set a passphrase");
    println!("  This encrypts all your local data. Choose something memorable.");
    println!();

    let passphrase = prompt("  Passphrase: ");
    if passphrase.len() < 4 {
        eprintln!("  Passphrase too short (minimum 4 characters).");
        std::process::exit(1);
    }

    let confirm = prompt("  Confirm: ");
    if passphrase != confirm {
        eprintln!("  Passphrases don't match.");
        std::process::exit(1);
    }

    // ── Step 2: Provider ────────────────────────────────────────
    println!();
    println!("  Step 2/3: Choose your LLM provider");
    println!();
    for (i, (_, label, _)) in PROVIDERS.iter().enumerate() {
        println!("    [{i}] {label}");
    }
    println!();

    let provider_idx: usize = loop {
        let input = prompt("  Provider [0]: ");
        let val = if input.is_empty() { 0 } else { input.parse().unwrap_or(99) };
        if val < PROVIDERS.len() { break val; }
        println!("  Invalid selection.");
    };

    let (provider_type, _label, suggestions) = PROVIDERS[provider_idx];

    // ── Step 3: Model ───────────────────────────────────────────
    println!();
    if !suggestions.is_empty() {
        println!("  Step 3/3: Choose a model");
        println!();
        for (i, model) in suggestions.iter().enumerate() {
            println!("    [{i}] {model}");
        }
        println!();
    }

    let model = if !suggestions.is_empty() {
        let input = prompt_default("  Model", suggestions[0]);
        if let Ok(idx) = input.parse::<usize>() {
            if idx < suggestions.len() {
                suggestions[idx].to_string()
            } else {
                input
            }
        } else {
            input
        }
    } else {
        prompt("  Model name: ")
    };

    // API key for cloud providers
    let api_key = if provider_type != "Ollama" {
        let key = prompt("  API Key: ");
        if key.is_empty() { None } else { Some(key) }
    } else {
        None
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

    // Store API key in encrypted keystore if provided
    if let Some(ref key) = api_key {
        let store = EncryptedStore::open(dirs.store_path())?;
        let key_name = match provider_type {
            "OpenAI" => "OPENAI_API_KEY",
            "Claude" => "ANTHROPIC_API_KEY",
            "OpenAICompatible" => "OPENROUTER_API_KEY",
            _ => "API_KEY",
        };
        store.put(key_name, key.as_bytes(), &master_key)?;
    }

    // Write config.toml (using exact enum variant names for serde(tag = "type"))
    let config_toml = match provider_type {
        "Ollama" => format!(
            r#"# Aivyx Personal Assistant Configuration

[provider]
type = "Ollama"
base_url = "http://localhost:11434"
model = "{model}"

[autonomy]
default_tier = "Trust"
"#
        ),
        "OpenAI" => format!(
            r#"# Aivyx Personal Assistant Configuration

[provider]
type = "OpenAI"
api_key_ref = "OPENAI_API_KEY"
model = "{model}"

[autonomy]
default_tier = "Trust"
"#
        ),
        "Claude" => format!(
            r#"# Aivyx Personal Assistant Configuration

[provider]
type = "Claude"
api_key_ref = "ANTHROPIC_API_KEY"
model = "{model}"

[autonomy]
default_tier = "Trust"
"#
        ),
        "OpenAICompatible" => format!(
            r#"# Aivyx Personal Assistant Configuration

[provider]
type = "OpenAICompatible"
api_key_ref = "OPENROUTER_API_KEY"
base_url = "https://openrouter.ai/api"
model = "{model}"

[autonomy]
default_tier = "Trust"
"#
        ),
        _ => unreachable!(),
    };

    std::fs::write(dirs.config_path(), config_toml)?;

    println!(" Done!");
    println!();
    println!("  Provider:  {provider_type}");
    println!("  Model:     {model}");
    println!("  Data:      {}", home.display());
    println!();
    println!("  Run `aivyx` to launch your assistant.");
    println!();

    Ok(())
}
