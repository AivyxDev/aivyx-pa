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
use aivyx_pa::passphrase;
use aivyx_pa::pidfile::PidFile;
use aivyx_pa::profile;

use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_crypto::{EncryptedStore, MasterKey, MasterKeyEnvelope};
use aivyx_llm::{
    CachingProvider, CircuitBreakerConfig, ComplexityLevel, LlmProvider, ProviderEvent,
    ResilientProvider, RoutingProvider, create_embedding_provider, create_provider,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use std::time::Duration;
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    name = "aivyx",
    about = "Your private AI personal assistant",
    version,
    after_help = "Run without arguments to launch the API server.\n\
                  Port resolution: --port flag → [server].port in config.toml → 3100.\n\
                  Use --profile <name> (or AIVYX_PROFILE) to target a named multi-agent profile."
)]
struct Cli {
    /// Named profile to target (enables multi-agent deployments on one host).
    ///
    /// When set, data is read from `~/.aivyx/profiles/<name>/` instead of
    /// the default `~/.aivyx/` root. Each profile is fully isolated: its
    /// own config, encrypted store, master key, and memory. Unset means
    /// the legacy single-agent directory — existing installs keep working.
    #[arg(long, global = true, env = "AIVYX_PROFILE")]
    profile: Option<String>,

    #[command(subcommand)]
    command: Option<Command>,
}

/// Resolve the `AivyxDirs` root for this invocation.
///
/// - `Some(name)` → `~/.aivyx/profiles/<name>/` (validated profile name).
/// - `None`       → legacy `~/.aivyx/` default root.
///
/// `AIVYX_HOME` (honoured inside `aivyx-config`) still wins as an escape
/// hatch for tests and power users, because it rebinds the base that both
/// branches resolve against.
fn resolve_dirs(profile: Option<&str>) -> anyhow::Result<AivyxDirs> {
    match profile {
        Some(name) => Ok(AivyxDirs::from_profile(name)?),
        None => Ok(AivyxDirs::from_default()?),
    }
}

/// Resolve the effective HTTP API port using the three-tier fallback:
///
///   1. explicit `--port` CLI flag (`cli_override`)
///   2. `[server].port` in the profile's `config.toml`
///   3. built-in default (`config::DEFAULT_API_PORT`)
///
/// Both the `Serve` subcommand and the no-subcommand default path go through
/// this helper so they can't drift. The `cli_override` is always `None` for
/// the no-subcommand branch.
fn resolve_effective_port(pa_config: &config::PaConfig, cli_override: Option<u16>) -> u16 {
    match pa_config.server.as_ref() {
        Some(server) => server.resolve_port(cli_override),
        None => cli_override.unwrap_or(config::DEFAULT_API_PORT),
    }
}

/// Name used for owner-comparison when the user is running the legacy
/// single-agent root (`~/.aivyx/`) rather than a named profile.
///
/// A user can opt this root in to desktop ownership by putting
/// `[desktop] owner = "(default)"` in their `config.toml`.
const DEFAULT_PROFILE_NAME: &str = "(default)";

