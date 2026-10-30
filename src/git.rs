//! Read-only Git access. Local arguments are passed directly to Git, never to a shell;
//! repositories at `ssh://host/path` run Git on that host with every argument quoted.
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    io::{BufRead, BufReader},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

pub const HISTORY_PAGE_SIZE: usize = 100;

#[derive(Clone, Debug)]
pub struct Repository {
    /// The opened worktree's top level.
    pub root: PathBuf,
    /// The main worktree, which names the repository and keys its settings.
    pub main: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Worktree {
    pub path: PathBuf,
    /// The checked-out branch's short name; None when HEAD is detached.
    pub branch: Option<String>,
    pub head: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Area {
    Unstaged,
    Staged,
    Untracked,
    Conflict,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub path: PathBuf,
    pub previous_path: Option<PathBuf>,
    pub status: char,
    pub area: Area,
}

/// Raw bytes from the same two versions used by the text diff.
pub struct FileVersions {
    pub original: Option<Vec<u8>>,
    pub modified: Option<Vec<u8>>,
}

/// Text-line totals; binary changes have no line count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DiffStats {
    pub additions: usize,
    pub deletions: usize,
}

impl std::ops::AddAssign for DiffStats {
    fn add_assign(&mut self, other: Self) {
        self.additions += other.additions;
        self.deletions += other.deletions;
    }
}

impl std::iter::Sum for DiffStats {
    fn sum<I: Iterator<Item = Self>>(iter: I) -> Self {
        iter.fold(Self::default(), |mut total, stats| {
            total += stats;
            total
        })
    }
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub branch: String,
    pub changes: Vec<Change>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub reference: String,
    pub name: String,
    pub revision: String,
    pub remote: bool,
    pub is_head: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub id: String,
    pub short_id: String,
    pub author: String,
    pub date: String,
    pub subject: String,
    pub body: String,
    pub parents: Vec<String>,
}

/// The two sides a file list compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Comparison {
    /// Index and working tree, listed by staged, unstaged and untracked area.
    Working,
    /// A commit against its first parent.
    Commit(Commit),
    /// Any base commit against a target commit, or the working tree when None.
    Range {
        base: String,
        target: Option<String>,
    },
}

#[derive(Clone, Debug)]
pub struct HistoryPage {
    pub commits: Vec<Commit>,
    pub has_more: bool,
    pub revision: Option<String>,
}

const REMOTE_PREFIX: &str = "ssh://";

/// Splits an `ssh://host/path` location into its host and remote path.
/// `ssh://host/~/path` is relative to the remote home, where SSH starts.
pub fn remote_location(root: &Path) -> Option<(&str, &str)> {
    let location = root.to_str()?.strip_prefix(REMOTE_PREFIX)?;
    let (host, path) = location.split_at(location.find('/')?);
    let path = match path.strip_prefix("/~") {
        Some("") => ".",
        Some(home) => home.strip_prefix('/')?,
        None => path,
    };
    Some((host, path))
}

/// Reads `host:/path`, `host:~/path` (scp style) or `ssh://host/path` as a remote location.
pub fn parse_remote(input: &str) -> Option<PathBuf> {
    let input = input.trim();
    let (host, path) = match input.strip_prefix(REMOTE_PREFIX) {
        Some(rest) => rest.split_at(rest.find('/')?),
        None => input.split_once(':')?,
    };
    let path = if path.starts_with('~') {
        format!("/{path}")
    } else {
        path.to_owned()
    };
    let valid = !host.is_empty() && !host.contains('/') && path.starts_with('/');
    valid.then(|| PathBuf::from(format!("{REMOTE_PREFIX}{host}{path}")))
}

/// Keeps a path Git reported on a remote host addressable through the same host.
fn localize(location: &Path, path: PathBuf) -> PathBuf {
    match remote_location(location) {
        Some((host, _)) => PathBuf::from(format!("{REMOTE_PREFIX}{host}{}", path.display())),
        None => path,
    }
}

/// Quotes one argument for the POSIX-style shell that SSH hands the command to.
fn shell_quote(arg: &OsStr) -> OsString {
    let text = arg.to_string_lossy();
    OsString::from(format!("'{}'", text.replace('\'', "'\\''")))
}

/// One SSH session per host is shared across the many short Git calls a view makes.
fn ssh(host: &str) -> Command {
    #[cfg(not(test))]
    let mut command = Command::new("ssh");
    #[cfg(test)]
    let mut command = Command::new(tests::fake_ssh());
    command.args(["-o", "BatchMode=yes"]);
    if cfg!(unix) {
        command.args([
            "-o",
            "ControlMaster=auto",
            "-o",
            "ControlPath=~/.ssh/patchlook-%C",
            "-o",
            "ControlPersist=600",
        ]);
    }
    command.args([host, "--"]);
    command
}

/// A Git command whose arguments are quoted when it runs on a remote host.
pub struct GitCommand {
    command: Command,
    remote: bool,
}

impl GitCommand {
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        if self.remote {
            self.command.arg(shell_quote(arg.as_ref()));
        } else {
            self.command.arg(arg);
        }
        self
    }

    pub fn args<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&mut self, args: I) -> &mut Self {
        for arg in args {
            self.arg(arg);
        }
        self
    }
}

impl Deref for GitCommand {
    type Target = Command;

    fn deref(&self) -> &Command {
        &self.command
    }
}

impl DerefMut for GitCommand {
    fn deref_mut(&mut self) -> &mut Command {
        &mut self.command
    }
}

fn command(root: &Path) -> GitCommand {
    let (mut command, directory, remote) = match remote_location(root) {
        Some((host, path)) => {
            let mut command = ssh(host);
            command.args(["env", "GIT_TERMINAL_PROMPT=0", "git"]);
            (command, OsStr::new(path), true)
        }
        None => (Command::new("git"), root.as_os_str(), false),
    };
    command.env("GIT_TERMINAL_PROMPT", "0");
    // A GUI should not open a console window for every Git request on Windows.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }
    let mut command = GitCommand { command, remote };
    command
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.quotepath=false",
            "-c",
            "color.ui=false",
            "-c",
            "core.fsmonitor=false",
            "-C",
        ])
        .arg(directory);
    command
}

fn output(command: &mut Command, allow_difference: bool) -> Result<Vec<u8>> {
    let Output {
        status,
        stdout,
        stderr,
    } = command
        .output()
        .context("Cannot start Git. Install Git and make it available on PATH.")?;
    if status.success() || (allow_difference && status.code() == Some(1)) {
        Ok(stdout)
    } else {
        bail!("{}", String::from_utf8_lossy(&stderr).trim());
    }
}

fn text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).context("This file is not UTF-8 text; its diff cannot be displayed.")
}

fn path(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes.to_vec())))
    }
    #[cfg(not(unix))]
    {
        Ok(PathBuf::from(
            std::str::from_utf8(bytes).context("Invalid Git path encoding")?,
        ))
    }
}

impl Repository {
    pub fn open(directory: &Path) -> Result<Self> {
        let bytes = output(
            command(directory).args(["rev-parse", "--show-toplevel"]),
            false,
        )?;
        let root = path(bytes.strip_suffix(b"\n").unwrap_or(&bytes))?;
        let mut repo = Self {
            root: localize(directory, root),
            main: PathBuf::new(),
        };
        repo.main = repo
            .worktrees()?
            .into_iter()
            .next()
            .map_or_else(|| repo.root.clone(), |main| main.path);
        Ok(repo)
    }

