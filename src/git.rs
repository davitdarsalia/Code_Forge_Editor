//! Git access. Local operations (status, log, index edits, commit, discard)
//! run in-process through libgit2 (`git2`), which avoids spawning a process
//! per refresh. Network operations (fetch/pull/push/clone) still shell out to
//! the `git` CLI because they need the user's credential helpers, ssh-agent and
//! ssh config, which libgit2 does not pick up on its own.
//!
//! Every call is blocking and meant to run on the background executor.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use git2::{
    Branch, ErrorCode, IndexAddOption, Repository, RepositoryState, Sort, Status, StatusOptions,
    StatusShow, build::CheckoutBuilder,
};

const LOG_LIMIT: usize = 40;

/// Hooks that `git commit` would run. If any exists we commit through the CLI
/// so the user's hooks still fire (libgit2 never runs hooks).
const COMMIT_HOOKS: [&str; 4] = ["pre-commit", "prepare-commit-msg", "commit-msg", "post-commit"];

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Relative to the repository's work tree.
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
    /// Absolute work tree root; change paths are relative to it.
    pub workdir: PathBuf,
    pub branch: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Index vs HEAD.
    pub staged: Vec<Change>,
    /// Work tree vs index, plus untracked files and conflicts.
    pub unstaged: Vec<Change>,
    pub log: Vec<Commit>,
}

impl RepoState {
    /// Absolute path -> status, used to color the file tree. The work tree
    /// status wins over the staged one since it is what's on disk.
    pub fn status_map(&self) -> HashMap<PathBuf, FileStatus> {
        let mut map: HashMap<PathBuf, FileStatus> = self
            .staged
            .iter()
            .map(|change| (self.workdir.join(&change.path), change.status))
            .collect();
        map.extend(self.unstaged.iter().map(|change| (self.workdir.join(&change.path), change.status)));
        map
    }

    /// Number of distinct changed files (a file can be both staged and modified).
    pub fn file_count(&self) -> usize {
        let unstaged: HashSet<&Path> = self.unstaged.iter().map(|u| u.path.as_path()).collect();
        unstaged.len() + self.staged.iter().filter(|s| !unstaged.contains(s.path.as_path())).count()
    }

    /// Everything the file tree and navigator badge need, computed once per
    /// status read (off the UI thread) so rendering is lookups only.
    pub fn tree_marks(&self) -> TreeMarks {
        let files = self.status_map();
        let mut dirs = HashSet::new();
        for path in files.keys() {
            for dir in path.ancestors().skip(1) {
                // Stop at the work tree root and at directories already
                // recorded (their ancestors are recorded too).
                if !dir.starts_with(&self.workdir) || !dirs.insert(dir.to_path_buf()) {
                    break;
                }
            }
        }
        TreeMarks { files, dirs, change_count: self.file_count() }
    }
}

/// Precomputed git decorations for the file tree.
#[derive(Debug, Default)]
pub struct TreeMarks {
    /// Absolute path -> status (see `RepoState::status_map`).
    pub files: HashMap<PathBuf, FileStatus>,
    /// Every directory, up to the work tree root, that contains a change.
    pub dirs: HashSet<PathBuf>,
    /// Distinct changed files (`RepoState::file_count`).
    pub change_count: usize,
}

fn open(dir: &Path) -> Result<Repository> {
    Repository::discover(dir).with_context(|| format!("{} is not a git repository", dir.display()))
}

fn workdir(repo: &Repository) -> Result<PathBuf> {
    repo.workdir().map(Path::to_path_buf).context("bare repositories are not supported")
}

/// libgit2 reports the work tree with symlinks resolved; express it through
/// the caller's spelling of `dir` (or its ancestor) so absolute paths line up
/// with the file tree's.
fn workdir_as_seen_from(dir: &Path, workdir: &Path) -> PathBuf {
    let Ok(target) = workdir.canonicalize() else { return workdir.to_path_buf() };
    dir.ancestors()
        .find(|candidate| candidate.canonicalize().is_ok_and(|c| c == target))
        .map_or_else(|| workdir.to_path_buf(), Path::to_path_buf)
}

