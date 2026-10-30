//! Links commits to GitHub: the commit page, or the pull request that introduced it.
use crate::git::Repository;
use anyhow::{Context, Result};
use std::process::Command;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepo {
    pub owner: String,
    pub name: String,
}

/// Reads `git@github.com:o/r.git`, `https://github.com/o/r` and `ssh://git@github.com/o/r`.
pub fn parse_remote_url(url: &str) -> Option<GitHubRepo> {
    let rest = url
        .split_once("github.com")?
        .1
        .trim_start_matches([':', '/']);
    let rest = rest.strip_suffix(".git").unwrap_or(rest);
    let (owner, name) = rest.trim_end_matches('/').split_once('/')?;
    (!owner.is_empty() && !name.is_empty() && !name.contains('/')).then(|| GitHubRepo {
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// GitHub remotes, `upstream` first since pull requests from a fork land there.
pub fn remotes(repo: &Repository) -> Result<Vec<GitHubRepo>> {
    let mut remotes: Vec<_> = repo
        .remote_urls()?
        .into_iter()
        .filter_map(|(name, url)| Some((name, parse_remote_url(&url)?)))
        .collect();
    remotes.sort_by_key(|(name, _)| match name.as_str() {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    });
    let mut unique: Vec<GitHubRepo> = Vec::new();
    for (_, remote) in remotes {
        if !unique.contains(&remote) {
            unique.push(remote);
        }
    }
    Ok(unique)
}

pub fn commit_url(remote: &GitHubRepo, sha: &str) -> String {
    format!(
        "https://github.com/{}/{}/commit/{sha}",
        remote.owner, remote.name
    )
}

/// A GUI launched from the Dock lacks the shell's PATH, so look where Homebrew installs.
fn gh() -> Command {
    let found = ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
        .into_iter()
        .find(|path| std::path::Path::new(path).exists());
    Command::new(found.unwrap_or("gh"))
}

fn gh_api(path: &str, jq: &str) -> Option<String> {
    let output = gh().args(["api", "--jq", jq, path]).output().ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (output.status.success() && !value.is_empty()).then_some(value)
}

/// The commit page, once GitHub is known to have the commit.
pub fn published_commit_url(repo: &Repository, sha: &str) -> Result<String> {
    let remotes = remotes(repo)?;
    anyhow::ensure!(!remotes.is_empty(), "This repository has no GitHub remote");
    remotes
        .iter()
        .find(|remote| {
            let path = format!("repos/{}/{}/commits/{sha}", remote.owner, remote.name);
            gh_api(&path, ".sha").is_some()
        })
        .map(|remote| commit_url(remote, sha))
        .with_context(|| format!("GitHub has no commit {}; push it first", &sha[..7]))
}

/// The pull request that brought a commit in. GitHub knows pushed commits by
/// SHA; an unpushed commit is matched through the branches that contain it.
pub fn pull_request_url(repo: &Repository, sha: &str) -> Result<String> {
    let remotes = remotes(repo)?;
    anyhow::ensure!(!remotes.is_empty(), "This repository has no GitHub remote");
    for remote in &remotes {
        let path = format!("repos/{}/{}/commits/{sha}/pulls", remote.owner, remote.name);
        if let Some(url) = gh_api(&path, ".[0].html_url // empty") {
            return Ok(url);
        }
    }
    let mut heads: Vec<String> = Vec::new();
    for reference in repo.branches_containing(sha)? {
        let head = match reference.strip_prefix("refs/heads/") {
            Some(branch) => branch,
            // `origin/x` and `x` name the same pull request head.
            None => match reference.strip_prefix("refs/remotes/") {
                Some(remote) => remote.split_once('/').map_or(remote, |(_, branch)| branch),
                None => continue,
            },
        };
        if head != "HEAD" && !heads.iter().any(|known| known == head) {
            heads.push(head.to_owned());
        }
    }
    for remote in &remotes {
        for head in &heads {
            let output = gh()
                .args(["pr", "list", "--state", "all", "--limit", "1"])
                .args(["--json", "url", "--jq", ".[0].url // empty"])
                .arg("--repo")
                .arg(format!("{}/{}", remote.owner, remote.name))
                .arg("--head")
                .arg(head)
                .output();
            if let Ok(output) = output {
                let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
                if output.status.success() && !url.is_empty() {
                    return Ok(url);
                }
            }
        }
    }
    anyhow::bail!(
        "No pull request contains commit {}",
        &sha[..7.min(sha.len())]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_name_their_repository() {
        let expected = Some(GitHubRepo {
            owner: "longcw".into(),
            name: "patchlook".into(),
        });
        for url in [
            "git@github.com:longcw/patchlook.git",
            "https://github.com/longcw/patchlook",
            "ssh://git@github.com/longcw/patchlook.git",
        ] {
            assert_eq!(parse_remote_url(url), expected, "{url}");
        }
        assert_eq!(parse_remote_url("https://gitlab.com/a/b"), None);
    }
}