    /// Every worktree of the repository, the main one first.
    pub fn worktrees(&self) -> Result<Vec<Worktree>> {
        let text = text(output(
            command(&self.root).args(["worktree", "list", "--porcelain"]),
            false,
        )?)?;
        let mut worktrees = Vec::new();
        for record in text.split("\n\n") {
            let mut worktree = None;
            for line in record.lines() {
                if let Some(path) = line.strip_prefix("worktree ") {
                    worktree = Some(Worktree {
                        path: localize(&self.root, PathBuf::from(path)),
                        branch: None,
                        head: String::new(),
                    });
                } else if let Some(worktree) = &mut worktree {
                    if let Some(head) = line.strip_prefix("HEAD ") {
                        worktree.head = head.to_owned();
                    } else if let Some(branch) = line.strip_prefix("branch ") {
                        let short = branch.strip_prefix("refs/heads/").unwrap_or(branch);
                        worktree.branch = Some(short.to_owned());
                    }
                }
            }
            // A bare main repository has no files to show.
            if !record.lines().any(|line| line == "bare") {
                worktrees.extend(worktree);
            }
        }
        Ok(worktrees)
    }

    fn has_head(&self) -> Result<bool> {
        let result = command(&self.root)
            .args(["rev-parse", "--verify", "--quiet", "HEAD"])
            .output()?;
        if result.status.success() {
            return Ok(true);
        }
        if result.status.code() == Some(1) {
            return Ok(false);
        }
        bail!("{}", String::from_utf8_lossy(&result.stderr).trim())
    }

    pub fn branch_name(&self) -> Result<String> {
        let branch_result = command(&self.root)
            .args(["symbolic-ref", "--quiet", "--short", "HEAD"])
            .output()?;
        let branch = if branch_result.status.success() {
            text(branch_result.stdout)?.trim_end().to_owned()
        } else if branch_result.status.code() == Some(1) {
            text(output(
                command(&self.root).args(["rev-parse", "--short", "HEAD"]),
                false,
            )?)?
            .trim_end()
            .to_owned()
        } else {
            bail!("{}", String::from_utf8_lossy(&branch_result.stderr).trim());
        };
        Ok(branch)
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let branch = self.branch_name()?;
        let bytes = output(
            command(&self.root).args([
                "status",
                "--porcelain=v1",
                "-z",
                "--untracked-files=all",
                "--ignore-submodules=none",
            ]),
            false,
        )?;
        Ok(Snapshot {
            branch,
            changes: parse_status(&bytes)?,
        })
    }

    pub fn branches(&self) -> Result<Vec<Branch>> {
        let bytes = output(
            command(&self.root).args([
                "for-each-ref",
                "--sort=refname",
                "--format=%(refname)%00%(refname:short)%00%(objectname)%00%(HEAD)%00%(symref)",
                "refs/heads",
                "refs/remotes",
            ]),
            false,
        )?;
        bytes
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
            .filter_map(|line| {
                let fields: Vec<_> = line.split(|byte| *byte == 0).collect();
                if fields.len() != 5 {
                    return Some(Err(anyhow::anyhow!("Invalid Git branch record")));
                }
                // Remote HEAD is an alias, not another independently browsable branch.
                if !fields[4].is_empty() {
                    return None;
                }
                Some(Ok(Branch {
                    reference: String::from_utf8_lossy(fields[0]).into_owned(),
                    name: String::from_utf8_lossy(fields[1]).into_owned(),
                    revision: String::from_utf8_lossy(fields[2]).into_owned(),
                    remote: fields[0].starts_with(b"refs/remotes/"),
                    is_head: fields[3] == b"*",
                }))
            })
            .collect()
    }

    pub fn history(&self, offset: usize, revision: Option<&str>) -> Result<HistoryPage> {
        self.history_filtered(offset, revision, "")
    }

