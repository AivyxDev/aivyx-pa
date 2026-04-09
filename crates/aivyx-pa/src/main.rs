#![allow(clippy::too_many_arguments)]

//! Aivyx Personal Assistant — main entry point.
//!
//! Usage:
//!   aivyx              Launch API server (default)
//!   aivyx init         First-time setup
//!   aivyx chat "..."   One-shot chat from terminal
//!   aivyx status       Show what the assistant has been doing
//!   aivyx config       View/edit configuration
//!   aivyx serve        Start HTTP API server (alias for default)

use aivyx_pa::agent;
use aivyx_pa::config;
use aivyx_pa::init;

use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{EncryptedStore, MasterKey, MasterKeyEnvelope};
use aivyx_llm::{
    CachingProvider, CircuitBreakerConfig, ComplexityLevel, LlmProvider, ProviderEvent,
    ResilientProvider, RoutingProvider, create_embedding_provider, create_provider,
};
use clap::{Parser, Subcommand};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    name = "aivyx",
    about = "Your private AI personal assistant",
    version,
    after_help = "Run without arguments to launch the API server on port 3100."
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
    /// Rotate the master encryption key (re-encrypts all secrets)
    RotateKey,
    /// Start the HTTP API server (headless, for frontend clients)
    Serve {
        /// Port to listen on (default: 3100)
        #[arg(short, long, default_value_t = 3100)]
        port: u16,
    },
}

/// Initialize tracing.
///
/// - **Server mode** (default / `serve`): logs to `~/.aivyx/pa.log` so
///   structured output isn't mixed with stderr.
/// - **CLI mode** (`chat`, `status`, etc.): logs to stderr as usual.
///
/// Returns a guard that must be held for the lifetime of the program
/// (dropping it flushes the file writer).
fn init_logging(
    dirs: &AivyxDirs,
    to_file: bool,
) -> anyhow::Result<Option<tracing_appender::non_blocking::WorkerGuard>> {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));

    if to_file {
        // Ensure the directory exists (it might not on first run before init)
        let _ = std::fs::create_dir_all(dirs.root());
        let log_file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dirs.root().join("pa.log"))?;
        let (non_blocking, guard) = tracing_appender::non_blocking(log_file);
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_writer(non_blocking)
            .with_ansi(false)
            .init();
        Ok(Some(guard))
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
        Ok(None)
    }
}

/// Prompt for passphrase with echo suppressed.
///
/// Falls back to plain stdin if echo suppression fails (e.g., piped input).
fn read_passphrase(msg: &str) -> Zeroizing<String> {
    eprint!("{msg}");
    let _ = io::stderr().flush();
    match rpassword::read_password() {
        Ok(p) => Zeroizing::new(p),
        Err(_) => {
            // Fallback for non-TTY environments
            let mut input = String::new();
            if io::stdin().lock().read_line(&mut input).is_err() {
                eprintln!("\nError reading input.");
                std::process::exit(1);
            }
            Zeroizing::new(input.trim().to_string())
        }
    }
}

/// Run first-time setup if ~/.aivyx is not initialized.
///
/// Returns `true` if setup ran (or was already initialized),
/// `false` if the user declined setup or setup was interrupted.
async fn ensure_initialized(dirs: &AivyxDirs) -> anyhow::Result<bool> {
    if dirs.is_initialized() {
        return Ok(true);
    }

    let _passphrase = init::run(dirs.root()).await?;

    if !dirs.is_initialized() {
        // Init was interrupted or failed
        eprintln!("  Setup did not complete. Please try again.");
        return Ok(false);
    }

    Ok(true)
}

