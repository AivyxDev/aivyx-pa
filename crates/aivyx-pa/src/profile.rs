//! Multi-agent profile lifecycle helpers.
//!
//! This module owns all the non-interactive logic behind `aivyx profile
//! list|show|remove|rename` — the bits that read filesystem state, allocate
//! ports, and mutate the profile tree. `profile new` stays in `main.rs`
//! because it has to drive the interactive init wizard (stdin prompts),
//! which makes it unsuitable for library reuse or integration testing.
//!
//! Everything here is deterministic and free of terminal I/O: the `list`
//! and `show` actions return rendered strings rather than writing to
//! stdout, so tests can assert on exact output without capturing pipes.
//!
//! Profiles live under `~/.aivyx/profiles/<name>/` and are created/destroyed
//! as whole directory trees. Every operation respects two invariants:
//!
//! 1. A profile whose pidfile is held by a live process is considered
//!    "locked" — we refuse to remove or rename it, because both operations
//!    would invalidate paths the running server holds in memory.
//! 2. Profile names are validated by `AivyxDirs::from_profile`, which
//!    rejects path traversal, empty strings, and illegal characters
//!    before any filesystem access happens.
//!
//! Submodules:
//!
//! - [`systemd`]: renders user-level `aivyx@.service` template units for
//!   Linux operators who want multi-profile process supervision under
//!   `systemctl --user`. Deterministic string generation — no filesystem
//!   I/O, no shelling out to `systemctl`.
//! - [`launchd`]: renders per-profile `com.aivyx.<name>.plist` files for
//!   macOS operators, to be loaded via `launchctl bootstrap gui/$(id -u)`.
//!   Launchd has no template-unit concept, so we emit one plist per
//!   profile rather than one shared template.
//! - [`service_install`]: pure helpers for install-path resolution and
//!   passphrase-sidecar permission auditing. Extracted from the
//!   `profile install-service` handler in `main.rs` so it's testable
//!   without subprocess plumbing.

pub mod launchd;
pub mod service_install;
pub mod systemd;
pub mod windows;

use crate::config;
use crate::pidfile::PidFile;
use aivyx_config::AivyxDirs;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::Path;

/// Display name used for the legacy single-agent root (`~/.aivyx/`) when
/// it shows up in listings alongside named profiles.
pub const DEFAULT_PROFILE_NAME: &str = "(default)";

/// Gathered status for one profile — used by `list` and `show`.
///
/// This is a view over *persisted* state only: no master-key decryption,
/// no encrypted-store access, so building one never prompts for a
/// passphrase. That's what makes `list` safe to run on a cold machine.
#[derive(Debug, Clone)]
pub struct ProfileStatus {
    /// Profile display name — `(default)` for the legacy root.
    pub name: String,
    /// Whether `dirs.is_initialized()` returns true (config + master key
    /// both present on disk).
    pub initialized: bool,
    /// Persona preset from `[agent].persona`, or `None` if the section
    /// is missing (fresh profile not yet run through init).
    pub persona: Option<String>,
    /// Effective server port after the three-tier resolution
    /// (`[server].port` → `DEFAULT_API_PORT`). There is no CLI override
    /// at list-time, so this is purely config-derived.
    pub port: u16,
    /// PID of the live process currently holding the pidfile, if any.
    /// `None` means either "pidfile absent" or "pidfile stale" — both
    /// are equivalent from the caller's point of view.
    pub running_pid: Option<u32>,
}

/// Collect status for one profile handle.
///
/// Never fails: unreadable config files are reported as `initialized =
/// false` rather than aborting, so one broken profile can't hide the
/// others when the caller is iterating over many.
pub fn collect_profile_status(name: &str, dirs: &AivyxDirs) -> ProfileStatus {
    let initialized = dirs.is_initialized();
    let (persona, port) = if dirs.config_path().exists() {
        let cfg = config::PaConfig::load(dirs.config_path());
        let persona = cfg.agent.as_ref().map(|a| a.persona.clone());
        let port = cfg
            .server
            .as_ref()
            .and_then(|s| s.port)
            .unwrap_or(config::DEFAULT_API_PORT);
        (persona, port)
    } else {
        (None, config::DEFAULT_API_PORT)
    };
    let running_pid = PidFile::read_peer(dirs.root().join("aivyx.pid"));
    ProfileStatus {
        name: name.to_string(),
        initialized,
        persona,
        port,
        running_pid,
    }
}