    /// Search the full reachable history, keeping only one page in memory.
    /// Message matching is case-insensitive; hashes match by prefix.
    pub fn history_filtered(
        &self,
        offset: usize,
        revision: Option<&str>,
        query: &str,
    ) -> Result<HistoryPage> {
        if revision.is_none() && !self.has_head()? {
            return Ok(HistoryPage {
                commits: vec![],
                has_more: false,
                revision: None,
            });
        }
        let revision = text(output(
            command(&self.root)
                .args(["rev-parse", "--verify"])
                .arg(format!("{}^{{commit}}", revision.unwrap_or("HEAD"))),
            false,
        )?)?
        .trim()
        .to_owned();
        let query = query.trim().to_lowercase();
        let mut cmd = command(&self.root);
        cmd.args([
            "log",
            "-z",
            "--date=short",
            "--format=%H%x00%h%x00%an%x00%ad%x00%s%x00%P%x00%b",
        ]);
        if query.is_empty() {
            cmd.arg(format!("--max-count={}", HISTORY_PAGE_SIZE + 1))
                .arg(format!("--skip={offset}"));
        }
        let mut child = cmd
            .args([revision.as_str(), "--"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("Cannot start Git history search")?;
        let result = read_history_matches(
            BufReader::new(child.stdout.take().expect("Git stdout was piped")),
            if query.is_empty() { 0 } else { offset },
            &query,
        );
        // Once a page and its lookahead are ready, stop scanning older commits.
        let stopped = result
            .as_ref()
            .map_or(true, |commits| commits.len() > HISTORY_PAGE_SIZE);
        if stopped {
            let _ = child.kill();
        }
        let output = child.wait_with_output()?;
        if !stopped && !output.status.success() {
            bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
        }
        let mut commits = result?;
        let has_more = commits.len() > HISTORY_PAGE_SIZE;
        commits.truncate(HISTORY_PAGE_SIZE);
        Ok(HistoryPage {
            commits,
            has_more,
            revision: Some(revision),
        })
    }

    pub fn changes(&self, comparison: &Comparison) -> Result<Vec<Change>> {
        match comparison {
            Comparison::Working => Ok(self.snapshot()?.changes),
            Comparison::Commit(commit) => self.commit_changes(commit),
            Comparison::Range { base, target } => {
                let mut cmd = command(&self.root);
                cmd.args(["diff", base]).args(target).args([
                    "--name-status",
                    "-z",
                    "--find-renames",
                    "--",
                ]);
                let mut changes = parse_names(&output(&mut cmd, false)?)?;
                if target.is_none() {
                    changes.extend(self.untracked()?);
                }
                Ok(changes)
            }
        }
    }

    fn untracked(&self) -> Result<Vec<Change>> {
        let bytes = output(
            command(&self.root).args(["ls-files", "-z", "--others", "--exclude-standard"]),
            false,
        )?;
        bytes
            .split(|b| *b == 0)
            .filter(|r| !r.is_empty())
            .map(|name| {
                Ok(Change {
                    path: path(name)?,
                    previous_path: None,
                    status: 'A',
                    area: Area::Untracked,
                })
            })
            .collect()
    }

    pub fn commit_changes(&self, commit: &Commit) -> Result<Vec<Change>> {
        let mut cmd = self.commit_diff_command(commit);
        cmd.args(["--name-status", "-z", "--find-renames", "--"]);
        parse_names(&output(&mut cmd, false)?)
    }

    /// Counts all files in one Git invocation. Renames use the destination path;
    /// None identifies binary changes, which have no text-line count.
    pub fn commit_file_stats(
        &self,
        commit: &Commit,
    ) -> Result<HashMap<PathBuf, Option<DiffStats>>> {
        let mut cmd = self.commit_diff_command(commit);
        cmd.args([
            "--numstat",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--find-renames",
            "--",
        ]);
        parse_numstat(&output(&mut cmd, false)?)
    }

    fn commit_diff_command(&self, commit: &Commit) -> GitCommand {
        let mut cmd = command(&self.root);
        if let Some(parent) = commit.parents.first() {
            cmd.args(["diff", parent, &commit.id]);
        } else {
            cmd.args(["diff-tree", "--root", "--no-commit-id", "-r", &commit.id]);
        }
        cmd
    }

    fn change_diff_command(&self, change: &Change, comparison: &Comparison) -> GitCommand {
        let mut cmd = command(&self.root);
        if change.area == Area::Untracked {
            // Untracked files only exist when the working tree is a side.
            cmd.args(["diff", "--no-index"]);
            return cmd;
        }
        match comparison {
            Comparison::Commit(commit) => return self.commit_diff_command(commit),
            Comparison::Range { base, target } => {
                cmd.args(["diff", base]).args(target);
            }
            Comparison::Working => {
                cmd.arg("diff");
                match change.area {
                    Area::Staged => {
                        cmd.arg("--cached");
                    }
                    Area::Conflict => {
                        cmd.arg("HEAD");
                    }
                    Area::Unstaged | Area::Untracked => {}
                }
            }
        }
        cmd
    }

    /// Sum the same staged/working/untracked entries that the file list exposes.
    pub fn change_stats(&self, changes: &[Change], comparison: &Comparison) -> Result<DiffStats> {
        Ok(self
            .file_stats(changes, comparison)?
            .into_iter()
            .flatten()
            .sum())
    }

    /// Line counts aligned with list entries, including distinct staged and
    /// working copies of the same path. None identifies a binary file.
    pub fn file_stats(
        &self,
        changes: &[Change],
        comparison: &Comparison,
    ) -> Result<Vec<Option<DiffStats>>> {
        if let Comparison::Commit(commit) = comparison {
            let stats = self.commit_file_stats(commit)?;
            return changes
                .iter()
                .map(|change| {
                    stats
                        .get(&change.path)
                        .copied()
                        .context("Missing Git line counts for changed file")
                })
                .collect();
        }
        // Git omits entries with no textual diff (for example an empty untracked file).
        let mut stats = vec![Some(DiffStats::default()); changes.len()];
        let areas: &[Area] = if *comparison == Comparison::Working {
            &[Area::Staged, Area::Unstaged, Area::Conflict]
        } else {
            // parse_names files every tracked change of a range under one area.
            &[Area::Staged]
        };
        for &area in areas {
            let entries: Vec<_> = changes
                .iter()
                .enumerate()
                .filter(|(_, change)| change.area == area)
                .collect();
            let Some((_, first)) = entries.first() else {
                continue;
            };
            let mut cmd = self.change_diff_command(first, comparison);
            cmd.args([
                "--numstat",
                "-z",
                "--no-ext-diff",
                "--no-textconv",
                "--find-renames",
                "--",
            ]);
            for (_, change) in &entries {
                if let Some(previous) = &change.previous_path {
                    cmd.arg(previous);
                }
                cmd.arg(&change.path);
            }
            let counts = parse_numstat(&output(&mut cmd, false)?)?;
            for (index, change) in entries {
                if let Some(lines) = counts.get(&change.path) {
                    stats[index] = *lines;
                }
            }
        }
        for (index, change) in changes
            .iter()
            .enumerate()
            .filter(|(_, change)| change.area == Area::Untracked)
        {
            let mut cmd = self.change_diff_command(change, comparison);
            cmd.args(["--numstat", "-z", "--no-ext-diff", "--no-textconv", "--"])
                .arg(self.null_device())
                .arg(&change.path);
            if let Some(lines) = parse_numstat(&output(&mut cmd, true)?)?
                .into_values()
                .next()
            {
                stats[index] = lines;
            }
        }
        Ok(stats)
    }

    pub fn patch(&self, change: &Change, comparison: &Comparison) -> Result<String> {
        let mut cmd = self.change_diff_command(change, comparison);
        let untracked = change.area == Area::Untracked;
        cmd.args([
            "-p",
            "--no-ext-diff",
            "--no-textconv",
            "--no-color",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--inter-hunk-context=0",
        ]);
        // Give the viewer all unchanged context so every folded wave can expand.
        // Older Git computes 2 * context + inter-hunk context in a signed long,
        // which is 32 bits on Windows. Leave room for doubling and override the
        // user's inter-hunk padding so the calculation cannot wrap negative.
        cmd.arg(format!("--unified={}", i32::MAX / 2));
        if !untracked {
            cmd.arg("--find-renames");
        }
        cmd.arg("--");
        if untracked {
            // Git's no-index mode recognizes the OS null device as an empty file.
            cmd.arg(self.null_device());
        } else if let Some(previous) = &change.previous_path {
            cmd.arg(previous);
        }
        cmd.arg(&change.path);
        text(output(&mut cmd, untracked)?)
    }

    pub fn file_versions(&self, change: &Change, comparison: &Comparison) -> Result<FileVersions> {
        let old_path = change.previous_path.as_deref().unwrap_or(&change.path);
        let original = if change.status == 'A' || change.area == Area::Untracked {
            None
        } else {
            match comparison {
                Comparison::Commit(commit) => match commit.parents.first() {
                    Some(parent) => self.tree_file(parent, old_path)?,
                    None => None,
                },
                Comparison::Range { base, .. } => self.tree_file(base, old_path)?,
                Comparison::Working if change.area == Area::Unstaged => {
                    Some(self.index_file(old_path)?)
                }
                Comparison::Working => self.tree_file("HEAD", old_path)?,
            }
        };
        let modified = if change.status == 'D' {
            None
        } else {
            match comparison {
                Comparison::Commit(commit) => self.tree_file(&commit.id, &change.path)?,
                Comparison::Range {
                    target: Some(target),
                    ..
                } => self.tree_file(target, &change.path)?,
                Comparison::Working if change.area == Area::Staged => {
                    Some(self.index_file(&change.path)?)
                }
                _ => match self.worktree_file(&change.path) {
                    Ok(bytes) => Some(bytes),
                    Err(error)
                        if change.area == Area::Conflict
                            && error.kind() == std::io::ErrorKind::NotFound =>
                    {
                        None
                    }
                    Err(error) => return Err(error).context("Cannot read working-tree image"),
                },
            }
        };
        Ok(FileVersions { original, modified })
    }

    pub fn is_remote(&self) -> bool {
        remote_location(&self.root).is_some()
    }

    fn null_device(&self) -> &'static str {
        if cfg!(windows) && !self.is_remote() {
            "NUL"
        } else {
            "/dev/null"
        }
    }

    /// Reads a working-tree file, over SSH for a remote repository.
    pub fn worktree_file(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        let Some((host, root)) = remote_location(&self.root) else {
            return std::fs::read(self.root.join(path));
        };
        let file = Path::new(root).join(path);
        let output = ssh(host)
            .arg("cat")
            .arg(shell_quote(file.as_os_str()))
            .output()?;
        if output.status.success() {
            return Ok(output.stdout);
        }
        let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        let kind = if message.contains("No such file") {
            std::io::ErrorKind::NotFound
        } else {
            std::io::ErrorKind::Other
        };
        Err(std::io::Error::new(kind, message))
    }

    fn index_file(&self, path: &Path) -> Result<Vec<u8>> {
        let mut object = std::ffi::OsString::from(":");
        object.push(path);
        output(
            command(&self.root).args(["cat-file", "blob"]).arg(object),
            false,
        )
    }

    fn tree_file(&self, revision: &str, path: &Path) -> Result<Option<Vec<u8>>> {
        let entry = output(
            command(&self.root)
                .args(["ls-tree", "-z", revision, "--"])
                .arg(path),
            false,
        )?;
        if entry.is_empty() {
            return Ok(None);
        }
        let metadata = entry.split(|byte| *byte == b'\t').next().unwrap();
        let object = std::str::from_utf8(metadata)?
            .split_whitespace()
            .nth(2)
            .context("Missing Git object ID")?;
        output(
            command(&self.root).args(["cat-file", "blob", object]),
            false,
        )
        .map(Some)
    }
}

fn parse_numstat(bytes: &[u8]) -> Result<HashMap<PathBuf, Option<DiffStats>>> {
    let mut fields = bytes.split(|b| *b == 0).peekable();
    let mut stats = HashMap::new();
    while let Some(record) = fields.next() {
        if record.is_empty() && fields.peek().is_none() {
            break;
        }
        let mut columns = record.splitn(3, |b| *b == b'\t');
        let added = columns.next().context("Missing added line count")?;
        let deleted = columns.next().context("Missing deleted line count")?;
        let mut name = columns.next().context("Missing numstat path")?;
        // With -z a rename has an empty name, then two separate NUL-terminated paths.
        if name.is_empty() {
            fields.next().context("Missing numstat rename source")?;
            name = fields
                .next()
                .context("Missing numstat rename destination")?;
        }
        let lines = if added == b"-" || deleted == b"-" {
            None
        } else {
            Some(DiffStats {
                additions: std::str::from_utf8(added)?.parse()?,
                deletions: std::str::from_utf8(deleted)?.parse()?,
            })
        };
        stats.insert(path(name)?, lines);
    }
    Ok(stats)
}

fn read_history_matches(
    mut reader: impl BufRead,
    offset: usize,
    query: &str,
) -> Result<Vec<Commit>> {
    let mut commits = Vec::new();
    let mut matched = 0;
    loop {
        let mut fields = Vec::with_capacity(7);
        for index in 0..7 {
            let mut field = Vec::new();
            if reader.read_until(0, &mut field)? == 0 {
                ensure!(index == 0, "Incomplete Git history record");
                return Ok(commits);
            }
            ensure!(field.pop() == Some(0), "Invalid Git history field");
            fields.push(String::from_utf8_lossy(&field).into_owned());
        }
        let commit = Commit {
            id: fields[0].clone(),
            short_id: fields[1].clone(),
            author: fields[2].clone(),
            date: fields[3].clone(),
            subject: fields[4].clone(),
            parents: fields[5].split_whitespace().map(str::to_owned).collect(),
            body: fields[6].trim_end().to_owned(),
        };
        if !query.is_empty()
            && !commit.id.starts_with(query)
            && !commit.subject.to_lowercase().contains(query)
            && !commit.body.to_lowercase().contains(query)
        {
            continue;
        }
        if matched >= offset {
            commits.push(commit);
        }
        matched += 1;
        if commits.len() > HISTORY_PAGE_SIZE {
            return Ok(commits);
        }
    }
}

fn parse_status(bytes: &[u8]) -> Result<Vec<Change>> {
    let mut records = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changes = vec![];
    while let Some(record) = records.next() {
        ensure!(
            record.len() >= 4 && record[2] == b' ',
            "Invalid Git status record"
        );
        let codes = &record[..2];
        let name = path(&record[3..])?;
        let previous = if codes.contains(&b'R') || codes.contains(&b'C') {
            Some(path(records.next().context("Missing rename source")?)?)
        } else {
            None
        };
        if codes == b"??" {
            changes.push(Change {
                path: name,
                previous_path: None,
                // The change is an addition; its untracked state belongs to `area`.
                status: 'A',
                area: Area::Untracked,
            });
        } else if matches!(codes, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU") {
            changes.push(Change {
                path: name,
                previous_path: None,
                status: 'U',
                area: Area::Conflict,
            });
        } else {
            for (code, area) in [(codes[0], Area::Staged), (codes[1], Area::Unstaged)] {
                if code != b' ' && code != b'!' {
                    changes.push(Change {
                        path: name.clone(),
                        previous_path: if matches!(code, b'R' | b'C') {
                            previous.clone()
                        } else {
                            None
                        },
                        status: code as char,
                        area,
                    });
                }
            }
        }
    }
    Ok(changes)
}

fn parse_names(bytes: &[u8]) -> Result<Vec<Change>> {
    let mut fields = bytes.split(|b| *b == 0).filter(|r| !r.is_empty());
    let mut changes = vec![];
    while let Some(status) = fields.next() {
        let code = status[0] as char;
        let first = path(fields.next().context("Missing changed path")?)?;
        let (name, previous) = if matches!(code, 'R' | 'C') {
            (
                path(fields.next().context("Missing rename destination")?)?,
                Some(first),
            )
        } else {
            (first, None)
        };
        changes.push(Change {
            path: name,
            previous_path: previous,
            status: code,
            area: Area::Staged,
        });
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    /// Stands in for ssh: drops options and host, then runs the joined command
    /// through a shell exactly as the remote side would.
    pub(super) fn fake_ssh() -> &'static Path {
        static FAKE: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
        let directory = FAKE.get_or_init(|| {
            let directory = tempfile::tempdir().unwrap();
            let script = directory.path().join("ssh");
            fs::write(
                &script,
                "#!/bin/sh\nwhile [ \"$1\" != -- ]; do shift; done\nshift\nexec sh -c \"$*\"\n",
            )
            .unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            }
            directory
        });
        Box::leak(directory.path().join("ssh").into_boxed_path())
    }

