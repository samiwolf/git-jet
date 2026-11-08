//! Git command orchestration for Git Jet.

use super::log::{parse_branches, parse_log_nodes, parse_log_records, parse_stashes};
use super::model::{
    CmdOutput, Commit, CommitNode, GitExecutor, Rendered, StashEntry, StatusReport,
};
use super::render;
use super::request::{DiffMode, GitRequest, RollbackLevel, StashAction};
use super::status::parse_status_v2;

const BRANCH_FORMAT: &str = "%(refname:short)\u{1f}%(HEAD)\u{1f}%(upstream:short)\u{1f}%(upstream:track)\u{1f}%(objectname:short)\u{1f}%(committerdate:relative)\u{1f}%(symref)";
const GRAPH_FORMAT: &str = "%h%d %s (%an, %ar)";

/// Resolves the repository root and executes Git commands in it.
pub struct GitClient<E: GitExecutor> {
    executor: E,
    worktree_root: String,
    repo_root: Option<String>,
}

impl<E: GitExecutor> GitClient<E> {
    pub fn new(executor: E, worktree_root: impl Into<String>) -> Self {
        Self {
            executor,
            worktree_root: worktree_root.into(),
            repo_root: None,
        }
    }

    fn repo_root(&mut self) -> Result<String, String> {
        if let Some(root) = &self.repo_root {
            return Ok(root.clone());
        }

        let output = self.executor.exec(
            &self.worktree_root,
            &["rev-parse".to_string(), "--show-toplevel".to_string()],
        )?;
        if !output.success() {
            let detail = clean_error(&output);
            return Err(format!(
                "Not a Git repository ({}). Open a folder inside a Git working tree.",
                detail
            ));
        }
        let root = output.stdout_lossy().trim().to_string();
        if root.is_empty() {
            return Err("The Git repository root is empty.".to_string());
        }
        self.repo_root = Some(root.clone());
        Ok(root)
    }

    fn exec_owned(&mut self, args: &[String]) -> Result<CmdOutput, String> {
        let root = self.repo_root()?;
        self.executor.exec(&root, args)
    }

