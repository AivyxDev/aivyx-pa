//! Application scanner — discovers installed GUI applications by parsing
//! `.desktop` files from standard XDG directories.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A discovered desktop application.
#[derive(Debug, Clone)]
pub struct DesktopApp {
    /// Binary name / command (e.g. "firefox", "nautilus").
    pub exec: String,
    /// Human-readable display name (e.g. "Firefox Web Browser").
    pub name: String,
    /// Icon name (for future TUI/desktop rendering).
    pub icon: Option<String>,
    /// `.desktop` file categories (e.g. "WebBrowser;Network").
    pub categories: Vec<String>,
    /// Whether the app should appear in menus (NoDisplay != true).
    pub visible: bool,
    /// Source `.desktop` file path.
    pub desktop_file: PathBuf,
}

/// Scan standard XDG application directories for `.desktop` files.
///
/// Returns a map of binary_name → DesktopApp, sorted alphabetically.
/// User-local apps (`~/.local/share/applications`) override system ones.
pub fn scan_applications() -> BTreeMap<String, DesktopApp> {
    let mut apps = BTreeMap::new();

    // System directories first, then user (user overrides system)
    let dirs = application_dirs();
    for dir in &dirs {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "desktop") {
                    if let Some(app) = parse_desktop_file(&path) {
                        if app.visible {
                            apps.insert(app.exec.clone(), app);
                        }
                    }
                }
            }
        }
    }

    apps
}

/// Return the list of directories to scan for `.desktop` files.
fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    // XDG_DATA_DIRS may contain additional app directories
    if let Ok(xdg_dirs) = std::env::var("XDG_DATA_DIRS") {
        for dir in xdg_dirs.split(':') {
            let app_dir = PathBuf::from(dir).join("applications");
            if !dirs.contains(&app_dir) {
                dirs.push(app_dir);
            }
        }
    }

    // Snap and Flatpak directories
    dirs.push(PathBuf::from("/var/lib/snapd/desktop/applications"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));

    // User-local last (overrides system entries)
    if let Some(home) = std::env::var_os("HOME") {
        let user_apps = PathBuf::from(home).join(".local/share/applications");
        dirs.push(user_apps);

        // User flatpak
        let user_flatpak = PathBuf::from(std::env::var_os("HOME").unwrap())
            .join(".local/share/flatpak/exports/share/applications");
        dirs.push(user_flatpak);
    }

    dirs
}

/// Parse a single `.desktop` file into a `DesktopApp`.
///
/// Returns `None` if the file is not a valid `[Desktop Entry]` with
/// Type=Application, or if it has no Exec field.
fn parse_desktop_file(path: &Path) -> Option<DesktopApp> {
    let content = std::fs::read_to_string(path).ok()?;

    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut categories = Vec::new();
    let mut no_display = false;
    let mut is_application = false;
    let mut in_desktop_entry = false;

    for line in content.lines() {
        let line = line.trim();

        // Track section headers
        if line.starts_with('[') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        if let Some(val) = line.strip_prefix("Name=") {
            // Only take the first Name= (not localized ones like Name[de]=)
            if name.is_none() {
                name = Some(val.to_string());
            }
        } else if let Some(val) = line.strip_prefix("Exec=") {
            exec = Some(extract_binary_name(val));
        } else if let Some(val) = line.strip_prefix("Icon=") {
            icon = Some(val.to_string());
        } else if let Some(val) = line.strip_prefix("Categories=") {
            categories = val.split(';')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if line.starts_with("NoDisplay=true") {
            no_display = true;
        } else if line.starts_with("Type=Application") {
            is_application = true;
        }
    }

    if !is_application {
        return None;
    }

    let exec = exec?;
    let display_name = name.unwrap_or_else(|| exec.clone());

    Some(DesktopApp {
        exec,
        name: display_name,
        icon,
        categories,
        visible: !no_display,
        desktop_file: path.to_path_buf(),
    })
}

/// Extract the binary name from a `.desktop` Exec= line.
///
/// Exec lines can contain field codes (%f, %u, %F, %U) and full paths.
/// We want just the binary name: `/usr/bin/firefox %u` → `firefox`.
fn extract_binary_name(exec_line: &str) -> String {
    let first_word = exec_line.split_whitespace().next().unwrap_or(exec_line);
    // Strip path: /usr/bin/firefox → firefox
    first_word
        .rsplit('/')
        .next()
        .unwrap_or(first_word)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn parse_valid_desktop_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("firefox.desktop");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[Desktop Entry]").unwrap();
        writeln!(f, "Type=Application").unwrap();
        writeln!(f, "Name=Firefox Web Browser").unwrap();
        writeln!(f, "Exec=/usr/bin/firefox %u").unwrap();
        writeln!(f, "Icon=firefox").unwrap();
        writeln!(f, "Categories=WebBrowser;Network;").unwrap();

        let app = parse_desktop_file(&path).unwrap();
        assert_eq!(app.exec, "firefox");
        assert_eq!(app.name, "Firefox Web Browser");
        assert_eq!(app.icon.as_deref(), Some("firefox"));
        assert_eq!(app.categories, vec!["WebBrowser", "Network"]);
        assert!(app.visible);
    }

    #[test]
    fn skips_no_display() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hidden.desktop");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[Desktop Entry]").unwrap();
        writeln!(f, "Type=Application").unwrap();
        writeln!(f, "Name=Hidden App").unwrap();
        writeln!(f, "Exec=hidden-app").unwrap();
        writeln!(f, "NoDisplay=true").unwrap();

        let app = parse_desktop_file(&path).unwrap();
        assert!(!app.visible);
    }

    #[test]
    fn skips_non_application() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("link.desktop");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[Desktop Entry]").unwrap();
        writeln!(f, "Type=Link").unwrap();
        writeln!(f, "Name=Some Link").unwrap();
        writeln!(f, "URL=https://example.com").unwrap();

        assert!(parse_desktop_file(&path).is_none());
    }

    #[test]
    fn extract_binary_from_exec() {
        assert_eq!(extract_binary_name("/usr/bin/firefox %u"), "firefox");
        assert_eq!(extract_binary_name("gimp-2.10 %f"), "gimp-2.10");
        assert_eq!(extract_binary_name("code --wait"), "code");
        assert_eq!(extract_binary_name("flatpak run org.app"), "flatpak");
        assert_eq!(extract_binary_name("alacritty"), "alacritty");
    }

    #[test]
    fn scan_finds_apps_in_temp_dir() {
        // We can't easily test scan_applications() since it reads system dirs,
        // but we can test parse_desktop_file with controlled input.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.desktop");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[Desktop Entry]\nType=Application\nName=Test\nExec=test-app\n").unwrap();

        let app = parse_desktop_file(&path).unwrap();
        assert_eq!(app.exec, "test-app");
        assert_eq!(app.name, "Test");
    }

    #[test]
    fn ignores_localized_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.desktop");
        let mut f = std::fs::File::create(&path).unwrap();
        writeln!(f, "[Desktop Entry]").unwrap();
        writeln!(f, "Type=Application").unwrap();
        writeln!(f, "Name=English Name").unwrap();
        writeln!(f, "Name[de]=German Name").unwrap();
        writeln!(f, "Exec=my-app").unwrap();

        let app = parse_desktop_file(&path).unwrap();
        assert_eq!(app.name, "English Name");
    }
}