pub fn is_repo(dir: &Path) -> bool {
    Repository::discover(dir).is_ok_and(|repo| !repo.is_bare())
}

pub fn read_state(dir: &Path) -> Result<RepoState> {
    let repo = open(dir)?;
    let mut state = RepoState { workdir: workdir_as_seen_from(dir, &workdir(&repo)?), ..Default::default() };
    read_branch(&repo, &mut state);

    let mut options = StatusOptions::new();
    options
        .show(StatusShow::IndexAndWorkdir)
        .include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .renames_head_to_index(true);
    let statuses = repo.statuses(Some(&mut options)).context("failed to read git status")?;
    for entry in statuses.iter() {
        let status = entry.status();
        if status.is_ignored() {
            continue;
        }
        // Renames report the new path through the head-to-index delta.
        let path = entry
            .head_to_index()
            .and_then(|delta| delta.new_file().path().map(Path::to_path_buf))
            .unwrap_or_else(|| PathBuf::from(String::from_utf8_lossy(entry.path_bytes()).into_owned()));
        let (staged, unstaged) = classify(status);
        if let Some(status) = staged {
            state.staged.push(Change { path: path.clone(), status, staged: true });
        }
        if let Some(status) = unstaged {
            state.unstaged.push(Change { path, status, staged: false });
        }
    }
    state.staged.sort_by(|a, b| a.path.cmp(&b.path));
    state.unstaged.sort_by(|a, b| a.path.cmp(&b.path));

    match read_log(&repo) {
        Ok(log) => state.log = log,
        Err(err) => eprintln!("forge: git log failed: {err}"),
    }
    Ok(state)
}

/// Splits a libgit2 status into its (index, work tree) halves.
fn classify(status: Status) -> (Option<FileStatus>, Option<FileStatus>) {
    if status.is_conflicted() {
        return (None, Some(FileStatus::Conflicted));
    }
    let staged = if status.is_index_new() {
        Some(FileStatus::Added)
    } else if status.is_index_deleted() {
        Some(FileStatus::Deleted)
    } else if status.is_index_renamed() {
        Some(FileStatus::Renamed)
    } else if status.is_index_modified() || status.is_index_typechange() {
        Some(FileStatus::Modified)
    } else {
        None
    };
    let unstaged = if status.is_wt_new() {
        Some(FileStatus::Untracked)
    } else if status.is_wt_deleted() {
        Some(FileStatus::Deleted)
    } else if status.is_wt_renamed() {
        Some(FileStatus::Renamed)
    } else if status.is_wt_modified() || status.is_wt_typechange() {
        Some(FileStatus::Modified)
    } else {
        None
    };
    (staged, unstaged)
}

fn read_branch(repo: &Repository, state: &mut RepoState) {
    let head = match repo.head() {
        Ok(head) => head,
        Err(err) if err.code() == ErrorCode::UnbornBranch => {
            // No commits yet: HEAD is a symbolic ref to a branch that doesn't exist.
            state.branch = repo
                .find_reference("HEAD")
                .ok()
                .and_then(|r| r.symbolic_target().ok().flatten().map(|t| t.trim_start_matches("refs/heads/").to_string()));
            return;
        }
        Err(err) => {
            eprintln!("forge: git HEAD unreadable: {err}");
            return;
        }
    };
    if !head.is_branch() {
        let short = head.target().map(oid_short).unwrap_or_default();
        state.branch = Some(format!("HEAD ({short})"));
        return;
    }
    state.branch = head.shorthand().ok().map(str::to_string);
    let local = head.target();
    let Ok(upstream) = Branch::wrap(head).upstream() else { return };
    if let (Some(local), Some(remote)) = (local, upstream.get().target()) {
        match repo.graph_ahead_behind(local, remote) {
            Ok((ahead, behind)) => (state.ahead, state.behind) = (ahead as u32, behind as u32),
            Err(err) => eprintln!("forge: git ahead/behind failed: {err}"),
        }
    }
}

