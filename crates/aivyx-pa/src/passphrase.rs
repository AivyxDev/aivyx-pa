//! Passphrase resolution for unlocking the master-key envelope.
//!
//! The aivyx binary needs to unlock its encrypted store on every start.
//! Three distinct deployment models drive three ways of supplying the
//! passphrase, each with different security and ergonomics trade-offs:
//!
//! - **Interactive CLI** — operator types the passphrase at a prompt.
//!   Most secure at rest (no on-disk copy), least automatable.
//! - **Environment variable** — `AIVYX_PASSPHRASE=<literal>`. Easy for
//!   one-shot CI runs and `docker run -e`, but leaks through
//!   `systemctl show`, `ps auxe`, and crash dumps. Documented as "avoid
//!   in production".
//! - **Credentials file** — `AIVYX_PASSPHRASE_FILE=<path>` points at a
//!   file whose contents are the passphrase. This is the path systemd
//!   `LoadCredential=` exposes under `$CREDENTIALS_DIRECTORY` and what
//!   launchd's `EnvironmentVariables=` sets at service start. Isolates
//!   the passphrase from process listings and makes filesystem ACLs
//!   (`chmod 0600`) the trust boundary.
//!
//! This module owns the precedence rules between those three, so every
//! caller (the server startup path, future tooling, tests) gets the
//! same semantics. Rules are documented on [`resolve_passphrase`].

use zeroize::Zeroizing;