/// Enumerate every profile visible on disk: the legacy default root (if
/// it exists) followed by every named profile under `profiles/`.
///
/// Profiles that fail `AivyxDirs::from_profile` validation are skipped
/// silently — this function is used by `list` and the port allocator,
/// both of which prefer "best effort" over "hard fail on one bad dir".
pub fn enumerate_profiles() -> Vec<(String, AivyxDirs)> {
    let mut out = Vec::new();

    if let Ok(default_dirs) = AivyxDirs::from_default() {
        if default_dirs.root().exists() {
            out.push((DEFAULT_PROFILE_NAME.to_string(), default_dirs));
        }
    }

    if let Ok(names) = AivyxDirs::list_profiles() {
        for name in names {
            if let Ok(dirs) = AivyxDirs::from_profile(&name) {
                out.push((name, dirs));
            }
        }
    }
    out
}

/// Pick the next free server port for a new profile.
///
/// Starts at `config::DEFAULT_API_PORT` and walks upward until it finds
/// a value not already claimed by any sibling profile's `[server].port`.
/// This is a *logical* allocation — it does not check bindability, because
/// that race is unwinnable at config time. Operators wanting a specific
/// port can still edit `[server].port` by hand after creation.
pub fn allocate_free_profile_port() -> anyhow::Result<u16> {
    let mut used: BTreeSet<u16> = BTreeSet::new();

    for (_, dirs) in enumerate_profiles() {
        if !dirs.config_path().exists() {
            continue;
        }
        let cfg = config::PaConfig::load(dirs.config_path());
        if let Some(p) = cfg.server.as_ref().and_then(|s| s.port) {
            used.insert(p);
        }
    }

    let mut candidate = config::DEFAULT_API_PORT;
    while used.contains(&candidate) {
        candidate = candidate
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("ran out of u16 ports scanning for free profile port"))?;
    }
    Ok(candidate)
}

/// Append a `[server]\nport = N` section to a profile's `config.toml`.
///
/// Uses append-mode rather than deserialize/reserialize because the init
/// wizard writes the config imperatively and we don't want to lose the
/// comments it emits. The profile is assumed to be brand-new with no
/// existing `[server]` section; if that invariant is ever violated the
/// lint in `PaConfig::lint` will surface the duplicate.
pub fn append_server_port(dirs: &AivyxDirs, port: u16) -> anyhow::Result<()> {
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(dirs.config_path())?;
    writeln!(f, "\n[server]\nport = {port}")?;
    f.sync_all()?;
    Ok(())
}