fn read_log(repo: &Repository) -> Result<Vec<Commit>> {
    let mut walk = repo.revwalk()?;
    if let Err(err) = walk.push_head() {
        // An unborn branch simply has no history.
        if err.code() == ErrorCode::UnbornBranch || err.code() == ErrorCode::NotFound {
            return Ok(Vec::new());
        }
        return Err(err.into());
    }
    walk.set_sorting(Sort::TIME)?;
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let mut log = Vec::with_capacity(LOG_LIMIT);
    for oid in walk.take(LOG_LIMIT) {
        let commit = repo.find_commit(oid?)?;
        let author = commit.author();
        log.push(Commit {
            hash: oid_short(commit.id()),
            subject: commit.summary().ok().flatten().unwrap_or_default().to_string(),
            author: author.name().unwrap_or_default().to_string(),
            when: relative_time(now, commit.time().seconds()),
        });
    }
    Ok(log)
}

fn oid_short(oid: git2::Oid) -> String {
    oid.to_string()[..7].to_string()
}

/// "3 hours ago", in the spirit of git's `%ar`.
pub fn relative_time(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    let (n, unit) = match secs {
        s if s < 60 => return "just now".into(),
        s if s < 3_600 => (s / 60, "minute"),
        s if s < 86_400 => (s / 3_600, "hour"),
        s if s < 86_400 * 14 => (s / 86_400, "day"),
        s if s < 86_400 * 60 => (s / (86_400 * 7), "week"),
        s if s < 86_400 * 365 => (s / (86_400 * 30), "month"),
        s => (s / (86_400 * 365), "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

// ---- Index ------------------------------------------------------------------

/// `git add <path>` (also records deletions and resolves conflicts).
pub fn stage_file(dir: &Path, path: &Path) -> Result<()> {
    let repo = open(dir)?;
    let mut index = repo.index()?;
    let result = if workdir(&repo)?.join(path).symlink_metadata().is_ok() {
        index.add_path(path)
    } else {
        index.remove_path(path)
    };
    result.with_context(|| format!("failed to stage {}", path.display()))?;
    index.write().context("failed to write the index")?;
    Ok(())
}

/// `git restore --staged <path>`.
pub fn unstage_file(dir: &Path, path: &Path) -> Result<()> {
    let repo = open(dir)?;
    unstage_paths(&repo, &[path.to_path_buf()])
}

/// `git add -A`.
pub fn stage_all(dir: &Path) -> Result<()> {
    let repo = open(dir)?;
    let mut index = repo.index()?;
    index.add_all(["*"], IndexAddOption::DEFAULT, None).context("failed to stage files")?;
    index.update_all(["*"], None).context("failed to stage deletions")?;
    index.write().context("failed to write the index")?;
    Ok(())
}

/// `git restore --staged :/`.
pub fn unstage_all(dir: &Path) -> Result<()> {
    let repo = open(dir)?;
    let mut options = StatusOptions::new();
    options.show(StatusShow::Index).renames_head_to_index(true);
    let mut paths = Vec::new();
    for entry in repo.statuses(Some(&mut options))?.iter() {
        // For renames both sides must be reset: the new path leaves the index
        // and the old one comes back.
        if let Some(delta) = entry.head_to_index() {
            paths.extend(delta.old_file().path().map(Path::to_path_buf));
            paths.extend(delta.new_file().path().map(Path::to_path_buf));
        }
    }
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Ok(());
    }
    unstage_paths(&repo, &paths)
}

fn unstage_paths(repo: &Repository, paths: &[PathBuf]) -> Result<()> {
    // With no HEAD (first commit) there is nothing to reset to: `None` removes
    // the entries from the index, which is what unstaging means there.
    let head = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?.into_object()),
        Err(err) if err.code() == ErrorCode::UnbornBranch => None,
        Err(err) => return Err(err.into()),
    };
    repo.reset_default(head.as_ref(), paths).context("failed to unstage")?;
    Ok(())
}

// ---- Commit -----------------------------------------------------------------

