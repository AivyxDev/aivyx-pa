//! Windows Service Control Manager installer-script renderer.
//!
//! Unlike systemd and launchd, Windows has no text-file format the
//! Service Control Manager reads directly — service configuration lives
//! in the registry under
//! `HKLM\SYSTEM\CurrentControlSet\Services\<name>` and is written via
//! `sc.exe create`, PowerShell's `New-Service`, or the Win32
//! `CreateServiceW` API. There is nothing to drop into a well-known
//! directory the way `aivyx@.service` drops into
//! `~/.config/systemd/user/`.
//!
//! So rather than emit a "unit file", this module renders a **PowerShell
//! installer script** (`aivyx-<profile>.install.ps1`) that the operator
//! can inspect, then run from an elevated prompt to register the
//! service with SCM. Running the script is idempotent: it removes an
//! existing service with the same name before (re-)installing, so
//! `--force` reinstalls behave sensibly.
//!
//! # Per-machine, not per-user
//!
//! Windows services always run under SCM, which is machine-wide. There
//! is no per-user service manager analogous to `systemctl --user` or
//! `launchctl gui/`. Multi-profile isolation comes from two things:
//!
//! 1. Each profile gets its own service name (`AivyxPA-<profile>`),
//!    registered separately with SCM.
//! 2. Each service's `binPath` includes `--profile <name>`, so the
//!    running binary targets the matching profile root under the
//!    operator's `%USERPROFILE%\.aivyx\profiles\<name>\`.
//!
//! Practically, this means Windows support assumes a single-operator
//! host. On a shared workstation, two users installing aivyx profiles
//! with the same name would collide at the SCM layer.
//!
//! # Passphrase handling
//!
//! Windows services don't have a `LoadCredential=` equivalent. The
//! installer script sets `AIVYX_PASSPHRASE_FILE` on the service's
//! environment via `Set-ItemProperty` on the registry, pointing at a
//! file the operator must create with a restrictive DACL (owner-only).
//! The rendered script emits a comment pointing at `icacls` so the
//! operator knows the exact invocation:
//!
//! ```powershell
//! icacls "$env:USERPROFILE\.aivyx\profiles\<name>\passphrase" `
//!   /inheritance:r `
//!   /grant:r "$env:USERNAME:(R)"
//! ```
//!
//! Unlike systemd credentials, the passphrase path *does* appear in the
//! registry (it's an env var, not a secret). The file contents are what
//! must be protected, and that's what the `icacls` comment addresses.
//!
//! # What this module does NOT do
//!
//! It does not touch the filesystem, does not invoke `sc.exe` or
//! `New-Service`, does not know where the script ends up. Those are the
//! `install-service` action's concerns. This module is a pure function
//! from `(profile_name, opts)` → `String`, for the same testability
//! reasons as `systemd.rs` and `launchd.rs`.

use std::fmt::Write as _;
use std::path::PathBuf;

/// Tunables for the rendered Windows installer script.
///
/// All fields have sensible defaults via [`WindowsOpts::new`] — the
/// caller only needs to construct one with the concrete binary path
/// and profile root; everything else is derived.
#[derive(Debug, Clone)]
pub struct WindowsOpts {
    /// Absolute path to the aivyx.exe binary. Embedded in the service's
    /// `binPath` verbatim (quoted). Callers should populate this with
    /// `std::env::current_exe()`.
    pub binary_path: PathBuf,
    /// Absolute path to the profile root
    /// (`%USERPROFILE%\.aivyx\profiles\<name>\`). Used as the service's
    /// working directory and as the base for the passphrase file path.
    pub profile_root: PathBuf,
    /// Whether to set `AIVYX_PASSPHRASE_FILE` in the service's
    /// environment. When true, the script emits a `Set-ItemProperty`
    /// call that points at `passphrase_file`.
    pub use_passphrase_file: bool,
    /// Absolute path to the passphrase file. Ignored when
    /// `use_passphrase_file` is false. The file is expected to exist
    /// with a restrictive DACL — the installer script emits a warning
    /// and the `icacls` remediation but does not create the file
    /// itself. Mirrors the Unix renderers' behavior.
    pub passphrase_file: Option<PathBuf>,
    /// `DisplayName` shown in `services.msc`. Defaults to
    /// "Aivyx Personal Assistant (<profile>)".
    pub display_name: String,
    /// `Description` shown in `services.msc` details.
    pub description: String,
}