/// Unlock the master key from the encrypted envelope on disk.
fn unlock(dirs: &AivyxDirs) -> anyhow::Result<MasterKey> {
    let envelope_json = std::fs::read_to_string(dirs.master_key_path())?;
    let envelope: MasterKeyEnvelope = serde_json::from_str(&envelope_json)?;

    // Check for AIVYX_PASSPHRASE env var first (for non-interactive use)
    let passphrase: Zeroizing<String> = match std::env::var("AIVYX_PASSPHRASE") {
        Ok(p) => Zeroizing::new(p),
        Err(_) => read_passphrase("Passphrase: "),
    };

    let master_key = MasterKey::decrypt_from_envelope(passphrase.as_bytes(), &envelope)
        .map_err(|_| anyhow::anyhow!("Wrong passphrase."))?;

    Ok(master_key)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dirs = AivyxDirs::from_default()?;

    // Route logs to a file for long-running server modes (default + serve).
    // Short CLI commands (chat, status, config) log to stderr as usual.
    let is_server = cli.command.is_none() || matches!(cli.command, Some(Command::Serve { .. }));
    let _log_guard = init_logging(&dirs, is_server)?;

    match cli.command {
        Some(Command::Init) => {
            let _passphrase = init::run(dirs.root()).await?;
        }

        // No subcommand → launch API server on default port
        None => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }

            let master_key = unlock(&dirs)?;
            let config = AivyxConfig::load(dirs.config_path())?;
            let pa_config = config::PaConfig::load(dirs.config_path());
            let store = EncryptedStore::open(dirs.store_path())?;

            // Lint config for common issues — surface warnings early.
            for warning in
                config::PaConfig::lint(&dirs.config_path(), Some(&store), Some(&master_key))
            {
                tracing::warn!("Config lint: {warning}");
                eprintln!("  \u{26a0} {warning}");
            }

            let mut provider = create_provider(&config.provider, &store, &master_key)?;
            let mut loop_provider = create_provider(&config.provider, &store, &master_key)?;

            if let Some(ref resilience) = pa_config.resilience {
                provider =
                    wrap_provider_resilient(provider, &config, resilience, &store, &master_key)?;
                loop_provider = wrap_provider_resilient(
                    loop_provider,
                    &config,
                    resilience,
                    &store,
                    &master_key,
                )?;
            }
            if let Some(ref routing) = pa_config.routing
                && routing.enabled
            {
                provider = wrap_provider_routed(provider, &config, routing, &store, &master_key)?;
                loop_provider =
                    wrap_provider_routed(loop_provider, &config, routing, &store, &master_key)?;
            }

            let services = resolve_services(&pa_config, &store, &master_key);
            serve_api(
                &dirs,
                config,
                pa_config,
                services,
                store,
                master_key,
                provider,
                loop_provider,
                3100,
            )
            .await?;
        }

        Some(Command::Chat { message }) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }

            let master_key = unlock(&dirs)?;
            let config = AivyxConfig::load(dirs.config_path())?;
            let pa_config = config::PaConfig::load(dirs.config_path());
            let store = EncryptedStore::open(dirs.store_path())?;
            let mut provider = create_provider(&config.provider, &store, &master_key)?;
            if let Some(ref resilience) = pa_config.resilience {
                provider =
                    wrap_provider_resilient(provider, &config, resilience, &store, &master_key)?;
            }
            if let Some(ref routing) = pa_config.routing
                && routing.enabled
            {
                provider = wrap_provider_routed(provider, &config, routing, &store, &master_key)?;
            }
            let services = resolve_services(&pa_config, &store, &master_key);

            chat_oneshot(
                &dirs, config, &pa_config, services, store, master_key, provider, &message,
            )
            .await?;
        }

        Some(Command::Status) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }
            let master_key = unlock(&dirs)?;
            print_status(&dirs, &master_key)?;
        }

        Some(Command::Config) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }
            let config = AivyxConfig::load(dirs.config_path())?;
            println!("{:#?}", config.provider);
        }

        Some(Command::Serve { port }) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }

            let master_key = unlock(&dirs)?;
            let config = AivyxConfig::load(dirs.config_path())?;
            let pa_config = config::PaConfig::load(dirs.config_path());
            let store = EncryptedStore::open(dirs.store_path())?;
            let mut provider = create_provider(&config.provider, &store, &master_key)?;
            let mut loop_provider = create_provider(&config.provider, &store, &master_key)?;

            if let Some(ref resilience) = pa_config.resilience {
                provider =
                    wrap_provider_resilient(provider, &config, resilience, &store, &master_key)?;
                loop_provider = wrap_provider_resilient(
                    loop_provider,
                    &config,
                    resilience,
                    &store,
                    &master_key,
                )?;
            }
            if let Some(ref routing) = pa_config.routing
                && routing.enabled
            {
                provider = wrap_provider_routed(provider, &config, routing, &store, &master_key)?;
                loop_provider =
                    wrap_provider_routed(loop_provider, &config, routing, &store, &master_key)?;
            }

            let services = resolve_services(&pa_config, &store, &master_key);
            serve_api(
                &dirs,
                config,
                pa_config,
                services,
                store,
                master_key,
                provider,
                loop_provider,
                port,
            )
            .await?;
        }

        Some(Command::RotateKey) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }
            rotate_key(&dirs)?;
        }
    }

    Ok(())
}

