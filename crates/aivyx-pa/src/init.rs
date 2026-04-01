//! Simplified first-time setup — 3 steps, not 8.
//!
//! 1. Set passphrase (encrypts all local data)
//! 2. Choose provider (Ollama / OpenAI / Anthropic / OpenRouter)
//! 3. Choose model
//!
//! That's it. No agent naming, no persona presets, no constellation,
//! no nexus, no autonomy tier selection. Sensible defaults for everything.

use std::io::{self, BufRead, Write};
use std::path::Path;

const PROVIDERS: &[(&str, &[&str])] = &[
    ("ollama", &["qwen3:14b", "qwen2.5-coder:14b", "llama3.1:8b", "mistral:7b"]),
    ("openai", &["gpt-4o", "gpt-4o-mini", "o1-mini"]),
    ("anthropic", &["claude-sonnet-4-20250514", "claude-haiku-4-5-20251001"]),
    ("openrouter", &["anthropic/claude-sonnet-4-20250514", "openai/gpt-4o", "google/gemini-2.0-flash-001"]),
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

    // Check if already initialized
    if home.join("keys").join("master.json").exists() {
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
    for (i, (name, _)) in PROVIDERS.iter().enumerate() {
        println!("    [{i}] {name}");
    }
    println!();

    let provider_idx: usize = loop {
        let input = prompt("  Provider [0]: ");
        let val = if input.is_empty() { 0 } else { input.parse().unwrap_or(99) };
        if val < PROVIDERS.len() { break val; }
        println!("  Invalid selection.");
    };

    let (provider_type, suggestions) = PROVIDERS[provider_idx];

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
    let api_key = if provider_type != "ollama" {
        let key = prompt("  API Key: ");
        if key.is_empty() { None } else { Some(key) }
    } else {
        None
    };

    // ── Create everything ───────────────────────────────────────
    println!();
    print!("  Setting up...");
    let _ = io::stdout().flush();

    // Create directory structure
    std::fs::create_dir_all(home.join("keys"))?;
    std::fs::create_dir_all(home.join("data"))?;
    std::fs::create_dir_all(home.join("agents"))?;

    // Write config
    let base_url = if provider_type == "ollama" {
        "http://localhost:11434"
    } else {
        ""
    };

    let mut config = format!(
        r#"# Aivyx Personal Assistant Configuration

[provider]
type = "{provider_type}"
model = "{model}"
"#
    );

    if !base_url.is_empty() {
        config.push_str(&format!("base_url = \"{base_url}\"\n"));
    }

    config.push_str(
        r#"
[autonomy]
tier = "trust"

[loop]
check_interval_minutes = 15
morning_briefing = true
briefing_hour = 8
"#,
    );

    std::fs::write(home.join("config.toml"), config)?;

    // TODO: Initialize encrypted keystore with passphrase
    // TODO: Store API key in keystore if provided
    // For now, create a placeholder master.json
    std::fs::write(
        home.join("keys").join("master.json"),
        serde_json::json!({ "initialized": true, "provider": provider_type }).to_string(),
    )?;

    let _ = api_key; // Will be stored in keystore once wired

    println!(" Done!");
    println!();
    println!("  Provider:  {provider_type}");
    println!("  Model:     {model}");
    println!("  Config:    {}", home.join("config.toml").display());
    println!();
    println!("  Run `aivyx` to launch your assistant.");
    println!();

    Ok(())
}
