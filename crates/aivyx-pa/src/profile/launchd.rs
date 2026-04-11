//! macOS launchd plist renderer.
//!
//! Produces the XML plist text for a `com.aivyx.<profile>.plist` file
//! that can be dropped into `~/Library/LaunchAgents/` and loaded via:
//!
//! ```text
//! launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.aivyx.work.plist
//! ```
//!
//! Unlike systemd, launchd has no template-unit concept, so we emit one
//! plist per profile. Each plist hardcodes the profile name into its
//! `Label` and `ProgramArguments`, and the install flow writes them to
//! distinct paths under `~/Library/LaunchAgents/`.
//!
//! # Format contract
//!
//! A launchd plist is an Apple property list (XML flavour). It has a
//! fixed preamble:
//!
//! ```xml
//! <?xml version="1.0" encoding="UTF-8"?>
//! <!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
//!     "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
//! <plist version="1.0">
//!   <dict>
//!     ...
//!   </dict>
//! </plist>
//! ```
//!
//! Modern `launchctl` accepts plists without the DOCTYPE, but every
//! Apple-provided example ships with one, and operators who diff this
//! file against system plists at `/Library/LaunchDaemons/` would notice
//! its absence. Keeping it in for stylistic parity.
//!
//! # Passphrase handling
//!
//! launchd has no equivalent to systemd's `LoadCredential=` mechanism
//! — there is no "mount a credentials file into the service namespace
//! at runtime" primitive. Instead, we set the `AIVYX_PASSPHRASE_FILE`
//! env var directly to an absolute path, and rely on filesystem
//! permissions (`chmod 0600`) to protect the file. The install command
//! documents this explicitly so operators understand the trust boundary
//! is weaker on macOS than on Linux.
//!
//! # What this module does NOT do
//!
//! It does not touch the filesystem, does not shell out to `launchctl`,
//! does not know where `~/Library/LaunchAgents/` lives. Those concerns
//! belong to the `install-service` action in `main.rs`. This module is
//! a pure function from `(profile_name, opts)` to `String`, which makes
//! it trivial to unit-test and impossible to accidentally mutate host
//! state from a test run.

use std::fmt::Write as _;
use std::path::PathBuf;

/// Tunables for the rendered launchd plist.
///
/// All fields are populated by the install command from the running
/// process's `current_exe()`, the resolved profile root, and sensible
/// defaults. Tests can construct this directly with concrete paths to
/// get deterministic output.
#[derive(Debug, Clone)]
pub struct LaunchdOpts {
    /// Absolute path to the aivyx binary. Embedded as the first element
    /// of `ProgramArguments`. launchd does not honour `$PATH` at service
    /// start any more reliably than systemd does, so the path must be
    /// absolute.
    pub binary_path: PathBuf,
    /// Absolute path to the profile root (`~/.aivyx/profiles/<name>/`).
    /// Embedded as `WorkingDirectory` so any relative path the binary
    /// resolves (e.g. from config) starts from a known location.
    pub profile_root: PathBuf,
    /// Absolute path to a directory where stdout/stderr logs will be
    /// written. Typically `<profile_root>/logs/`. The install command
    /// creates this directory with mode `0755` before loading the
    /// plist; launchd itself will fail to start the service if the
    /// directory does not exist.
    pub log_dir: PathBuf,
    /// Whether to set `AIVYX_PASSPHRASE_FILE` in `EnvironmentVariables`.
    /// When true, the caller must supply `passphrase_file`. Set to
    /// `false` to use an interactive passphrase (only works if stdin
    /// is redirected from a file — launchd has no TTY).
    pub use_credential: bool,
    /// Absolute path to the file containing the aivyx master-key
    /// passphrase. Must be `chmod 0600` and owned by the running user.
    /// Ignored when `use_credential` is false.
    pub passphrase_file: Option<PathBuf>,
}

impl LaunchdOpts {
    /// Build a sensible default `LaunchdOpts` for a profile rooted at
    /// `profile_root`, with the binary at `binary_path`. Log directory
    /// defaults to `<profile_root>/logs/` and the passphrase file to
    /// `<profile_root>/passphrase`.
    pub fn new(binary_path: PathBuf, profile_root: PathBuf) -> Self {
        let log_dir = profile_root.join("logs");
        let passphrase_file = profile_root.join("passphrase");
        Self {
            binary_path,
            profile_root,
            log_dir,
            use_credential: true,
            passphrase_file: Some(passphrase_file),
        }
    }
}