/// Acquire the per-profile pidfile and enforce desktop-tool exclusivity
/// across sibling profiles on the same host.
///
/// Returns the pidfile guard — the caller **must** keep it alive for the
/// lifetime of the server process. Dropping the guard removes the pidfile
/// and releases ownership.
///
/// Exclusivity rules (advisory; best-effort):
///
/// - Acquiring the pidfile is hard-required: if another live aivyx process
///   already holds it for this profile, startup fails. This prevents two
///   processes from fighting over the same encrypted store.
/// - Desktop tool exclusivity is softer: if we detect a *sibling* profile
///   with a live pidfile AND enabled desktop tools AND this profile also
///   has enabled desktop tools, we emit a warning and disable our own
///   desktop tools (by setting `pa_config.desktop = None`) so two agents
///   don't fight over the mouse.
/// - The warning is suppressed — and desktop tools are kept — when
///   `[desktop].owner` is set to the current profile name, which is the
///   user's explicit opt-in.
///
/// This function mutates `pa_config` in place when it decides to disable
/// desktop tools, so the disabled state flows through to service resolution
/// and the agent-tool registry downstream.
fn enforce_startup_exclusivity(
    dirs: &AivyxDirs,
    profile: Option<&str>,
    pa_config: &mut config::PaConfig,
) -> anyhow::Result<PidFile> {
    let pidfile_path = dirs.root().join("aivyx.pid");
    let guard = PidFile::acquire(&pidfile_path)?;

    // Only the rest of this function is desktop-specific — if this profile
    // didn't configure desktop tools, there's nothing to arbitrate.
    if pa_config.desktop.is_none() {
        return Ok(guard);
    }

    let self_name = profile.unwrap_or(DEFAULT_PROFILE_NAME);
    let declared_owner = pa_config.desktop.as_ref().and_then(|d| d.owner.as_deref());

    // If the config explicitly names an owner that is NOT us, bail out
    // immediately: the user has told us another profile is authoritative
    // for the physical desktop, so we must not register desktop tools at all.
    if let Some(owner) = declared_owner {
        if owner != self_name {
            tracing::warn!(
                "[desktop].owner = \"{owner}\" in this profile's config.toml; \
                 disabling desktop tools for profile \"{self_name}\" to avoid \
                 contention with the declared owner"
            );
            eprintln!(
                "  ⚠ desktop tools disabled: [desktop].owner = \"{owner}\" \
                 (this profile is \"{self_name}\")"
            );
            pa_config.desktop = None;
            return Ok(guard);
        }
        // owner == self_name → user explicitly opted us in; skip the
        // sibling scan entirely.
        return Ok(guard);
    }

    // No explicit owner — scan sibling profiles for live peers that are also
    // registering desktop tools. We read peer configs best-effort: if a
    // sibling config can't be parsed, we skip it rather than failing startup.
    let peers = match AivyxDirs::list_profiles() {
        Ok(names) => names,
        Err(e) => {
            tracing::debug!("could not enumerate profiles for desktop exclusivity check: {e}");
            return Ok(guard);
        }
    };

    for peer_name in peers {
        // Don't compare against ourselves.
        if Some(peer_name.as_str()) == profile {
            continue;
        }
        let peer_dirs = match AivyxDirs::from_profile(&peer_name) {
            Ok(d) => d,
            Err(_) => continue,
        };

        let peer_pidfile = peer_dirs.root().join("aivyx.pid");
        let Some(peer_pid) = PidFile::read_peer(&peer_pidfile) else {
            continue; // peer is not currently running
        };

        // Peer is running. Is it using desktop tools?
        let peer_cfg = config::PaConfig::load(peer_dirs.config_path());
        if peer_cfg.desktop.is_none() {
            continue;
        }

        // Contention detected. Disable our desktop tools and warn.
        tracing::warn!(
            "profile \"{peer_name}\" (pid {peer_pid}) is already running with desktop \
             tools enabled; disabling desktop tools for this profile \"{self_name}\" to \
             avoid UI-automation contention. Set [desktop].owner in one of the \
             config.toml files to make the assignment explicit."
        );
        eprintln!(
            "  ⚠ desktop tools disabled: profile \"{peer_name}\" (pid {peer_pid}) already \
             owns the desktop. Set [desktop].owner in config.toml to silence this warning."
        );
        pa_config.desktop = None;
        break;
    }

    Ok(guard)
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
        /// Port to listen on. Overrides `[server].port` in config.toml.
        /// Falls back to 3100 when neither is set.
        //
        // Implementation note (not user-facing): `Option<u16>` is load-bearing.
        // Using `u16` with `default_value_t = 3100` would erase the distinction
        // between "user passed --port 3100" and "user didn't pass --port",
        // which breaks the three-tier fallback resolved in `resolve_effective_port`.
        #[arg(short, long)]
        port: Option<u16>,
    },
    /// Manage multi-agent profiles (create, list, inspect, remove, rename)
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },
}