/// Resolve all service configs from PaConfig + encrypted store.
fn resolve_services(
    pa_config: &config::PaConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> agent::ServiceConfigs {
    agent::ServiceConfigs {
        email: pa_config.resolve_email_config(store, master_key),
        calendar: pa_config.resolve_calendar_config(store, master_key),
        contacts: pa_config.resolve_contacts_config(store, master_key),
        vault: pa_config.resolve_vault_config(),
        telegram: pa_config.resolve_telegram_config(store, master_key),
        matrix: pa_config.resolve_matrix_config(store, master_key),
        devtools: pa_config.resolve_devtools_config(store, master_key),
        signal: pa_config.resolve_signal_config(),
        sms: pa_config.resolve_sms_config(store, master_key),
    }
}

/// Wrap a provider in resilience layers (circuit breaker + fallback + caching).
///
/// The wrapping is transparent — `ResilientProvider` and `CachingProvider` both
/// implement `LlmProvider`, so the returned `Box<dyn LlmProvider>` is a drop-in
/// replacement. Must be called before `master_key` is moved.
fn wrap_provider_resilient(
    provider: Box<dyn LlmProvider>,
    config: &AivyxConfig,
    resilience: &config::PaResilienceConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> anyhow::Result<Box<dyn LlmProvider>> {
    let mut wrapped: Box<dyn LlmProvider> = provider;

    // 1. Circuit breaker + fallback chain
    if resilience.circuit_breaker {
        let cb_config = CircuitBreakerConfig {
            failure_threshold: resilience.failure_threshold,
            recovery_timeout: Duration::from_secs(resilience.recovery_timeout_secs),
            success_threshold: resilience.success_threshold,
        };
        let primary_name = wrapped.name().to_string();
        let mut resilient = ResilientProvider::new(wrapped, primary_name, cb_config.clone());

        // Chain fallback providers from config.providers HashMap
        for fb_name in &resilience.fallback_providers {
            if let Some(fb_config) = config.providers.get(fb_name) {
                match create_provider(fb_config, store, master_key) {
                    Ok(fb_provider) => {
                        tracing::info!(provider = fb_name, "Fallback provider registered");
                        resilient = resilient.with_fallback(
                            fb_provider,
                            fb_name.clone(),
                            cb_config.clone(),
                        );
                    }
                    Err(e) => {
                        tracing::warn!(provider = fb_name, error = %e, "Failed to create fallback provider")
                    }
                }
            } else {
                tracing::warn!(
                    provider = fb_name,
                    "Fallback provider not found in [providers] table"
                );
            }
        }

        // Attach observer for circuit state changes
        resilient = resilient.with_observer(Arc::new(|event: ProviderEvent| match event {
            ProviderEvent::CircuitOpened {
                ref provider,
                failures,
            } => tracing::warn!(provider, failures, "Circuit breaker opened"),
            ProviderEvent::CircuitClosed { ref provider } => {
                tracing::info!(provider, "Circuit breaker closed — provider recovered")
            }
            ProviderEvent::FailoverActivated { ref from, ref to } => {
                tracing::warn!(from, to, "Provider failover activated")
            }
            ProviderEvent::AllProvidersDown => {
                tracing::error!("All LLM providers down — requests will fail")
            }
        }));

        wrapped = Box::new(resilient);
    }

    // 2. Response caching
    if resilience.cache_enabled {
        let cache_config = config.cache.clone().unwrap_or_default();
        let mut caching = CachingProvider::new(wrapped, &cache_config);

        // Attach semantic caching if embedding provider is available
        if cache_config.semantic_enabled
            && let Some(ref emb_config) = config.embedding
        {
            match create_embedding_provider(emb_config, store, master_key) {
                Ok(emb) => {
                    tracing::info!("Semantic response cache enabled");
                    caching = caching.with_semantic(Arc::from(emb));
                }
                Err(e) => tracing::warn!(error = %e, "Semantic cache requires embedding provider"),
            }
        }

        tracing::info!(
            ttl_secs = cache_config.ttl_secs,
            max_entries = cache_config.max_entries,
            "LLM response cache enabled"
        );
        wrapped = Box::new(caching);
    }

    Ok(wrapped)
}

/// Wrap a provider in complexity-based routing.
///
/// Classifies each request as Simple/Medium/Complex and routes to the
/// tier-specific provider. Unset tiers fall back to the given `provider`.
fn wrap_provider_routed(
    provider: Box<dyn LlmProvider>,
    config: &AivyxConfig,
    routing: &config::PaRoutingConfig,
    store: &EncryptedStore,
    master_key: &MasterKey,
) -> anyhow::Result<Box<dyn LlmProvider>> {
    use std::collections::HashMap;

    let mut tier_providers: HashMap<ComplexityLevel, Box<dyn LlmProvider>> = HashMap::new();

    let resolve = |name: &Option<String>| -> Option<Box<dyn LlmProvider>> {
        let name = name.as_deref()?;
        let provider_cfg = config.providers.get(name)?;
        match create_provider(provider_cfg, store, master_key) {
            Ok(p) => {
                tracing::info!(tier_provider = name, "Routing provider resolved");
                Some(p)
            }
            Err(e) => {
                tracing::warn!(tier_provider = name, error = %e, "Failed to create routing tier provider");
                None
            }
        }
    };

    if let Some(p) = resolve(&routing.simple) {
        tier_providers.insert(ComplexityLevel::Simple, p);
    }
    if let Some(p) = resolve(&routing.medium) {
        tier_providers.insert(ComplexityLevel::Medium, p);
    }
    if let Some(p) = resolve(&routing.complex) {
        tier_providers.insert(ComplexityLevel::Complex, p);
    }

    if tier_providers.is_empty() {
        tracing::warn!("Routing enabled but no tier providers resolved — routing is a no-op");
        return Ok(provider);
    }

    let routed =
        RoutingProvider::new(provider, tier_providers).with_observer(Arc::new(
            |event| match event {
                aivyx_llm::RoutingEvent::Routed {
                    complexity,
                    provider,
                } => tracing::info!(?complexity, provider, "Request routed by complexity"),
            },
        ));

    tracing::info!("Complexity-based model routing enabled");
    Ok(Box::new(routed))
}

/// One-shot chat: send a message, print the response, exit.
async fn chat_oneshot(
    dirs: &AivyxDirs,
    config: AivyxConfig,
    pa_config: &config::PaConfig,
    services: agent::ServiceConfigs,
    store: aivyx_crypto::EncryptedStore,
    master_key: MasterKey,
    provider: Box<dyn aivyx_llm::LlmProvider>,
    message: &str,
) -> anyhow::Result<()> {
    let audit_key = aivyx_crypto::derive_audit_key(&master_key);
    let audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &audit_key);
    let store = std::sync::Arc::new(store);
    let built = crate::agent::build_agent(
        dirs,
        &config,
        pa_config,
        services,
        store,
        master_key,
        provider,
        Some(audit_log),
    )
    .await?;
    let mut agent = built.agent;
    let response = agent.turn(message, None).await?;
    println!("{response}");
    Ok(())
}

/// Start the API server.
///
/// Builds the agent and background loop, then exposes an HTTP/SSE API
/// for frontend clients to connect to.
async fn serve_api(
    dirs: &AivyxDirs,
    config: AivyxConfig,
    pa_config: config::PaConfig,
    services: agent::ServiceConfigs,
    store: aivyx_crypto::EncryptedStore,
    master_key: MasterKey,
    provider: Box<dyn aivyx_llm::LlmProvider>,
    loop_provider: Box<dyn aivyx_llm::LlmProvider>,
    port: u16,
) -> anyhow::Result<()> {
    use aivyx_loop::AgentLoop;
    use aivyx_pa::api;
    use tokio::sync::broadcast;

    // Copy master key bytes before build_agent consumes it (MasterKey is not Clone)
    let master_key_bytes: [u8; 32] = master_key.expose_secret().try_into().unwrap();

    // Derive all keys and resolve loop inputs before master_key is consumed
    use aivyx_pa::runtime;
    let mut keys = runtime::derive_all_keys(
        &master_key,
        &pa_config,
        services.vault.is_some(),
        services.contacts.is_some(),
    );
    let loop_inputs = runtime::LoopInputs::from_services(&services, &pa_config);

    let store = std::sync::Arc::new(store);
    let agent_audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &keys.audit_key);
    // Separate read-only audit log for API queries (same file, same key)
    let api_audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &keys.ui_audit_key);

    let mut built = crate::agent::build_agent(
        dirs,
        &config,
        &pa_config,
        services,
        std::sync::Arc::clone(&store),
        master_key,
        provider,
        Some(agent_audit_log),
    )
    .await?;

    // Clone brain store for API handlers before build_loop_context takes it
    let brain_store_for_api = built.brain_store.as_ref().map(std::sync::Arc::clone);

    // Build schedule tools, loop context, and loop config via shared runtime
    let schedule_tools = runtime::build_schedule_tools(&loop_inputs, built.imap_pool.clone());
    let loop_context = runtime::build_loop_context(
        &mut built,
        &mut keys,
        loop_inputs,
        loop_provider,
        schedule_tools,
        std::sync::Arc::clone(&store),
        dirs,
        &pa_config,
    );
    let loop_config = runtime::build_loop_config(&pa_config);

    // Clone memory manager and mission context for API before built is consumed
    let mission_ctx_for_api = built.mission_ctx.clone();
    let memory_manager_for_api = built.memory_manager.as_ref().map(std::sync::Arc::clone);
    let agent_name = pa_config
        .agent
        .as_ref()
        .map(|a| a.name.clone())
        .unwrap_or_else(|| "assistant".into());

    let (_agent_loop, mut notification_rx) = AgentLoop::start(loop_config, loop_context);

    // Shared approval queue and notification history
    let approvals: std::sync::Arc<tokio::sync::Mutex<Vec<api::ApprovalItem>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));
    let notification_history: std::sync::Arc<tokio::sync::Mutex<Vec<aivyx_loop::Notification>>> =
        std::sync::Arc::new(tokio::sync::Mutex::new(Vec::new()));

    // Bridge mpsc notifications into broadcast + approval queue + history buffer
    // Also tee to the proactive dispatcher if [notifications] is configured.
    let (notification_tx, _) = broadcast::channel::<aivyx_loop::Notification>(256);
    let broadcast_tx = notification_tx.clone();
    let bridge_approvals = std::sync::Arc::clone(&approvals);
    let bridge_history = std::sync::Arc::clone(&notification_history);

    // Build dispatcher channel if notification dispatch is configured.
    let dispatch_sender = pa_config.notifications.as_ref().map(|n| {
        let (tx, rx) = tokio::sync::mpsc::channel::<aivyx_loop::Notification>(256);
        let dispatch_ctx = aivyx_loop::DispatchContext {
            config: aivyx_loop::NotificationDispatchConfig {
                desktop: n.desktop,
                urgency_level: n.urgency_level.clone(),
                telegram: n.telegram,
                signal: n.signal,
                quiet_hours_start: n.quiet_hours_start,
                quiet_hours_end: n.quiet_hours_end,
                min_kind: n.min_kind.clone(),
            },
            // Provide messaging context for Telegram/Signal forwarding.
            telegram: None, // TODO: thread MessagingCtx through if telegram/signal enabled
        };
        aivyx_loop::notify_dispatch::spawn_dispatcher(rx, dispatch_ctx);
        tracing::info!(
            desktop = n.desktop,
            telegram = n.telegram,
            signal = n.signal,
            "Proactive notification dispatcher started"
        );
        tx
    });

    tokio::spawn(async move {
        while let Some(notif) = notification_rx.recv().await {
            // Route approval-requiring notifications to the queue
            if notif.requires_approval {
                bridge_approvals.lock().await.push(api::ApprovalItem {
                    expires_at: Some(
                        notif.timestamp + chrono::TimeDelta::try_seconds(120).unwrap(),
                    ),
                    notification: notif.clone(),
                    status: api::ApprovalStatus::Pending,
                    resolved_at: None,
                });
            }

            // Buffer for history queries
            let mut hist = bridge_history.lock().await;
            hist.push(notif.clone());
            // Cap history to prevent unbounded growth
            if hist.len() > 500 {
                let excess = hist.len() - 500;
                hist.drain(..excess);
            }

            // Forward to proactive dispatcher (desktop/Telegram/Signal)
            if let Some(ref tx) = dispatch_sender {
                let _ = tx.try_send(notif.clone());
            }

            let _ = broadcast_tx.send(notif);
        }
    });

    // Start webhook HTTP server if [webhook] is configured and enabled.
    // The server receives POST /webhooks/{name} requests from external services
    // (GitHub, Stripe, smart home hubs, IFTTT, etc.) and queues them as
    // agent notifications. The webhook key was derived above but not yet used.
    if let Some(ref webhook_cfg) = pa_config.webhook
        && webhook_cfg.enabled
    {
        let wh_store = std::sync::Arc::clone(&store);
        let wh_key = keys.webhook_key.take().unwrap_or_else(|| {
            MasterKey::from_bytes([0u8; 32]) // should not happen if derive_all_keys ran
        });
        // Give the webhook server an mpsc sender that feeds into the notification broadcast.
        let (wh_tx, mut wh_rx) = tokio::sync::mpsc::channel::<aivyx_loop::Notification>(64);
        let wh_broadcast_tx = notification_tx.clone();
        tokio::spawn(async move {
            while let Some(n) = wh_rx.recv().await {
                let _ = wh_broadcast_tx.send(n);
            }
        });
        match aivyx_pa::webhook::spawn_webhook_server(webhook_cfg, wh_store, &wh_key, wh_tx).await {
            Ok(_handle) => {
                eprintln!("  Webhook server on http://127.0.0.1:{}", webhook_cfg.port);
            }
            Err(e) => {
                tracing::warn!(error = %e, "Failed to start webhook server — webhooks disabled");
                eprintln!("  ⚠ Webhook server failed to start: {e}");
            }
        }
    }

    let config_path = dirs.config_path();

    let state = api::AppState {
        agent: std::sync::Arc::new(tokio::sync::Mutex::new(built.agent)),
        brain_store: brain_store_for_api,
        brain_key: Some(std::sync::Arc::new(keys.ui_brain_key)),
        audit_log: std::sync::Arc::new(api_audit_log),
        notification_tx,
        pa_config: std::sync::Arc::new(pa_config),
        dirs: std::sync::Arc::new(dirs.clone()),
        store,
        conversation_key: std::sync::Arc::new(keys.conversation_key),
        master_key: std::sync::Arc::new(MasterKey::from_bytes(master_key_bytes)),
        memory_manager: memory_manager_for_api,
        approvals,
        notification_history,
        config_path,
        agent_name,
        mission_ctx: mission_ctx_for_api,
        health: std::sync::Arc::new(tokio::sync::RwLock::new(api::HealthStatus::default())),
        approval_tx: Some(_agent_loop.approval_tx.clone()),
    };

    let (_handle, cancel) = api::spawn_api_server(state, port).await?;
    eprintln!("Aivyx API server running on http://127.0.0.1:{port}");
    eprintln!("Press Ctrl+C to stop.");

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    cancel.cancel();
    eprintln!("\nShutting down...");

    Ok(())
}