/// Commits the index. When nothing is staged and `all_if_empty` is set, stages
/// everything first (the old "Commit All" behaviour). Returns the short hash.
pub fn commit(dir: &Path, message: &str, all_if_empty: bool) -> Result<String> {
    let repo = open(dir)?;
    let message = git2::message_prettify(message, Some(b'#')).context("invalid commit message")?;
    if message.trim().is_empty() {
        bail!("Write a commit message first");
    }
    if repo.index()?.has_conflicts() {
        bail!("Resolve merge conflicts before committing");
    }
    let staged_any = has_staged(&repo)?;
    if !staged_any && !all_if_empty {
        bail!("Nothing staged to commit");
    }

    // Hooks, signing and merge/cherry-pick commits are git's job; libgit2
    // would silently skip them.
    if needs_cli_commit(&repo) {
        let workdir = workdir(&repo)?;
        if !staged_any {
            git(&workdir, &["add", "-A"])?;
        }
        git(&workdir, &["commit", "-m", &message])?;
        return git(&workdir, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_string());
    }

    if !staged_any {
        stage_all(dir)?;
        if !has_staged(&repo)? {
            bail!("Nothing to commit");
        }
    }
    let mut index = repo.index()?;
    let tree = repo.find_tree(index.write_tree()?)?;
    let signature = repo
        .signature()
        .context("set user.name and user.email in your git config to commit")?;
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit()?),
        Err(err) if err.code() == ErrorCode::UnbornBranch => None,
        Err(err) => return Err(err.into()),
    };
    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo.commit(Some("HEAD"), &signature, &signature, &message, &tree, &parents)?;
    Ok(oid_short(oid))
}

fn has_staged(repo: &Repository) -> Result<bool> {
    let mut options = StatusOptions::new();
    options.show(StatusShow::Index);
    Ok(!repo.statuses(Some(&mut options))?.is_empty())
}

fn needs_cli_commit(repo: &Repository) -> bool {
    if repo.state() != RepositoryState::Clean {
        return true;
    }
    let config = match repo.config() {
        Ok(config) => config,
        Err(err) => {
            eprintln!("forge: git config unreadable: {err}");
            return true;
        }
    };
    if config.get_bool("commit.gpgsign").unwrap_or(false) {
        return true;
    }
    let hooks = match config.get_path("core.hooksPath") {
        Ok(path) if path.is_absolute() => path,
        Ok(path) => repo.workdir().unwrap_or(repo.path()).join(path),
        Err(_) => repo.path().join("hooks"),
    };
    COMMIT_HOOKS.iter().any(|hook| hooks.join(hook).is_file())
}

// ---- Discard ----------------------------------------------------------------

/// Throws away the work tree change of `change`: untracked files are deleted,
/// tracked ones are restored from the index (conflicts from HEAD).
pub fn discard(dir: &Path, change: &Change) -> Result<()> {
    let repo = open(dir)?;
    let absolute = workdir(&repo)?.join(&change.path);
    let path = change.path.display();
    let mut checkout = CheckoutBuilder::new();
    checkout.force().disable_pathspec_match(true).path(&change.path);
    match change.status {
        FileStatus::Untracked => {
            let result = if absolute.is_dir() {
                std::fs::remove_dir_all(&absolute)
            } else {
                std::fs::remove_file(&absolute)
            };
            result.with_context(|| format!("failed to delete {path}"))
        }
        FileStatus::Conflicted => {
            repo.checkout_head(Some(&mut checkout)).with_context(|| format!("failed to restore {path}"))
        }
        _ => repo
            .checkout_index(None, Some(&mut checkout))
            .with_context(|| format!("failed to restore {path}")),
    }
}

// ---- Network (git CLI) ------------------------------------------------------

/// Runs the git CLI. Used only where libgit2 would lack the user's
/// credentials/ssh-agent setup (network) or skip behaviour (hooks, signing).
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

pub fn pull(dir: &Path) -> Result<String> {
    git(dir, &["pull", "--ff-only"])
}

pub fn push(dir: &Path) -> Result<String> {
    git(dir, &["push"])
}

