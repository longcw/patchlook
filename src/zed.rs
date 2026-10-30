//! Opens repository files in Zed the way the gh-zed helper does: a file link
//! reuses a window that already holds its project, and otherwise the project
//! opens first so the file does not land in a window of its own.
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

fn launch(url: &str) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let status = Command::new(program)
        .arg(url)
        .status()
        .with_context(|| format!("Cannot run {program}"))?;
    anyhow::ensure!(status.success(), "{program} could not open {url}");
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
pub fn open(worktree: &Path, file: Option<(&Path, usize)>) -> Result<()> {
    let (host, root) = match remote_location(worktree) {
        Some((host, root)) => (Some(host), PathBuf::from(root)),
        None => (None, worktree.to_path_buf()),
    };
    let base = host.map_or_else(
        || "zed://file".to_owned(),
        |host| format!("zed://ssh/{host}"),
    );
    let folder = format!("{base}{}", quote(&root));
    let Some((path, line)) = file else {
        return launch(&folder);
    };
    let file = root.join(path);
    let file_url = format!("{base}{}:{line}", quote(&file));
    let holds_file = |roots: &[PathBuf]| roots.iter().any(|open| file.starts_with(open));
    match open_roots(host) {
        Some(roots) if !holds_file(&roots) => {
            // Zed takes one path per request, and a second request before the new
            // window connects starts a competing connection.
            launch(&folder)?;
            let deadline = Instant::now() + Duration::from_secs(30);
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(500));
                if open_roots(host).is_some_and(|roots| roots.contains(&root)) {
                    if host.is_some() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    return launch(&file_url);
                }
            }
            anyhow::bail!("Zed did not open {} in time", root.display())
        }
        _ => launch(&file_url),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_escape_everything_but_slashes() {
        assert_eq!(quote(Path::new("/a b/c#d/é.rs")), "/a%20b/c%23d/%C3%A9.rs");
    }
}