/// Actions for the `aivyx profile` subcommand tree.
///
/// Each profile lives under `~/.aivyx/profiles/<name>/` and is fully isolated
/// (its own config, encrypted store, master key, pidfile, and memory). These
/// actions manipulate profiles as first-class lifecycle objects — the global
/// `--profile` flag on `Cli` targets an *existing* profile for runtime
/// operations, whereas this subcommand tree creates and destroys them.
#[derive(Subcommand)]
enum ProfileAction {
    /// List all profiles with their status, persona, and configured port
    List,
    /// Create a new profile and run the init wizard against it.
    ///
    /// Auto-assigns a free port (starting at 3100) and writes it into the
    /// new profile's `[server].port` so multiple profiles can coexist on
    /// one host without manual port juggling.
    New {
        /// Profile name (1-32 chars, `[A-Za-z0-9_-]`)
        name: String,
    },
    /// Show a profile's resolved directories and non-secret config summary
    Show {
        /// Profile name
        name: String,
    },
    /// Remove a profile and all of its data.
    ///
    /// Refuses if the profile's pidfile is held by a live process. Zero-
    /// overwrites `keys/master.json` before deleting to best-effort wipe
    /// the encrypted master key envelope.
    Remove {
        /// Profile name
        name: String,
        /// Confirm destructive deletion (required)
        #[arg(long)]
        yes: bool,
    },
    /// Rename a profile on disk.
    ///
    /// Refuses if the source profile is running (pidfile held) or if the
    /// destination already exists. Does not rewrite references inside the
    /// profile's config — those are already relative to the directory root.
    Rename {
        /// Current profile name
        from: String,
        /// New profile name
        to: String,
    },
    /// Render a supervisor service file for a profile.
    ///
    /// Emits the text of a systemd `aivyx@.service` template or a launchd
    /// `com.aivyx.<name>.plist` file to stdout (or `--output <path>`)
    /// without touching any install locations. Pair with shell redirection
    /// for a classic Unix install:
    ///
    ///   aivyx profile render-service work > ~/.config/systemd/user/aivyx@.service
    ///
    /// Use `profile install-service` for a fully-automated install.
    RenderService {
        /// Profile name (determines paths embedded in the rendered unit)
        name: String,
        /// Which supervisor's format to generate.
        ///
        /// `auto` picks based on the current platform: `systemd` on Linux,
        /// `launchd` on macOS. Both renderers compile on every platform so
        /// you can render a macOS plist from a Linux dev machine.
        #[arg(long, value_enum, default_value_t = ServiceKind::Auto)]
        kind: ServiceKind,
        /// Write the rendered text to this path instead of stdout.
        /// Refuses to overwrite an existing file unless `--force` is set.
        #[arg(long)]
        output: Option<std::path::PathBuf>,
        /// Allow `--output` to overwrite an existing file.
        #[arg(long)]
        force: bool,
    },
    /// Install the rendered supervisor unit into the canonical
    /// user-level location for the current platform.
    ///
    /// * systemd: `$XDG_CONFIG_HOME/systemd/user/aivyx@.service`
    ///   (defaults to `~/.config/systemd/user/`)
    /// * launchd: `~/Library/LaunchAgents/com.aivyx.<name>.plist`
    /// * windows: `<profile_root>\install-service.ps1`
    ///
    /// Does NOT invoke `systemctl`, `launchctl`, or `New-Service` —
    /// that's left to the operator so they can see the output and
    /// pick the exact moment to start the service. The command we'd
    /// run is printed on success.
    ///
    /// Refuses to overwrite an existing installed unit unless `--force`
    /// is set, so hand-edited units are preserved by default.
    InstallService {
        /// Profile name (determines paths embedded in the rendered unit)
        name: String,
        /// Which supervisor's format to install.
        #[arg(long, value_enum, default_value_t = ServiceKind::Auto)]
        kind: ServiceKind,
        /// Overwrite an existing installed unit.
        #[arg(long)]
        force: bool,
    },
    /// Remove a previously installed supervisor unit from its
    /// canonical location.
    ///
    /// Symmetric with `install-service`: uses the same path-
    /// resolution helper, deletes the file, and prints the
    /// disable/stop command the operator should run next. Does NOT
    /// invoke `systemctl` / `launchctl` / `sc.exe` — the operator
    /// still needs to disable and stop the live service manually,
    /// because we have no good way to know whether they want to
    /// flush in-flight state first.
    ///
    /// Refuses to run if the profile's own server process is alive
    /// (pidfile held), unless `--force` is set. Removing the unit
    /// file out from under a running service would leave it
    /// un-recoverable across the next restart.
    UninstallService {
        /// Profile name (determines which installed unit to find)
        name: String,
        /// Which supervisor's format to remove.
        #[arg(long, value_enum, default_value_t = ServiceKind::Auto)]
        kind: ServiceKind,
        /// Remove the unit even if the profile's server is live.
        #[arg(long)]
        force: bool,
    },
}

/// Which supervisor format `profile render-service` / `install-service`
/// should target.
///
/// `Auto` resolves at runtime via `cfg!(target_os)` — Linux → `Systemd`,
/// macOS → `Launchd`, anything else falls back to `Systemd` (because the
/// rendered text is deterministic and platform-agnostic; it's the
/// install step that's per-OS). The resolution happens in
/// [`ServiceKind::resolve`] so both render and install take the same
/// path.
#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum ServiceKind {
    /// systemd user-level template unit (`aivyx@.service`)
    Systemd,
    /// macOS launchd plist (`com.aivyx.<name>.plist`)
    Launchd,
    /// Windows Service Control Manager PowerShell installer script
    /// (`install-service.ps1`). Run from an elevated PowerShell prompt
    /// to register the aivyx service with SCM.
    Windows,
    /// Auto-detect from the current platform
    Auto,
}

impl ServiceKind {
    /// Resolve `Auto` to a concrete platform kind. Returns self unchanged
    /// for the non-`Auto` variants.
    fn resolve(self) -> Self {
        match self {
            Self::Auto => {
                if cfg!(target_os = "macos") {
                    Self::Launchd
                } else if cfg!(target_os = "windows") {
                    Self::Windows
                } else {
                    // Linux and everything else default to systemd.
                    // The "everything else" bucket catches BSD and
                    // WSL, where systemd-user is usually available
                    // and the template unit just works.
                    Self::Systemd
                }
            }
            k => k,
        }
    }