/// Print a summary of the assistant's current state.
fn print_status(dirs: &AivyxDirs, master_key: &MasterKey) -> anyhow::Result<()> {
    let pa_config = config::PaConfig::load(dirs.config_path());
    let agent_cfg = pa_config.agent_config();
    let loop_cfg = pa_config.loop_config();

    // Header
    println!("Aivyx Personal Assistant — Status");
    println!("──────────────────────────────────");
    println!("Agent:    {} ({})", agent_cfg.name, agent_cfg.persona);
    println!(
        "Briefing: {:02}:00 ({})",
        loop_cfg.briefing_hour,
        if loop_cfg.morning_briefing {
            "enabled"
        } else {
            "disabled"
        }
    );
    println!("Interval: {} min", loop_cfg.check_interval_minutes);
    println!();

    // Active goals
    let brain_key = aivyx_crypto::derive_brain_key(master_key);
    let brain_path = dirs.agent_brain_path(&agent_cfg.name);
    if brain_path.exists() {
        match aivyx_brain::BrainStore::open(&brain_path) {
            Ok(store) => {
                let filter = aivyx_brain::GoalFilter {
                    status: Some(aivyx_brain::GoalStatus::Active),
                    ..Default::default()
                };
                match store.list_goals(&filter, &brain_key) {
                    Ok(goals) => {
                        println!("Active Goals ({})", goals.len());
                        println!("──────────────────────────────────");
                        if goals.is_empty() {
                            println!("  (none)");
                        }
                        for goal in &goals {
                            let pct = (goal.progress * 100.0) as u8;
                            let priority = format!("{:?}", goal.priority).to_lowercase();
                            println!("  [{pct:>3}%] [{priority}] {}", goal.description);
                        }
                        println!();
                    }
                    Err(e) => println!("  Could not read goals: {e}\n"),
                }
            }
            Err(e) => println!("  Brain store unavailable: {e}\n"),
        }
    } else {
        println!("Active Goals");
        println!("──────────────────────────────────");
        println!("  (brain not initialized — run the server first)");
        println!();
    }

    // Schedules
    if !pa_config.schedules.is_empty() {
        println!("Schedules ({})", pa_config.schedules.len());
        println!("──────────────────────────────────");
        for s in &pa_config.schedules {
            let status = if s.enabled { "active" } else { "paused" };
            println!("  [{}] {} — {}", status, s.name, s.cron);
        }
        println!();
    }

    // Recent audit entries
    let audit_key = aivyx_crypto::derive_audit_key(master_key);
    let audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &audit_key);
    match audit_log.recent(10) {
        Ok(entries) if !entries.is_empty() => {
            // Metrics summary (last 24 hours)
            let now = chrono::Utc::now();
            let day_ago = now - chrono::Duration::hours(24);
            let zero_cost = |_i: u32, _o: u32, _p: &str| 0.0_f64;
            let metrics = aivyx_audit::compute_summary(&entries, day_ago, now, &zero_cost);
            if metrics.llm_requests > 0 || metrics.tool_executions > 0 {
                println!("Metrics (last 24h)");
                println!("──────────────────────────────────");
                println!(
                    "  LLM calls: {}  Tokens: {}in / {}out",
                    metrics.llm_requests, metrics.total_input_tokens, metrics.total_output_tokens
                );
                println!(
                    "  Tool executions: {}  Denied: {}  Agent turns: {}",
                    metrics.tool_executions, metrics.tool_denials, metrics.agent_turns
                );
                println!();
            }

            println!("Recent Activity ({} entries)", entries.len());
            println!("──────────────────────────────────");
            for entry in &entries {
                let event_desc = format_audit_event(&entry.event);
                println!(
                    "  {} {}",
                    entry.timestamp.get(..19).unwrap_or(&entry.timestamp),
                    event_desc
                );
            }
            println!();
        }
        Ok(_) => {
            println!("Recent Activity");
            println!("──────────────────────────────────");
            println!("  (no audit entries yet)");
            println!();
        }
        Err(e) => {
            println!("Recent Activity");
            println!("──────────────────────────────────");
            println!("  Could not read audit log: {e}");
            println!();
        }
    }

    Ok(())
}