/// Resolve the aivyx master-key passphrase using a three-tier precedence,
/// checked in order — more explicit wins:
///
/// 1. **`AIVYX_PASSPHRASE`** env var set to a literal string. Highest
///    precedence so an operator setting it for a specific invocation
///    overrides whatever a supervisor (systemd/launchd) configured.
/// 2. **`AIVYX_PASSPHRASE_FILE`** env var set to a path. The file is
///    read, `trim_end`'d (to strip an accidental trailing newline from
///    `echo "secret" > file`), and the result becomes the passphrase.
///    Leading whitespace is preserved because it could theoretically
///    be part of the passphrase — if that matters, document it in a
///    comment next to whatever writes the file.
/// 3. **Interactive prompt** — a closure the caller provides. This is
///    the CLI fallback. Passed in (rather than hardcoded to reading
///    stdin) so tests can exercise the full precedence chain without
///    needing a TTY.
///
/// The returned value is `Zeroizing<String>` so the bytes are scrubbed
/// from memory when the caller drops it, regardless of which tier
/// produced them.
///
/// # Errors
///
/// Returns an error only when tier 2 is selected and the file cannot
/// be read. Tier 1 (env var read) and tier 3 (prompt) cannot fail from
/// this function's perspective — if the prompt itself fails, that's
/// the caller's closure's problem.
pub fn resolve_passphrase<F>(prompt: F) -> anyhow::Result<Zeroizing<String>>
where
    F: FnOnce() -> Zeroizing<String>,
{
    // Tier 1: literal env var wins over everything.
    if let Ok(p) = std::env::var("AIVYX_PASSPHRASE") {
        return Ok(Zeroizing::new(p));
    }

    // Tier 2: credentials file. The aivyx service units set this via
    // systemd `LoadCredential=` or launchd `EnvironmentVariables=`.
    if let Ok(path) = std::env::var("AIVYX_PASSPHRASE_FILE") {
        let contents = std::fs::read_to_string(&path).map_err(|e| {
            anyhow::anyhow!("failed to read AIVYX_PASSPHRASE_FILE at {path}: {e}")
        })?;
        return Ok(Zeroizing::new(contents.trim_end().to_string()));
    }

    // Tier 3: interactive prompt. Last resort.
    Ok(prompt())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Serializes env-var mutation across tests. `AIVYX_PASSPHRASE` and
    /// `AIVYX_PASSPHRASE_FILE` are process-global, so two tests flipping
    /// them concurrently would stomp on each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that records the current values of both passphrase
    /// env vars and restores them on drop. Lets a test freely mutate
    /// the env without leaving stale state for sibling tests.
    struct EnvGuard {
        saved_literal: Option<std::ffi::OsString>,
        saved_file: Option<std::ffi::OsString>,
    }
    impl EnvGuard {
        fn new() -> Self {
            let saved_literal = std::env::var_os("AIVYX_PASSPHRASE");
            let saved_file = std::env::var_os("AIVYX_PASSPHRASE_FILE");
            // SAFETY: Rust 2024 requires unsafe around remove_var.
            // ENV_LOCK serializes tests in this module, so no other
            // thread is reading these vars concurrently.
            unsafe {
                std::env::remove_var("AIVYX_PASSPHRASE");
                std::env::remove_var("AIVYX_PASSPHRASE_FILE");
            }
            Self { saved_literal, saved_file }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            // SAFETY: same as `new` — ENV_LOCK holds.
            unsafe {
                match &self.saved_literal {
                    Some(v) => std::env::set_var("AIVYX_PASSPHRASE", v),
                    None => std::env::remove_var("AIVYX_PASSPHRASE"),
                }
                match &self.saved_file {
                    Some(v) => std::env::set_var("AIVYX_PASSPHRASE_FILE", v),
                    None => std::env::remove_var("AIVYX_PASSPHRASE_FILE"),
                }
            }
        }
    }

    /// A prompt closure that panics if invoked. Tests that expect to
    /// resolve via an env var (tier 1 or 2) use this to confirm the
    /// prompt path was NOT taken — an accidental fall-through would
    /// turn into a loud test failure instead of silently succeeding
    /// on the wrong tier.
    fn never_prompt() -> Zeroizing<String> {
        panic!("resolve_passphrase should not have fallen through to the interactive prompt");
    }

    #[test]
    fn tier_1_literal_env_var_wins() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // SAFETY: ENV_LOCK held.
        unsafe {
            std::env::set_var("AIVYX_PASSPHRASE", "hunter2");
        }
        let got = resolve_passphrase(never_prompt).unwrap();
        assert_eq!(&*got, "hunter2");
    }

    #[test]
    fn tier_1_beats_tier_2_when_both_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // Write a tempfile containing "from-file" and point TIER 2 at it,
        // but also set TIER 1 — the literal should win.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "from-file").unwrap();
        unsafe {
            std::env::set_var("AIVYX_PASSPHRASE", "from-literal");
            std::env::set_var("AIVYX_PASSPHRASE_FILE", f.path());
        }
        let got = resolve_passphrase(never_prompt).unwrap();
        assert_eq!(&*got, "from-literal");
    }

    #[test]
    fn tier_2_credentials_file_strips_trailing_newline() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // `echo "secret" > file` leaves a trailing `\n` that would
        // silently break unlock if not trimmed. This test pins that
        // behaviour so a future refactor can't drop it.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "secret").unwrap();
        unsafe {
            std::env::set_var("AIVYX_PASSPHRASE_FILE", f.path());
        }
        let got = resolve_passphrase(never_prompt).unwrap();
        assert_eq!(&*got, "secret");
    }

    #[test]
    fn tier_2_credentials_file_preserves_leading_whitespace() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // Leading whitespace could in principle be part of the
        // passphrase. We use `trim_end` not `trim`, so a leading space
        // must survive.
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(b" secret\n").unwrap();
        unsafe {
            std::env::set_var("AIVYX_PASSPHRASE_FILE", f.path());
        }
        let got = resolve_passphrase(never_prompt).unwrap();
        assert_eq!(&*got, " secret");
    }

    #[test]
    fn tier_2_credentials_file_missing_path_errors() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        unsafe {
            std::env::set_var(
                "AIVYX_PASSPHRASE_FILE",
                "/nonexistent/path/this-should-not-exist-12345",
            );
        }
        let err = resolve_passphrase(never_prompt).unwrap_err();
        assert!(
            err.to_string().contains("AIVYX_PASSPHRASE_FILE"),
            "error message should name the env var; got: {err}"
        );
    }

    #[test]
    fn tier_3_prompt_fallback_when_nothing_set() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // Neither env var set — the closure must run.
        let got = resolve_passphrase(|| Zeroizing::new("prompted".to_string())).unwrap();
        assert_eq!(&*got, "prompted");
    }

    #[test]
    fn tier_1_wins_even_when_file_would_error() {
        let _lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let _g = EnvGuard::new();
        // Set TIER 1 to a valid literal and TIER 2 to a bad path. The
        // bad path must never be read because TIER 1 short-circuits.
        unsafe {
            std::env::set_var("AIVYX_PASSPHRASE", "literal-wins");
            std::env::set_var(
                "AIVYX_PASSPHRASE_FILE",
                "/nonexistent/would-error-if-read",
            );
        }
        let got = resolve_passphrase(never_prompt).unwrap();
        assert_eq!(&*got, "literal-wins");
    }
}
