//! Install-path resolution and passphrase sidecar auditing for
//! `profile install-service`.
//!
//! This module is the pure, testable slice of the install flow. It
//! deliberately takes `config_dir` / `home_dir` as injected parameters
//! rather than calling `dirs::config_dir()` / `dirs::home_dir()`
//! internally, so tests can feed it a tempdir without touching the
//! real user environment.
//!
//! The actual `profile install-service` handler in `main.rs` wraps
//! these helpers, adds the `current_exe()` + `render_service_text`
//! + filesystem-write layer on top, and prints the operator-facing
//!   next-step hints. Everything in this module is side-effect-free
//!   except `audit_passphrase_sidecar`, which only *reads* metadata.

use std::path::{Path, PathBuf};

/// Which supervisor to target. Kept in parallel with the `ServiceKind`
/// enum in `main.rs` so we can test install-path resolution without
/// dragging clap types into the library.
///
/// The `Auto` variant is deliberately absent: callers at this layer
/// have already resolved the platform. Reintroducing `Auto` here would
/// just push an `unreachable!()` into every match.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResolvedServiceKind {
    /// systemd user-level template unit
    Systemd,
    /// macOS launchd per-profile plist
    Launchd,
    /// Windows Service Control Manager PowerShell installer script.
    ///
    /// Unlike systemd and launchd, there is no "canonical drop-in
    /// directory" for Windows — SCM reads its config from the
    /// registry. So the rendered script is written *into the profile
    /// root itself* (`<profile_root>\install-service.ps1`), where the
    /// operator can inspect and run it from an elevated PowerShell
    /// prompt.
    Windows,
}

/// Compute the canonical user-level install path for a unit of `kind`
/// targeting `profile_name`.
///
/// Parameters are injected rather than resolved internally so tests
/// can substitute a tempdir for `$HOME` / `$XDG_CONFIG_HOME`.
///
/// * `config_dir` — what `dirs::config_dir()` would return on the
///   target platform. For systemd this maps to
///   `<config_dir>/systemd/user/aivyx@.service`.
/// * `home_dir` — what `dirs::home_dir()` would return. For launchd
///   this maps to `<home_dir>/Library/LaunchAgents/com.aivyx.<name>.plist`.
/// * `profile_root` — the concrete profile root
///   (`<aivyx_home>/profiles/<name>/`). Used only on Windows, where
///   the rendered PowerShell installer script lives inside the
///   profile tree at `<profile_root>\install-service.ps1` because
///   SCM has no canonical drop-in directory.
///
/// Returns `None` if the required directory argument is missing for
/// the chosen kind (`config_dir` for systemd, `home_dir` for launchd,
/// `profile_root` for Windows). The caller turns that into a
/// user-facing "could not resolve ..." error.
///
/// Notes:
///
/// * systemd uses a *template* unit, so the filename is
///   `aivyx@.service` regardless of `profile_name`. The profile is
///   encoded in the instance name at `systemctl start` time, not in
///   the filename. We accept `profile_name` for symmetry with launchd
///   but the systemd branch ignores it.
/// * launchd embeds the profile name in the filename via the
///   reverse-DNS label `com.aivyx.<profile_name>.plist`. This matches
///   what `profile::launchd::label_for_profile` produces.
/// * Windows has no SCM drop-in directory; the rendered script lives
///   inside `profile_root`. The filename is fixed at
///   `install-service.ps1` so `uninstall-service` can find it
///   deterministically without scanning.
pub fn resolve_install_path(
    kind: ResolvedServiceKind,
    profile_name: &str,
    config_dir: Option<&Path>,
    home_dir: Option<&Path>,
    profile_root: Option<&Path>,
) -> Option<PathBuf> {
    match kind {
        ResolvedServiceKind::Systemd => {
            // Template unit: one filename for all profiles. See doc.
            let _ = profile_name;
            let _ = profile_root;
            config_dir.map(|c| c.join("systemd").join("user").join("aivyx@.service"))
        }
        ResolvedServiceKind::Launchd => {
            let _ = profile_root;
            home_dir.map(|h| {
                h.join("Library")
                    .join("LaunchAgents")
                    .join(format!("com.aivyx.{profile_name}.plist"))
            })
        }
        ResolvedServiceKind::Windows => {
            // No canonical drop-in dir → live alongside the profile.
            // Fixed filename so uninstall can find it without scanning.
            let _ = profile_name;
            let _ = config_dir;
            let _ = home_dir;
            profile_root.map(|p| p.join("install-service.ps1"))
        }
    }
}