    #[test]
    fn linked_worktrees_name_the_main_repository() {
        let (_temp, main) = fixture();
        fs::write(main.root.join("file.txt"), "one\n").unwrap();
        commit(&main, "Initial");
        let elsewhere = tempfile::tempdir().unwrap();
        let linked_path = elsewhere.path().join("linked");
        output(
            command(&main.root)
                .args(["worktree", "add", "-q", "-b", "feature"])
                .arg(&linked_path),
            false,
        )
        .unwrap();
        let linked = Repository::open(&linked_path).unwrap();
        assert_eq!(linked.main, main.root);
        let worktrees = linked.worktrees().unwrap();
        assert_eq!(worktrees.len(), 2);
        assert_eq!(worktrees[0].branch.as_deref(), Some("main"));
        assert_eq!(worktrees[1].branch.as_deref(), Some("feature"));
        assert_eq!(worktrees[1].path, linked.root);
    }

    #[test]
    fn remote_locations_parse_scp_and_url_forms() {
        for (input, expected) in [
            ("box:/srv/repo", Some("ssh://box/srv/repo")),
            ("user@box:~/code/repo", Some("ssh://user@box/~/code/repo")),
            ("ssh://box/srv/repo", Some("ssh://box/srv/repo")),
            ("box:relative", None),
            ("/local/path", None),
        ] {
            assert_eq!(parse_remote(input), expected.map(PathBuf::from), "{input}");
        }
        assert_eq!(
            remote_location(Path::new("ssh://box/~/code/repo")),
            Some(("box", "code/repo"))
        );
        assert_eq!(remote_location(Path::new("/srv/repo")), None);
    }