impl WindowsOpts {
    /// Build sensible defaults for a profile rooted at `profile_root`,
    /// with the aivyx.exe binary at `binary_path`. The passphrase file
    /// defaults to `<profile_root>\passphrase`.
    pub fn new(binary_path: PathBuf, profile_root: PathBuf, profile_name: &str) -> Self {
        let passphrase_file = profile_root.join("passphrase");
        Self {
            binary_path,
            profile_root,
            use_passphrase_file: true,
            passphrase_file: Some(passphrase_file),
            display_name: format!("Aivyx Personal Assistant ({profile_name})"),
            description: format!(
                "Aivyx personal assistant — profile {profile_name}. Private, local-first AI."
            ),
        }
    }
}

/// The canonical Windows service-name prefix. Full service names are
/// always `AIVYX_SERVICE_PREFIX` + `-` + profile name, so `AivyxPA-work`,
/// `AivyxPA-research`, etc.
///
/// Hyphen (not period) because SCM service names can contain `-` freely
/// but `.` gets special-cased in some older tools. The same prefix is
/// re-used by `profile install-service` / `uninstall-service` to locate
/// and remove existing registrations.
pub const AIVYX_SERVICE_PREFIX: &str = "AivyxPA";

/// Build the full service name for a profile (e.g. `AivyxPA-work`).
///
/// Centralized here so render, install, and uninstall all agree on the
/// exact string — mismatches would strand services that the CLI can't
/// find later.
pub fn service_name_for_profile(profile_name: &str) -> String {
    format!("{AIVYX_SERVICE_PREFIX}-{profile_name}")
}

