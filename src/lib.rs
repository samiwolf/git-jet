//! Git Jet engine.
//!
//! The platform-independent logic lives in [`core`]. This crate root supplies
//! the host executor backed by `std::process`, which powers the Tauri desktop
//! application and the host test suite.

pub mod core;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::UNIX_EPOCH;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows `CREATE_NO_WINDOW` flag: keeps console processes spawned from the
/// GUI shell from flashing a console window per git invocation.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub use core::{
    argument_completions, clone_repository, parse_request, ArgumentCompletion, Branch, BranchInfo,
    ChangedFile, CmdOutput, Commit, CommitDetail, CommitNode, DiffMode, EntryKind, GitClient,
    GitExecutor, GitRequest, Head, LogFilter, RemoteEntry, Rendered, RenderedSection,
    RollbackLevel, StashAction, StashEntry, StatusEntry, StatusReport, TagEntry,
};

/// Builds a `git` invocation with the shared arguments every command uses.
///
/// Arguments are passed verbatim (never through a shell), so paths and commit
/// messages containing spaces or quotes are safe.
///
/// `core.fsmonitor` (git's filesystem-watch daemon) and the untracked cache
/// make repeated `status`/`diff` runs on large repositories dramatically
/// cheaper: after the first warm-up, git asks the daemon what changed instead
/// of scanning the worktree. Both are passed per invocation with `-c`, so the
/// user's repository configuration is never touched.
fn git_command(root: &str) -> Command {
    let mut command = Command::new("git");
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
    command
        .arg("-C")
        .arg(root)
        .arg("--no-pager")
        .arg("-c")
        .arg("color.ui=false")
        .arg("-c")
        .arg("core.quotepath=false")
        .arg("-c")
        .arg("core.fsmonitor=true")
        .arg("-c")
        .arg("core.untrackedcache=true")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .env("GIT_OPTIONAL_LOCKS", "0");
    command
}

/// Runs `git` in a directory using the host operating system.
///
/// Arguments are passed verbatim (never through a shell), so paths and commit
/// messages containing spaces or quotes are safe.
#[derive(Debug, Clone, Copy, Default)]
pub struct StdExecutor;

impl GitExecutor for StdExecutor {
    fn exec(&mut self, root: &str, args: &[String]) -> Result<CmdOutput, String> {
        let output = git_command(root)
            .args(args)
            .output()
            .map_err(|error| error.to_string())?;
        Ok(CmdOutput {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }

    fn exec_stdin(
        &mut self,
        root: &str,
        args: &[String],
        stdin: &[u8],
    ) -> Result<CmdOutput, String> {
        let mut child = git_command(root)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| error.to_string())?;
        child
            .stdin
            .take()
            .ok_or_else(|| "Failed to open git stdin.".to_string())?
            .write_all(stdin)
            .map_err(|error| error.to_string())?;
        let output = child
            .wait_with_output()
            .map_err(|error| error.to_string())?;
        Ok(CmdOutput {
            status: output.status.code(),
            stdout: output.stdout,
            stderr: output.stderr,
        })
    }
}

/// A Git Jet client bound to a working directory on the host.
///
/// Wraps [`GitClient`] with a [`StdExecutor`] so Tauri commands can call the
/// engine directly.
pub struct Engine {
    client: GitClient<StdExecutor>,
}

impl Engine {
    pub fn new(worktree_root: impl Into<String>) -> Self {
        Self {
            client: GitClient::new(StdExecutor, worktree_root),
        }
    }

    /// Full access to the underlying client for request-style operations.
    pub fn client(&mut self) -> &mut GitClient<StdExecutor> {
        &mut self.client
    }
}

/// Cheap filesystem signature of the parts of a repository the UI reads.
///
/// The auto-refresh timer computes this stamp (a handful of `stat` calls,
/// no git process) and skips the full `status` re-read while it is
/// unchanged. It covers the index, `HEAD`, fetch state, merge-state files,
/// `packed-refs`, the ref files themselves and the worktree root directory.
///
/// Returns `None` when the repository layout cannot be resolved; callers
/// then simply refresh.
pub fn repo_stamp(worktree_root: &str) -> Option<String> {
    let root = Path::new(worktree_root);
    let dot_git = root.join(".git");
    let git_dir = resolve_git_dir(root, &dot_git)?;
    let common_dir = resolve_common_dir(&git_dir);

    let mut stamp = String::with_capacity(256);
    for file in ["index", "HEAD", "FETCH_HEAD", "MERGE_HEAD", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
        push_entry(&mut stamp, &git_dir.join(file));
    }
    push_entry(&mut stamp, &common_dir.join("packed-refs"));
    push_dir_stamp(&mut stamp, &common_dir.join("refs"));
    push_entry(&mut stamp, root);
    Some(stamp)
}

/// `.git` as a directory, or the target of a `.git` file (worktrees,
/// submodules), resolved relative to the worktree root.
fn resolve_git_dir(root: &Path, dot_git: &Path) -> Option<PathBuf> {
    if dot_git.is_dir() {
        return Some(dot_git.to_path_buf());
    }
    let pointer = std::fs::read_to_string(dot_git).ok()?;
    let target = pointer.strip_prefix("gitdir:")?.trim();
    if target.is_empty() {
        return None;
    }
    let path = Path::new(target);
    Some(if path.is_absolute() { path.to_path_buf() } else { root.join(path) })
}

/// Linked worktrees keep `index`/`HEAD` in their own git dir but share refs
/// through a `commondir` pointer.
fn resolve_common_dir(git_dir: &Path) -> PathBuf {
    match std::fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) if !common.trim().is_empty() => {
            let path = Path::new(common.trim());
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                git_dir.join(path)
            }
        }
        _ => git_dir.to_path_buf(),
    }
}

/// Appends `mtime+size` of one file, or a distinct marker when it is absent.
fn push_entry(stamp: &mut String, path: &Path) {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            let nanos = metadata
                .modified()
                .ok()
                .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
                .map(|since_epoch| since_epoch.as_nanos())
                .unwrap_or(0);
            let _ = std::fmt::Write::write_fmt(
                stamp,
                format_args!("{:x}.{:x};", nanos, metadata.len()),
            );
        }
        Err(_) => stamp.push('-'),
    }
}

/// Appends the recursive maximum stamp of every file below `dir`, in a
/// deterministic order (ref directories are small).
fn push_dir_stamp(stamp: &mut String, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        stamp.push('-');
        return;
    };
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            push_dir_stamp(stamp, &path);
        } else {
            push_entry(stamp, &path);
        }
    }
}