pub fn fetch(dir: &Path) -> Result<String> {
    git(dir, &["fetch", "--prune"])
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
    fn tree_marks_mark_every_ancestor_dir_and_count_distinct_files() {
        let change = |path: &str, status, staged| Change { path: path.into(), status, staged };
        let state = RepoState {
            workdir: "/w".into(),
            staged: vec![change("a/b/c.rs", FileStatus::Added, true), change("top.txt", FileStatus::Modified, true)],
            unstaged: vec![change("a/b/c.rs", FileStatus::Modified, false), change("a/d.rs", FileStatus::Untracked, false)],
            ..Default::default()
        };
        let marks = state.tree_marks();
        assert_eq!(marks.change_count, 3);
        assert_eq!(marks.files.get(Path::new("/w/a/b/c.rs")), Some(&FileStatus::Modified));
        let mut dirs: Vec<_> = marks.dirs.iter().map(|d| d.display().to_string()).collect();
        dirs.sort();
        assert_eq!(dirs, ["/w", "/w/a", "/w/a/b"]);
    }

    #[test]
    fn repo_name_handles_common_url_shapes() {
        assert_eq!(repo_name("https://github.com/a/b.git").as_deref(), Some("b"));
        assert_eq!(repo_name("git@github.com:a/b.git").as_deref(), Some("b"));
        assert_eq!(repo_name("https://github.com/a/b/").as_deref(), Some("b"));
        assert_eq!(repo_name("  ").as_deref(), None);
    }

    #[test]
    fn relative_time_buckets() {
        assert_eq!(relative_time(100, 90), "just now");
        assert_eq!(relative_time(3_600 * 3, 0), "3 hours ago");
        assert_eq!(relative_time(86_400, 0), "1 day ago");
        assert_eq!(relative_time(86_400 * 365 * 2, 0), "2 years ago");
    }

    /// A throwaway repo under the system temp dir, removed on drop.
    struct TempRepo {
        dir: PathBuf,
        repo: Repository,
    }

    impl TempRepo {
        fn new(name: &str) -> Self {
            let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let dir = std::env::temp_dir().join(format!("forge-git-{name}-{}-{nanos}", std::process::id()));
            let repo = Repository::init(&dir).unwrap();
            let mut config = repo.config().unwrap();
            config.set_str("user.name", "Forge Test").unwrap();
            config.set_str("user.email", "forge@example.com").unwrap();
            // Keep a global hooksPath/gpgsign from routing commits to the CLI.
            config.set_str("core.hooksPath", ".git/no-hooks").unwrap();
            config.set_bool("commit.gpgsign", false).unwrap();
            drop(config);
            TempRepo { dir, repo }
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.dir.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn state(&self) -> RepoState {
            read_state(&self.dir).unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            if let Err(err) = std::fs::remove_dir_all(&self.dir) {
                eprintln!("forge: failed to remove {}: {err}", self.dir.display());
            }
        }
    }

    fn paths(changes: &[Change]) -> Vec<(String, FileStatus)> {
        changes.iter().map(|c| (c.path.display().to_string(), c.status)).collect()
    }

    #[test]
    fn status_stage_unstage_commit_roundtrip() {
        let t = TempRepo::new("roundtrip");
        t.write("a.txt", "one\n");
        t.write("dir/b.txt", "two\n");

        let state = t.state();
        assert!(state.branch.is_some(), "an unborn branch still has a name");
        assert!(state.staged.is_empty());
        assert_eq!(
            paths(&state.unstaged),
            vec![("a.txt".into(), FileStatus::Untracked), ("dir/b.txt".into(), FileStatus::Untracked)]
        );

        // Stage and unstage a single file before the first commit (no HEAD).
        stage_file(&t.dir, Path::new("a.txt")).unwrap();
        assert_eq!(paths(&t.state().staged), vec![("a.txt".into(), FileStatus::Added)]);
        unstage_file(&t.dir, Path::new("a.txt")).unwrap();
        assert!(t.state().staged.is_empty());

        // Nothing staged: refused unless commit-all is allowed.
        assert!(commit(&t.dir, "nothing", false).is_err());
        commit(&t.dir, "initial", true).unwrap();
        let state = t.state();
        assert!(state.staged.is_empty() && state.unstaged.is_empty());
        assert_eq!(state.log.len(), 1);
        assert_eq!(state.log[0].subject, "initial");
        assert_eq!(state.log[0].author, "Forge Test");

        // Modify both, stage one: only that one is committed.
        t.write("a.txt", "one changed\n");
        t.write("dir/b.txt", "two changed\n");
        stage_file(&t.dir, Path::new("a.txt")).unwrap();
        let state = t.state();
        assert_eq!(paths(&state.staged), vec![("a.txt".into(), FileStatus::Modified)]);
        assert_eq!(paths(&state.unstaged), vec![("dir/b.txt".into(), FileStatus::Modified)]);
        assert_eq!(state.file_count(), 2);
        assert_eq!(state.status_map().len(), 2);

        commit(&t.dir, "change a", true).unwrap();
        let state = t.state();
        assert!(state.staged.is_empty());
        assert_eq!(paths(&state.unstaged), vec![("dir/b.txt".into(), FileStatus::Modified)]);
        assert_eq!(state.log[0].subject, "change a");
        assert_eq!(state.log.len(), 2);

        // A file both staged and further modified shows up in both lists.
        stage_file(&t.dir, Path::new("dir/b.txt")).unwrap();
        t.write("dir/b.txt", "two changed again\n");
        let state = t.state();
        assert_eq!(paths(&state.staged), vec![("dir/b.txt".into(), FileStatus::Modified)]);
        assert_eq!(paths(&state.unstaged), vec![("dir/b.txt".into(), FileStatus::Modified)]);
        assert_eq!(state.file_count(), 1);

        unstage_all(&t.dir).unwrap();
        assert!(t.state().staged.is_empty());
    }

    #[test]
    fn stage_all_records_deletions_and_discard_restores() {
        let t = TempRepo::new("deletions");
        t.write("keep.txt", "keep\n");
        t.write("gone.txt", "gone\n");
        commit(&t.dir, "initial", true).unwrap();

        std::fs::remove_file(t.dir.join("gone.txt")).unwrap();
        t.write("new.txt", "new\n");
        t.write("keep.txt", "edited\n");
        stage_all(&t.dir).unwrap();
        let state = t.state();
        assert_eq!(
            paths(&state.staged),
            vec![
                ("gone.txt".into(), FileStatus::Deleted),
                ("keep.txt".into(), FileStatus::Modified),
                ("new.txt".into(), FileStatus::Added),
            ]
        );
        assert!(state.unstaged.is_empty());

        unstage_all(&t.dir).unwrap();
        let state = t.state();
        assert!(state.staged.is_empty());
        assert_eq!(state.unstaged.len(), 3);

        // Discard restores tracked files and deletes untracked ones.
        for change in &state.unstaged {
            discard(&t.dir, change).unwrap();
        }
        let state = t.state();
        assert!(state.unstaged.is_empty() && state.staged.is_empty());
        assert_eq!(std::fs::read_to_string(t.dir.join("keep.txt")).unwrap(), "keep\n");
        assert!(t.dir.join("gone.txt").exists());
        assert!(!t.dir.join("new.txt").exists());
    }

    #[test]
    fn stage_file_records_a_deletion() {
        let t = TempRepo::new("rm");
        t.write("x.txt", "x\n");
        commit(&t.dir, "initial", true).unwrap();
        std::fs::remove_file(t.dir.join("x.txt")).unwrap();
        stage_file(&t.dir, Path::new("x.txt")).unwrap();
        let state = t.state();
        assert_eq!(paths(&state.staged), vec![("x.txt".into(), FileStatus::Deleted)]);
        assert!(state.unstaged.is_empty());
        assert!(t.repo.index().unwrap().get_path(Path::new("x.txt"), 0).is_none());
    }

    #[test]
    fn status_paths_are_relative_to_the_workdir_from_a_subdirectory() {
        let t = TempRepo::new("subdir");
        t.write("sub/f.txt", "f\n");
        let state = read_state(&t.dir.join("sub")).unwrap();
        assert_eq!(paths(&state.unstaged), vec![("sub/f.txt".into(), FileStatus::Untracked)]);
        assert!(state.status_map().contains_key(&t.dir.join("sub/f.txt")), "keys use the caller's path spelling");
    }
}