    #[cfg(unix)]
    #[test]
    fn remote_repositories_run_git_over_ssh_with_quoted_arguments() {
        let (_temp, local) = fixture();
        let name = "it's a $HOME file.txt";
        fs::write(local.root.join(name), "before\n").unwrap();
        commit(&local, "Initial");
        fs::write(local.root.join(name), "after\n").unwrap();

        let location = PathBuf::from(format!("ssh://box{}", local.root.display()));
        let repo = Repository::open(&location).unwrap();
        assert!(repo.is_remote());
        assert_eq!(remote_location(&repo.root).unwrap().0, "box");
        // Worktree paths reported by the host stay reachable through it.
        assert_eq!(repo.main, repo.root);
        let changes = repo.changes(&Comparison::Working).unwrap();
        assert_eq!(changes[0].path, PathBuf::from(name));
        let patch = repo.patch(&changes[0], &Comparison::Working).unwrap();
        assert!(patch.contains("-before\n+after"));
        assert_eq!(repo.worktree_file(Path::new(name)).unwrap(), b"after\n");
        let missing = repo.worktree_file(Path::new("missing")).unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
    }

    fn run(root: &Path, args: &[&str]) {
        output(command(root).args(args), false).unwrap();
    }
    fn fixture() -> (TempDir, Repository) {
        let temp = tempfile::tempdir().unwrap();
        run(temp.path(), &["init", "-b", "main"]);
        run(temp.path(), &["config", "user.name", "Diff Test"]);
        run(temp.path(), &["config", "user.email", "diff@example.test"]);
        run(temp.path(), &["config", "commit.gpgsign", "false"]);
        run(temp.path(), &["config", "core.autocrlf", "false"]);
        let repo = Repository::open(temp.path()).unwrap();
        (temp, repo)
    }
    fn commit(repo: &Repository, message: &str) {
        run(&repo.root, &["add", "--all"]);
        run(&repo.root, &["commit", "-m", message]);
    }

