//! Aivyx Personal Assistant — main entry point.
//!
//! Usage:
//!   aivyx              Launch TUI (default)
//!   aivyx init         First-time setup
//!   aivyx chat "..."   One-shot chat from terminal
//!   aivyx status       Show what the assistant has been doing
//!   aivyx config       View/edit configuration

mod init;
mod tui;

use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{EncryptedStore, MasterKey, MasterKeyEnvelope};
use aivyx_llm::create_provider;
use clap::{Parser, Subcommand};
use std::io::{self, BufRead, Write};

#[derive(Parser)]
#[command(
    name = "aivyx",
    about = "Your private AI personal assistant",
    version,
    after_help = "Run without arguments to launch the interactive TUI."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// First-time setup: passphrase, provider, model
    Init,
    /// One-shot chat (non-interactive)
    Chat {
        /// Your message
        message: String,
    },
    /// Show recent assistant activity
    Status,
    /// View or edit configuration
    Config,
}

/// Prompt for passphrase (no echo in future, plain for now).
fn read_passphrase(msg: &str) -> String {
    eprint!("{msg}");
    let _ = io::stderr().flush();
    let mut input = String::new();
    if io::stdin().lock().read_line(&mut input).is_err() {
        eprintln!("\nError reading input.");
        std::process::exit(1);
    }
    input.trim().to_string()
}

/// Unlock the master key from the encrypted envelope on disk.
fn unlock(dirs: &AivyxDirs) -> anyhow::Result<MasterKey> {
    let envelope_json = std::fs::read_to_string(dirs.master_key_path())?;
    let envelope: MasterKeyEnvelope = serde_json::from_str(&envelope_json)?;

    // Check for AIVYX_PASSPHRASE env var first (for non-interactive use)
    let passphrase = std::env::var("AIVYX_PASSPHRASE")
        .unwrap_or_else(|_| read_passphrase("Passphrase: "));

    let master_key = MasterKey::decrypt_from_envelope(passphrase.as_bytes(), &envelope)
        .map_err(|_| anyhow::anyhow!("Wrong passphrase."))?;

    Ok(master_key)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    let dirs = AivyxDirs::from_default()?;

    match cli.command {
        Some(Command::Init) => {
            init::run(dirs.root()).await?;
        }

        // No subcommand → launch TUI
        None => {
            if !dirs.is_initialized() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }

            let master_key = unlock(&dirs)?;
            let config = AivyxConfig::load(dirs.config_path())?;
            let store = EncryptedStore::open(dirs.store_path())?;
            let provider = create_provider(&config.provider, &store, &master_key)?;

            tui::run(&dirs, config, master_key, provider).await?;
        }

        Some(Command::Chat { message }) => {
            if !dirs.is_initialized() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }

            let master_key = unlock(&dirs)?;
            let config = AivyxConfig::load(dirs.config_path())?;
            let store = EncryptedStore::open(dirs.store_path())?;
            let provider = create_provider(&config.provider, &store, &master_key)?;

            chat_oneshot(&dirs, config, master_key, provider, &message).await?;
        }

        Some(Command::Status) => {
            if !dirs.is_initialized() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }
            println!("(status view not yet implemented)");
        }

        Some(Command::Config) => {
            if !dirs.is_initialized() {
                eprintln!("Aivyx is not set up yet. Run `aivyx init` first.");
                std::process::exit(1);
            }
            let config = AivyxConfig::load(dirs.config_path())?;
            println!("{:#?}", config.provider);
        }
    }

    Ok(())
}

/// One-shot chat: send a message, print the response, exit.
async fn chat_oneshot(
    dirs: &AivyxDirs,
    _config: AivyxConfig,
    master_key: MasterKey,
    provider: Box<dyn aivyx_llm::LlmProvider>,
    message: &str,
) -> anyhow::Result<()> {
    let mut agent = crate::tui::build_agent(dirs, master_key, provider)?;
    let response = agent.turn(message, None).await?;
    println!("{response}");
    Ok(())
}