    /// Human-readable name for the supervisor, used in install/render
    /// messages. Centralised so every caller prints the same thing.
    /// Never call on `Auto` — callers must `resolve()` first.
    fn display_name(self) -> &'static str {
        match self {
            Self::Systemd => "systemd",
            Self::Launchd => "launchd",
            Self::Windows => "windows",
            Self::Auto => unreachable!("ServiceKind::display_name called on Auto"),
        }
    }
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
///
/// Passphrase resolution follows a three-tier precedence, checked in
/// order — more explicit wins:
///
/// 1. **`AIVYX_PASSPHRASE` env var** — the passphrase as a literal
///    string. Highest precedence because an operator who sets this
///    for a specific invocation wants it to override whatever the
///    service manager configured.
/// 2. **`AIVYX_PASSPHRASE_FILE` env var** — path to a file whose
///    (trim-end'd) contents are the passphrase. This is the path
///    systemd `LoadCredential=` and launchd `EnvironmentVariables=`
///    take, so service-managed installs never need to pass plaintext
///    through environment variables visible to `systemctl show`.
/// 3. **Interactive prompt** — falls back to stdin when neither env
///    var is set. This is the path CLI users take on their own
///    laptops.
///
/// The resolved passphrase is held in a `Zeroizing<String>` so the
/// bytes are scrubbed from memory when the function returns, regardless
/// of which branch produced them.
fn unlock(dirs: &AivyxDirs) -> anyhow::Result<MasterKey> {
    let envelope_json = std::fs::read_to_string(dirs.master_key_path())?;
    let envelope: MasterKeyEnvelope = serde_json::from_str(&envelope_json)?;

    // Delegates the three-tier precedence (`AIVYX_PASSPHRASE` >
    // `AIVYX_PASSPHRASE_FILE` > interactive prompt) to the
    // `passphrase` library module so the non-interactive tiers can
    // be unit-tested without touching the master-key envelope.
    let pp = passphrase::resolve_passphrase(|| read_passphrase("Passphrase: "))?;

    let master_key = MasterKey::decrypt_from_envelope(pp.as_bytes(), &envelope)
        .map_err(|_| anyhow::anyhow!("Wrong passphrase."))?;

    Ok(master_key)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let dirs = resolve_dirs(cli.profile.as_deref())?;

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
            let mut pa_config = config::PaConfig::load(dirs.config_path());
            let store = EncryptedStore::open(dirs.store_path())?;

            // Acquire the per-profile pidfile and enforce desktop-tool
            // exclusivity across sibling profiles. The returned guard must
            // live for the full server lifetime — binding it to `_pidfile`
            // keeps it alive until this match arm returns.
            let _pidfile =
                enforce_startup_exclusivity(&dirs, cli.profile.as_deref(), &mut pa_config)?;

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
            // No-subcommand path: no CLI port override, so rely on
            // [server].port → built-in default. Previously this branch
            // hardcoded 3100, which silently ignored per-profile config.
            let port = resolve_effective_port(&pa_config, None);
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
            let mut pa_config = config::PaConfig::load(dirs.config_path());
            let store = EncryptedStore::open(dirs.store_path())?;

            // Acquire the per-profile pidfile and enforce desktop-tool
            // exclusivity. See the no-subcommand branch above for rationale.
            let _pidfile =
                enforce_startup_exclusivity(&dirs, cli.profile.as_deref(), &mut pa_config)?;

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
            // Three-tier resolution: CLI flag → [server].port → default.
            let effective_port = resolve_effective_port(&pa_config, port);
            serve_api(
                &dirs,
                config,
                pa_config,
                services,
                store,
                master_key,
                provider,
                loop_provider,
                effective_port,
            )
            .await?;
        }

        Some(Command::RotateKey) => {
            if !ensure_initialized(&dirs).await? {
                return Ok(());
            }
            rotate_key(&dirs)?;
        }

        Some(Command::Profile { action }) => {
            // Profile lifecycle commands ignore the global `--profile` flag:
            // they always target the `name` argument in the action itself.
            // (Using both at once is not meaningful — the global flag selects
            // a runtime target, but these commands are the ones that create,
            // destroy, and rename those targets.)
            run_profile_action(action).await?;
        }
    }

    Ok(())
}

/// Dispatch an `aivyx profile <action>` invocation.
///
/// All the non-interactive logic (list/show/remove/rename) lives in
/// `aivyx_pa::profile` so it can be exercised from integration tests.
/// Only `new` stays here because it has to drive the interactive init
/// wizard via stdin.
async fn run_profile_action(action: ProfileAction) -> anyhow::Result<()> {
    match action {
        ProfileAction::List => {
            print!("{}", profile::render_profile_list());
            Ok(())
        }
        ProfileAction::New { name } => profile_new(&name).await,
        ProfileAction::Show { name } => {
            print!("{}", profile::render_profile_show(&name)?);
            Ok(())
        }
        ProfileAction::Remove { name, yes } => {
            profile::remove_profile(&name, yes)?;
            println!("  ✓ profile \"{name}\" removed");
            Ok(())
        }
        ProfileAction::Rename { from, to } => {
            profile::rename_profile(&from, &to)?;
            println!("  ✓ renamed \"{from}\" → \"{to}\"");
            Ok(())
        }
        ProfileAction::RenderService {
            name,
            kind,
            output,
            force,
        } => profile_render_service(&name, kind, output, force),
        ProfileAction::InstallService { name, kind, force } => {
            profile_install_service(&name, kind, force)
        }
        ProfileAction::UninstallService { name, kind, force } => {
            profile_uninstall_service(&name, kind, force)
        }
    }
}