    #[test]
    fn totals_include_all_areas_and_commit_files_without_counting_binary_lines() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("tracked.rs"), "one\nsame\n").unwrap();
        fs::write(repo.root.join("old.txt"), "unchanged\n".repeat(20)).unwrap();
        commit(&repo, "Initial");
        let initial = repo.history(0, None).unwrap().commits.remove(0);
        let changes = repo.commit_changes(&initial).unwrap();
        let stats = repo.commit_file_stats(&initial).unwrap();
        assert_eq!(
            stats[Path::new("tracked.rs")],
            Some(DiffStats {
                additions: 2,
                deletions: 0
            })
        );
        assert_eq!(
            stats[Path::new("old.txt")],
            Some(DiffStats {
                additions: 20,
                deletions: 0
            })
        );
        assert_eq!(
            repo.change_stats(&changes, &Comparison::Commit(initial.clone()))
                .unwrap(),
            DiffStats {
                additions: 22,
                deletions: 0
            }
        );

        let renamed = if cfg!(windows) {
            "new name.txt"
        } else {
            "new\tname\n.txt"
        };
        run(&repo.root, &["mv", "old.txt", renamed]);
        fs::write(
            repo.root.join(renamed),
            format!("{}extra\n", "unchanged\n".repeat(20)),
        )
        .unwrap();
        fs::write(repo.root.join("tracked.rs"), "staged\nsame\n").unwrap();
        run(&repo.root, &["add", "--all"]);
        fs::write(repo.root.join("tracked.rs"), "working\nextra\nsame\n").unwrap();
        fs::write(repo.root.join("new.rs"), "a\nb").unwrap();
        fs::write(repo.root.join("binary.bin"), [0, 1, 2]).unwrap();
        let changes = repo.snapshot().unwrap().changes;
        let stats = repo.file_stats(&changes, &Comparison::Working).unwrap();
        for (change, stats) in changes.iter().zip(stats) {
            let expected = if change.path == Path::new("binary.bin") {
                None
            } else if change.path == Path::new("tracked.rs") {
                Some(DiffStats {
                    additions: if change.area == Area::Staged { 1 } else { 2 },
                    deletions: 1,
                })
            } else if change.path == Path::new(renamed) {
                Some(DiffStats {
                    additions: 1,
                    deletions: 0,
                })
            } else {
                Some(DiffStats {
                    additions: 2,
                    deletions: 0,
                })
            };
            assert_eq!(stats, expected, "{change:?}");
        }
        assert_eq!(
            repo.change_stats(&changes, &Comparison::Working).unwrap(),
            DiffStats {
                additions: 6,
                deletions: 2
            }
        );
        commit(&repo, "All changes");
        let selected = repo.history(0, None).unwrap().commits.remove(0);
        let changes = repo.commit_changes(&selected).unwrap();
        let stats = repo.commit_file_stats(&selected).unwrap();
        assert_eq!(stats.len(), changes.len());
        assert_eq!(
            stats[Path::new(renamed)],
            Some(DiffStats {
                additions: 1,
                deletions: 0
            })
        );
        assert!(!stats.contains_key(Path::new("old.txt")));
        assert_eq!(
            stats[Path::new("tracked.rs")],
            Some(DiffStats {
                additions: 2,
                deletions: 1
            })
        );
        assert_eq!(
            stats[Path::new("new.rs")],
            Some(DiffStats {
                additions: 2,
                deletions: 0
            })
        );
        assert_eq!(stats[Path::new("binary.bin")], None);
        assert_eq!(
            repo.change_stats(&changes, &Comparison::Commit(selected.clone()))
                .unwrap(),
            DiffStats {
                additions: 5,
                deletions: 1
            }
        );
        assert_eq!(
            repo.change_stats(&repo.snapshot().unwrap().changes, &Comparison::Working)
                .unwrap(),
            DiffStats::default()
        );
    }

    #[test]
    fn per_file_stats_include_deleted_empty_and_metadata_only_changes() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("remove.txt"), "one\ntwo\n").unwrap();
        fs::write(repo.root.join("script.sh"), "echo hello\n").unwrap();
        commit(&repo, "Initial");
        fs::remove_file(repo.root.join("remove.txt")).unwrap();
        fs::write(repo.root.join("empty.txt"), "").unwrap();
        run(&repo.root, &["add", "--all"]);
        run(&repo.root, &["update-index", "--chmod=+x", "script.sh"]);
        run(
            &repo.root,
            &["commit", "-m", "Remove, add empty and change mode"],
        );
        let commit = repo.history(0, None).unwrap().commits.remove(0);
        let changes = repo.commit_changes(&commit).unwrap();
        let stats = repo.commit_file_stats(&commit).unwrap();
        assert_eq!(stats.len(), changes.len());
        assert_eq!(
            stats[Path::new("remove.txt")],
            Some(DiffStats {
                additions: 0,
                deletions: 2
            })
        );
        assert_eq!(stats[Path::new("empty.txt")], Some(DiffStats::default()));
        assert_eq!(stats[Path::new("script.sh")], Some(DiffStats::default()));
    }

    #[test]
    fn unborn_repo_and_untracked_text_binary_and_empty_files() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("中文 file.rs"), "fn main() {}\n").unwrap();
        fs::write(repo.root.join("empty.txt"), "").unwrap();
        fs::write(repo.root.join("image.bin"), [0, 1, 2, 3]).unwrap();
        assert!(repo.history(0, None).unwrap().commits.is_empty());
        let snapshot = repo.snapshot().unwrap();
        assert_eq!(snapshot.branch, "main");
        assert_eq!(snapshot.changes.len(), 3);
        let stats = repo
            .file_stats(&snapshot.changes, &Comparison::Working)
            .unwrap();
        for (change, stats) in snapshot.changes.iter().zip(stats) {
            assert_eq!(
                stats,
                match change.path.to_str().unwrap() {
                    "empty.txt" => Some(DiffStats::default()),
                    "image.bin" => None,
                    _ => Some(DiffStats {
                        additions: 1,
                        deletions: 0
                    }),
                }
            );
        }
        for change in snapshot.changes {
            assert_eq!(change.area, Area::Untracked);
            assert_eq!(change.status, 'A');
            let patch = repo.patch(&change, &Comparison::Working).unwrap();
            assert!(patch.contains("new file mode"));
            if change.path.ends_with("image.bin") {
                assert!(patch.contains("Binary files"));
            }
            if change.path.ends_with("中文 file.rs") {
                assert!(patch.contains("+fn main()"));
            }
        }
    }

    #[test]
    fn staged_and_unstaged_use_different_bases() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("test.rs"), "base\n").unwrap();
        commit(&repo, "Initial");
        fs::write(repo.root.join("test.rs"), "staged\n").unwrap();
        run(&repo.root, &["add", "test.rs"]);
        fs::write(repo.root.join("test.rs"), "working\n").unwrap();
        let changes = repo.snapshot().unwrap().changes;
        assert_eq!(changes.len(), 2);
        let staged = changes.iter().find(|c| c.area == Area::Staged).unwrap();
        let unstaged = changes.iter().find(|c| c.area == Area::Unstaged).unwrap();
        assert!(
            repo.patch(staged, &Comparison::Working)
                .unwrap()
                .contains("-base\n+staged")
        );
        assert!(
            repo.patch(unstaged, &Comparison::Working)
                .unwrap()
                .contains("-staged\n+working")
        );
    }

    #[test]
    fn image_versions_follow_index_history_renames_additions_and_deletions() {
        let (_temp, repo) = fixture();
        let name = "[图] image.png";
        let base = b"base\0image";
        let staged = b"staged\0image";
        let working = b"working\0image";
        fs::write(repo.root.join(name), base).unwrap();
        let untracked = repo.snapshot().unwrap().changes.remove(0);
        let versions = repo
            .file_versions(&untracked, &Comparison::Working)
            .unwrap();
        assert_eq!(versions.original, None);
        assert_eq!(versions.modified.as_deref(), Some(base.as_slice()));
        commit(&repo, "Initial image");
        let initial = repo.history(0, None).unwrap().commits.remove(0);
        fs::write(repo.root.join(name), staged).unwrap();
        run(&repo.root, &["add", "--", name]);
        fs::write(repo.root.join(name), working).unwrap();
        let changes = repo.snapshot().unwrap().changes;
        for change in &changes {
            let versions = repo.file_versions(change, &Comparison::Working).unwrap();
            let (old, new) = if change.area == Area::Staged {
                (base.as_slice(), staged.as_slice())
            } else {
                (staged.as_slice(), working.as_slice())
            };
            assert_eq!(versions.original.as_deref(), Some(old));
            assert_eq!(versions.modified.as_deref(), Some(new));
        }
        commit(&repo, "Modify image");
        let modified = repo.history(0, None).unwrap().commits.remove(0);
        run(&repo.root, &["mv", name, "renamed.png"]);
        let rename = repo.snapshot().unwrap().changes.remove(0);
        let versions = repo.file_versions(&rename, &Comparison::Working).unwrap();
        assert_eq!(versions.original.as_deref(), Some(working.as_slice()));
        assert_eq!(versions.modified.as_deref(), Some(working.as_slice()));
        commit(&repo, "Rename image");
        let renamed = repo.history(0, None).unwrap().commits.remove(0);
        fs::remove_file(repo.root.join("renamed.png")).unwrap();
        let deleted = repo.snapshot().unwrap().changes.remove(0);
        let versions = repo.file_versions(&deleted, &Comparison::Working).unwrap();
        assert_eq!(versions.original.as_deref(), Some(working.as_slice()));
        assert_eq!(versions.modified, None);
        run(&repo.root, &["add", "--all"]);
        let deleted = repo.snapshot().unwrap().changes.remove(0);
        assert_eq!(
            repo.file_versions(&deleted, &Comparison::Working)
                .unwrap()
                .modified,
            None
        );
        commit(&repo, "Delete image");
        let removed = repo.history(0, None).unwrap().commits.remove(0);
        // No image remains on disk; every historical version must still load.
        for (commit, old, new) in [
            (initial, None, Some(base.as_slice())),
            (modified, Some(base.as_slice()), Some(working.as_slice())),
            (renamed, Some(working.as_slice()), Some(working.as_slice())),
            (removed, Some(working.as_slice()), None),
        ] {
            let change = repo.commit_changes(&commit).unwrap().remove(0);
            let versions = repo
                .file_versions(&change, &Comparison::Commit(commit.clone()))
                .unwrap();
            assert_eq!(versions.original.as_deref(), old);
            assert_eq!(versions.modified.as_deref(), new);
        }
    }

    #[test]
    fn root_commit_history_and_renames_preserve_paths() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("old name.rs"), "let value = 1;\n").unwrap();
        commit(&repo, "First subject");
        run(&repo.root, &["mv", "old name.rs", "新 name.rs"]);
        let change = repo.snapshot().unwrap().changes.remove(0);
        assert_eq!(change.status, 'R');
        assert_eq!(change.previous_path, Some(PathBuf::from("old name.rs")));
        assert!(
            repo.patch(&change, &Comparison::Working)
                .unwrap()
                .contains("rename to 新 name.rs")
        );
        commit(&repo, "Rename");
        let history = repo.history(0, None).unwrap();
        assert_eq!(history.commits.len(), 2);
        assert_eq!(history.commits[0].subject, "Rename");
        let rename = repo.commit_changes(&history.commits[0]).unwrap().remove(0);
        assert_eq!(rename.status, 'R');
        let initial = &history.commits[1];
        assert!(initial.parents.is_empty());
        let added = repo.commit_changes(initial).unwrap().remove(0);
        assert!(
            repo.patch(&added, &Comparison::Commit(initial.clone()))
                .unwrap()
                .contains("+let value = 1;")
        );
    }

    #[test]
    fn literal_paths_do_not_expand_globs() {
        let (_temp, repo) = fixture();
        for name in ["[file].rs", "f.rs"] {
            fs::write(repo.root.join(name), "before\n").unwrap();
        }
        commit(&repo, "Initial");
        for name in ["[file].rs", "f.rs"] {
            fs::write(repo.root.join(name), name).unwrap();
        }
        let changes = repo.snapshot().unwrap().changes;
        let target = changes
            .iter()
            .find(|c| c.path == Path::new("[file].rs"))
            .unwrap();
        let patch = repo.patch(target, &Comparison::Working).unwrap();
        assert!(patch.contains("+[file].rs"));
        assert!(!patch.contains("+f.rs"));
    }

    #[test]
    fn ranges_compare_any_commit_with_a_later_commit_or_the_working_tree() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("file.txt"), "one\n").unwrap();
        commit(&repo, "One");
        fs::write(repo.root.join("file.txt"), "two\n").unwrap();
        commit(&repo, "Two");
        fs::write(repo.root.join("file.txt"), "three\n").unwrap();
        commit(&repo, "Three");
        fs::write(repo.root.join("file.txt"), "working\n").unwrap();
        fs::write(repo.root.join("new.txt"), "new\n").unwrap();
        let commits = repo.history(0, None).unwrap().commits;

        let skipped = Comparison::Range {
            base: commits[2].id.clone(),
            target: Some(commits[0].id.clone()),
        };
        let changes = repo.changes(&skipped).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(
            repo.patch(&changes[0], &skipped)
                .unwrap()
                .contains("-one\n+three")
        );
        let versions = repo.file_versions(&changes[0], &skipped).unwrap();
        assert_eq!(versions.original.as_deref(), Some(b"one\n".as_slice()));
        assert_eq!(versions.modified.as_deref(), Some(b"three\n".as_slice()));

        let working = Comparison::Range {
            base: commits[1].id.clone(),
            target: None,
        };
        let changes = repo.changes(&working).unwrap();
        let paths: Vec<_> = changes
            .iter()
            .map(|change| (change.path.clone(), change.area))
            .collect();
        assert_eq!(
            paths,
            [
                ("file.txt".into(), Area::Staged),
                ("new.txt".into(), Area::Untracked)
            ]
        );
        assert!(
            repo.patch(&changes[0], &working)
                .unwrap()
                .contains("-two\n+working")
        );
        assert!(repo.patch(&changes[1], &working).unwrap().contains("+new"));
        assert_eq!(
            repo.change_stats(&changes, &working).unwrap(),
            DiffStats {
                additions: 2,
                deletions: 1
            }
        );
    }

    #[test]
    fn merge_commit_compares_against_first_parent() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("base.txt"), "base\n").unwrap();
        commit(&repo, "Initial");
        run(&repo.root, &["checkout", "-b", "feature"]);
        fs::write(repo.root.join("feature.rs"), "feature\n").unwrap();
        commit(&repo, "Feature");
        run(&repo.root, &["checkout", "main"]);
        fs::write(repo.root.join("main.rs"), "main\n").unwrap();
        commit(&repo, "Main");
        run(&repo.root, &["merge", "--no-ff", "feature", "-m", "Merge"]);
        let merged = repo.history(0, None).unwrap().commits.remove(0);
        assert_eq!(merged.parents.len(), 2);
        let changes = repo.commit_changes(&merged).unwrap();
        assert_eq!(changes.len(), 1);
        let stats = repo.commit_file_stats(&merged).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(
            stats[Path::new("feature.rs")],
            Some(DiffStats {
                additions: 1,
                deletions: 0
            })
        );
        assert_eq!(changes[0].path, PathBuf::from("feature.rs"));
        assert!(
            repo.patch(&changes[0], &Comparison::Commit(merged.clone()))
                .unwrap()
                .contains("+feature")
        );
    }

    #[test]
    fn patches_are_accepted_by_the_diff_component() {
        use gpui_kit::component::diff::DiffFile;
        let (_temp, repo) = fixture();
        for (name, content) in [
            ("中文 file.rs", "fn main() {}\n"),
            ("empty.txt", ""),
            ("no-newline.py", "print('hello')"),
        ] {
            fs::write(repo.root.join(name), content).unwrap();
        }
        fs::write(repo.root.join("binary.bin"), [0, 1, 2]).unwrap();
        for change in repo.snapshot().unwrap().changes {
            let patch = repo.patch(&change, &Comparison::Working).unwrap();
            let files = DiffFile::parse(&patch).unwrap();
            assert_eq!(files.len(), 1, "{patch}");
            assert_eq!(files[0].path().as_ref(), change.path.to_str().unwrap());
            assert_eq!(files[0].is_binary(), change.path.ends_with("binary.bin"));
            assert!(files[0].has_changes());
        }
    }

    #[test]
    fn full_context_patches_are_parseable_for_worktree_index_and_history() {
        use gpui_kit::component::diff::DiffFile;

        for eol in ["\n", "\r\n"] {
            let (_temp, repo) = fixture();
            // A user setting must not push Git's hunk arithmetic past LONG_MAX.
            run(
                &repo.root,
                &["config", "diff.interHunkContext", &i32::MAX.to_string()],
            );
            let before = (0..120)
                .map(|i| format!("line_{i}{eol}"))
                .collect::<String>();
            fs::write(repo.root.join("context.rs"), &before).unwrap();
            commit(&repo, "Full context");
            let after = before
                .replace(&format!("line_10{eol}"), &format!("changed_10{eol}"))
                .replace(&format!("line_100{eol}"), &format!("changed_100{eol}"));
            fs::write(repo.root.join("context.rs"), after).unwrap();

            let verify = |change: &Change, comparison: &Comparison| {
                let patch = repo.patch(change, comparison).unwrap();
                let files = DiffFile::parse(&patch).unwrap();
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].additions(), 2);
                assert_eq!(files[0].deletions(), 2);
                assert_eq!(
                    patch.lines().filter(|line| line.starts_with("@@ ")).count(),
                    1
                );
                // File edges and distant unchanged rows remain expandable.
                for line in [0, 50, 119] {
                    assert!(patch.contains(&format!(" line_{line}{eol}")));
                }
            };
            let unstaged = repo.snapshot().unwrap().changes.remove(0);
            assert_eq!(unstaged.area, Area::Unstaged);
            verify(&unstaged, &Comparison::Working);

            run(&repo.root, &["add", "--all"]);
            let staged = repo.snapshot().unwrap().changes.remove(0);
            assert_eq!(staged.area, Area::Staged);
            verify(&staged, &Comparison::Working);

            commit(&repo, "Two separated changes");
            let selected = repo.history(0, None).unwrap().commits.remove(0);
            let historical = repo.commit_changes(&selected).unwrap().remove(0);
            verify(&historical, &Comparison::Commit(selected.clone()));
        }
    }

    #[test]
    fn history_search_covers_older_messages_hashes_and_stable_filtered_pages() {
        let (_temp, repo) = fixture();
        for index in 0..HISTORY_PAGE_SIZE + 3 {
            run(
                &repo.root,
                &[
                    "commit",
                    "--allow-empty",
                    "-qm",
                    &format!("Earlier change {index}"),
                    "-m",
                    "Body includes Needle[.] and 中文关键词",
                ],
            );
        }
        for index in 0..HISTORY_PAGE_SIZE {
            run(
                &repo.root,
                &[
                    "commit",
                    "--allow-empty",
                    "-qm",
                    &format!("Recent change {index}"),
                ],
            );
        }
        let recent = repo.history(0, None).unwrap();
        assert!(
            recent
                .commits
                .iter()
                .all(|commit| commit.subject.starts_with("Recent"))
        );
        let first = repo.history_filtered(0, None, "nEEdLe[.]").unwrap();
        assert_eq!(first.commits.len(), HISTORY_PAGE_SIZE);
        assert!(first.has_more);
        assert_eq!(first.revision, recent.revision);
        assert!(
            first
                .commits
                .iter()
                .all(|commit| commit.subject.starts_with("Earlier"))
        );
        run(
            &repo.root,
            &["commit", "--allow-empty", "-qm", "New Needle[.] match"],
        );
        let second = repo
            .history_filtered(HISTORY_PAGE_SIZE, first.revision.as_deref(), "Needle[.]")
            .unwrap();
        assert_eq!(second.commits.len(), 3);
        assert!(!second.has_more);
        assert!(
            second
                .commits
                .iter()
                .all(|commit| !first.commits.iter().any(|old| old.id == commit.id))
        );
        let hash = &second.commits[0].id;
        let by_hash = repo
            .history_filtered(0, None, &hash[..10].to_uppercase())
            .unwrap();
        assert_eq!(by_hash.commits.len(), 1);
        assert_eq!(&by_hash.commits[0].id, hash);
        assert_eq!(
            repo.history_filtered(0, None, "中文关键词")
                .unwrap()
                .commits
                .len(),
            HISTORY_PAGE_SIZE
        );
        assert!(
            repo.history_filtered(0, None, "Needle.*")
                .unwrap()
                .commits
                .is_empty()
        );
        run(&repo.root, &["checkout", "-qb", "other"]);
        run(
            &repo.root,
            &["commit", "--allow-empty", "-qm", "Only on other branch"],
        );
        assert!(
            repo.history_filtered(0, Some("refs/heads/main"), "Only on other")
                .unwrap()
                .commits
                .is_empty()
        );
        assert_eq!(
            repo.history_filtered(0, Some("refs/heads/other"), "Only on other")
                .unwrap()
                .commits
                .len(),
            1
        );
    }

    #[test]
    fn history_pagination_stays_on_the_original_revision() {
        let (_temp, repo) = fixture();
        for i in 0..=HISTORY_PAGE_SIZE {
            run(
                &repo.root,
                &["commit", "--allow-empty", "-m", &format!("Commit {i}")],
            );
        }
        let first = repo.history(0, None).unwrap();
        assert_eq!(first.commits.len(), HISTORY_PAGE_SIZE);
        assert!(first.has_more);
        run(
            &repo.root,
            &["commit", "--allow-empty", "-m", "New external commit"],
        );
        let second = repo
            .history(first.commits.len(), Some(&first.commits[0].id))
            .unwrap();
        assert_eq!(second.commits.len(), 1);
        assert_eq!(second.commits[0].subject, "Commit 0");
        assert!(!second.has_more);
    }

    #[test]
    fn browsing_local_and_remote_branches_does_not_change_head_index_or_worktree() {
        let (_temp, repo) = fixture();
        fs::write(repo.root.join("file.txt"), "base\n").unwrap();
        commit(&repo, "Initial");
        run(&repo.root, &["checkout", "-b", "feature"]);
        fs::write(repo.root.join("file.txt"), "feature\n").unwrap();
        run(&repo.root, &["add", "--all"]);
        run(
            &repo.root,
            &[
                "commit",
                "-m",
                "Feature subject",
                "-m",
                "Full message\n\nSecond paragraph",
            ],
        );
        let feature = repo.history(0, None).unwrap().commits.remove(0);
        run(
            &repo.root,
            &["update-ref", "refs/remotes/origin/feature", &feature.id],
        );
        run(
            &repo.root,
            &[
                "symbolic-ref",
                "refs/remotes/origin/HEAD",
                "refs/remotes/origin/feature",
            ],
        );
        run(&repo.root, &["checkout", "main"]);
        fs::write(repo.root.join("file.txt"), "staged\n").unwrap();
        run(&repo.root, &["add", "--all"]);
        fs::write(repo.root.join("file.txt"), "working\n").unwrap();
        let head = output(command(&repo.root).args(["rev-parse", "HEAD"]), false).unwrap();
        let index = fs::read(repo.root.join(".git/index")).unwrap();
        let status = repo.snapshot().unwrap();
        let branches = repo.branches().unwrap();
        assert_eq!(branches.len(), 3);
        assert!(
            branches
                .iter()
                .any(|branch| branch.name == "main" && branch.is_head)
        );
        for reference in ["refs/heads/feature", "refs/remotes/origin/feature"] {
            let branch = branches
                .iter()
                .find(|branch| branch.reference == reference)
                .unwrap();
            assert_eq!(branch.remote, reference.starts_with("refs/remotes/"));
            let history = repo.history(0, Some(&branch.revision)).unwrap();
            assert_eq!(history.commits[0].subject, "Feature subject");
            assert_eq!(history.commits[0].body, "Full message\n\nSecond paragraph");
            let changes = repo.commit_changes(&history.commits[0]).unwrap();
            assert!(
                repo.patch(&changes[0], &Comparison::Commit(history.commits[0].clone()))
                    .unwrap()
                    .contains("+feature")
            );
        }
        assert_eq!(
            output(command(&repo.root).args(["rev-parse", "HEAD"]), false).unwrap(),
            head
        );
        assert_eq!(fs::read(repo.root.join(".git/index")).unwrap(), index);
        assert_eq!(
            fs::read_to_string(repo.root.join("file.txt")).unwrap(),
            "working\n"
        );
        assert_eq!(repo.snapshot().unwrap().branch, status.branch);
        assert_eq!(repo.snapshot().unwrap().changes, status.changes);
    }

    #[test]
    fn conflicts_remain_visible_and_worktree_git_files_are_supported() {
        let (temp, repo) = fixture();
        fs::write(repo.root.join("file.txt"), "base\n").unwrap();
        commit(&repo, "Base");
        run(&repo.root, &["checkout", "-b", "other"]);
        fs::write(repo.root.join("file.txt"), "other\n").unwrap();
        commit(&repo, "Other");
        run(&repo.root, &["checkout", "main"]);
        fs::write(repo.root.join("file.txt"), "main\n").unwrap();
        commit(&repo, "Main");
        assert!(
            !command(&repo.root)
                .args(["merge", "other"])
                .output()
                .unwrap()
                .status
                .success()
        );
        let conflict = repo.snapshot().unwrap().changes.remove(0);
        assert_eq!(conflict.area, Area::Conflict);
        assert!(
            repo.patch(&conflict, &Comparison::Working)
                .unwrap()
                .contains("+<<<<<<< HEAD")
        );

        let worktree = temp.path().join("linked");
        output(
            command(&repo.root)
                .args(["worktree", "add", "--detach"])
                .arg(&worktree)
                .arg("HEAD"),
            false,
        )
        .unwrap();
        let linked = Repository::open(&worktree).unwrap();
        assert!(worktree.join(".git").is_file());
        assert!(linked.snapshot().unwrap().changes.is_empty());
        assert!(!linked.history(0, None).unwrap().commits.is_empty());
    }
}