/// Result of auditing the passphrase sidecar file. Used by the install
/// handler to decide what remediation hint (if any) to print.
#[derive(Debug, PartialEq, Eq)]
pub enum PassphraseSidecarStatus {
    /// File exists with mode `0600` (or tighter — e.g. `0400`).
    /// Nothing to do; safe to start the service.
    OkSecure,
    /// File does not exist. Operator must create it before starting
    /// the service, or the unlock step will fail.
    Missing,
    /// File exists but the group / other bits are non-zero, meaning
    /// the passphrase is readable by more than just the owner. The
    /// masked low-9 mode bits are included so the warning can name
    /// the actual value.
    InsecurePerms(u32),
}

/// Check whether the passphrase file at `path` exists and whether its
/// permissions are tight enough for a service that trusts its
/// contents.
///
/// On Unix, "tight enough" means the group and other bits are all zero
/// (`0600` is the canonical form; `0400` is also acceptable and
/// common when operators write the file once and drop write
/// permission). On non-Unix we can't inspect POSIX mode bits, so we
/// return `OkSecure` if the file exists at all — callers on those
/// platforms should treat the check as advisory.
///
/// A missing file returns [`PassphraseSidecarStatus::Missing`]; a
/// stat failure for an existing file also returns `Missing`, so the
/// caller prints the "create it" hint rather than a more confusing
/// "permission denied reading the passphrase file" one.
pub fn audit_passphrase_sidecar(path: &Path) -> PassphraseSidecarStatus {
    if !path.exists() {
        return PassphraseSidecarStatus::Missing;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match std::fs::metadata(path) {
            Ok(m) => {
                let mode = m.permissions().mode() & 0o777;
                // Group and other bits must all be zero. Owner bits
                // we don't care about — `0400` and `0600` are both fine.
                if mode & 0o077 == 0 {
                    PassphraseSidecarStatus::OkSecure
                } else {
                    PassphraseSidecarStatus::InsecurePerms(mode)
                }
            }
            Err(_) => PassphraseSidecarStatus::Missing,
        }
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        PassphraseSidecarStatus::OkSecure
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- resolve_install_path ----

    #[test]
    fn systemd_install_path_uses_config_dir_and_template_filename() {
        let config = PathBuf::from("/home/alice/.config");
        let got = resolve_install_path(
            ResolvedServiceKind::Systemd,
            "work",
            Some(&config),
            None, // home_dir is unused on systemd
            None, // profile_root is unused on systemd
        )
        .unwrap();
        assert_eq!(
            got,
            PathBuf::from("/home/alice/.config/systemd/user/aivyx@.service")
        );
    }

    #[test]
    fn systemd_install_path_ignores_profile_name() {
        // Template unit: same filename regardless of profile. This
        // test pins that behaviour so a future refactor can't
        // accidentally start embedding the profile in the filename
        // and breaking the template-unit contract.
        let config = PathBuf::from("/home/alice/.config");
        let a = resolve_install_path(
            ResolvedServiceKind::Systemd,
            "work",
            Some(&config),
            None,
            None,
        )
        .unwrap();
        let b = resolve_install_path(
            ResolvedServiceKind::Systemd,
            "research",
            Some(&config),
            None,
            None,
        )
        .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn systemd_install_path_none_when_config_dir_missing() {
        let got = resolve_install_path(ResolvedServiceKind::Systemd, "work", None, None, None);
        assert!(got.is_none());
    }

    #[test]
    fn systemd_install_path_honours_xdg_config_home_override() {
        // Operator who sets $XDG_CONFIG_HOME=/srv/aivyx-config expects
        // the unit to land there, not in ~/.config/.
        let xdg = PathBuf::from("/srv/aivyx-config");
        let got =
            resolve_install_path(ResolvedServiceKind::Systemd, "work", Some(&xdg), None, None)
                .unwrap();
        assert_eq!(
            got,
            PathBuf::from("/srv/aivyx-config/systemd/user/aivyx@.service")
        );
    }

    #[test]
    fn launchd_install_path_uses_home_and_embeds_profile_name() {
        let home = PathBuf::from("/Users/alice");
        let got = resolve_install_path(
            ResolvedServiceKind::Launchd,
            "work",
            None, // config_dir is unused on launchd
            Some(&home),
            None,
        )
        .unwrap();
        assert_eq!(
            got,
            PathBuf::from("/Users/alice/Library/LaunchAgents/com.aivyx.work.plist")
        );
    }

    #[test]
    fn launchd_install_path_varies_by_profile_name() {
        let home = PathBuf::from("/Users/alice");
        let a = resolve_install_path(
            ResolvedServiceKind::Launchd,
            "work",
            None,
            Some(&home),
            None,
        )
        .unwrap();
        let b = resolve_install_path(
            ResolvedServiceKind::Launchd,
            "research",
            None,
            Some(&home),
            None,
        )
        .unwrap();
        assert_ne!(a, b);
        assert!(a.to_string_lossy().ends_with("com.aivyx.work.plist"));
        assert!(b.to_string_lossy().ends_with("com.aivyx.research.plist"));
    }

    #[test]
    fn launchd_install_path_none_when_home_dir_missing() {
        let got = resolve_install_path(ResolvedServiceKind::Launchd, "work", None, None, None);
        assert!(got.is_none());
    }

    // ---- Windows branch ----

    #[test]
    fn windows_install_path_lives_inside_profile_root() {
        // Windows has no canonical drop-in directory, so the
        // installer script lands inside the profile tree itself.
        // The filename is fixed so `uninstall-service` can find it.
        let profile = PathBuf::from("C:\\Users\\alice\\.aivyx\\profiles\\work");
        let got = resolve_install_path(
            ResolvedServiceKind::Windows,
            "work",
            None,
            None,
            Some(&profile),
        )
        .unwrap();
        assert_eq!(
            got,
            PathBuf::from("C:\\Users\\alice\\.aivyx\\profiles\\work").join("install-service.ps1")
        );
    }

    #[test]
    fn windows_install_path_filename_is_independent_of_profile_name() {
        // The filename `install-service.ps1` must be the same
        // regardless of profile name, because it lives inside the
        // profile root which is already profile-unique. Pinning this
        // prevents a future refactor from embedding the name into
        // both the directory *and* the filename (double-scoping).
        let profile_a = PathBuf::from("/aivyx/work");
        let profile_b = PathBuf::from("/aivyx/research");
        let a = resolve_install_path(
            ResolvedServiceKind::Windows,
            "work",
            None,
            None,
            Some(&profile_a),
        )
        .unwrap();
        let b = resolve_install_path(
            ResolvedServiceKind::Windows,
            "research",
            None,
            None,
            Some(&profile_b),
        )
        .unwrap();
        assert_eq!(a.file_name(), b.file_name());
        assert_ne!(a, b); // but the parent directories differ
    }

    #[test]
    fn windows_install_path_none_when_profile_root_missing() {
        let got = resolve_install_path(ResolvedServiceKind::Windows, "work", None, None, None);
        assert!(got.is_none());
    }

    // ---- audit_passphrase_sidecar ----

    #[test]
    fn audit_reports_missing_for_nonexistent_path() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("no-such-file");
        assert_eq!(
            audit_passphrase_sidecar(&missing),
            PassphraseSidecarStatus::Missing
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_reports_ok_for_0600_file() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("passphrase");
        std::fs::write(&f, "secret\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            audit_passphrase_sidecar(&f),
            PassphraseSidecarStatus::OkSecure
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_reports_ok_for_0400_file() {
        // `install -m 0400` is a common "write once, never touch
        // again" pattern. Should also be accepted.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("passphrase");
        std::fs::write(&f, "secret\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o400)).unwrap();
        assert_eq!(
            audit_passphrase_sidecar(&f),
            PassphraseSidecarStatus::OkSecure
        );
    }

    #[cfg(unix)]
    #[test]
    fn audit_reports_insecure_for_0644_file() {
        // The classic "echo secret > file" trap — umask leaves the
        // file world-readable. Must be flagged.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("passphrase");
        std::fs::write(&f, "secret\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        match audit_passphrase_sidecar(&f) {
            PassphraseSidecarStatus::InsecurePerms(mode) => assert_eq!(mode, 0o644),
            other => panic!("expected InsecurePerms(0o644), got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn audit_reports_insecure_for_group_readable_file() {
        // Group-readable (0640) is still too open — many systems have
        // service accounts in the operator's primary group.
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let f = tmp.path().join("passphrase");
        std::fs::write(&f, "secret\n").unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o640)).unwrap();
        match audit_passphrase_sidecar(&f) {
            PassphraseSidecarStatus::InsecurePerms(mode) => assert_eq!(mode, 0o640),
            other => panic!("expected InsecurePerms(0o640), got {other:?}"),
        }
    }
}
