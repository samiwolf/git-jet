//! Git Jet engine.
//!
//! The platform-independent logic lives in [`core`]. This crate root supplies
//! the host executor backed by `std::process`, which powers the Tauri desktop
//! application and the host test suite.

pub mod core;

use std::io::Write;
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Windows `CREATE_NO_WINDOW` flag: keeps console processes spawned from the
/// GUI shell from flashing a console window per git invocation.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub use core::{
    argument_completions, parse_request, ArgumentCompletion, Branch, BranchInfo, ChangedFile,
    CmdOutput, Commit, CommitDetail, CommitNode, DiffMode, EntryKind, GitClient, GitExecutor,
    GitRequest, Head, LogFilter, Rendered, RenderedSection, RollbackLevel, StashAction,
    StashEntry, StatusEntry, StatusReport, TagEntry,
};

/// Builds a `git` invocation with the shared arguments every command uses.
///
/// Arguments are passed verbatim (never through a shell), so paths and commit
/// messages containing spaces or quotes are safe.
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
