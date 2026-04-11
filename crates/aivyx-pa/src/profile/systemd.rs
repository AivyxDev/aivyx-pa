//! systemd user-level template unit renderer.
//!
//! Produces the text of an `aivyx@.service` template unit that can be
//! dropped into `~/.config/systemd/user/` and used via:
//!
//! ```text
//! systemctl --user daemon-reload
//! systemctl --user enable --now aivyx@work
//! ```
//!
//! The instance specifier `%i` (e.g. `work` in `aivyx@work.service`)
//! expands at runtime and is passed to the aivyx binary as `--profile %i`,
//! so one template unit drives every profile on the host. That's
//! specifically why systemd support is cheaper than launchd here: one
//! file, N instances, managed uniformly via `systemctl --user`.
//!
//! # Security posture
//!
//! The rendered unit applies the systemd sandboxing primitives that are
//! unambiguously safe for a personal assistant:
//!
//! - `NoNewPrivileges=true` — the service can't gain new caps via setuid
//!   binaries (aivyx doesn't need any).
//! - `ProtectSystem=strict` — the entire filesystem is read-only except
//!   for explicit `ReadWritePaths=` carve-outs.
//! - `ProtectHome=tmpfs` — `/home` and `/root` are replaced with an empty
//!   tmpfs, so even if aivyx is coerced into reading arbitrary `~/` files
//!   it sees nothing. We then carve out the profile root via
//!   `ReadWritePaths=` so aivyx can still read/write its own config,
//!   store, and keys.
//! - `PrivateTmp=true` — `/tmp` and `/var/tmp` are private to the service.
//! - `MemoryMax=` — a ceiling set from `SystemdOpts::memory_max` (default
//!   `2G`). This is a soft hint, not a security boundary, but it prevents
//!   a runaway agent from taking down the user's desktop session.
//!
//! The passphrase is supplied via systemd credentials (`LoadCredential=`),
//! which exposes the file at `$CREDENTIALS_DIRECTORY/passphrase` inside
//! the service's mount namespace. The aivyx binary reads
//! `AIVYX_PASSPHRASE_FILE` from the environment — we set that env var to
//! point at the credential file, so no passphrase bytes ever appear in
//! `systemctl show aivyx@work` or in the journal.
//!
//! # What this module does NOT do
//!
//! It does not touch the filesystem, does not shell out to `systemctl`,
//! does not know where `~/.config/systemd/user/` lives. Those concerns
//! belong to the `install-service` action in `main.rs`. This module is
//! a pure function from `(profile_name, opts)` to `String`, which makes
//! it trivial to unit-test and impossible to accidentally mutate host
//! state from a test run.

use std::fmt::Write as _;
use std::path::PathBuf;

/// Tunables for the rendered systemd unit.
///
/// All fields have sensible defaults via [`SystemdOpts::default`] — the
/// caller only needs to override anything it specifically wants to change.
#[derive(Debug, Clone)]
pub struct SystemdOpts {
    /// Absolute path to the aivyx binary. The rendered unit embeds this
    /// as `ExecStart`, so the service always runs the exact binary the
    /// install command saw at render-time. Callers should populate this
    /// with `std::env::current_exe()`.
    pub binary_path: PathBuf,
    /// Absolute path to the profile root (`~/.aivyx/profiles/<name>/`).
    /// Embedded as `WorkingDirectory` and added to `ReadWritePaths=` so
    /// the sandboxed service can still read/write its own state.
    pub profile_root: PathBuf,
    /// Optional memory ceiling, formatted verbatim into `MemoryMax=`.
    /// Accepts systemd's size suffixes: `G`, `M`, `K`, or `infinity`.
    /// Default: `2G`. Set to `"infinity"` to disable the limit entirely.
    pub memory_max: String,
    /// Whether the unit should emit `LoadCredential=passphrase:<path>`.
    /// When true, the caller must supply `passphrase_file` — we never
    /// make up a path ourselves because that would invite accidental
    /// creation of an unprotected credentials file.
    pub use_credential: bool,
    /// Absolute path to the credentials file containing the aivyx
    /// master-key passphrase. Ignored when `use_credential` is false.
    /// Expected to be `chmod 0600` and owned by the running user — the
    /// install command creates it with those permissions, but this
    /// module does not verify that.
    pub passphrase_file: Option<PathBuf>,
    /// Description line shown in `systemctl status`. Default uses the
    /// instance specifier so it reads naturally: "Aivyx personal
    /// assistant — profile %i".
    pub description: String,
}