/// Render a supervisor unit file for `name` and either print it to stdout
/// or write it to `output`.
///
/// This is a pure side-effect-light operation: it validates the profile
/// exists, picks the renderer for the requested `ServiceKind`, and emits
/// text. It never calls `systemctl` or `launchctl` — that's the job of
/// `profile install-service`. Keeping render and install split lets
/// operators review the generated unit (`| less`, diff against the
/// installed copy, pipe into a config-management tool) before anything
/// touches the supervisor.
///
/// # Path handling asymmetry
///
/// * **systemd** uses a *template* unit (`aivyx@.service`), so the
///   rendered text keeps `%h`/`%i` specifiers — one file serves every
///   profile and systemd expands the specifiers at load time. That's why
///   we pass `SystemdOpts::default()` on this branch: it already embeds
///   `%h/.aivyx/profiles/%i` and only the binary path needs overriding.
/// * **launchd** has no templating, so each profile needs its own plist
///   with concrete absolute paths. We materialize `dirs.root()` into the
///   `LaunchdOpts` for that branch.
fn profile_render_service(
    name: &str,
    kind: ServiceKind,
    output: Option<std::path::PathBuf>,
    force: bool,
) -> anyhow::Result<()> {
    let (resolved_kind, text, _dirs) = render_service_text(name, kind)?;

    match output {
        None => {
            // Print to stdout. Using `print!` (not `println!`) because
            // both renderers already terminate their output with a
            // trailing newline — double-newlining would break systemd
            // strict parsers that complain about blank lines after
            // `[Install]`.
            print!("{text}");
            Ok(())
        }
        Some(path) => {
            // Refuse-to-clobber unless `--force`. An operator who
            // hand-edited `aivyx@.service` with custom `ReadWritePaths=`
            // would lose that edit silently otherwise, and the whole
            // point of the render/install split is to keep operators in
            // control.
            if path.exists() && !force {
                anyhow::bail!(
                    "{} already exists; pass --force to overwrite",
                    path.display()
                );
            }
            std::fs::write(&path, &text).map_err(|e| {
                anyhow::anyhow!("failed to write rendered unit to {}: {e}", path.display())
            })?;
            println!(
                "  ✓ wrote {} unit to {}",
                resolved_kind.display_name(),
                path.display()
            );
            Ok(())
        }
    }
}

/// Shared entry point used by both `render-service` and `install-service`.
///
/// Validates the profile name, confirms the profile exists on disk,
/// resolves `ServiceKind::Auto`, pulls the running binary path via
/// `current_exe()`, and dispatches to the appropriate renderer. Returns
/// the resolved (non-`Auto`) kind, the rendered text, and the
/// `AivyxDirs` for the profile so callers can reach the profile root
/// without re-resolving it.
///
/// Extracting this keeps render-vs-install differences limited to their
/// output behavior (print/write vs. install-path resolution + perm
/// checks) — the "produce the text" contract is identical.
fn render_service_text(
    name: &str,
    kind: ServiceKind,
) -> anyhow::Result<(ServiceKind, String, AivyxDirs)> {
    // Validate the profile name (via `from_profile`) and confirm it
    // exists on disk. We refuse to render a unit for a profile that
    // hasn't been created, because the generated paths would point at
    // nothing and `systemctl start` would hard-fail with a confusing
    // "working directory not found" error at service start.
    let dirs = AivyxDirs::from_profile(name)
        .map_err(|e| anyhow::anyhow!("invalid profile name \"{name}\": {e}"))?;
    if !dirs.root().exists() {
        anyhow::bail!(
            "profile \"{name}\" does not exist at {}; create it first with `aivyx profile new {name}`",
            dirs.root().display()
        );
    }

    // `current_exe()` returns the path of the running binary. This is
    // what we embed in the rendered unit so the service always starts
    // the exact binary the operator just ran render-service from —
    // important when multiple aivyx versions coexist (e.g. a staged
    // upgrade under `/opt/aivyx/<version>/bin/aivyx`).
    let binary_path = std::env::current_exe()
        .map_err(|e| anyhow::anyhow!("failed to resolve current executable path: {e}"))?;

    let resolved_kind = kind.resolve();
    let text = match resolved_kind {
        ServiceKind::Systemd => {
            // Template unit: keep `%h`/`%i` in the default opts so one
            // file serves every profile. We only override the binary
            // path; everything else stays templated.
            let opts = profile::systemd::SystemdOpts {
                binary_path,
                ..Default::default()
            };
            profile::systemd::render_systemd_unit(&opts)
        }
        ServiceKind::Launchd => {
            // Per-profile plist: embed the concrete profile root.
            let opts = profile::launchd::LaunchdOpts::new(binary_path, dirs.root().to_path_buf());
            profile::launchd::render_launchd_plist(name, &opts)
        }
        ServiceKind::Windows => {
            // Per-profile PowerShell installer script. Windows has no
            // drop-in unit directory, so this script *is* the
            // deliverable — the operator runs it from an elevated
            // prompt to register with SCM.
            let opts =
                profile::windows::WindowsOpts::new(binary_path, dirs.root().to_path_buf(), name);
            profile::windows::render_windows_installer(name, &opts)
        }
        ServiceKind::Auto => {
            // `resolve()` never returns `Auto`; matching it keeps the
            // compiler happy without an `unreachable!()` panic path.
            unreachable!("ServiceKind::resolve never returns Auto")
        }
    };

    Ok((resolved_kind, text, dirs))
}