/// Render a PowerShell installer script for the aivyx service
/// targeting `profile_name`.
///
/// The script, when executed from an elevated PowerShell prompt, does:
///
/// 1. Removes any existing `AivyxPA-<profile>` service (idempotent
///    reinstall).
/// 2. Creates a new service with `New-Service`, pointing at the
///    resolved `binary_path` + `--profile <name>` arguments.
/// 3. Sets the service's `Environment` registry value to include
///    `AIVYX_PASSPHRASE_FILE=<passphrase_file>` (if enabled).
/// 4. Warns if the passphrase file is missing and prints the `icacls`
///    remediation for locking its DACL down.
///
/// The returned text is deterministic for a given `opts` — stable field
/// ordering, no timestamps — so the install command can diff against an
/// existing script at the target path to decide whether to refuse or
/// proceed with `--force`.
pub fn render_windows_installer(profile_name: &str, opts: &WindowsOpts) -> String {
    let mut out = String::with_capacity(2048);
    let service_name = service_name_for_profile(profile_name);

    // --- Header ---
    let _ = writeln!(
        out,
        "# Aivyx Personal Assistant — Windows service installer"
    );
    let _ = writeln!(out, "# Profile: {profile_name}");
    let _ = writeln!(out, "# Service name: {service_name}");
    let _ = writeln!(
        out,
        "# Generated by `aivyx profile render-service --kind windows`."
    );
    let _ = writeln!(
        out,
        "# Run from an elevated PowerShell prompt (Right-click → Run as Administrator)."
    );
    let _ = writeln!(out, "#");
    let _ = writeln!(
        out,
        "# Inspect the service after install: `Get-Service {service_name}`"
    );
    let _ = writeln!(
        out,
        "# Remove it later: `aivyx profile uninstall-service {profile_name} --kind windows`"
    );
    let _ = writeln!(out);

    // --- Strict mode ---
    // PowerShell's default behaviour is to keep going after non-
    // terminating errors, which silently strands a half-installed
    // service. `$ErrorActionPreference = 'Stop'` turns every error into
    // a terminating one, so the script fails loud.
    let _ = writeln!(out, "$ErrorActionPreference = 'Stop'");
    let _ = writeln!(out, "Set-StrictMode -Version Latest");
    let _ = writeln!(out);

    // --- Elevation check ---
    // `New-Service` requires admin. Checking explicitly gives a nicer
    // error than the cryptic COM exception you'd get otherwise.
    let _ = writeln!(
        out,
        "$principal = New-Object Security.Principal.WindowsPrincipal ([Security.Principal.WindowsIdentity]::GetCurrent())"
    );
    let _ = writeln!(
        out,
        "if (-not $principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)) {{"
    );
    let _ = writeln!(
        out,
        "    throw 'This script must be run as Administrator (right-click → Run as Administrator).'"
    );
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    // --- Service parameters ---
    let _ = writeln!(out, "$serviceName   = '{service_name}'");
    let _ = writeln!(
        out,
        "$displayName   = '{}'",
        ps_single_quote_escape(&opts.display_name)
    );
    let _ = writeln!(
        out,
        "$description   = '{}'",
        ps_single_quote_escape(&opts.description)
    );
    // binPath is the full command line: "<exe>" --profile <name>
    // We quote the exe path but *not* the --profile argument because
    // SCM tokenizes binPath with a simple Windows argv split, and
    // single-word args don't need quoting.
    let _ = writeln!(
        out,
        "$binaryPath    = '\"{}\" --profile {}'",
        ps_single_quote_escape(&opts.binary_path.display().to_string()),
        profile_name
    );
    let _ = writeln!(
        out,
        "$workingDir    = '{}'",
        ps_single_quote_escape(&opts.profile_root.display().to_string())
    );
    let _ = writeln!(out);

    // --- Idempotent removal of existing service ---
    // `Get-Service -ErrorAction SilentlyContinue` returns $null instead
    // of throwing when the service doesn't exist, which is what we
    // want here: if it's already registered, stop + remove it before
    // we recreate. `Remove-Service` was added in PS 6, so we fall
    // back to `sc.exe delete` for older Windows PowerShell 5.1 (the
    // default on Windows 10).
    let _ = writeln!(
        out,
        "$existing = Get-Service -Name $serviceName -ErrorAction SilentlyContinue"
    );
    let _ = writeln!(out, "if ($existing) {{");
    let _ = writeln!(
        out,
        "    Write-Host \"Stopping existing service $serviceName...\""
    );
    let _ = writeln!(out, "    if ($existing.Status -ne 'Stopped') {{");
    let _ = writeln!(out, "        Stop-Service -Name $serviceName -Force");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(
        out,
        "    Write-Host \"Removing existing service $serviceName...\""
    );
    let _ = writeln!(
        out,
        "    if (Get-Command Remove-Service -ErrorAction SilentlyContinue) {{"
    );
    let _ = writeln!(out, "        Remove-Service -Name $serviceName");
    let _ = writeln!(out, "    }} else {{");
    let _ = writeln!(out, "        & sc.exe delete $serviceName | Out-Null");
    let _ = writeln!(out, "    }}");
    let _ = writeln!(out, "}}");
    let _ = writeln!(out);

    // --- Create the service ---
    // `New-Service` is the idiomatic way in PS 5.1+. `-StartupType
    // Automatic` means SCM starts it at boot; the operator can change
    // this to `Manual` via `Set-Service` if they prefer on-demand
    // launch. We don't default to `AutomaticDelayedStart` because
    // aivyx has no "wait for network" dependency — it's a local
    // agent.
    let _ = writeln!(out, "Write-Host \"Creating service $serviceName...\"");
    let _ = writeln!(out, "New-Service `");
    let _ = writeln!(out, "    -Name        $serviceName `");
    let _ = writeln!(out, "    -DisplayName $displayName `");
    let _ = writeln!(out, "    -Description $description `");
    let _ = writeln!(out, "    -BinaryPathName $binaryPath `");
    let _ = writeln!(out, "    -StartupType Automatic");
    let _ = writeln!(out);

    // --- Environment variables ---
    // New-Service doesn't expose an -Environment parameter, so we
    // write the `Environment` REG_MULTI_SZ value directly. SCM picks
    // these up when starting the service.
    if opts.use_passphrase_file {
        if let Some(pass_file) = opts.passphrase_file.as_ref() {
            let _ = writeln!(
                out,
                "$passphraseFile = '{}'",
                ps_single_quote_escape(&pass_file.display().to_string())
            );
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "Write-Host \"Setting AIVYX_PASSPHRASE_FILE in service environment...\""
            );
            let _ = writeln!(
                out,
                "$regKey = \"HKLM:\\SYSTEM\\CurrentControlSet\\Services\\$serviceName\""
            );
            // `Environment` is a REG_MULTI_SZ (`MultiString`), one
            // entry per env var. We set exactly one entry; operators
            // who need more can append by hand.
            let _ = writeln!(
                out,
                "Set-ItemProperty -Path $regKey -Name 'Environment' -Value @(\"AIVYX_PASSPHRASE_FILE=$passphraseFile\") -Type MultiString"
            );
            let _ = writeln!(out);

            // Audit + remediation hint for the passphrase file DACL.
            let _ = writeln!(out, "if (-not (Test-Path $passphraseFile)) {{");
            let _ = writeln!(
                out,
                "    Write-Warning \"Passphrase file not found at $passphraseFile\""
            );
            let _ = writeln!(
                out,
                "    Write-Warning \"The service will fail to unlock until you create it. After creating, lock its DACL with:\""
            );
            let _ = writeln!(
                out,
                "    Write-Warning \"  icacls `\"$passphraseFile`\" /inheritance:r /grant:r `\"$env:USERNAME:(R)`\"\""
            );
            let _ = writeln!(out, "}}");
            let _ = writeln!(out);
        }
    }

    // --- Working directory ---
    // Windows services don't have a `WorkingDirectory` parameter on
    // `New-Service`, but SCM honours `ImagePath`'s directory by
    // default. We set it explicitly via the `ImagePath` registry
    // value's cwd semantics — actually, there's no clean way to set
    // cwd for a Windows service without a wrapper. We instead rely on
    // the aivyx binary using absolute paths everywhere, which it
    // already does. Document this so a future operator doesn't try to
    // wire a `Set-Location` in.
    let _ = writeln!(
        out,
        "# Note: Windows services have no service-level WorkingDirectory."
    );
    let _ = writeln!(
        out,
        "# The aivyx binary uses absolute paths throughout, so cwd doesn't"
    );
    let _ = writeln!(
        out,
        "# matter. The $workingDir variable above is informational only."
    );
    let _ = writeln!(
        out,
        "$null = $workingDir  # Suppress unused-variable warning in StrictMode"
    );
    let _ = writeln!(out);

    // --- Done ---
    let _ = writeln!(
        out,
        "Write-Host \"Service $serviceName installed. Start it with: Start-Service $serviceName\""
    );
    let _ = writeln!(
        out,
        "Write-Host \"Inspect with: Get-Service $serviceName | Format-List *\""
    );

    out
}