    fn exec(&mut self, args: &[&str]) -> Result<CmdOutput, String> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        self.exec_owned(&owned)
    }

    fn text_owned(&mut self, args: &[String]) -> Result<String, String> {
        let output = self.exec_owned(args)?;
        if !output.success() {
            return Err(git_failure(args, &output));
        }
        Ok(output.stdout_lossy())
    }

    fn text(&mut self, args: &[&str]) -> Result<String, String> {
        let owned: Vec<String> = args.iter().map(|value| (*value).to_string()).collect();
        self.text_owned(&owned)
    }

    fn text_or_empty(&mut self, args: &[&str]) -> String {
        match self.exec(args) {
            Ok(output) if output.success() => output.stdout_lossy(),
            _ => String::new(),
        }
    }

    fn exec_stdin_owned(&mut self, args: &[String], stdin: &[u8]) -> Result<CmdOutput, String> {
        let root = self.repo_root()?;
        self.executor.exec_stdin(&root, args, stdin)
    }

    fn status_report(&mut self) -> Result<StatusReport, String> {
        let output = self.exec(&["status", "--porcelain=v2", "--branch", "-z"])?;
        if !output.success() {
            return Err(git_failure(
                &[
                    "status".to_string(),
                    "--porcelain=v2".to_string(),
                    "--branch".to_string(),
                    "-z".to_string(),
                ],
                &output,
            ));
        }
        let mut report = parse_status_v2(&output.stdout)?;
        report.stash_count = self.stash_count().unwrap_or(0);
        report.last_commit = self.last_commit().ok().flatten();
        Ok(report)
    }

    fn stash_count(&mut self) -> Result<usize, String> {
        let output = self.text(&["stash", "list", "--format=%gd"])?;
        Ok(output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count())
    }

    fn last_commit(&mut self) -> Result<Option<Commit>, String> {
        let output = self.exec(&["log", "-1", "--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s"])?;
        if !output.success() {
            return Ok(None);
        }
        Ok(parse_log_records(&format!(
            "{}{}",
            output.stdout_lossy(),
            super::log::LOG_RECORD_SEPARATOR
        ))
        .into_iter()
        .next())
    }

    fn has_head(&mut self) -> Result<bool, String> {
        let output = self.exec(&["rev-parse", "--verify", "--quiet", "HEAD"])?;
        Ok(output.success())
    }

    fn branches(&mut self, remote: bool) -> Result<Vec<super::model::Branch>, String> {
        let mut args = vec!["branch".to_string(), format!("--format={BRANCH_FORMAT}")];
        if remote {
            args.insert(1, "-r".to_string());
        }
        let output = self.text_owned(&args)?;
        Ok(parse_branches(&output, remote))
    }

    /// Run one parsed request and render it as a Git Jet panel.
    pub fn run(&mut self, request: &GitRequest) -> Result<Rendered, String> {
        if matches!(request, GitRequest::Help) {
            return Ok(render::help());
        }

        // Resolve the repository once up front so every command shares the
        // same root and a non-repository gets a single friendly error.
        self.repo_root()?;

        match request {
            GitRequest::Dashboard => {
                let report = self.status_report()?;
                let pretty = format!("--pretty=format:{GRAPH_FORMAT}");
                let log = self.text_or_empty(&[
                    "log",
                    "--graph",
                    "--decorate",
                    "--color=never",
                    "--date=relative",
                    &pretty,
                    "-n",
                    "6",
                ]);
                Ok(render::dashboard(&report, &log))
            }
            GitRequest::Status => {
                let report = self.status_report()?;
                Ok(render::status(&report))
            }
            GitRequest::Log { count, all } => {
                let report = self.status_report()?;
                let count_string = count.to_string();
                let mut args = vec![
                    "log".to_string(),
                    "--graph".to_string(),
                    "--decorate".to_string(),
                    "--color=never".to_string(),
                    "--date=relative".to_string(),
                    format!("--pretty=format:{GRAPH_FORMAT}"),
                    "-n".to_string(),
                    count_string,
                ];
                if *all {
                    args.push("--all".to_string());
                }
                let log = if self.has_head()? {
                    self.text_owned(&args)?
                } else {
                    String::new()
                };
                Ok(render::log(&report, &log, *count, *all))
            }
            GitRequest::Diff { mode, paths } => {
                let report = self.status_report()?;
                let diff = self.diff(mode, paths)?;
                Ok(render::diff(&report, &diff, mode, paths))
            }
            GitRequest::Branches => {
                let report = self.status_report()?;
                let local = self.branches(false)?;
                let remote = self.branches(true)?;
                Ok(render::branches(&report, &local, &remote))
            }
            GitRequest::Commit { message, amend } => {
                let mut args = vec!["commit".to_string(), "-m".to_string(), message.clone()];
                if *amend {
                    args.push("--amend".to_string());
                }
                let output = self.text_owned(&args)?;
                let report = self.status_report()?;
                Ok(render::commit(&report, &output))
            }
            GitRequest::Stage { paths } => {
                let output = self.stage(paths)?;
                let report = self.status_report()?;
                let subtitle = if paths.is_empty() {
                    "All changes staged".to_string()
                } else {
                    format!("Staged {} path{}", paths.len(), plural(paths.len()))
                };
                Ok(render::action("Stage", &subtitle, &output, Some(&report)))
            }
            GitRequest::Unstage { paths } => {
                let output = self.unstage(paths)?;
                let report = self.status_report()?;
                let subtitle = if paths.is_empty() {
                    "All changes unstaged".to_string()
                } else {
                    format!("Unstaged {} path{}", paths.len(), plural(paths.len()))
                };
                Ok(render::action("Unstage", &subtitle, &output, Some(&report)))
            }
            GitRequest::Rollback { paths, level } => {
                let output = self.rollback_files(paths, *level)?;
                let report = self.status_report()?;
                let subtitle = match level {
                    RollbackLevel::Staged => format!(
                        "Rolled back staged changes in {} path{}",
                        paths.len(),
                        plural(paths.len())
                    ),
                    RollbackLevel::Untracked => format!(
                        "Deleted {} untracked path{}",
                        paths.len(),
                        plural(paths.len())
                    ),
                    RollbackLevel::Unstaged => format!(
                        "Rolled back worktree changes in {} path{}",
                        paths.len(),
                        plural(paths.len())
                    ),
                };
                Ok(render::action(
                    "Rollback",
                    &subtitle,
                    &output,
                    Some(&report),
                ))
            }
            GitRequest::Switch { branch } => {
                let output = self.text(&["switch", branch.as_str()])?;
                let report = self.status_report()?;
                let subtitle = format!("Switched to {branch}");
                Ok(render::action("Switch", &subtitle, &output, Some(&report)))
            }
            GitRequest::CreateBranch { name } => {
                let output = self.text(&["switch", "-c", name.as_str()])?;
                let report = self.status_report()?;
                let subtitle = format!("Created and switched to {name}");
                Ok(render::action(
                    "New Branch",
                    &subtitle,
                    &output,
                    Some(&report),
                ))
            }
            GitRequest::Stash { action } => self.run_stash(action),
            GitRequest::Fetch { all } => {
                let mut args = vec!["fetch".to_string()];
                if *all {
                    args.push("--all".to_string());
                }
                let output = self.text_owned(&args)?;
                Ok(render::action(
                    "Fetch",
                    if *all {
                        "Fetched all remotes"
                    } else {
                        "Fetched"
                    },
                    &output,
                    None,
                ))
            }
            GitRequest::Pull { args } => {
                let mut command = vec!["pull".to_string()];
                command.extend(args.iter().cloned());
                let output = self.text_owned(&command)?;
                let report = self.status_report()?;
                Ok(render::action(
                    "Pull",
                    "Pull complete",
                    &output,
                    Some(&report),
                ))
            }
            GitRequest::Push { args } => {
                let mut command = vec!["push".to_string()];
                command.extend(args.iter().cloned());
                let output = self.text_owned(&command)?;
                Ok(render::action("Push", "Push complete", &output, None))
            }
            GitRequest::Show { rev } => {
                let output = self.show_text(rev.as_str())?;
                Ok(render::show(rev, &output))
            }
            GitRequest::Blame { path } => {
                let output = self.text(&["blame", "--date=short", "-w", "--", path.as_str()])?;
                Ok(render::blame(path, &output))
            }
            GitRequest::Help => Ok(render::help()),
        }
    }

    fn diff(&mut self, mode: &DiffMode, paths: &[String]) -> Result<String, String> {
        if *mode == DiffMode::All && !self.has_head()? {
            let staged = self.diff_inner(DiffMode::Staged, paths)?;
            let unstaged = self.diff_inner(DiffMode::Unstaged, paths)?;
            return Ok(format!("{staged}{unstaged}"));
        }
        self.diff_inner(mode.clone(), paths)
    }

    fn diff_inner(&mut self, mode: DiffMode, paths: &[String]) -> Result<String, String> {
        let mut args = vec![
            "diff".to_string(),
            "--no-color".to_string(),
            "--find-renames".to_string(),
        ];
        match mode {
            DiffMode::Staged => args.push("--staged".to_string()),
            DiffMode::All => args.push("HEAD".to_string()),
            DiffMode::Unstaged => {}
        }
        if !paths.is_empty() {
            args.push("--".to_string());
            args.extend(paths.iter().cloned());
        }
        self.text_owned(&args)
    }

    /// Stage all changes or the given paths.
    pub fn stage(&mut self, paths: &[String]) -> Result<String, String> {
        if paths.is_empty() {
            return self.text(&["add", "--all"]);
        }
        let mut args = vec!["add".to_string(), "--".to_string()];
        args.extend(paths.iter().cloned());
        self.text_owned(&args)
    }

    /// Unstage all changes or the given paths.
    pub fn unstage(&mut self, paths: &[String]) -> Result<String, String> {
        let mut args = if self.has_head()? {
            vec![
                "restore".to_string(),
                "--staged".to_string(),
                "--".to_string(),
            ]
        } else {
            vec![
                "rm".to_string(),
                "--cached".to_string(),
                "-r".to_string(),
                "--".to_string(),
            ]
        };
        if paths.is_empty() {
            args.push(".".to_string());
        } else {
            args.extend(paths.iter().cloned());
        }
        self.text_owned(&args)
    }

    /// Roll back changes in the given paths, depending on `level`.
    ///
    /// * [`RollbackLevel::Staged`] reverts index **and** worktree back to
    ///   `HEAD`. Files that `HEAD` does not know (staged additions, staged
    ///   rename targets) are removed entirely.
    /// * [`RollbackLevel::Unstaged`] discards worktree changes, restoring the
    ///   paths to their staged (index) state. Staged changes survive.
    /// * [`RollbackLevel::Untracked`] deletes the untracked files.
    pub fn rollback_files(
        &mut self,
        paths: &[String],
        level: RollbackLevel,
    ) -> Result<String, String> {
        if paths.is_empty() {
            return Err("No paths were selected for rollback.".to_string());
        }
        match level {
            RollbackLevel::Untracked => self.rollback_untracked(paths),
            RollbackLevel::Unstaged => self.rollback_worktree(paths),
            RollbackLevel::Staged => self.rollback_staged(paths),
        }
    }

    fn rollback_untracked(&mut self, paths: &[String]) -> Result<String, String> {
        let mut args = vec!["clean".to_string(), "-f".to_string(), "-d".to_string()];
        args.push("--".to_string());
        args.extend(paths.iter().cloned());
        self.text_owned(&args)
    }

    fn rollback_worktree(&mut self, paths: &[String]) -> Result<String, String> {
        let mut args = vec!["restore".to_string()];
        args.push("--".to_string());
        args.extend(paths.iter().cloned());
        self.text_owned(&args)
    }

    fn rollback_staged(&mut self, paths: &[String]) -> Result<String, String> {
        let report = self.status_report()?;
        let mut in_head = Vec::new();
        let mut added = Vec::new();
        let mut rename_sources = Vec::new();
        for path in paths {
            let entry = report.entries.iter().find(|entry| entry.path == *path);
            match entry {
                Some(entry) if entry.is_conflict() => {
                    return Err(format!(
                        "Cannot roll back `{}`: resolve the conflict first.",
                        path
                    ));
                }
                // Staged renames and copies: restore the original path from
                // `HEAD` and drop the newly added path.
                Some(entry) if entry.index == 'R' || entry.index == 'C' => {
                    if let Some(orig) = &entry.orig_path {
                        rename_sources.push(orig.clone());
                    }
                    added.push(entry.path.clone());
                }
                // Files `HEAD` does not have: staged additions.
                Some(entry) if entry.index == 'A' => added.push(entry.path.clone()),
                _ => in_head.push(path.clone()),
            }
        }

        let mut output = String::new();
        if !in_head.is_empty() {
            output.push_str(&self.restore_from_head(&in_head)?);
        }
        if !rename_sources.is_empty() {
            output.push_str(&self.restore_from_head(&rename_sources)?);
        }
        if !added.is_empty() {
            let mut args = vec!["rm".to_string(), "-f".to_string(), "--".to_string()];
            args.extend(added);
            output.push_str(&self.text_owned(&args)?);
        }
        Ok(output)
    }

    fn restore_from_head(&mut self, paths: &[String]) -> Result<String, String> {
        let mut args = vec![
            "restore".to_string(),
            "--source=HEAD".to_string(),
            "--staged".to_string(),
            "--worktree".to_string(),
            "--".to_string(),
        ];
        args.extend(paths.iter().cloned());
        self.text_owned(&args)
    }

    /// Reverse-apply one hunk patch.
    ///
    /// With `staged`, the hunk is removed from the index (`git apply -R
    /// --cached`); otherwise it is discarded from the worktree. `patch` must
    /// be a valid unified diff fragment including the `diff --git`/`---`/`+++`
    /// headers and the `@@ … @@` hunk.
    pub fn rollback_hunk(&mut self, patch: &str, staged: bool) -> Result<String, String> {
        let mut args = vec![
            "apply".to_string(),
            "-R".to_string(),
            "--whitespace=nowarn".to_string(),
        ];
        if staged {
            args.push("--cached".to_string());
        }
        let output = self.exec_stdin_owned(&args, patch.as_bytes())?;
        if !output.success() {
            return Err(git_failure(&args, &output));
        }
        Ok(output.stdout_lossy())
    }

    fn run_stash(&mut self, action: &StashAction) -> Result<Rendered, String> {
        match action {
            StashAction::List => {
                let output = self.text(&["stash", "list", "--format=%gd%x1f%gs%x1f%cr"])?;
                Ok(render::stashes(&parse_stashes(&output)))
            }
            StashAction::Push { message } => {
                let mut args = vec!["stash".to_string(), "push".to_string()];
                if let Some(message) = message {
                    args.push("-m".to_string());
                    args.push(message.clone());
                }
                let output = self.text_owned(&args)?;
                let report = self.status_report()?;
                Ok(render::action(
                    "Stash",
                    "Changes stashed",
                    &output,
                    Some(&report),
                ))
            }
            StashAction::Pop { index } => {
                let output = self.stash_command("pop", index)?;
                let report = self.status_report()?;
                Ok(render::action(
                    "Stash",
                    "Stash popped",
                    &output,
                    Some(&report),
                ))
            }
            StashAction::Apply { index } => {
                let output = self.stash_command("apply", index)?;
                let report = self.status_report()?;
                Ok(render::action(
                    "Stash",
                    "Stash applied",
                    &output,
                    Some(&report),
                ))
            }
            StashAction::Drop { index } => {
                let output = self.stash_command("drop", index)?;
                let report = self.status_report()?;
                Ok(render::action(
                    "Stash",
                    "Stash dropped",
                    &output,
                    Some(&report),
                ))
            }
        }
    }

    fn stash_command(&mut self, command: &str, index: &Option<String>) -> Result<String, String> {
        let mut args = vec!["stash".to_string(), command.to_string()];
        if let Some(index) = index {
            args.push(index.clone());
        }
        self.text_owned(&args)
    }

    /// Resolve and cache the repository root, `Err` when not a repository.
    pub fn root(&mut self) -> Result<String, String> {
        self.repo_root()
    }

    /// Parsed `git status --porcelain=v2` report including stash count and
    /// the last commit.
    pub fn status(&mut self) -> Result<StatusReport, String> {
        self.status_report()
    }

    /// Commits with parents and ref decorations for the visual graph.
    pub fn log_nodes(&mut self, count: usize, all: bool) -> Result<Vec<CommitNode>, String> {
        if !self.has_head()? {
            return Ok(Vec::new());
        }
        let pretty = format!("--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1f%P%x1f%d%x1e");
        let mut args = vec![
            "log".to_string(),
            "--no-color".to_string(),
            pretty,
            "-n".to_string(),
            count.to_string(),
        ];
        if all {
            args.push("--all".to_string());
        }
        Ok(parse_log_nodes(&self.text_owned(&args)?))
    }

    /// Local and remote branches.
    pub fn list_branches(
        &mut self,
    ) -> Result<(Vec<super::model::Branch>, Vec<super::model::Branch>), String> {
        Ok((self.branches(false)?, self.branches(true)?))
    }

    /// All stash entries.
    pub fn stash_list(&mut self) -> Result<Vec<StashEntry>, String> {
        let output = self.text(&["stash", "list", "--format=%gd%x1f%gs%x1f%cr"])?;
        Ok(parse_stashes(&output))
    }

    /// Unified diff text for the given mode and paths.
    pub fn diff_text(&mut self, mode: &DiffMode, paths: &[String]) -> Result<String, String> {
        self.diff(mode, paths)
    }

    /// Commit staged changes and return the raw git output.
    pub fn commit(&mut self, message: &str, amend: bool) -> Result<String, String> {
        let mut args = vec!["commit".to_string(), "-m".to_string(), message.to_string()];
        if amend {
            args.push("--amend".to_string());
        }
        self.text_owned(&args)
    }

    /// Create and switch to a new branch.
    pub fn create_branch(&mut self, name: &str) -> Result<String, String> {
        self.text(&["switch", "-c", name])
    }

    /// Switch branches.
    pub fn switch(&mut self, branch: &str) -> Result<String, String> {
        self.text(&["switch", branch])
    }

    /// Check out a commit directly, leaving HEAD detached.
    pub fn checkout_commit(&mut self, rev: &str) -> Result<String, String> {
        self.text(&["switch", "--detach", rev])
    }

    /// Stash the current changes, optionally with a message.
    pub fn stash_push(&mut self, message: Option<&str>) -> Result<String, String> {
        let mut args = vec!["stash".to_string(), "push".to_string()];
        if let Some(text) = message {
            args.push("-m".to_string());
            args.push(text.to_string());
        }
        self.text_owned(&args)
    }

    /// Stash pop/apply/drop with an optional selector.
    pub fn stash_apply(&mut self, command: &str, index: Option<&str>) -> Result<String, String> {
        self.stash_command(command, &index.map(str::to_string))
    }

    /// Commit detail for a revision (`git show` patch plus stats).
    ///
    /// `--diff-merges=first-parent` makes merge commits show the patch
    /// against their first parent; without it `git show` suppresses the
    /// patch for merges and returns only the metadata and stat block.
    pub fn show_text(&mut self, rev: &str) -> Result<String, String> {
        self.text(&["show", "--no-color", "--stat", "--patch", "--diff-merges=first-parent", rev])
    }

    /// Annotated blame for a file.
    pub fn blame_text(&mut self, path: &str) -> Result<String, String> {
        self.text(&["blame", "--date=short", "-w", "--", path])
    }

    /// Fetch remotes.
    pub fn fetch(&mut self, all: bool) -> Result<String, String> {
        if all {
            self.text_owned(&["fetch".to_string(), "--all".to_string()])
        } else {
            self.text(&["fetch"])
        }
    }

    /// Pull the current branch.
    pub fn pull(&mut self) -> Result<String, String> {
        self.text(&["pull"])
    }

    /// Push the current branch. Untracked upstreams are pushed with `-u`.
    pub fn push(&mut self) -> Result<String, String> {
        self.text_owned(&["push".to_string(), "-u".to_string(), "HEAD".to_string()])
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

fn clean_error(output: &CmdOutput) -> String {
    let stderr = output.stderr_lossy();
    let detail = if stderr.trim().is_empty() {
        output.stdout_lossy()
    } else {
        stderr
    };
    detail.trim().to_string()
}

fn git_failure(args: &[String], output: &CmdOutput) -> String {
    let detail = clean_error(output);
    if detail.is_empty() {
        match output.status {
            Some(status) => format!("git {} failed with exit code {status}", args.join(" ")),
            None => format!("git {} was terminated by a signal", args.join(" ")),
        }
    } else {
        format!("git {} failed: {detail}", args.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct MockExecutor {
        calls: Vec<Vec<String>>,
        stdin_log: Vec<Vec<u8>>,
        responses: VecDeque<Result<CmdOutput, String>>,
    }

    impl MockExecutor {
        fn with(mut self, status: i32, stdout: &str) -> Self {
            self.responses.push_back(Ok(CmdOutput {
                status: Some(status),
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            }));
            self
        }

        fn with_error(mut self, status: i32, stderr: &str) -> Self {
            self.responses.push_back(Ok(CmdOutput {
                status: Some(status),
                stdout: Vec::new(),
                stderr: stderr.as_bytes().to_vec(),
            }));
            self
        }
    }

    impl GitExecutor for MockExecutor {
        fn exec(&mut self, _root: &str, args: &[String]) -> Result<CmdOutput, String> {
            self.calls.push(args.to_vec());
            self.responses.pop_front().unwrap_or_else(|| {
                Ok(CmdOutput {
                    status: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                })
            })
        }

        fn exec_stdin(
            &mut self,
            _root: &str,
            args: &[String],
            stdin: &[u8],
        ) -> Result<CmdOutput, String> {
            self.stdin_log.push(stdin.to_vec());
            self.exec(_root, args)
        }
    }

    fn client_with(mock: MockExecutor) -> GitClient<MockExecutor> {
        GitClient::new(mock, "C:/project")
    }

    #[test]
    fn resolves_repository_root_once() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "# branch.head main\0")
                .with(0, "")
                .with_error(128, "no commits"),
        );
        // The second status call should reuse the cached root.
        client.status_report().unwrap();
        client.status_report().unwrap();
        assert_eq!(
            client.executor.calls[0],
            vec!["rev-parse", "--show-toplevel"]
        );
        assert_eq!(
            client
                .executor
                .calls
                .iter()
                .filter(|call| call[0] == "rev-parse")
                .count(),
            1
        );
    }

    #[test]
    fn reports_non_repository_roots() {
        let mut client =
            client_with(MockExecutor::default().with_error(128, "fatal: not a git repository"));
        let error = client.status_report().unwrap_err();
        assert!(error.contains("Not a Git repository"));
        assert!(error.contains("not a git repository"));
    }

    #[test]
    fn stage_all_and_named_paths_use_different_add_forms() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "")
                .with(0, "# branch.head main\0")
                .with(0, "")
                .with_error(128, "no commits"),
        );
        client.stage(&[]).unwrap();
        let add = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "add")
            .unwrap();
        assert_eq!(add, &vec!["add", "--all"]);

        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "")
                .with(0, ""),
        );
        client
            .stage(&["src/main.rs".to_string(), "README.md".to_string()])
            .unwrap();
        let add = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "add")
            .unwrap();
        assert_eq!(add, &vec!["add", "--", "src/main.rs", "README.md"]);
    }

    #[test]
    fn unstage_uses_restore_with_head_and_rm_without_head() {
        let mut with_head = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "abc\n")
                .with(0, ""),
        );
        with_head.unstage(&[]).unwrap();
        let restore = with_head
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "restore")
            .unwrap();
        assert_eq!(restore, &vec!["restore", "--staged", "--", "."]);

        let mut without_head = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(1, "")
                .with(0, ""),
        );
        without_head.unstage(&["new.rs".to_string()]).unwrap();
        let rm = without_head
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "rm")
            .unwrap();
        assert_eq!(rm, &vec!["rm", "--cached", "-r", "--", "new.rs"]);
    }

    #[test]
    fn rollback_worktree_uses_plain_restore() {
        let mut client = client_with(MockExecutor::default().with(0, "C:/project\n").with(0, ""));
        client
            .rollback_files(&["src/main.rs".to_string()], RollbackLevel::Unstaged)
            .unwrap();
        let restore = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "restore")
            .unwrap();
        assert_eq!(restore, &vec!["restore", "--", "src/main.rs"]);
    }

    #[test]
    fn rollback_untracked_cleans_the_paths() {
        let mut client = client_with(MockExecutor::default().with(0, "C:/project\n").with(0, ""));
        client
            .rollback_files(&["notes.md".to_string()], RollbackLevel::Untracked)
            .unwrap();
        let clean = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "clean")
            .unwrap();
        assert_eq!(clean, &vec!["clean", "-f", "-d", "--", "notes.md"]);
    }

    #[test]
    fn rollback_staged_restores_tracked_and_removes_added_files() {
        // Status reports one staged modification (`a.txt`) and one staged
        // addition (`new.txt`).
        let status = "# branch.head main\u{0}\
                      1 M. N... 100644 100644 100644 aaaaaaa bbbbbbb a.txt\u{0}\
                      1 A. N... 000000 100644 100644 0000000 bbbbbbb new.txt\u{0}";
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, status)
                .with(0, "")
                .with_error(128, "no commits")
                .with(0, "")
                .with(0, ""),
        );
        client
            .rollback_files(
                &["a.txt".to_string(), "new.txt".to_string()],
                RollbackLevel::Staged,
            )
            .unwrap();
        let restore = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "restore")
            .unwrap();
        assert_eq!(
            restore,
            &vec![
                "restore",
                "--source=HEAD",
                "--staged",
                "--worktree",
                "--",
                "a.txt"
            ]
        );
        let rm = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "rm")
            .unwrap();
        assert_eq!(rm, &vec!["rm", "-f", "--", "new.txt"]);
    }

    #[test]
    fn rollback_staged_refuses_conflicted_paths() {
        let status = "# branch.head main\u{0}\
                      u UU N... 100644 100644 100644 100644 aaaaaaa bbbbbbb ccccccc conflict.rs\u{0}";
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, status)
                .with(0, "")
                .with_error(128, "no commits"),
        );
        let error = client
            .rollback_files(&["conflict.rs".to_string()], RollbackLevel::Staged)
            .unwrap_err();
        assert!(error.contains("resolve the conflict"), "{error}");
    }

    #[test]
    fn rollback_hunk_reverse_applies_the_patch() {
        let mut client = client_with(MockExecutor::default().with(0, "C:/project\n").with(0, ""));
        client.rollback_hunk("patch body\u{a}", true).unwrap();
        let call = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "apply")
            .unwrap();
        assert_eq!(
            call,
            &vec!["apply", "-R", "--whitespace=nowarn", "--cached"]
        );
        assert_eq!(client.executor.stdin_log[0], b"patch body\n");

        client.rollback_hunk("patch body\u{a}", false).unwrap();
        let call = client
            .executor
            .calls
            .iter()
            .rfind(|call| call[0] == "apply")
            .unwrap();
        assert_eq!(call, &vec!["apply", "-R", "--whitespace=nowarn"]);
    }

    #[test]
    fn commit_amend_adds_flag() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "[main abc] Better\n")
                .with(0, "# branch.head main\0")
                .with(0, "")
                .with_error(128, "no commits"),
        );
        client
            .run(&GitRequest::Commit {
                message: "Better".to_string(),
                amend: true,
            })
            .unwrap();
        let commit = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "commit")
            .unwrap();
        assert_eq!(commit, &vec!["commit", "-m", "Better", "--amend"]);
    }

    #[test]
    fn stash_pop_accepts_selector() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "popped\n")
                .with(0, "# branch.head main\0")
                .with(0, "")
                .with_error(128, "no commits"),
        );
        client
            .run(&GitRequest::Stash {
                action: StashAction::Pop {
                    index: Some("stash@{2}".to_string()),
                },
            })
            .unwrap();
        let pop = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "stash" && call.get(1) == Some(&"pop".to_string()))
            .unwrap();
        assert_eq!(pop, &vec!["stash", "pop", "stash@{2}"]);
    }

    #[test]
    fn all_diff_falls_back_to_staged_plus_unstaged_on_unborn_branch() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(1, "")
                .with(0, "staged diff\n")
                .with(0, "unstaged diff\n"),
        );
        let diff = client.diff(&DiffMode::All, &[]).unwrap();
        assert_eq!(diff, "staged diff\nunstaged diff\n");
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["diff", "--no-color", "--find-renames", "--staged"]));
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["diff", "--no-color", "--find-renames"]));
    }

    #[test]
    fn failure_messages_include_command_and_stderr() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with_error(128, "fatal: invalid reference"),
        );
        let error = client.text(&["show", "nope"]).unwrap_err();
        assert!(error.contains("git show nope failed"));
        assert!(error.contains("fatal: invalid reference"));
    }

    #[test]
    fn fetch_and_push_forward_arguments() {
        let mut fetch = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "fetched\n"),
        );
        let rendered = fetch.run(&GitRequest::Fetch { all: true }).unwrap();
        assert!(rendered.text.contains("fetched"));
        assert!(fetch
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["fetch", "--all"]));

        let mut push = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "pushed\n"),
        );
        let rendered = push
            .run(&GitRequest::Push {
                args: vec!["origin".to_string(), "main".to_string()],
            })
            .unwrap();
        assert!(rendered.text.contains("pushed"));
        assert!(push
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "origin", "main"]));
    }

    #[test]
    fn dashboard_renders_from_canned_git_output() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(
                    0,
                    "# branch.oid abc\u{0}# branch.head main\u{0}# branch.upstream origin/main\u{0}# branch.ab +1 -0\u{0}1 .M N... 100644 100644 100644 aaaaaaa bbbbbbb src/main.rs\u{0}? notes.md\u{0}",
                )
                .with(0, "stash@{0}\n")
                .with(0, "abc\u{1f}abc\u{1f}Alice\u{1f}2 hours ago\u{1f}Initial")
                .with(0, "* abc (HEAD -> main) Initial (Alice, 2 hours ago)\n"),
        );
        let rendered = client.run(&GitRequest::Dashboard).unwrap();
        assert!(rendered.text.contains("2 changes"));
        assert!(rendered.text.contains("src/main.rs"));
        assert!(rendered.text.contains("notes.md"));
        assert!(rendered
            .sections
            .iter()
            .any(|section| section.label == "Recent Commits"));
        assert!(rendered.text.contains("* abc"));
    }
}