/// Rotate the master encryption key via direct terminal interaction.
///
/// SECURITY: This is deliberately a CLI command, not an LLM tool. The
/// passphrase is read directly from the terminal with echo suppression
/// and never flows through the LLM provider's API or conversation history.
fn rotate_key(dirs: &AivyxDirs) -> anyhow::Result<()> {
    println!();
    println!("  Master Key Rotation");
    println!("  ────────────────────────────────");
    println!("  This re-encrypts all stored secrets with a new passphrase.");
    println!("  The old passphrase will no longer work after rotation.");
    println!();

    // Unlock with current passphrase
    let old_master_key = unlock(dirs)?;

    // Get and confirm new passphrase (echo suppressed)
    println!();
    let new_passphrase = read_passphrase("  New passphrase: ");
    if new_passphrase.len() < 8 {
        anyhow::bail!("New passphrase too short (minimum 8 characters).");
    }
    let confirm = read_passphrase("  Confirm new passphrase: ");
    if *new_passphrase != *confirm {
        anyhow::bail!("Passphrases don't match.");
    }

    // Open the store and re-encrypt
    let store = EncryptedStore::open(dirs.store_path())?;
    let new_master_key = MasterKey::generate();

    print!("  Re-encrypting...");
    let _ = io::stdout().flush();
    let result = store
        .re_encrypt_all(&old_master_key, &new_master_key)
        .map_err(|e| anyhow::anyhow!("Re-encryption failed: {e}"))?;

    if !result.errors.is_empty() {
        eprintln!(" Partial failure!");
        for err in &result.errors {
            eprintln!("    Error: {err}");
        }
        anyhow::bail!(
            "{} keys migrated, {} errors. Old passphrase still works.",
            result.keys_migrated,
            result.errors.len()
        );
    }

    // Write new envelope
    let envelope = new_master_key.encrypt_to_envelope(new_passphrase.as_bytes())?;
    let envelope_json = serde_json::to_string_pretty(&envelope)?;
    std::fs::write(dirs.master_key_path(), envelope_json)?;

    println!(" Done!");
    println!("  {} keys re-encrypted.", result.keys_migrated);
    println!("  Your new passphrase is now required for all future access.");
    println!();

    Ok(())
}