/// Install a rendered supervisor unit into the canonical user-level
/// location for the current platform, without invoking `systemctl`,
/// `launchctl`, or `New-Service`.
///
/// The install step is intentionally small and observable: write the
/// file, audit the passphrase sidecar, print the enable/start command.
/// It never starts the service itself — operators want to see
/// `systemctl --user status` / `launchctl print` / `Get-Service` output
/// with their own eyes and pick when to cut over, especially on
/// production hosts.
///
/// # Install paths
///
/// * **systemd** → `$XDG_CONFIG_HOME/systemd/user/aivyx@.service` (via
///   `dirs::config_dir()`, which falls back to `~/.config/`). It's a
///   template unit, so the filename is `aivyx@.service` regardless of
///   profile — instances are started as `aivyx@work.service` etc.
/// * **launchd** → `~/Library/LaunchAgents/com.aivyx.<name>.plist`. Per
///   profile, because launchd has no template mechanism.
/// * **windows** → `<profile_root>\install-service.ps1`. Windows has
///   no SCM drop-in directory, so the PowerShell installer script
///   lives inside the profile tree. The operator runs it from an
///   elevated prompt to actually register with SCM.
///
/// # Passphrase sidecar audit
///
/// All three supervisors pass the unlock passphrase via
/// `AIVYX_PASSPHRASE_FILE`. On systemd this is wired through
/// `LoadCredential=passphrase:<path>` → `%d/passphrase`, on launchd via
/// `EnvironmentVariables.AIVYX_PASSPHRASE_FILE=<profile_root>/passphrase`,
/// on Windows via the service's registry `Environment` REG_MULTI_SZ.
/// Either way the file must exist with owner-only read perms before
/// the service starts or unlock will fail with a confusing error
/// buried in journald / the log file / Windows Event Viewer. We check
/// and print actionable remediation instead — `chmod` on Unix,
/// `icacls` on Windows.
fn profile_install_service(name: &str, kind: ServiceKind, force: bool) -> anyhow::Result<()> {
    let (resolved_kind, text, dirs) = render_service_text(name, kind)?;

    // Resolve the canonical install path per platform via the
    // library-side helper. Injecting `config_dir` / `home_dir` /
    // `profile_root` keeps `resolve_install_path` pure and unit-
    // testable; we source the real values from `dirs::` and the
    // `AivyxDirs` here in the binary layer.
    use profile::service_install::{
        PassphraseSidecarStatus, ResolvedServiceKind, audit_passphrase_sidecar as audit_pp,
        resolve_install_path,
    };
    let resolved_for_lib = match resolved_kind {
        ServiceKind::Systemd => ResolvedServiceKind::Systemd,
        ServiceKind::Launchd => ResolvedServiceKind::Launchd,
        ServiceKind::Windows => ResolvedServiceKind::Windows,
        ServiceKind::Auto => unreachable!("ServiceKind::resolve never returns Auto"),
    };
    let config_dir = dirs::config_dir();
    let home_dir = dirs::home_dir();
    let profile_root = dirs.root().to_path_buf();
    let install_path = resolve_install_path(
        resolved_for_lib,
        name,
        config_dir.as_deref(),
        home_dir.as_deref(),
        Some(profile_root.as_path()),
    )
    .ok_or_else(|| match resolved_for_lib {
        ResolvedServiceKind::Systemd => {
            anyhow::anyhow!("could not resolve XDG config directory (no $HOME set?)")
        }
        ResolvedServiceKind::Launchd => {
            anyhow::anyhow!("could not resolve home directory (no $HOME set?)")
        }
        ResolvedServiceKind::Windows => {
            // Should be unreachable — profile_root is always Some
            // because we just resolved it from `dirs.root()`. But if
            // a future refactor makes that optional, we want a
            // user-facing error, not a panic.
            anyhow::anyhow!("could not resolve profile root for Windows installer script")
        }
    })?;

    // Refuse-to-clobber unless `--force`. Preserves operator edits.
    if install_path.exists() && !force {
        anyhow::bail!(
            "{} already exists; pass --force to overwrite",
            install_path.display()
        );
    }

    // Create the parent directory if it doesn't exist yet (common on
    // fresh systems — `~/.config/systemd/user/` is not created until
    // the first user unit lands there).
    if let Some(parent) = install_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            anyhow::anyhow!(
                "failed to create install directory {}: {e}",
                parent.display()
            )
        })?;
    }

    std::fs::write(&install_path, &text)
        .map_err(|e| anyhow::anyhow!("failed to write unit to {}: {e}", install_path.display()))?;

    // Audit the passphrase sidecar. All three renderers default to
    // `<profile_root>/passphrase` as the AIVYX_PASSPHRASE_FILE target,
    // so we check that path. A missing file is a warning, not an
    // error — the operator may be about to create it.
    let pp_path = dirs.root().join("passphrase");
    let pp_status = audit_pp(&pp_path);

    println!(
        "  ✓ installed {} unit to {}",
        resolved_kind.display_name(),
        install_path.display()
    );

    // The remediation hint differs between Unix and Windows because
    // the tooling differs (`chmod` vs `icacls`). Route on
    // `resolved_kind` rather than `cfg!(windows)` so a Linux dev
    // rendering a Windows unit still sees the correct icacls hint.
    let is_windows = resolved_kind == ServiceKind::Windows;
    match pp_status {
        PassphraseSidecarStatus::OkSecure => {
            if is_windows {
                println!("  ✓ passphrase sidecar {} is present", pp_path.display());
                println!(
                    "    (DACL not checked on Windows — verify owner-only access with `icacls`)"
                );
            } else {
                println!(
                    "  ✓ passphrase sidecar {} is present and 0600",
                    pp_path.display()
                );
            }
        }
        PassphraseSidecarStatus::Missing => {
            println!();
            println!("  ! passphrase sidecar not found at {}", pp_path.display());
            println!("    The service will fail to unlock until you create it. Run:");
            println!();
            if is_windows {
                println!(
                    "      New-Item -ItemType File -Path \"{}\"",
                    pp_path.display()
                );
                println!("      notepad \"{}\"", pp_path.display());
                println!(
                    "      icacls \"{}\" /inheritance:r /grant:r \"$env:USERNAME:(R)\"",
                    pp_path.display()
                );
            } else {
                println!("      install -m 0600 /dev/null {}", pp_path.display());
                println!("      $EDITOR {}", pp_path.display());
            }
            println!();
        }
        PassphraseSidecarStatus::InsecurePerms(mode) => {
            println!();
            println!(
                "  ! passphrase sidecar {} has insecure perms 0{:o}",
                pp_path.display(),
                mode
            );
            println!("    Tighten it before starting the service:");
            println!();
            println!("      chmod 0600 {}", pp_path.display());
            println!();
        }
    }

    // Print the enable/start command. We deliberately do NOT run it
    // ourselves — see the function doc for why.
    println!();
    match resolved_kind {
        ServiceKind::Systemd => {
            println!("  Next: enable and start the service:");
            println!();
            println!("      systemctl --user daemon-reload");
            println!("      systemctl --user enable --now aivyx@{name}.service");
            println!();
            println!("  Inspect with:");
            println!();
            println!("      systemctl --user status aivyx@{name}.service");
            println!("      journalctl --user -u aivyx@{name}.service -f");
        }
        ServiceKind::Launchd => {
            println!("  Next: load the agent:");
            println!();
            println!(
                "      launchctl bootstrap gui/$(id -u) {}",
                install_path.display()
            );
            println!("      launchctl enable gui/$(id -u)/com.aivyx.{name}");
            println!();
            println!("  Inspect with:");
            println!();
            println!("      launchctl print gui/$(id -u)/com.aivyx.{name}");
        }
        ServiceKind::Windows => {
            // The deliverable on Windows is a script — the operator
            // has to actually *run* it from an elevated prompt to
            // register with SCM. Unlike systemd/launchd, we cannot
            // merely drop a file in place and have the supervisor
            // pick it up, so the next-step hint is a manual
            // invocation.
            let service_name = profile::windows::service_name_for_profile(name);
            println!("  Next: run the installer from an elevated PowerShell prompt:");
            println!();
            println!(
                "      powershell.exe -ExecutionPolicy Bypass -File \"{}\"",
                install_path.display()
            );
            println!();
            println!("  Then start the service:");
            println!();
            println!("      Start-Service {service_name}");
            println!();
            println!("  Inspect with:");
            println!();
            println!("      Get-Service {service_name} | Format-List *");
        }
        ServiceKind::Auto => unreachable!(),
    }

    Ok(())
}

