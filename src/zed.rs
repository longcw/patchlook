//! Opens repository files in Zed: a file reuses a window that already holds
//! its project, and otherwise the project opens first so the file does not
//! land in a window of its own.
use crate::git::remote_location;
use anyhow::{Context, Result};
use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

/// Percent-encodes a path for a `zed://` link, keeping its slashes.
fn quote(path: &Path) -> String {
    let mut quoted = String::new();
    for byte in path.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            quoted.push(byte as char);
        } else {
            quoted.push_str(&format!("%{byte:02X}"));
        }
    }
    quoted
}

pub const DOWNLOAD_URL: &str = "https://zed.dev/download";

/// How Zed is reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Launcher {
    /// `zed://` links through macOS `open`, which reuse Zed's windows best.
    App,
    /// Zed's command-line tool.
    Cli(PathBuf),
}

/// A program on PATH, as the shell would find it.
fn on_path(name: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_owned()
    };
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(&name))
        .find(|path| path.is_file())
}

/// Finds Zed: the configured command, then the macOS app, then the command-line
/// tool on PATH or where Zed's installers put it. A GUI started from the Dock
/// or a desktop menu has a short PATH, hence the known locations.
pub fn find(configured: Option<&Path>) -> Option<Launcher> {
    if let Some(command) = configured {
        let found = if command.components().count() > 1 {
            command.is_file().then(|| command.to_path_buf())
        } else {
            on_path(&command.to_string_lossy())
        };
        return found.map(Launcher::Cli);
    }
    let home = directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf());
    if cfg!(target_os = "macos") {
        let apps = [
            PathBuf::from("/Applications"),
            home.clone()?.join("Applications"),
        ];
        let names = ["Zed.app", "Zed Preview.app"];
        if apps
            .iter()
            .any(|dir| names.iter().any(|name| dir.join(name).is_dir()))
        {
            return Some(Launcher::App);
        }
    }
    let mut known = vec![
        PathBuf::from("/usr/local/bin/zed"),
        PathBuf::from("/opt/homebrew/bin/zed"),
    ];
    if let Some(home) = &home {
        known.push(home.join(".local/bin/zed"));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        known.push(PathBuf::from(local).join("Programs/Zed/bin/zed.exe"));
    }
    ["zed", "zeditor"]
        .into_iter()
        .find_map(on_path)
        .or_else(|| known.into_iter().find(|path| path.is_file()))
        .map(Launcher::Cli)
}

/// What to tell someone whose Zed cannot be found.
pub fn missing(configured: Option<&Path>) -> anyhow::Error {
    match configured {
        Some(command) => anyhow::anyhow!(
            "The Zed command {} was not found; fix it in Settings or clear it to detect Zed",
            command.display()
        ),
        None => anyhow::anyhow!(
            "Zed was not found. Install it from {DOWNLOAD_URL}, or set the Zed command in Settings"
        ),
    }
}

/// Opens a target: a `zed://` link for the app, or `path:line` /
/// `ssh://host/path:line` for the command-line tool.
fn launch(launcher: &Launcher, host: Option<&str>, path: &Path, line: Option<usize>) -> Result<()> {
    let line = line.map(|line| format!(":{line}")).unwrap_or_default();
    let (program, target) = match launcher {
        Launcher::App => {
            let base = host.map_or_else(
                || "zed://file".to_owned(),
                |host| format!("zed://ssh/{host}"),
            );
            (
                PathBuf::from("open"),
                format!("{base}{}{line}", quote(path)),
            )
        }
        Launcher::Cli(cli) => {
            let target = match host {
                Some(host) => format!("ssh://{host}{}{line}", path.display()),
                None => format!("{}{line}", path.display()),
            };
            (cli.clone(), target)
        }
    };
    let status = Command::new(&program)
        .arg(&target)
        .status()
        .with_context(|| format!("Cannot run {}", program.display()))?;
    anyhow::ensure!(status.success(), "Zed could not open {target}");
    Ok(())
}

/// Project folders of the Zed windows open right now for a host, or None when unknown.
#[cfg(target_os = "macos")]
fn open_roots(host: Option<&str>) -> Option<Vec<PathBuf>> {
    let running = Command::new("pgrep")
        .args(["-f", "Zed.app/Contents/MacOS/zed"])
        .output()
        .ok()?;
    if !running.status.success() {
        return Some(Vec::new());
    }
    let database = directories::BaseDirs::new()?
        .home_dir()
        .join("Library/Application Support/Zed/db/0-stable/db.sqlite");
    // Zed stamps a workspace with the running session's id while its window is open.
    let output = Command::new("sqlite3")
        .args(["-readonly", "-json"])
        .arg(database)
        .arg(
            "select w.paths as paths, r.host as host from workspaces w \
             left join remote_connections r on r.id = w.remote_connection_id \
             where w.session_id = (select value from kv_store where key = 'session_id')",
        )
        .output()
        .ok()?;
    let rows: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap_or_default();
    let want = host.map(|host| {
        let host = host.rsplit('@').next().unwrap_or(host);
        host.split(':').next().unwrap_or(host).to_owned()
    });
    Some(
        rows.iter()
            .filter(|row| row["host"].as_str().map(str::to_owned) == want)
            .flat_map(|row| {
                row["paths"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .filter(|path| !path.is_empty())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .collect(),
    )
}

#[cfg(not(target_os = "macos"))]
fn open_roots(_: Option<&str>) -> Option<Vec<PathBuf>> {
    None
}

/// Opens a worktree (local, or `ssh://host/path`), or one of its files at a line, in Zed.
/// Blocks for up to half a minute while a newly opened project connects.
pub fn open(launcher: &Launcher, worktree: &Path, file: Option<(&Path, usize)>) -> Result<()> {
    let (host, root) = match remote_location(worktree) {
        Some((host, root)) => (Some(host), PathBuf::from(root)),
        None => (None, worktree.to_path_buf()),
    };
    let Some((path, line)) = file else {
        return launch(launcher, host, &root, None);
    };
    let file = root.join(path);
    let holds_file = |roots: &[PathBuf]| roots.iter().any(|open| file.starts_with(open));
    match open_roots(host) {
        Some(roots) if !holds_file(&roots) => {
            // Zed takes one path per request, and a second request before the new
            // window connects starts a competing connection.
            launch(launcher, host, &root, None)?;
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(500));
                if open_roots(host).is_some_and(|roots| roots.contains(&root)) {
                    if host.is_some() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    return launch(launcher, host, &file, Some(line));
                }
            }
            anyhow::bail!("Zed did not open {} in time", root.display())
        }
        _ => launch(launcher, host, &file, Some(line)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_command_must_exist() {
        let directory = tempfile::tempdir().unwrap();
        let cli = directory.path().join("zed");
        assert_eq!(find(Some(&cli)), None);
        std::fs::write(&cli, "").unwrap();
        assert_eq!(find(Some(&cli)), Some(Launcher::Cli(cli.clone())));
        assert!(missing(Some(&cli)).to_string().contains("Settings"));
    }

    #[test]
    fn links_escape_everything_but_slashes() {
        assert_eq!(quote(Path::new("/a b/c#d/é.rs")), "/a%20b/c%23d/%C3%A9.rs");
    }
}