/// Format an audit event into a brief one-line description.
fn format_audit_event(event: &aivyx_audit::AuditEvent) -> String {
    use aivyx_audit::AuditEvent;
    match event {
        AuditEvent::SystemInit { .. } => "System initialized".into(),
        AuditEvent::ToolExecuted { action, .. } => format!("Tool executed: {action}"),
        AuditEvent::ToolDenied { action, reason, .. } => {
            format!("Tool denied: {action} ({reason})")
        }
        AuditEvent::ToolExecutionFailed { action, error, .. } => {
            format!("Tool failed: {action} ({error})")
        }
        AuditEvent::AgentTurnStarted { .. } => "Agent turn started".into(),
        AuditEvent::AgentTurnCompleted { .. } => "Agent turn completed".into(),
        AuditEvent::ScheduleFired { schedule_name, .. } => {
            format!("Schedule fired: {schedule_name}")
        }
        AuditEvent::ScheduleCompleted { schedule_name, .. } => {
            format!("Schedule done: {schedule_name}")
        }
        AuditEvent::MemoryStored { .. } => "Memory stored".into(),
        AuditEvent::CapabilityGranted { scope_summary, .. } => {
            format!("Capability granted: {scope_summary}")
        }
        AuditEvent::CapabilityRevoked { .. } => "Capability revoked".into(),
        AuditEvent::ConfigChanged { key, .. } => format!("Config changed: {key}"),
        AuditEvent::HeartbeatFired {
            context_sections, ..
        } => format!("Heartbeat fired ({context_sections} sections)"),
        AuditEvent::HeartbeatCompleted {
            actions_dispatched, ..
        } => format!("Heartbeat done ({actions_dispatched} actions)"),
        AuditEvent::HeartbeatSkipped { reason } => format!("Heartbeat skipped: {reason}"),
        AuditEvent::BriefingGenerated { item_count, .. } => {
            format!("Briefing generated ({item_count} items)")
        }
        AuditEvent::TriageCompleted { processed, .. } => {
            format!("Triage done ({processed} emails)")
        }
        AuditEvent::BackupCompleted { .. } => "Backup completed".into(),
        AuditEvent::BackupFailed { reason } => format!("Backup failed: {reason}"),
        // Catch-all for the 80+ other event types — use Debug for now
        other => {
            let debug = format!("{other:?}");
            // Trim to just the variant name (before the first `{` or `(`)
            let variant = debug.split(['{', '(']).next().unwrap_or(&debug);
            variant.trim().to_string()
        }
    }
}
