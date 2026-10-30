//! Links commits to GitHub: the commit page, or the pull request that introduced it.
use crate::git::Repository;
use anyhow::{Context, Result};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitHubRepo {
    /// The Git remote that points at it, such as `origin`.
    pub remote: String,
    pub owner: String,
    pub name: String,
}

impl GitHubRepo {
    fn same_repository(&self, other: &Self) -> bool {
        self.owner == other.owner && self.name == other.name
    }
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
        remote: String::new(),
        owner: owner.to_owned(),
        name: name.to_owned(),
    })
}

/// GitHub remotes, `upstream` first since pull requests from a fork land there.
pub fn remotes(repo: &Repository) -> Result<Vec<GitHubRepo>> {
    let mut remotes: Vec<_> = repo
        .remote_urls()?
        .into_iter()
        .filter_map(|(name, url)| {
            Some(GitHubRepo {
                remote: name,
                ..parse_remote_url(&url)?
            })
        })
        .collect();
    remotes.sort_by_key(|remote| match remote.remote.as_str() {
        "upstream" => 0,
        "origin" => 1,
        _ => 2,
    });
    let mut unique: Vec<GitHubRepo> = Vec::new();
    for remote in remotes {
        if !unique.iter().any(|known| known.same_repository(&remote)) {
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

/// Runs `gh` where the repository lives, so a remote repository uses the host's login.
fn gh(repo: &Repository, args: &[&str]) -> Result<Option<String>> {
    let program = if repo.is_remote() {
        "gh"
    } else {
        // A GUI launched from the Dock lacks the shell's PATH, so look where Homebrew installs.
        ["/opt/homebrew/bin/gh", "/usr/local/bin/gh"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
            .unwrap_or("gh")
    };
    let output = repo
        .host_command(program)
        .args(args)
        .output()
        .context("GitHub CLI (gh) is not installed")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if output.status.success() {
        return Ok(Some(stdout).filter(|value| !value.is_empty()));
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    // GitHub answers 404 or 422 for a commit it has never seen.
    if stderr.contains("HTTP 404") || stderr.contains("HTTP 422") {
        return Ok(None);
    }
    if output.status.code() == Some(127) {
        anyhow::bail!("GitHub CLI (gh) is not installed on the repository's host");
    }
    anyhow::bail!("gh: {stderr}")
}

/// The commit page, once GitHub is known to have the commit.
pub fn published_commit_url(
    repo: &Repository,
    sha: &str,
    remotes: &[GitHubRepo],
) -> Result<String> {
    anyhow::ensure!(!remotes.is_empty(), "This repository has no GitHub remote");
    for remote in remotes {
        let path = format!("repos/{}/{}/commits/{sha}", remote.owner, remote.name);
        if gh(repo, &["api", "--jq", ".sha", &path])?.is_some() {
            return Ok(commit_url(remote, sha));
        }
    }
    anyhow::bail!("GitHub has no commit {}; push it first", short(sha))
}

fn short(sha: &str) -> &str {
    &sha[..7.min(sha.len())]
}

/// Runs `gh` queries side by side, each costing a network round trip and,
/// for a remote repository, an SSH session, and returns the first answer in
/// query order. A stage stays within SSH's default limit of ten sessions.
fn first_answer(repo: &Repository, queries: &[Vec<String>]) -> Result<Option<String>> {
    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = queries
            .iter()
            .map(|query| {
                scope.spawn(move || {
                    let args: Vec<&str> = query.iter().map(String::as_str).collect();
                    gh(repo, &args)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().expect("gh query panicked"))
            .collect()
    });
    let mut error = None;
    for result in results {
        match result {
            Ok(Some(url)) => return Ok(Some(url)),
            Ok(None) => {}
            Err(failure) => error = error.or(Some(failure)),
        }
    }
    error.map_or(Ok(None), Err)
}

/// The pull request that brought a commit in or contains it: GitHub's link for
/// a pushed commit or a search of pull request commits, then the pull request
/// of a branch holding the commit, which also finds unpushed commits.
pub fn pull_request_url(repo: &Repository, sha: &str, remotes: &[GitHubRepo]) -> Result<String> {
    anyhow::ensure!(!remotes.is_empty(), "This repository has no GitHub remote");
    let owned = |args: &[&str]| args.iter().map(|arg| arg.to_string()).collect::<Vec<_>>();
    let mut queries = Vec::new();
    for remote in remotes {
        let path = format!("repos/{}/{}/commits/{sha}/pulls", remote.owner, remote.name);
        queries.push(owned(&["api", "--jq", ".[0].html_url // empty", &path]));
    }
    for remote in remotes {
        let query = format!("q={sha} is:pr repo:{}/{}", remote.owner, remote.name);
        let jq = ".items[0].html_url // empty";
        queries.push(owned(&[
            "api",
            "-X",
            "GET",
            "search/issues",
            "-f",
            &query,
            "--jq",
            jq,
        ]));
    }
    queries.truncate(MAX_PARALLEL);
    if let Some(url) = first_answer(repo, &queries)? {
        return Ok(url);
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
    let mut queries = Vec::new();
    for head in &heads {
        for remote in remotes {
            let name = format!("{}/{}", remote.owner, remote.name);
            queries.push(owned(&[
                "pr",
                "list",
                "--state",
                "all",
                "--limit",
                "1",
                "--json",
                "url",
                "--jq",
                ".[0].url // empty",
                "--repo",
                &name,
                "--head",
                head,
            ]));
        }
    }
    queries.truncate(MAX_PARALLEL);
    first_answer(repo, &queries)?
        .with_context(|| format!("No pull request contains commit {}", short(sha)))
}

const MAX_PARALLEL: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remote_urls_name_their_repository() {
        let expected = Some(GitHubRepo {
            remote: String::new(),
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