/// The reverse-DNS prefix every aivyx service uses. The full label is
/// always `AIVYX_LABEL_PREFIX . profile_name`, so `com.aivyx.work`,
/// `com.aivyx.research`, etc.
pub const AIVYX_LABEL_PREFIX: &str = "com.aivyx";

/// Compute the launchd `Label` for a given profile.
///
/// This is the same string used in the plist filename (`com.aivyx.<name>.plist`)
/// and in `launchctl` commands. Exposed separately because the install
/// command needs both the filename and the in-file label, and they
/// must never drift apart.
pub fn label_for_profile(profile_name: &str) -> String {
    format!("{AIVYX_LABEL_PREFIX}.{profile_name}")
}

/// Render a user-level `com.aivyx.<profile>.plist` file.
///
/// The rendered text is deterministic for a given `(profile_name, opts)`
/// pair — stable key ordering, consistent two-space indentation, no
/// timestamps. That matters because the install command diffs against
/// any existing plist at the target path to decide whether to refuse
/// or proceed with `--force`.
///
/// # Panics
///
/// Does not panic. If `opts.use_credential` is true but
/// `opts.passphrase_file` is `None`, the `EnvironmentVariables` dict
/// is silently omitted — the caller is responsible for supplying a
/// valid combination.
pub fn render_launchd_plist(profile_name: &str, opts: &LaunchdOpts) -> String {
    let mut out = String::with_capacity(1024);

    // Preamble — XML declaration, DOCTYPE, root `<plist>` element.
    // All three are mandatory for plist files loaded by `launchctl`
    // in strict mode; tolerated-but-missing in lax mode. We always
    // emit them so operators diffing our output against Apple-provided
    // examples don't get confused by spurious differences.
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
    );
    out.push_str("<plist version=\"1.0\">\n");
    out.push_str("<dict>\n");

    // Label — reverse-DNS identifier for this service. Must match the
    // filename (minus `.plist`) for `launchctl bootstrap` to succeed.
    let label = label_for_profile(profile_name);
    out.push_str("  <key>Label</key>\n");
    let _ = writeln!(out, "  <string>{}</string>", xml_escape(&label));

    // ProgramArguments — the argv that launchd execs. First element is
    // the binary, rest are its arguments. We pass `--profile <name>`
    // so the binary knows which profile root to mount.
    out.push_str("  <key>ProgramArguments</key>\n");
    out.push_str("  <array>\n");
    let _ = writeln!(
        out,
        "    <string>{}</string>",
        xml_escape(&opts.binary_path.to_string_lossy())
    );
    out.push_str("    <string>--profile</string>\n");
    let _ = writeln!(out, "    <string>{}</string>", xml_escape(profile_name));
    out.push_str("  </array>\n");

    // WorkingDirectory — where the process starts. Config paths are
    // all absolute, but we still set this so crash dumps and anything
    // else that writes to cwd ends up inside the profile root.
    out.push_str("  <key>WorkingDirectory</key>\n");
    let _ = writeln!(
        out,
        "  <string>{}</string>",
        xml_escape(&opts.profile_root.to_string_lossy())
    );

    // RunAtLoad — start the service when launchd loads the plist.
    // Combined with the file living in `~/Library/LaunchAgents/`, this
    // means "start at login".
    out.push_str("  <key>RunAtLoad</key>\n");
    out.push_str("  <true/>\n");

    // KeepAlive — restart policy. The dict form with `SuccessfulExit=false`
    // means "restart only on crash, not on clean exit" — matching systemd's
    // `Restart=on-failure`. The bare `<true/>` form would restart even
    // after a clean shutdown, which would fight the user.
    out.push_str("  <key>KeepAlive</key>\n");
    out.push_str("  <dict>\n");
    out.push_str("    <key>SuccessfulExit</key>\n");
    out.push_str("    <false/>\n");
    // Crashed=true means "restart when the process exits with a signal",
    // which covers SIGSEGV, SIGKILL from OOM killer, etc.
    out.push_str("    <key>Crashed</key>\n");
    out.push_str("    <true/>\n");
    out.push_str("  </dict>\n");

    // ThrottleInterval — minimum seconds between relaunch attempts.
    // launchd's default is 10 seconds. We make it explicit so operators
    // can find and tune it without having to know the default.
    out.push_str("  <key>ThrottleInterval</key>\n");
    out.push_str("  <integer>10</integer>\n");

    // ProcessType — tells launchd how aggressive the scheduler should
    // be for this service. `Interactive` is for user-facing apps;
    // `Adaptive` is for background agents that should yield to the
    // foreground. aivyx is a background agent that occasionally does
    // interactive work via the API, so `Adaptive` is the right fit.
    out.push_str("  <key>ProcessType</key>\n");
    out.push_str("  <string>Adaptive</string>\n");

    // StandardOutPath and StandardErrorPath — absolute paths where
    // launchd will redirect stdout and stderr. These are appended to,
    // not truncated, on each service start. The install command
    // creates the log directory before loading the plist.
    let out_path = opts.log_dir.join("stdout.log");
    let err_path = opts.log_dir.join("stderr.log");
    out.push_str("  <key>StandardOutPath</key>\n");
    let _ = writeln!(
        out,
        "  <string>{}</string>",
        xml_escape(&out_path.to_string_lossy())
    );
    out.push_str("  <key>StandardErrorPath</key>\n");
    let _ = writeln!(
        out,
        "  <string>{}</string>",
        xml_escape(&err_path.to_string_lossy())
    );

    // EnvironmentVariables — sets AIVYX_PASSPHRASE_FILE so the binary
    // can read the passphrase without an interactive prompt. launchd
    // has no LoadCredential equivalent, so we rely on filesystem
    // permissions (`chmod 0600`) to protect the file at rest.
    if opts.use_credential {
        if let Some(pass_file) = opts.passphrase_file.as_ref() {
            out.push_str("  <key>EnvironmentVariables</key>\n");
            out.push_str("  <dict>\n");
            out.push_str("    <key>AIVYX_PASSPHRASE_FILE</key>\n");
            let _ = writeln!(
                out,
                "    <string>{}</string>",
                xml_escape(&pass_file.to_string_lossy())
            );
            out.push_str("  </dict>\n");
        }
    }

    out.push_str("</dict>\n");
    out.push_str("</plist>\n");

    out
}