/// Render `aivyx profile list` output as a string.
///
/// Returning a string (rather than writing to stdout) keeps this pure and
/// testable. Callers do the actual printing. Columns auto-size to the
/// longest value so narrow terminals don't get wrecked.
pub fn render_profile_list() -> String {
    let mut out = String::new();
    let statuses: Vec<ProfileStatus> = enumerate_profiles()
        .into_iter()
        .map(|(name, dirs)| collect_profile_status(&name, &dirs))
        .collect();

    if statuses.is_empty() {
        out.push_str("  no profiles found\n");
        out.push_str("  create one with: aivyx profile new <name>\n");
        return out;
    }

    let name_w = statuses.iter().map(|s| s.name.len()).max().unwrap_or(4).max(4);
    let persona_w = statuses
        .iter()
        .map(|s| s.persona.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(7)
        .max(7);

    let _ = writeln!(
        out,
        "  {:<name_w$}  {:<8}  {:<persona_w$}  {:<5}  {}",
        "NAME",
        "STATUS",
        "PERSONA",
        "PORT",
        "PID",
        name_w = name_w,
        persona_w = persona_w,
    );
    for s in &statuses {
        let status = if !s.initialized {
            "new"
        } else if s.running_pid.is_some() {
            "running"
        } else {
            "idle"
        };
        let persona = s.persona.as_deref().unwrap_or("-");
        let pid = s
            .running_pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".to_string());
        let _ = writeln!(
            out,
            "  {:<name_w$}  {:<8}  {:<persona_w$}  {:<5}  {}",
            s.name,
            status,
            persona,
            s.port,
            pid,
            name_w = name_w,
            persona_w = persona_w,
        );
    }
    out
}

/// Render `aivyx profile show <name>` output as a string, or error if
/// the profile does not exist / the name is invalid.
pub fn render_profile_show(name: &str) -> anyhow::Result<String> {
    let dirs = AivyxDirs::from_profile(name)
        .map_err(|e| anyhow::anyhow!("invalid profile name \"{name}\": {e}"))?;

    if !dirs.root().exists() {
        anyhow::bail!("profile \"{name}\" does not exist");
    }

    let status = collect_profile_status(name, &dirs);
    let mut out = String::new();
    let _ = writeln!(out, "  profile: {}", status.name);
    let _ = writeln!(out, "  root:    {}", dirs.root().display());
    let _ = writeln!(out, "  config:  {}", dirs.config_path().display());
    let _ = writeln!(out, "  store:   {}", dirs.store_path().display());
    let _ = writeln!(out, "  keys:    {}", dirs.master_key_path().display());
    let _ = writeln!(out);
    let _ = writeln!(out, "  initialized: {}", status.initialized);
    let _ = writeln!(out, "  persona:     {}", status.persona.as_deref().unwrap_or("-"));
    let _ = writeln!(out, "  port:        {}", status.port);
    match status.running_pid {
        Some(pid) => {
            let _ = writeln!(out, "  running:     yes (pid {pid})");
        }
        None => {
            let _ = writeln!(out, "  running:     no");
        }
    }
    Ok(out)
}

/// Remove a profile and all of its data.
///
/// Safety rails applied in this order before any filesystem mutation:
/// 1. Name validates via `from_profile`.
/// 2. Profile root exists.
/// 3. `confirm` is true (`--yes` was passed).
/// 4. Pidfile is not held by a live process.
/// 5. Best-effort zero-overwrite the master key envelope.
/// 6. `remove_dir_all` the root.
pub fn remove_profile(name: &str, confirm: bool) -> anyhow::Result<()> {
    let dirs = AivyxDirs::from_profile(name)
        .map_err(|e| anyhow::anyhow!("invalid profile name \"{name}\": {e}"))?;

    if !dirs.root().exists() {
        anyhow::bail!("profile \"{name}\" does not exist");
    }

    if !confirm {
        anyhow::bail!(
            "refusing to remove profile \"{name}\" without --yes. \
             This operation is permanent and deletes all encrypted data for this profile."
        );
    }

    if let Some(pid) = PidFile::read_peer(dirs.root().join("aivyx.pid")) {
        anyhow::bail!(
            "profile \"{name}\" is currently running (pid {pid}); stop it before removing"
        );
    }

    let key_path = dirs.master_key_path();
    if key_path.exists() {
        if let Err(e) = zero_overwrite_file(&key_path) {
            tracing::warn!(
                "could not zero-overwrite master key envelope at {}: {e}",
                key_path.display()
            );
        }
    }

    std::fs::remove_dir_all(dirs.root())?;
    Ok(())
}

/// Rename a profile directory.
///
/// Refuses if either name fails validation, source doesn't exist,
/// destination already exists, or source is running. Config contents
/// are not rewritten — every path inside a profile is resolved relative
/// to `dirs.root()` at runtime, so moving the directory tree suffices.
pub fn rename_profile(from: &str, to: &str) -> anyhow::Result<()> {
    let from_dirs = AivyxDirs::from_profile(from)
        .map_err(|e| anyhow::anyhow!("invalid source profile name \"{from}\": {e}"))?;
    let to_dirs = AivyxDirs::from_profile(to)
        .map_err(|e| anyhow::anyhow!("invalid destination profile name \"{to}\": {e}"))?;

    if !from_dirs.root().exists() {
        anyhow::bail!("profile \"{from}\" does not exist");
    }
    if to_dirs.root().exists() {
        anyhow::bail!("profile \"{to}\" already exists");
    }

    if let Some(pid) = PidFile::read_peer(from_dirs.root().join("aivyx.pid")) {
        anyhow::bail!(
            "profile \"{from}\" is currently running (pid {pid}); stop it before renaming"
        );
    }

    if let Some(parent) = to_dirs.root().parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::rename(from_dirs.root(), to_dirs.root())?;
    Ok(())
}

/// Overwrite a file's contents with zeros of the same length, then `sync`.
///
/// Best-effort only: on copy-on-write filesystems (btrfs, APFS, ZFS) the
/// original blocks are retained until the next GC pass, and flash wear-
/// levelling defeats the overwrite entirely. For true erasure users
/// should run `shred` or equivalent themselves. This exists to reduce
/// (not eliminate) the window where a master-key envelope sits in free
/// space after `remove_dir_all`.
pub fn zero_overwrite_file(path: &Path) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().write(true).open(path)?;
    let len = f.metadata()?.len() as usize;
    f.seek(SeekFrom::Start(0))?;
    let zeros = [0u8; 4096];
    let mut remaining = len;
    while remaining > 0 {
        let n = remaining.min(zeros.len());
        f.write_all(&zeros[..n])?;
        remaining -= n;
    }
    f.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Nanosecond-precision, per-process-unique suffix for tempdir names.
    /// Same pattern `pidfile.rs` uses — avoids pulling in `rand` just for
    /// collision-free test paths.
    fn unique_suffix() -> u128 {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        ts.wrapping_mul(1_000_003) ^ (std::process::id() as u128)
    }

    /// RAII guard that points `HOME` at a fresh tempdir and restores the
    /// original on drop. `AivyxDirs::default_root()` reads `dirs::home_dir()`
    /// which in turn reads `$HOME` on Unix, so rebinding `HOME` is the actual
    /// way to redirect profile discovery onto a tempdir. Using a guard
    /// (rather than leaking the env var across tests) is critical — without
    /// restoration, a panicking test would leave `HOME` pointing at a
    /// deleted tempdir and corrupt every subsequent test in the process.
    struct HomeGuard {
        path: std::path::PathBuf,
        original: Option<std::ffi::OsString>,
    }
    impl HomeGuard {
        fn new() -> Self {
            let original = std::env::var_os("HOME");
            let path = std::env::temp_dir().join(format!("aivyx-profile-test-{}", unique_suffix()));
            // `.aivyx/` sub-directory mirrors the real layout so `default_root()`
            // (which computes `$HOME/.aivyx`) points inside our sandbox. We do
            // NOT `create_dir_all` that subdir — leaving it absent lets tests
            // distinguish "no profiles at all" from "empty default root".
            std::fs::create_dir_all(&path).unwrap();
            // SAFETY: Rust 2024 requires unsafe around set_var. Safe here
            // because the enclosing ENV_LOCK mutex serializes all tests in
            // this module, so no other thread is reading `HOME` concurrently.
            unsafe {
                std::env::set_var("HOME", &path);
            }
            Self { path, original }
        }
        fn aivyx_root(&self) -> std::path::PathBuf {
            self.path.join(".aivyx")
        }
        fn ensure_aivyx_root(&self) {
            std::fs::create_dir_all(self.aivyx_root()).unwrap();
        }
    }
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            // SAFETY: same as set_var above — ENV_LOCK serializes access.
            unsafe {
                match &self.original {
                    Some(v) => std::env::set_var("HOME", v),
                    None => std::env::remove_var("HOME"),
                }
            }
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Write a minimal, valid `config.toml` with only an `[agent]` persona
    /// and optional `[server].port`. Mimics what `init::run` would emit
    /// for a freshly-created profile without actually running the wizard.
    fn write_minimal_config(dirs: &AivyxDirs, persona: &str, port: Option<u16>) {
        std::fs::create_dir_all(dirs.root()).unwrap();
        let mut toml = format!("[agent]\npersona = \"{persona}\"\n");
        if let Some(p) = port {
            toml.push_str(&format!("\n[server]\nport = {p}\n"));
        }
        std::fs::write(dirs.config_path(), toml).unwrap();
        // Also create a fake master-key file so `is_initialized()` returns
        // true — content doesn't matter for these tests since we never
        // decrypt.
        std::fs::create_dir_all(dirs.master_key_path().parent().unwrap()).unwrap();
        std::fs::write(dirs.master_key_path(), "{}").unwrap();
    }

    // NOTE: These tests all touch the global `AIVYX_HOME` env var, which
    // is process-wide state. They must run serially. Cargo's test runner
    // defaults to parallel threads, so we use a mutex to serialize them.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn allocate_port_starts_at_default_when_empty() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();
        let port = allocate_free_profile_port().unwrap();
        assert_eq!(port, config::DEFAULT_API_PORT);
    }

    #[test]
    fn allocate_port_skips_used_ports() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        // Create two profiles claiming ports 3100 and 3101; next should be 3102.
        let a = AivyxDirs::from_profile("alpha").unwrap();
        write_minimal_config(&a, "assistant", Some(3100));
        let b = AivyxDirs::from_profile("bravo").unwrap();
        write_minimal_config(&b, "coder", Some(3101));

        let port = allocate_free_profile_port().unwrap();
        assert_eq!(port, 3102);
    }

    #[test]
    fn append_server_port_writes_section() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("appendtest").unwrap();
        std::fs::create_dir_all(dirs.root()).unwrap();
        std::fs::write(dirs.config_path(), "[agent]\npersona = \"assistant\"\n").unwrap();

        append_server_port(&dirs, 3199).unwrap();
        let text = std::fs::read_to_string(dirs.config_path()).unwrap();
        assert!(text.contains("[server]"));
        assert!(text.contains("port = 3199"));

        // And it should round-trip through PaConfig.
        let cfg = config::PaConfig::load(dirs.config_path());
        assert_eq!(cfg.server.and_then(|s| s.port), Some(3199));
    }

    #[test]
    fn render_list_handles_truly_empty_home() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // HomeGuard creates the outer tempdir but NOT the `.aivyx/`
        // subdirectory, so `AivyxDirs::default_root().exists()` returns
        // false and `list_profiles` sees no named profiles either. This
        // is the only path to a "no profiles found" listing.
        let _home = HomeGuard::new();
        let out = render_profile_list();
        assert!(out.contains("no profiles found"), "output was: {out}");
        assert!(out.contains("aivyx profile new"));
    }

    #[test]
    fn render_list_reports_existing_default_root_as_new() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();
        // Create the `.aivyx/` root but leave it empty — no config, no keys.
        // This documents the intentional behaviour that an existing-but-
        // empty default root is reported as `(default)` with status `new`.
        _home.ensure_aivyx_root();
        let out = render_profile_list();
        assert!(out.contains("(default)"), "output was: {out}");
        assert!(out.contains("new"));
    }

    #[test]
    fn render_list_includes_created_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("listtest").unwrap();
        write_minimal_config(&dirs, "researcher", Some(3105));

        let out = render_profile_list();
        assert!(out.contains("listtest"), "output was: {out}");
        assert!(out.contains("researcher"));
        assert!(out.contains("3105"));
        assert!(out.contains("idle")); // not running
    }

    #[test]
    fn render_show_errors_for_missing_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();
        let err = render_profile_show("ghost").unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn render_show_errors_for_invalid_name() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();
        let err = render_profile_show("../escape").unwrap_err();
        assert!(
            err.to_string().contains("invalid profile name"),
            "got: {err}"
        );
    }

    #[test]
    fn render_show_reports_existing_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("showtest").unwrap();
        write_minimal_config(&dirs, "coder", Some(3110));

        let out = render_profile_show("showtest").unwrap();
        assert!(out.contains("profile: showtest"));
        assert!(out.contains("coder"));
        assert!(out.contains("3110"));
        assert!(out.contains("initialized: true"));
        assert!(out.contains("running:     no"));
    }

    #[test]
    fn remove_profile_refuses_without_confirm() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("keepme").unwrap();
        write_minimal_config(&dirs, "assistant", None);

        let err = remove_profile("keepme", false).unwrap_err();
        assert!(err.to_string().contains("--yes"));
        assert!(dirs.root().exists(), "profile should still exist");
    }

    #[test]
    fn remove_profile_deletes_tree_with_confirm() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("deleteme").unwrap();
        write_minimal_config(&dirs, "assistant", None);
        assert!(dirs.root().exists());

        remove_profile("deleteme", true).unwrap();
        assert!(!dirs.root().exists());
    }

    #[test]
    fn remove_profile_errors_for_missing() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();
        let err = remove_profile("nope", true).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn rename_profile_moves_directory() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let src = AivyxDirs::from_profile("oldname").unwrap();
        write_minimal_config(&src, "writer", Some(3120));
        assert!(src.root().exists());

        rename_profile("oldname", "newname").unwrap();

        assert!(!src.root().exists());
        let dst = AivyxDirs::from_profile("newname").unwrap();
        assert!(dst.root().exists());
        // Config content survives the rename untouched.
        let cfg = config::PaConfig::load(dst.config_path());
        assert_eq!(cfg.agent.map(|a| a.persona), Some("writer".to_string()));
        assert_eq!(cfg.server.and_then(|s| s.port), Some(3120));
    }

    #[test]
    fn rename_profile_refuses_existing_destination() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let a = AivyxDirs::from_profile("src").unwrap();
        write_minimal_config(&a, "assistant", None);
        let b = AivyxDirs::from_profile("dst").unwrap();
        write_minimal_config(&b, "assistant", None);

        let err = rename_profile("src", "dst").unwrap_err();
        assert!(err.to_string().contains("already exists"));
        // Both should still be there.
        assert!(a.root().exists());
        assert!(b.root().exists());
    }

    #[test]
    fn rename_profile_refuses_invalid_target() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let a = AivyxDirs::from_profile("validsrc").unwrap();
        write_minimal_config(&a, "assistant", None);

        let err = rename_profile("validsrc", "../escape").unwrap_err();
        assert!(err.to_string().contains("invalid destination profile name"));
        assert!(a.root().exists());
    }

    #[test]
    fn collect_status_for_uninitialized_profile() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _home = HomeGuard::new();

        let dirs = AivyxDirs::from_profile("partial").unwrap();
        std::fs::create_dir_all(dirs.root()).unwrap();
        // No config, no keys — directory exists but nothing inside.

        let status = collect_profile_status("partial", &dirs);
        assert_eq!(status.name, "partial");
        assert!(!status.initialized);
        assert_eq!(status.persona, None);
        assert_eq!(status.port, config::DEFAULT_API_PORT);
        assert_eq!(status.running_pid, None);
    }

    #[test]
    fn zero_overwrite_replaces_contents() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = HomeGuard::new();
        let path = home.path.join("sensitive.bin");
        std::fs::write(&path, b"secret-master-key-envelope").unwrap();

        zero_overwrite_file(&path).unwrap();

        let after = std::fs::read(&path).unwrap();
        assert_eq!(after.len(), "secret-master-key-envelope".len());
        assert!(after.iter().all(|&b| b == 0));
    }
}