/// Remove a previously installed supervisor unit from its canonical
/// location.
///
/// Symmetric with [`profile_install_service`]: uses the same
/// `resolve_install_path` helper so they can never disagree on where
/// the file lives. Refuses to run if the profile's own server is live
/// (pidfile held) — removing the unit out from under a running
/// service would leave it un-recoverable across the next restart.
///
/// # What this does NOT do
///
/// * It does NOT stop the live service (`systemctl --user stop`,
///   `launchctl bootout`, `Stop-Service`). Operators may have
///   in-flight work they want to flush first; we print the stop
///   command and let them run it.
/// * It does NOT disable auto-start (`systemctl --user disable`,
///   `launchctl disable`, `Set-Service -StartupType Disabled`).
///   Same reason — the operator may want to re-enable later.
/// * It does NOT remove the passphrase sidecar. That's the operator's
///   secret; touching it here would be surprising and destructive.
/// * On Windows, it removes only the installer *script*. The actual
///   SCM registration (if the operator has already run the script)
///   must be undone separately via `sc.exe delete AivyxPA-<name>`
///   or `Remove-Service`. We print that command.
fn profile_uninstall_service(name: &str, kind: ServiceKind, force: bool) -> anyhow::Result<()> {
    // Validate the profile name and confirm it exists on disk. We
    // need `dirs.root()` to pass as `profile_root` for the Windows
    // branch of `resolve_install_path`, and to check the pidfile.
    let dirs = AivyxDirs::from_profile(name)
        .map_err(|e| anyhow::anyhow!("invalid profile name \"{name}\": {e}"))?;
    if !dirs.root().exists() {
        anyhow::bail!(
            "profile \"{name}\" does not exist at {}; nothing to uninstall",
            dirs.root().display()
        );
    }

    // Refuse if the profile's server is live, unless --force. This
    // mirrors `profile remove` and `profile rename`, which use the
    // same pidfile probe for the same reason: mutating paths under a
    // running server invalidates handles it holds in memory.
    if !force && let Some(pid) = PidFile::read_peer(dirs.root().join("aivyx.pid")) {
        anyhow::bail!(
            "profile \"{name}\" is currently running (pid {pid}); \
                 stop it before uninstalling, or pass --force if you know what you're doing"
        );
    }

    let resolved_kind = kind.resolve();
    use profile::service_install::{ResolvedServiceKind, resolve_install_path};
    let resolved_for_lib = match resolved_kind {
        ServiceKind::Systemd => ResolvedServiceKind::Systemd,
        ServiceKind::Launchd => ResolvedServiceKind::Launchd,
        ServiceKind::Windows => ResolvedServiceKind::Windows,
        ServiceKind::Auto => unreachable!("ServiceKind::resolve never returns Auto"),
    };
    let install_path = resolve_install_path(
        resolved_for_lib,
        name,
        dirs::config_dir().as_deref(),
        dirs::home_dir().as_deref(),
        Some(dirs.root()),
    )
    .ok_or_else(|| anyhow::anyhow!("could not resolve install path for {resolved_kind:?}"))?;

    if !install_path.exists() {
        // Not an error — the operator may be re-running this to make
        // sure the file is gone, or the unit may have been removed by
        // hand earlier. Print a friendly note and still print the
        // stop/disable hints so they have a copy of the cleanup
        // commands.
        println!(
            "  (no {} unit at {} — nothing to remove)",
            resolved_kind.display_name(),
            install_path.display()
        );
    } else {
        std::fs::remove_file(&install_path)
            .map_err(|e| anyhow::anyhow!("failed to remove {}: {e}", install_path.display()))?;
        println!(
            "  ✓ removed {} unit at {}",
            resolved_kind.display_name(),
            install_path.display()
        );
    }

    // Print the stop/disable command. Symmetric with the install
    // handler's enable/start hint — same reasons for not running it
    // ourselves.
    println!();
    match resolved_kind {
        ServiceKind::Systemd => {
            println!("  If the service is still running, stop and disable it:");
            println!();
            println!("      systemctl --user disable --now aivyx@{name}.service");
            println!("      systemctl --user daemon-reload");
        }
        ServiceKind::Launchd => {
            println!("  If the agent is still loaded, bootout and disable it:");
            println!();
            println!("      launchctl bootout gui/$(id -u)/com.aivyx.{name}");
            println!("      launchctl disable gui/$(id -u)/com.aivyx.{name}");
        }
        ServiceKind::Windows => {
            // On Windows, removing the installer script does nothing
            // to the SCM registration — the operator still has to
            // delete the service itself. Give them the exact
            // invocation.
            let service_name = profile::windows::service_name_for_profile(name);
            println!("  If the service is registered with SCM, stop and delete it:");
            println!();
            println!("      Stop-Service {service_name} -Force");
            println!("      if (Get-Command Remove-Service -ErrorAction SilentlyContinue) {{");
            println!("          Remove-Service {service_name}");
            println!("      }} else {{");
            println!("          sc.exe delete {service_name}");
            println!("      }}");
        }
        ServiceKind::Auto => unreachable!(),
    }

    Ok(())
}