/// Escape a string for inclusion in a PowerShell single-quoted literal.
///
/// Inside single quotes, PowerShell treats every character literally
/// *except* `'` itself, which must be doubled (`''`). No backslash
/// escaping, no `\n` expansion — that's the whole point of single
/// quotes. This mirrors the `xml_escape` helper in `launchd.rs` in
/// purpose: it's the minimal escaping the chosen literal form needs.
///
/// We use single-quoted literals throughout the rendered script so
/// paths with spaces (`C:\Program Files\...`) and embedded dollar
/// signs don't trigger variable interpolation.
fn ps_single_quote_escape(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_opts() -> WindowsOpts {
        WindowsOpts::new(
            PathBuf::from("C:\\Program Files\\Aivyx\\aivyx.exe"),
            PathBuf::from("C:\\Users\\alice\\.aivyx\\profiles\\work"),
            "work",
        )
    }

    // ---- service name helpers ----

    #[test]
    fn service_name_has_prefix_and_profile() {
        assert_eq!(service_name_for_profile("work"), "AivyxPA-work");
        assert_eq!(service_name_for_profile("research"), "AivyxPA-research");
    }

    #[test]
    fn prefix_constant_matches_service_name_output() {
        // Pin the constant so the install/uninstall handlers can rely
        // on `service_name.starts_with(AIVYX_SERVICE_PREFIX)`.
        let name = service_name_for_profile("anything");
        assert!(name.starts_with(AIVYX_SERVICE_PREFIX));
    }

    // ---- render_windows_installer: structural ----

    #[test]
    fn rendered_script_names_the_profile_in_header() {
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("# Profile: work"));
        assert!(out.contains("# Service name: AivyxPA-work"));
    }

    #[test]
    fn rendered_script_enforces_strict_error_handling() {
        // Non-terminating errors silently stranding a half-installed
        // service is the single most common PowerShell footgun. We pin
        // strict mode so a future refactor can't drop it.
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("$ErrorActionPreference = 'Stop'"));
        assert!(out.contains("Set-StrictMode -Version Latest"));
    }

    #[test]
    fn rendered_script_checks_for_admin_privileges() {
        // New-Service requires admin. Pin the explicit check because
        // the default error message from a non-elevated invocation is
        // a cryptic COM exception.
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("WindowsBuiltInRole]::Administrator"));
        assert!(out.contains("throw 'This script must be run as Administrator"));
    }

    #[test]
    fn rendered_script_removes_existing_service_before_install() {
        // Idempotent reinstall is the behaviour the `--force` flag on
        // `install-service` relies on — the CLI copies the script
        // into place and re-running it must not leave two services
        // registered.
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("Get-Service -Name $serviceName -ErrorAction SilentlyContinue"));
        assert!(out.contains("Stop-Service -Name $serviceName -Force"));
        // Both the PS 6+ and PS 5.1 removal paths are present.
        assert!(out.contains("Remove-Service -Name $serviceName"));
        assert!(out.contains("sc.exe delete $serviceName"));
    }

    #[test]
    fn rendered_script_creates_service_with_new_service() {
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("New-Service"));
        assert!(out.contains("-Name        $serviceName"));
        assert!(out.contains("-DisplayName $displayName"));
        assert!(out.contains("-BinaryPathName $binaryPath"));
        assert!(out.contains("-StartupType Automatic"));
    }

    // ---- binary path and --profile wiring ----

    #[test]
    fn rendered_script_wires_profile_flag_into_binary_path() {
        // The rendered binPath must be `"<exe>" --profile <name>` so
        // the running service hits the right profile root.
        let out = render_windows_installer("work", &default_opts());
        assert!(
            out.contains(r#"'"C:\Program Files\Aivyx\aivyx.exe" --profile work'"#),
            "binPath missing or wrong; script was:\n{out}"
        );
    }

    #[test]
    fn rendered_script_profile_name_flows_through_to_service_name() {
        let out = render_windows_installer("research", &default_opts());
        assert!(out.contains("$serviceName   = 'AivyxPA-research'"));
        assert!(out.contains("--profile research"));
    }

    // ---- passphrase handling ----

    #[test]
    fn rendered_script_sets_passphrase_env_var() {
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("AIVYX_PASSPHRASE_FILE="));
        assert!(out.contains("Set-ItemProperty"));
        assert!(out.contains("'Environment'"));
        // Uses MultiString, not a plain String — Environment is a
        // REG_MULTI_SZ value.
        assert!(out.contains("-Type MultiString"));
    }

    #[test]
    fn rendered_script_emits_icacls_remediation_hint() {
        // If a future refactor drops the icacls hint, operators will
        // install the service, start it, and find a world-readable
        // passphrase file on a multi-user host. Pin the hint here.
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("icacls"));
        assert!(out.contains("/inheritance:r"));
        assert!(out.contains("/grant:r"));
    }

    #[test]
    fn rendered_script_omits_passphrase_block_when_disabled() {
        let mut opts = default_opts();
        opts.use_passphrase_file = false;
        let out = render_windows_installer("work", &opts);
        assert!(!out.contains("AIVYX_PASSPHRASE_FILE"));
        assert!(!out.contains("Set-ItemProperty"));
        assert!(!out.contains("icacls"));
    }

    // ---- escaping ----

    #[test]
    fn ps_single_quote_escape_doubles_apostrophes() {
        // Inside a PS single-quoted literal, `'` is the only character
        // that needs escaping, and the rule is to double it.
        assert_eq!(ps_single_quote_escape("plain"), "plain");
        assert_eq!(ps_single_quote_escape("it's"), "it''s");
        assert_eq!(
            ps_single_quote_escape("multiple ' and '"),
            "multiple '' and ''"
        );
    }

    #[test]
    fn ps_single_quote_escape_leaves_dollar_signs_alone() {
        // Dollar signs are *not* variable interpolation inside single
        // quotes — that's the whole point of using single quotes. If
        // this test fails, someone tried to "helpfully" escape `$`
        // and broke paths like `C:\$Recycle.Bin`.
        assert_eq!(ps_single_quote_escape("$env:USERNAME"), "$env:USERNAME");
    }

    #[test]
    fn rendered_script_survives_apostrophe_in_display_name() {
        // A display name like "Alice's Aivyx" would break the
        // single-quoted literal without escaping. This end-to-end test
        // confirms the escape is applied at the call site.
        let mut opts = default_opts();
        opts.display_name = "Alice's Aivyx".to_string();
        let out = render_windows_installer("work", &opts);
        assert!(out.contains("'Alice''s Aivyx'"));
    }

    // ---- determinism ----

    #[test]
    fn rendered_script_is_deterministic() {
        // Same opts → same text. This is what the install-service
        // diff-before-clobber logic depends on.
        let a = render_windows_installer("work", &default_opts());
        let b = render_windows_installer("work", &default_opts());
        assert_eq!(a, b);
    }

    #[test]
    fn rendered_script_varies_by_profile_name() {
        // Different profile → different script (at minimum, the
        // service name and --profile flag change).
        let a = render_windows_installer("work", &default_opts());
        let b = render_windows_installer("research", &default_opts());
        assert_ne!(a, b);
    }

    // ---- removal path completeness ----

    #[test]
    fn rendered_script_points_user_at_uninstall_subcommand() {
        // The header comment must tell the operator how to remove the
        // service later. This is the discoverability contract with
        // `profile uninstall-service`.
        let out = render_windows_installer("work", &default_opts());
        assert!(out.contains("aivyx profile uninstall-service work --kind windows"));
    }
}
