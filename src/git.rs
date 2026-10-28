//! Thin wrapper over the `git` CLI. Every call is blocking and meant to run on
//! the background executor.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context as _, Result, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileStatus {
    Modified,
    Added,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl FileStatus {
    pub fn letter(self) -> &'static str {
        match self {
            FileStatus::Modified => "M",
            FileStatus::Added => "A",
            FileStatus::Deleted => "D",
            FileStatus::Renamed => "R",
            FileStatus::Untracked => "?",
            FileStatus::Conflicted => "!",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub status: FileStatus,
    pub staged: bool,
}

#[derive(Clone, Debug)]
pub struct Commit {
    pub hash: String,
    pub subject: String,
    pub author: String,
    pub when: String,
}

#[derive(Clone, Debug, Default)]
pub struct RepoState {
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
    pub log: Vec<Commit>,
}

impl RepoState {
    /// Absolute path -> status, used to color the file tree.
    pub fn status_map(&self, root: &Path) -> HashMap<PathBuf, FileStatus> {
        self.changes
            .iter()
            .map(|change| (root.join(&change.path), change.status))
            .collect()
    }
}

fn git(dir: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .with_context(|| format!("failed to run git {}", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("{}", stderr.trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn is_repo(dir: &Path) -> bool {
    git(dir, &["rev-parse", "--is-inside-work-tree"]).is_ok()
}

pub fn read_state(dir: &Path) -> Result<RepoState> {
    let mut state = RepoState::default();
    let status = git(dir, &["status", "--porcelain=v1", "--branch", "-z"])?;
    let mut entries = status.split('\0').filter(|s| !s.is_empty());
    while let Some(entry) = entries.next() {
        if let Some(header) = entry.strip_prefix("## ") {
            parse_branch_header(header, &mut state);
            continue;
        }
        if entry.len() < 4 {
            continue;
        }
        let (x, y) = (entry.as_bytes()[0], entry.as_bytes()[1]);
        let path = PathBuf::from(&entry[3..]);
        if x == b'R' || x == b'C' {
            // Renames carry the original path as the next NUL-separated field.
            entries.next();
        }
        let status = match (x, y) {
            (b'?', b'?') => FileStatus::Untracked,
            (b'U', _) | (_, b'U') | (b'A', b'A') | (b'D', b'D') => FileStatus::Conflicted,
            (b'A', _) => FileStatus::Added,
            (b'R', _) => FileStatus::Renamed,
            (b'D', _) | (_, b'D') => FileStatus::Deleted,
            _ => FileStatus::Modified,
        };
        let staged = x != b' ' && x != b'?';
        state.changes.push(Change { path, status, staged });
    }

    if let Ok(log) = git(dir, &["log", "-n", "40", "--pretty=format:%h%x1f%s%x1f%an%x1f%ar"]) {
        state.log = log
            .lines()
            .filter_map(|line| {
                let mut parts = line.split('\u{1f}');
                Some(Commit {
                    hash: parts.next()?.to_string(),
                    subject: parts.next()?.to_string(),
                    author: parts.next()?.to_string(),
                    when: parts.next()?.to_string(),
                })
            })
            .collect();
    }
    Ok(state)
}

fn parse_branch_header(header: &str, state: &mut RepoState) {
    // "main...origin/main [ahead 1, behind 2]" or "No commits yet on main"
    let (names, tracking) = match header.split_once(" [") {
        Some((names, rest)) => (names, rest.trim_end_matches(']')),
        None => (header, ""),
    };
    let branch = names
        .strip_prefix("No commits yet on ")
        .unwrap_or(names)
        .split("...")
        .next()
        .unwrap_or_default();
    state.branch = Some(branch.to_string());
    for part in tracking.split(", ") {
        if let Some(n) = part.strip_prefix("ahead ") {
            state.ahead = n.parse().unwrap_or(0);
        } else if let Some(n) = part.strip_prefix("behind ") {
            state.behind = n.parse().unwrap_or(0);
        }
    }
}

pub fn commit_all(dir: &Path, message: &str) -> Result<String> {
    git(dir, &["add", "-A"])?;
    git(dir, &["commit", "-m", message])
}

pub fn pull(dir: &Path) -> Result<String> {
    git(dir, &["pull", "--ff-only"])
}

pub fn push(dir: &Path) -> Result<String> {
    git(dir, &["push"])
}

pub fn fetch(dir: &Path) -> Result<String> {
    git(dir, &["fetch", "--prune"])
}

pub fn discard(dir: &Path, change: &Change) -> Result<()> {
    let path = change.path.to_string_lossy();
    match change.status {
        FileStatus::Untracked => std::fs::remove_file(dir.join(&change.path))
            .with_context(|| format!("failed to delete {path}")),
        _ => git(dir, &["checkout", "HEAD", "--", &path]).map(|_| ()),
    }
}

/// Clones `url` into `parent/<repo name>` and returns the new checkout path.
pub fn clone(url: &str, parent: &Path) -> Result<PathBuf> {
    let name = repo_name(url).context("could not derive a folder name from the URL")?;
    let target = parent.join(&name);
    if target.exists() {
        bail!("{} already exists", target.display());
    }
    git(parent, &["clone", url, &name])?;
    Ok(target)
}

pub fn repo_name(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let name = trimmed.rsplit(['/', ':']).next()?;
    (!name.is_empty()).then(|| name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repo_name_handles_common_url_shapes() {
        assert_eq!(repo_name("https://github.com/a/b.git").as_deref(), Some("b"));
        assert_eq!(repo_name("git@github.com:a/b.git").as_deref(), Some("b"));
        assert_eq!(repo_name("https://github.com/a/b/").as_deref(), Some("b"));
        assert_eq!(repo_name("  ").as_deref(), None);
    }

    #[test]
    fn branch_header_parses_tracking() {
        let mut state = RepoState::default();
        parse_branch_header("main...origin/main [ahead 2, behind 1]", &mut state);
        assert_eq!(state.branch.as_deref(), Some("main"));
        assert_eq!((state.ahead, state.behind), (2, 1));
    }
}
