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

/// The pull request that brought a commit in, or its commit page when none is found.
pub fn pull_request_url(repo: &Repository, sha: &str) -> Result<String> {
    let remotes = remotes(repo)?;
    let first = remotes
        .first()
        .context("This repository has no GitHub remote")?;
    for remote in &remotes {
        let output = gh()
            .args(["api", "--jq", ".[0].html_url // empty"])
            .arg(format!(
                "repos/{}/{}/commits/{sha}/pulls",
                remote.owner, remote.name
            ))
            .output();
        if let Ok(output) = output {
            let url = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            if output.status.success() && !url.is_empty() {
                return Ok(url);
            }
        }
    }
    Ok(commit_url(first, sha))
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