/// Create a new profile at `~/.aivyx/profiles/<name>/`, run the interactive
/// init wizard against it, and auto-assign a free server port.
///
/// Stays in `main.rs` (not the `profile` module) because it drives the
/// interactive init wizard via stdin, which makes it unsuitable for
/// library reuse or unit testing. All the non-interactive plumbing —
/// port allocation, config append, name validation — lives in
/// `aivyx_pa::profile`.
async fn profile_new(name: &str) -> anyhow::Result<()> {
    let dirs = AivyxDirs::from_profile(name)
        .map_err(|e| anyhow::anyhow!("invalid profile name \"{name}\": {e}"))?;

    if dirs.root().exists() {
        anyhow::bail!(
            "profile \"{name}\" already exists at {}",
            dirs.root().display()
        );
    }

    let port = profile::allocate_free_profile_port()?;

    println!(
        "  → creating profile \"{name}\" at {}",
        dirs.root().display()
    );
    println!("  → assigned port {port}");

    let _passphrase = init::run(dirs.root()).await?;

    if !dirs.is_initialized() {
        anyhow::bail!("init wizard did not complete; profile \"{name}\" left in partial state");
    }

    profile::append_server_port(&dirs, port)?;

    println!();
    println!("  ✓ profile \"{name}\" ready");
    println!("    run: aivyx --profile {name}");
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

    // C1: Run one-shot legacy-prefix migrations on the PA store before
    // any component can read it. Both migrations are idempotent and
    // gated by marker keys, so after the first successful run every
    // subsequent startup is an O(1) marker check.
    //
    // The PA store holds two different namespaces encrypted under the
    // derived `conversation_key`: (a) chat session records (`pa_chat/`)
    // and (b) conversation snapshots the TUI wrote directly via the old
    // `snapshot:` prefix before the C1 refactor routed them through the
    // typed `session_store::save_snapshot` API. The agent-session
    // migration catches orphaned snapshot records so the refreshed
    // branch manager can still see them under the new prefix.
    if let Err(e) = aivyx_pa::sessions::run_pa_chat_migration(&store, &keys.conversation_key) {
        tracing::warn!("pa_chat namespace migration failed: {e}");
    }
    if let Err(e) =
        aivyx_agent::session_store::run_migration_on_store(&store, &keys.conversation_key)
    {
        tracing::warn!("agent_snapshot namespace migration failed on PA store: {e}");
    }

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