/// Escape the five XML predefined entities in a string.
///
/// Profile names are pre-validated to `[A-Za-z0-9_-]` by
/// `AivyxDirs::from_profile`, so they never need escaping in practice.
/// Paths could contain `&` (rare) and `'` / `"` (extremely rare but
/// possible on macOS where filenames can contain almost anything), so
/// we escape them defensively. The order of replacements matters — `&`
/// must be replaced first, otherwise subsequent replacements would
/// double-escape.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse a rendered plist with DTD allowed.
    ///
    /// `roxmltree::Document::parse` refuses DOCTYPE by default as an XXE
    /// mitigation — since we're parsing our own output (not attacker-
    /// controlled XML), flipping `allow_dtd` to true is safe. Centralised
    /// into a helper so every test that wants to parse our plist goes
    /// through the same options.
    fn parse_plist(text: &str) -> roxmltree::Document<'_> {
        let opts = roxmltree::ParsingOptions {
            allow_dtd: true,
            ..roxmltree::ParsingOptions::default()
        };
        roxmltree::Document::parse_with_options(text, opts)
            .unwrap_or_else(|e| panic!("plist failed to parse: {e}\n---\n{text}"))
    }

    fn concrete_opts() -> LaunchdOpts {
        LaunchdOpts {
            binary_path: PathBuf::from("/Users/alice/.cargo/bin/aivyx"),
            profile_root: PathBuf::from("/Users/alice/.aivyx/profiles/work"),
            log_dir: PathBuf::from("/Users/alice/.aivyx/profiles/work/logs"),
            use_credential: true,
            passphrase_file: Some(PathBuf::from(
                "/Users/alice/.aivyx/profiles/work/passphrase",
            )),
        }
    }

    #[test]
    fn label_uses_reverse_dns_prefix() {
        assert_eq!(label_for_profile("work"), "com.aivyx.work");
        assert_eq!(label_for_profile("research"), "com.aivyx.research");
    }

    #[test]
    fn rendered_plist_is_well_formed_xml() {
        // The whole point of pulling in roxmltree as a dev-dep: confirm
        // the plist parses cleanly. A subtle bug like an unclosed tag
        // or a stray `&` would ship without this test.
        let out = render_launchd_plist("work", &concrete_opts());
        let doc = parse_plist(&out);
        let root = doc.root_element();
        assert_eq!(root.tag_name().name(), "plist");
        assert_eq!(root.attribute("version"), Some("1.0"));
    }

    #[test]
    fn rendered_plist_has_doctype() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(
            out.contains("<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\""),
            "plist was:\n{out}"
        );
    }

    #[test]
    fn rendered_plist_contains_label_matching_profile() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>Label</key>"));
        assert!(out.contains("<string>com.aivyx.work</string>"));
    }

    #[test]
    fn rendered_plist_wires_program_arguments() {
        let out = render_launchd_plist("work", &concrete_opts());
        // argv[0]
        assert!(out.contains("<string>/Users/alice/.cargo/bin/aivyx</string>"));
        // argv[1]
        assert!(out.contains("<string>--profile</string>"));
        // argv[2]
        assert!(out.contains("<string>work</string>"));
        // And the parent <array> opens / closes correctly.
        assert!(out.contains("<key>ProgramArguments</key>"));
        assert!(out.contains("<array>"));
        assert!(out.contains("</array>"));
    }

    #[test]
    fn rendered_plist_sets_run_at_load() {
        let out = render_launchd_plist("work", &concrete_opts());
        // Look for the key + value pair on adjacent lines so we can't
        // false-positive on some other `<true/>` elsewhere in the plist.
        let needle = "<key>RunAtLoad</key>\n  <true/>";
        assert!(out.contains(needle), "plist was:\n{out}");
    }

    #[test]
    fn rendered_plist_restarts_only_on_crash() {
        // KeepAlive must be a dict with SuccessfulExit=false so clean
        // exits are NOT restarted. Bare `<true/>` would be wrong here.
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>KeepAlive</key>"));
        assert!(
            out.contains("<key>SuccessfulExit</key>"),
            "plist was:\n{out}"
        );
        // The KeepAlive dict should contain a `<false/>` for SuccessfulExit.
        // We parse the structure to confirm rather than string-match,
        // because a stray false elsewhere could false-positive.
        let doc = parse_plist(&out);
        let dict = doc
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "dict")
            .unwrap();
        // Find the KeepAlive key within the top-level dict.
        let mut seen_keep_alive_dict = false;
        let mut children = dict.children().filter(|c| c.is_element());
        while let Some(node) = children.next() {
            if node.tag_name().name() == "key" && node.text() == Some("KeepAlive") {
                // The next sibling element should be the dict value.
                if let Some(value) = children.next() {
                    assert_eq!(value.tag_name().name(), "dict");
                    seen_keep_alive_dict = true;
                    break;
                }
            }
        }
        assert!(
            seen_keep_alive_dict,
            "KeepAlive key not followed by a dict; plist was:\n{out}"
        );
    }

    #[test]
    fn rendered_plist_sets_working_directory() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>WorkingDirectory</key>"));
        assert!(out.contains("<string>/Users/alice/.aivyx/profiles/work</string>"));
    }

    #[test]
    fn rendered_plist_sets_log_paths() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>StandardOutPath</key>"));
        assert!(out.contains("<key>StandardErrorPath</key>"));
        assert!(out.contains("/Users/alice/.aivyx/profiles/work/logs/stdout.log"));
        assert!(out.contains("/Users/alice/.aivyx/profiles/work/logs/stderr.log"));
    }

    #[test]
    fn rendered_plist_sets_passphrase_env_when_enabled() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>EnvironmentVariables</key>"));
        assert!(out.contains("<key>AIVYX_PASSPHRASE_FILE</key>"));
        assert!(out.contains("<string>/Users/alice/.aivyx/profiles/work/passphrase</string>"));
    }

    #[test]
    fn rendered_plist_omits_env_when_credential_disabled() {
        let mut opts = concrete_opts();
        opts.use_credential = false;
        opts.passphrase_file = None;
        let out = render_launchd_plist("work", &opts);
        assert!(!out.contains("EnvironmentVariables"), "plist was:\n{out}");
        assert!(!out.contains("AIVYX_PASSPHRASE_FILE"), "plist was:\n{out}");
    }

    #[test]
    fn rendered_plist_sets_adaptive_process_type() {
        let out = render_launchd_plist("work", &concrete_opts());
        assert!(out.contains("<key>ProcessType</key>"));
        assert!(out.contains("<string>Adaptive</string>"));
    }

    #[test]
    fn rendered_plist_is_deterministic() {
        // Two renders with the same input must be byte-identical.
        // The install command diffs against the target file to decide
        // whether to refuse, so any non-determinism would break that.
        let a = render_launchd_plist("work", &concrete_opts());
        let b = render_launchd_plist("work", &concrete_opts());
        assert_eq!(a, b);
    }

    #[test]
    fn xml_escape_handles_all_five_entities() {
        assert_eq!(xml_escape("a&b"), "a&amp;b");
        assert_eq!(xml_escape("a<b"), "a&lt;b");
        assert_eq!(xml_escape("a>b"), "a&gt;b");
        assert_eq!(xml_escape("a\"b"), "a&quot;b");
        assert_eq!(xml_escape("a'b"), "a&apos;b");
        // Ampersand must be replaced first — otherwise `<` → `&lt;` → `&amp;lt;`.
        assert_eq!(xml_escape("<&>"), "&lt;&amp;&gt;");
    }

    #[test]
    fn rendered_plist_escapes_ampersand_in_path() {
        // If the operator's home contains an ampersand (rare but legal
        // on macOS), the path must be escaped in the plist. Otherwise
        // launchctl would reject the file.
        let mut opts = concrete_opts();
        opts.profile_root = PathBuf::from("/Users/a&b/work");
        let out = render_launchd_plist("work", &opts);
        // The raw ampersand must NOT appear inside a <string>.
        assert!(out.contains("/Users/a&amp;b/work"), "plist was:\n{out}");
        // And the plist must still parse as valid XML.
        let _ = parse_plist(&out);
    }

    #[test]
    fn rendered_plist_for_different_profiles_is_different() {
        let a = render_launchd_plist("work", &concrete_opts());
        let b = render_launchd_plist("research", &concrete_opts());
        assert_ne!(a, b);
        assert!(a.contains("com.aivyx.work"));
        assert!(b.contains("com.aivyx.research"));
    }

    /// End-to-end smoke test: render a plist and hand it to `plutil
    /// -lint` to confirm macOS itself accepts the format.
    ///
    /// `plutil -lint` is a much stricter parser than roxmltree — it
    /// validates both the XML structure *and* the plist schema (types
    /// match the DTD, dict keys are paired with values, boolean
    /// elements are self-closing, etc.). A plist that passes the
    /// in-module roxmltree round-trip can still be rejected by
    /// launchd at `launchctl bootstrap` time if, say, a `<key>` is
    /// followed by another `<key>` instead of a value element.
    ///
    /// # Skip behavior
    ///
    /// `plutil` ships with macOS but is not available on Linux or
    /// minimal CI images. When the binary is missing, we log to
    /// stderr and return early so the test passes on those hosts.
    /// This keeps the suite portable while still giving macOS
    /// developers stronger validation when they run locally.
    #[test]
    fn rendered_plist_passes_plutil_lint() {
        // Skip gracefully if plutil isn't on PATH. This will always
        // be the case on Linux CI.
        if std::process::Command::new("plutil")
            .arg("-help")
            .output()
            .is_err()
        {
            eprintln!(
                "SKIP: plutil not found on PATH; \
                 this test runs only on macOS hosts"
            );
            return;
        }

        let tmp = tempfile::tempdir().expect("create tempdir");
        let plist_path = tmp.path().join("com.aivyx.work.plist");
        let plist_text = render_launchd_plist("work", &concrete_opts());
        std::fs::write(&plist_path, &plist_text).expect("write plist");

        let output = std::process::Command::new("plutil")
            .arg("-lint")
            .arg(plist_path.to_str().expect("tempdir utf-8"))
            .output()
            .expect("spawn plutil");

        if !output.status.success() {
            panic!(
                "plutil -lint rejected rendered plist\n\
                 --- stdout ---\n{}\n\
                 --- stderr ---\n{}\n\
                 --- plist ---\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
                plist_text
            );
        }
    }
}