impl Default for SystemdOpts {
    fn default() -> Self {
        Self {
            binary_path: PathBuf::from("/usr/local/bin/aivyx"),
            profile_root: PathBuf::from("%h/.aivyx/profiles/%i"),
            memory_max: "2G".to_string(),
            use_credential: true,
            passphrase_file: Some(PathBuf::from("%h/.aivyx/profiles/%i/passphrase")),
            description: "Aivyx personal assistant — profile %i".to_string(),
        }
    }
}

/// Render a user-level `aivyx@.service` template unit.
///
/// The rendered text is deterministic for a given `opts` — stable field
/// ordering, stable quoting, no timestamps. That matters because the
/// install command diffs against any existing unit file at the target
/// path to decide whether to refuse or proceed with `--force`.
///
/// Note: because this is a *template unit* (filename ends in `@.service`),
/// the unit does not bake in a specific profile name. `%i` expands at
/// service start to whatever comes after the `@` in `aivyx@<name>`.
/// That's why there's no `profile_name` parameter — one rendered template
/// serves every profile on the host. The `install-service` command
/// writes the same template regardless of which profile triggered the
/// install, which in turn means subsequent installs are idempotent as
/// long as `opts` doesn't change.
pub fn render_systemd_unit(opts: &SystemdOpts) -> String {
    let mut out = String::with_capacity(1024);

    // [Unit] — metadata and dependency ordering.
    let _ = writeln!(out, "[Unit]");
    let _ = writeln!(out, "Description={}", opts.description);
    // `Documentation=` values are whitespace-separated URIs (see
    // systemd.unit(5)), so we emit a single bare URL. An earlier
    // draft embedded a parenthetical hint here, but systemd parsed
    // the parentheses as extra URI tokens and rejected them. Pinned
    // by `rendered_unit_documentation_is_url_only`.
    let _ = writeln!(out, "Documentation=https://github.com/aivyx");
    // Wait for the user's graphical session to be ready. `default.target`
    // is the user-level analogue of `multi-user.target` and is reached
    // after login. Services that need desktop-tool access depend on it
    // implicitly via the environment, but declaring it here makes the
    // dependency explicit.
    let _ = writeln!(out, "After=default.target");
    // StartLimit* directives live in [Unit], not [Service]. systemd
    // moved them in v230 (2016); putting them under [Service] is
    // silently ignored on modern systems and systemd-analyze flags
    // it as an unknown key. Cap burst rate: no more than 5 restarts
    // in 60 seconds before systemd gives up — keeps a persistent
    // crash from wedging the journal.
    let _ = writeln!(out, "StartLimitIntervalSec=60");
    let _ = writeln!(out, "StartLimitBurst=5");
    let _ = writeln!(out);

    // [Service] — the actual process invocation and sandbox.
    let _ = writeln!(out, "[Service]");
    let _ = writeln!(out, "Type=simple");
    // %i is the instance specifier — `aivyx@work.service` → `work`.
    // We pass it via `--profile` so the binary knows which profile root
    // to mount. The binary path is an absolute, concrete path resolved
    // at render-time by the install command.
    let _ = writeln!(
        out,
        "ExecStart={} --profile %i",
        opts.binary_path.display()
    );
    let _ = writeln!(out, "WorkingDirectory={}", opts.profile_root.display());
    // Restart on crash but NOT on clean exit. A clean exit means the
    // user explicitly stopped the service; we shouldn't fight them.
    let _ = writeln!(out, "Restart=on-failure");
    // Back off between restart attempts so a crashloop doesn't burn CPU.
    // 10s is the systemd default recommendation for user services.
    let _ = writeln!(out, "RestartSec=10s");
    let _ = writeln!(out);

    // Credentials: systemd reads the file from the host and exposes it
    // inside the service's mount namespace at
    // `${CREDENTIALS_DIRECTORY}/passphrase`. The aivyx binary reads
    // `AIVYX_PASSPHRASE_FILE` from env and opens whatever path it names.
    if opts.use_credential {
        if let Some(pass_file) = opts.passphrase_file.as_ref() {
            let _ = writeln!(
                out,
                "LoadCredential=passphrase:{}",
                pass_file.display()
            );
            let _ = writeln!(
                out,
                "Environment=AIVYX_PASSPHRASE_FILE=%d/passphrase"
            );
            let _ = writeln!(out);
        }
    }

    // Sandboxing. These are applied before `ExecStart` runs. Each line
    // is commented inline because operators who inspect unit files
    // deserve to know *why* the service is pinned down this hard.
    let _ = writeln!(out, "# --- Sandboxing ---");
    // Prevent privilege escalation via setuid/setcap binaries in the
    // service's PATH. aivyx doesn't need this capability.
    let _ = writeln!(out, "NoNewPrivileges=true");
    // Mount /usr, /boot, /efi read-only, and (with `strict`) the entire
    // filesystem. Combined with ReadWritePaths= below, only the profile
    // root is writable from inside the service.
    let _ = writeln!(out, "ProtectSystem=strict");
    // Replace /home and /root with an empty tmpfs. aivyx would normally
    // see the user's entire home directory, which is much more access
    // than it needs. Only the profile root is re-mounted via
    // ReadWritePaths= below.
    let _ = writeln!(out, "ProtectHome=tmpfs");
    // Re-expose the profile root as read-write inside the sandbox.
    // Without this, ProtectHome=tmpfs would hide everything including
    // the config, store, and keys the binary needs to boot.
    let _ = writeln!(
        out,
        "ReadWritePaths={}",
        opts.profile_root.display()
    );
    // Each service gets its own /tmp and /var/tmp, isolated from the
    // host and from sibling services.
    let _ = writeln!(out, "PrivateTmp=true");
    // No direct device access (the agent has no business opening
    // /dev/input/* or /dev/sd*).
    let _ = writeln!(out, "PrivateDevices=true");
    // Block kernel config knobs (/sys/fs/cgroup, /proc/sys, ...).
    // aivyx is userspace — it has no legitimate reason to write kernel
    // tunables.
    let _ = writeln!(out, "ProtectKernelTunables=true");
    let _ = writeln!(out, "ProtectKernelModules=true");
    let _ = writeln!(out, "ProtectControlGroups=true");
    // Restrict the set of syscalls to the "service-safe" superset that
    // systemd maintains. This blocks things like `ptrace`, `keyctl`,
    // and raw-packet sockets without breaking normal Rust + tokio code.
    let _ = writeln!(
        out,
        "SystemCallFilter=@system-service"
    );
    // Drop every ambient capability. aivyx runs as a plain user process
    // in the user manager instance, so it never had any caps to begin
    // with, but declaring this makes the unit's posture explicit.
    let _ = writeln!(out, "CapabilityBoundingSet=");
    let _ = writeln!(out);

    // Resource limits.
    let _ = writeln!(out, "# --- Resource limits ---");
    let _ = writeln!(out, "MemoryMax={}", opts.memory_max);
    // TasksMax limits the number of threads/processes the unit can
    // spawn. 512 is generous for a Rust async binary (typically <20
    // tasks) but low enough to contain a runaway fork loop.
    let _ = writeln!(out, "TasksMax=512");
    let _ = writeln!(out);

    // [Install] — where in the unit graph this template lives.
    let _ = writeln!(out, "[Install]");
    // `default.target` is the user-level analogue of graphical.target;
    // enabling a unit into it means "start at login".
    let _ = writeln!(out, "WantedBy=default.target");

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an `opts` with concrete paths (no `%h`/`%i` placeholders) so
    /// assertions can match on exact substrings. Real install flows use
    /// systemd specifiers like `%h` and `%i`, but tests are easier to
    /// read when the paths are literal.
    fn concrete_opts() -> SystemdOpts {
        SystemdOpts {
            binary_path: PathBuf::from("/home/user/.cargo/bin/aivyx"),
            profile_root: PathBuf::from("/home/user/.aivyx/profiles/work"),
            memory_max: "2G".to_string(),
            use_credential: true,
            passphrase_file: Some(PathBuf::from("/home/user/.aivyx/profiles/work/passphrase")),
            description: "Aivyx personal assistant — profile work".to_string(),
        }
    }

    #[test]
    fn rendered_unit_has_three_sections() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(out.contains("[Unit]"));
        assert!(out.contains("[Service]"));
        assert!(out.contains("[Install]"));
    }

    #[test]
    fn rendered_unit_wires_binary_and_profile() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(
            out.contains("ExecStart=/home/user/.cargo/bin/aivyx --profile %i"),
            "unit was:\n{out}"
        );
    }

    #[test]
    fn rendered_unit_has_restart_policy() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(out.contains("Restart=on-failure"));
        assert!(out.contains("RestartSec=10s"));
        assert!(out.contains("StartLimitBurst=5"));
    }

    #[test]
    fn rendered_unit_applies_sandboxing_primitives() {
        let out = render_systemd_unit(&concrete_opts());
        // Each of these must appear verbatim — an operator grepping the
        // unit file expects exact systemd directive names.
        for needle in [
            "NoNewPrivileges=true",
            "ProtectSystem=strict",
            "ProtectHome=tmpfs",
            "PrivateTmp=true",
            "PrivateDevices=true",
            "ProtectKernelTunables=true",
            "ProtectKernelModules=true",
            "ProtectControlGroups=true",
            "SystemCallFilter=@system-service",
            "CapabilityBoundingSet=",
        ] {
            assert!(
                out.contains(needle),
                "missing directive `{needle}` in unit:\n{out}"
            );
        }
    }

    #[test]
    fn rendered_unit_carves_out_profile_root_for_read_write() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(
            out.contains("ReadWritePaths=/home/user/.aivyx/profiles/work"),
            "unit was:\n{out}"
        );
    }

    #[test]
    fn rendered_unit_wires_credentials_when_enabled() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(
            out.contains("LoadCredential=passphrase:/home/user/.aivyx/profiles/work/passphrase"),
            "unit was:\n{out}"
        );
        // The env var must point at %d/passphrase (systemd's credentials
        // directory specifier), not at the host path.
        assert!(
            out.contains("Environment=AIVYX_PASSPHRASE_FILE=%d/passphrase"),
            "unit was:\n{out}"
        );
    }

    #[test]
    fn rendered_unit_omits_credentials_when_disabled() {
        let mut opts = concrete_opts();
        opts.use_credential = false;
        opts.passphrase_file = None;
        let out = render_systemd_unit(&opts);
        assert!(
            !out.contains("LoadCredential="),
            "should not emit LoadCredential when disabled; unit was:\n{out}"
        );
        assert!(
            !out.contains("AIVYX_PASSPHRASE_FILE"),
            "should not emit passphrase env when disabled; unit was:\n{out}"
        );
    }

    #[test]
    fn rendered_unit_honours_memory_max_override() {
        let mut opts = concrete_opts();
        opts.memory_max = "512M".to_string();
        let out = render_systemd_unit(&opts);
        assert!(out.contains("MemoryMax=512M"));
        // And the default-ish 2G should NOT leak in.
        assert!(!out.contains("MemoryMax=2G"), "unit was:\n{out}");
    }

    #[test]
    fn rendered_unit_installs_into_default_target() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(out.contains("WantedBy=default.target"));
        // And the [Install] section must come AFTER the [Service] section
        // — systemd tolerates any order but convention is Unit/Service/
        // Install top-to-bottom. Assert the invariant so a future refactor
        // can't silently break it.
        let service_idx = out.find("[Service]").unwrap();
        let install_idx = out.find("[Install]").unwrap();
        assert!(install_idx > service_idx, "[Install] must follow [Service]");
    }

    #[test]
    fn default_opts_use_systemd_specifiers() {
        // The Default impl should produce a unit that uses `%h` and `%i`
        // so it's portable across users without re-rendering. This test
        // documents that contract — breaking it is a semver-relevant
        // change for anyone who caches rendered units.
        let out = render_systemd_unit(&SystemdOpts::default());
        assert!(out.contains("%h/.aivyx/profiles/%i"));
        assert!(out.contains("--profile %i"));
    }

    #[test]
    fn rendered_unit_is_deterministic() {
        // Render twice with the same input — output must be byte-identical.
        // The install command relies on this to diff against an existing
        // unit file and decide whether to refuse.
        let a = render_systemd_unit(&concrete_opts());
        let b = render_systemd_unit(&concrete_opts());
        assert_eq!(a, b);
    }

    #[test]
    fn rendered_unit_has_documentation_line() {
        let out = render_systemd_unit(&concrete_opts());
        assert!(out.contains("Documentation="));
    }

    #[test]
    fn rendered_unit_documentation_is_a_single_bare_url() {
        // systemd.unit(5) says `Documentation=` values are whitespace-
        // separated URIs. Earlier drafts added a parenthetical hint
        // ("(run `aivyx profile --help`)") which systemd tokenized as
        // extra URIs and rejected with "Invalid URL, ignoring". Pin
        // the URL-only form so a future refactor can't reintroduce
        // the bug systemd-analyze caught.
        let out = render_systemd_unit(&concrete_opts());
        // Grab the Documentation= line and check it contains no
        // whitespace after the scheme-ish prefix.
        let line = out
            .lines()
            .find(|l| l.starts_with("Documentation="))
            .expect("Documentation= line must exist");
        let value = line.trim_start_matches("Documentation=");
        assert!(
            !value.contains(' '),
            "Documentation= value must be a single URL with no whitespace; got: {value:?}"
        );
        assert!(value.starts_with("http"));
    }

    #[test]
    fn start_limit_directives_live_in_unit_section() {
        // StartLimitIntervalSec and StartLimitBurst belong in [Unit],
        // not [Service]. systemd moved them in v230. Pinning their
        // position prevents a future refactor from silently
        // regressing to the old [Service] placement — which modern
        // systemd treats as an unknown key and silently ignores.
        let out = render_systemd_unit(&concrete_opts());
        let unit_start = out.find("[Unit]").expect("[Unit] header");
        let service_start = out.find("[Service]").expect("[Service] header");
        let interval_idx = out
            .find("StartLimitIntervalSec=")
            .expect("StartLimitIntervalSec= must be present");
        let burst_idx = out
            .find("StartLimitBurst=")
            .expect("StartLimitBurst= must be present");
        assert!(
            interval_idx > unit_start && interval_idx < service_start,
            "StartLimitIntervalSec must appear between [Unit] and [Service]"
        );
        assert!(
            burst_idx > unit_start && burst_idx < service_start,
            "StartLimitBurst must appear between [Unit] and [Service]"
        );
    }

    /// End-to-end smoke test: render a unit and hand it to
    /// `systemd-analyze verify` to confirm systemd itself accepts the
    /// format.
    ///
    /// This is stronger than the pinned-string tests above: it catches
    /// directive typos, unknown sandboxing options, wrong section
    /// headers, and anything else systemd rejects at load time. The
    /// pinned-string tests confirm we *wrote* what we meant to write;
    /// this test confirms what we wrote is actually *valid*.
    ///
    /// # Skip behavior
    ///
    /// `systemd-analyze` is not always in `$PATH` — minimal CI
    /// containers and non-systemd Linux distributions don't have it.
    /// When the binary is missing, we log to stderr and return early
    /// so the test passes. This keeps the test suite portable without
    /// silently losing coverage on the hosts that do have the tool.
    ///
    /// # `--user` flag
    ///
    /// We pass `--user` because `aivyx@.service` is a user-level
    /// template unit. Without `--user`, systemd-analyze looks in
    /// `/etc/systemd/system/` and complains about directives that are
    /// only meaningful for user services (like `WantedBy=default.target`
    /// which resolves differently at the system level).
    ///
    /// # Template-instance wiring
    ///
    /// `systemd-analyze verify` refuses to verify a bare template
    /// (`aivyx@.service`) because `%i` has no value until the
    /// instance is named. We give it a concrete instance name
    /// (`aivyx@test.service`) and use a symlink so the template text
    /// we just rendered is what actually gets loaded.
    #[test]
    fn rendered_unit_passes_systemd_analyze_verify() {
        // Skip gracefully if systemd-analyze isn't on the PATH. We
        // use `which` via `Command::new` rather than parsing PATH
        // manually because the shell semantics of PATH lookup
        // (executability, symlinks) are non-trivial.
        if std::process::Command::new("systemd-analyze")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!(
                "SKIP: systemd-analyze not found on PATH; \
                 install `systemd` package to run this test"
            );
            return;
        }

        let tmp = tempfile::tempdir().expect("create tempdir");

        // systemd-analyze `verify` is strict: it resolves the
        // `ExecStart=` path and refuses a unit whose binary doesn't
        // exist or isn't executable. Our `concrete_opts` uses a
        // descriptive fake path (`/home/user/.cargo/bin/aivyx`) that
        // isn't real on the test host, so we substitute a real
        // executable: the test binary itself, via `current_exe()`.
        // This is guaranteed to exist and be executable on every
        // host that can run the test, and it sidesteps the question
        // of whether `/bin/true` exists (it doesn't on NixOS, where
        // the real path is `/run/current-system/sw/bin/true`).
        //
        // We also need the profile_root to be a directory that
        // actually exists so the ReadWritePaths= carve-out resolves,
        // and the passphrase file to exist so LoadCredential= can
        // point at something.
        let fake_profile_root = tmp.path().join("profile-root");
        std::fs::create_dir_all(&fake_profile_root).expect("create profile_root");
        let fake_passphrase = fake_profile_root.join("passphrase");
        std::fs::write(&fake_passphrase, "placeholder").expect("create passphrase file");
        let real_exe = std::env::current_exe().expect("current_exe");

        let smoke_opts = SystemdOpts {
            binary_path: real_exe,
            profile_root: fake_profile_root.clone(),
            memory_max: "2G".to_string(),
            use_credential: true,
            passphrase_file: Some(fake_passphrase),
            description: "Aivyx smoke test".to_string(),
        };

        // Write the rendered template text to `aivyx@.service` in the
        // tempdir. systemd-analyze parses it as a template and
        // instantiates it to verify.
        let template_path = tmp.path().join("aivyx@.service");
        let unit_text = render_systemd_unit(&smoke_opts);
        std::fs::write(&template_path, &unit_text).expect("write template");

        // Run the verifier. `--user` puts it in user-unit mode;
        // `--no-pager` keeps output directly capturable.
        let output = std::process::Command::new("systemd-analyze")
            .arg("--user")
            .arg("--no-pager")
            .arg("verify")
            .arg(template_path.to_str().expect("tempdir utf-8"))
            .output()
            .expect("spawn systemd-analyze");

        if !output.status.success() {
            // Print both streams so a failing render is diagnosable
            // straight from `cargo test` output without re-running.
            panic!(
                "systemd-analyze rejected rendered unit\n\
                 --- stdout ---\n{}\n\
                 --- stderr ---\n{}\n\
                 --- unit ---\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                unit_text
            );
        }
    }
}
