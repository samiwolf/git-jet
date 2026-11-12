//! Git command orchestration for Git Jet.

use std::time::Instant;

use super::log::{parse_branches, parse_log_nodes, parse_log_records, parse_raw_entries, parse_stashes, parse_tags};
use super::model::{
    CmdOutput, Commit, CommitDetail, CommitNode, GitExecutor, LogFilter, RemoteEntry, Rendered,
    StashEntry, StatusReport, TagEntry,
};
use super::oplog;
use super::render;
use super::request::{DiffMode, GitRequest, RollbackLevel, StashAction};
use super::status::parse_status_v2;

/// Fields: `name`, `HEAD`, `upstream:short`, `upstream:track`,
/// `objectname:short`, `committerdate:relative`, `symref`, `refname`.
/// The full refname (last field) tells local (`refs/heads/…`) from remote
/// (`refs/remotes/…`) branches when both are listed in one call.
const BRANCH_FORMAT: &str = "%(refname:short)\u{1f}%(HEAD)\u{1f}%(upstream:short)\u{1f}%(upstream:track)\u{1f}%(objectname:short)\u{1f}%(committerdate:relative)\u{1f}%(symref)\u{1f}%(refname)";
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

        let args = ["rev-parse".to_string(), "--show-toplevel".to_string()];
        let started = Instant::now();
        let output = self.executor.exec(&self.worktree_root, &args);
        oplog::record(&args, &output, started.elapsed(), None);
        let output = output?;
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
        let started = Instant::now();
        let output = self.executor.exec(&root, args);
        oplog::record(args, &output, started.elapsed(), None);
        output
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
        let started = Instant::now();
        let output = self.executor.exec_stdin(&root, args, stdin);
        oplog::record(args, &output, started.elapsed(), Some(stdin.len()));
        output
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
        // Resolving-merge state is only relevant while files are unmerged,
        // so the extra `rev-parse` probes stay limited to that case.
        if report.conflicts().is_empty() {
            return Ok(report);
        }
        report.merge_in_progress = self.head_ref_exists("MERGE_HEAD");
        report.cherry_pick_in_progress = self.head_ref_exists("CHERRY_PICK_HEAD");
        report.revert_in_progress = self.head_ref_exists("REVERT_HEAD");
        Ok(report)
    }

    /// Quietly checks whether a special ref like `MERGE_HEAD` exists.
    fn head_ref_exists(&mut self, name: &str) -> bool {
        let output = self.exec(&["rev-parse", "-q", "--verify", name]);
        matches!(output, Ok(output) if output.success())
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

    /// Local and remote branches in a single `for-each-ref` invocation
    /// (previously two `git branch` processes).
    fn branch_lists(&mut self) -> Result<(Vec<super::model::Branch>, Vec<super::model::Branch>), String> {
        let output = self.text_owned(&[
            "for-each-ref".to_string(),
            "refs/heads".to_string(),
            "refs/remotes".to_string(),
            format!("--format={BRANCH_FORMAT}"),
        ])?;
        let branches = parse_branches(&output);
        let local = branches.iter().filter(|b| !b.remote).cloned().collect();
        let remote = branches.into_iter().filter(|b| b.remote).collect();
        Ok((local, remote))
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
                let log = match self.text_owned(&args) {
                    Ok(output) => output,
                    Err(error) if unborn_head_error(&error) => String::new(),
                    Err(error) => return Err(error),
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
                let (local, remote) = self.branch_lists()?;
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
        if *mode == DiffMode::All {
            return match self.diff_inner(mode.clone(), paths) {
                Ok(text) => Ok(text),
                // Unborn HEAD: `git diff HEAD` is invalid, fall back to the
                // staged + unstaged diffs (fresh repository before commit #1).
                Err(error) if unborn_head_error(&error) => {
                    let staged = self.diff_inner(DiffMode::Staged, paths)?;
                    let unstaged = self.diff_inner(DiffMode::Unstaged, paths)?;
                    Ok(format!("{staged}{unstaged}"))
                }
                Err(error) => Err(error),
            };
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

    /// Resolves a repo-relative file path for working-tree access and
    /// rejects absolute paths and `..` escapes.
    fn worktree_path(&mut self, path: &str) -> Result<std::path::PathBuf, String> {
        let path = path.trim();
        if path.is_empty() {
            return Err("A file path is required.".to_string());
        }
        if path.starts_with('-') {
            return Err(format!("Invalid file path `{path}`."));
        }
        let relative = std::path::Path::new(path);
        if relative.is_absolute() {
            return Err(format!("Invalid file path `{path}`."));
        }
        for component in relative.components() {
            match component {
                std::path::Component::Prefix(_)
                | std::path::Component::RootDir
                | std::path::Component::ParentDir => {
                    return Err(format!("Invalid file path `{path}`."));
                }
                _ => {}
            }
        }
        let root = self.repo_root()?;
        Ok(std::path::Path::new(&root).join(relative))
    }

    /// Reads one file's working-tree content, relative to the repo root.
    ///
    /// Used by the merge editor to inspect a conflicted file together with
    /// its `<<<<<<<`/`=======`/`>>>>>>>` markers.
    pub fn worktree_file_content(&mut self, path: &str) -> Result<String, String> {
        let path = path.trim();
        let target = self.worktree_path(path)?;
        std::fs::read_to_string(&target)
            .map_err(|error| format!("Could not read `{path}`: {error}"))
    }

    /// Writes a resolved merge result to the working tree and stages the
    /// file (`git add`), which marks the conflict as resolved.
    pub fn write_merge_result(&mut self, path: &str, content: &str) -> Result<String, String> {
        let path = path.trim();
        let target = self.worktree_path(path)?;
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("Could not create `{path}`: {error}"))?;
        }
        std::fs::write(&target, content)
            .map_err(|error| format!("Could not write `{path}`: {error}"))?;
        self.stage(&[path.to_string()])
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
    ///
    /// `skip` pages through history (`--skip`) so "load more" can fetch just
    /// the next page instead of the whole prefix.
    ///
    /// Optional filters narrow the set via git itself (`--grep`, `--author`,
    /// path restriction) or switch the base from the checkout to another
    /// branch.
    pub fn log_nodes(
        &mut self,
        count: usize,
        skip: usize,
        all: bool,
        filter: &LogFilter,
    ) -> Result<Vec<CommitNode>, String> {
        let pretty = format!("--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1f%P%x1f%d%x1e");
        let mut args = vec![
            "log".to_string(),
            "--no-color".to_string(),
            pretty,
            "-n".to_string(),
            count.to_string(),
        ];
        if skip > 0 {
            args.push(format!("--skip={skip}"));
        }
        match &filter.branch {
            Some(branch) => args.push(branch.clone()),
            None => {
                if all {
                    args.push("--all".to_string());
                }
            }
        }
        if let Some(text) = &filter.text {
            args.push("--regexp-ignore-case".to_string());
            args.push(format!("--grep={text}"));
        }
        if let Some(author) = &filter.author {
            args.push("--regexp-ignore-case".to_string());
            args.push(format!("--author={author}"));
        }
        if let Some(path) = &filter.path {
            args.push("--".to_string());
            args.push(path.clone());
        }
        match self.text_owned(&args) {
            Ok(output) => Ok(parse_log_nodes(&output)),
            Err(error) if unborn_head_error(&error) => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }

    /// Local and remote branches.
    pub fn list_branches(
        &mut self,
    ) -> Result<(Vec<super::model::Branch>, Vec<super::model::Branch>), String> {
        self.branch_lists()
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

    /// Pull with dialog options: optional remote/branch and fast-forward-only.
    ///
    /// Only merge and fast-forward strategies are offered — this tool window
    /// deliberately has no rebase support.
    pub fn pull_with_options(
        &mut self,
        remote: Option<&str>,
        branch: Option<&str>,
        ff_only: bool,
    ) -> Result<String, String> {
        let mut args = vec!["pull".to_string()];
        if ff_only {
            args.push("--ff-only".to_string());
        }
        if let Some(remote) = remote.map(str::trim).filter(|value| !value.is_empty()) {
            args.push(remote.to_string());
        }
        if let Some(branch) = branch.map(str::trim).filter(|value| !value.is_empty()) {
            args.push(branch.to_string());
        }
        self.text_owned(&args)
    }

    /// Push the current branch. With a tracked upstream a plain `git push`
    /// updates the upstream and keeps the tracking. Without one the branch
    /// is pushed to the default remote by name, which creates the tracking.
    pub fn push(&mut self) -> Result<String, String> {
        if self.head_ref_exists("@{upstream}") {
            // A bare `git push HEAD` fails: git reads the lone `HEAD` as a
            // repository name, not a refspec. With a tracked upstream a
            // plain push is the equivalent and keeps the tracking.
            return self.text(&["push"]);
        }
        // Fresh clones have no `branch.<name>.remote` config yet, where a
        // plain push would guess. Push by name to the default remote.
        self.push_current_branch_to_default(true, false)
    }

    /// Push the current branch to the default remote (usually origin) under
    /// its own name, e.g. `git push -u origin main`.
    ///
    /// A bare `git push HEAD` cannot work because git reads the lone `HEAD`
    /// as a repository name, so the branch must be named explicitly.
    fn push_current_branch_to_default(
        &mut self,
        set_upstream: bool,
        force: bool,
    ) -> Result<String, String> {
        let remote = match self.default_remote() {
            Some(remote) => remote,
            None => {
                return Err(
                    "Push needs either a tracked upstream or a configured remote."
                        .to_string(),
                )
            }
        };
        let branch = match self.current_branch() {
            Some(branch) => branch,
            None => {
                return Err(
                    "Push needs a branch; commit inside a branch first (detached HEAD)."
                        .to_string(),
                )
            }
        };
        let mut args = vec!["push".to_string()];
        if set_upstream {
            args.push("-u".to_string());
        }
        if force {
            args.push("--force-with-lease".to_string());
        }
        args.push(remote);
        args.push(format!("{branch}:{branch}"));
        self.text_owned(&args)
    }

    /// First configured remote name, e.g. `origin`.
    fn default_remote(&mut self) -> Option<String> {
        let output = self.text(&["remote"]).ok()?;
        output
            .lines()
            .map(|line| line.trim())
            .find(|line| !line.is_empty())
            .map(|line| line.to_string())
    }

    /// Current branch name; `None` while detached.
    fn current_branch(&mut self) -> Option<String> {
        let output = self.exec(&["branch", "--show-current"]).ok()?;
        if !output.success() {
            return None;
        }
        let name = output.stdout_lossy().trim().to_string();
        if name.is_empty() {
            None
        } else {
            Some(name)
        }
    }

    /// Push with dialog options: optional remote and branch, upstream
    /// tracking and force push.
    ///
    /// Empty fields fall back like `pull`: no remote pushes the current
    /// branch to the default remote; a remote without a branch pushes the
    /// current branch under its own name.
    ///
    /// Force pushes use `--force-with-lease` so remote commits fetched after
    /// the checkout cannot be silently discarded.
    pub fn push_with_options(
        &mut self,
        remote: Option<&str>,
        branch: Option<&str>,
        force: bool,
        set_upstream: bool,
    ) -> Result<String, String> {
        let remote = remote.map(str::trim).filter(|value| !value.is_empty());
        let branch = branch.map(str::trim).filter(|value| !value.is_empty());
        let mut args = vec!["push".to_string()];
        if set_upstream {
            args.push("-u".to_string());
        }
        if force {
            args.push("--force-with-lease".to_string());
        }
        match (remote, branch) {
            (Some(remote), Some(branch)) => {
                if branch.starts_with('-') {
                    return Err(format!("Invalid branch name `{branch}`."));
                }
                args.push(remote.to_string());
                args.push(format!("{branch}:{branch}"));
            }
            (Some(remote), None) => {
                // Remote without an explicit branch: push the current branch
                // under its own name, like the toolbar Push button does.
                let current = match self.current_branch() {
                    Some(current) => current,
                    None => {
                        return Err(
                            "Push needs a branch; commit inside a branch first (detached HEAD)."
                                .to_string(),
                        )
                    }
                };
                args.push(remote.to_string());
                args.push(format!("{current}:{current}"));
            }
            (None, _) => {
                // No remote given: behave like the toolbar Push button — the
                // current branch to the default remote. A bare
                // `git push HEAD` cannot work because git reads the lone
                // `HEAD` as a repository name.
                let remote = match self.default_remote() {
                    Some(remote) => remote,
                    None => {
                        return Err(
                            "Push needs either a tracked upstream or a configured remote."
                                .to_string(),
                        )
                    }
                };
                let current = match branch {
                    Some(branch) => branch.to_string(),
                    None => match self.current_branch() {
                        Some(current) => current,
                        None => {
                            return Err(
                                "Push needs a branch; commit inside a branch first (detached HEAD)."
                                    .to_string(),
                            )
                        }
                    },
                };
                if current.starts_with('-') {
                    return Err(format!("Invalid branch name `{current}`."));
                }
                args.push(remote);
                args.push(format!("{current}:{current}"));
            }
        }
        self.text_owned(&args)
    }

    /// Commits that would be sent by Push: everything ahead of the tracking
    /// upstream, or the whole current history when the branch has none.
    ///
    /// Returns `(commits, has_upstream)`.
    pub fn outgoing_commits(&mut self, limit: usize) -> Result<(Vec<Commit>, bool), String> {
        if !self.has_head()? {
            return Ok((Vec::new(), false));
        }
        let base = if self.head_ref_exists("@{upstream}") {
            "@{upstream}".to_string()
        } else {
            String::new()
        };
        if base.is_empty() {
            let commits = self.log_first_level(limit, "HEAD")?;
            return Ok((commits, false));
        }
        let range = format!("{base}..HEAD");
        let commits = self.log_first_level(limit, &range)?;
        Ok((commits, true))
    }

    /// Simple (non-graph) log listing, first parent, newest first.
    fn log_first_level(&mut self, limit: usize, range: &str) -> Result<Vec<Commit>, String> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let output = self.text(&[
            "log",
            "--no-color",
            "--first-parent",
            format!("-n{}", limit).as_str(),
            format!("--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1e").as_str(),
            range,
        ])?;
        Ok(parse_log_records(&output))
    }

    /// Merge a branch into the current branch, optionally with a merge commit
    /// even when a fast-forward would be possible.
    pub fn merge_branch(&mut self, target: &str, no_ff: bool) -> Result<String, String> {
        let target = target.trim();
        if target.is_empty() {
            return Err("A branch name is required.".to_string());
        }
        if target.starts_with('-') {
            return Err(format!("Invalid merge target `{target}`."));
        }
        let mut args = vec!["merge".to_string()];
        if no_ff {
            args.push("--no-ff".to_string());
        }
        args.push(target.to_string());
        self.text_owned(&args)
    }

    /// Abort the merge in progress and restore the pre-merge state.
    pub fn merge_abort(&mut self) -> Result<String, String> {
        self.text(&["merge", "--abort"])
    }

    /// Delete a local branch. With `force`, uncommitted-to-upstream branches
    /// are deleted too (`-D` instead of `-d`).
    pub fn delete_branch(&mut self, name: &str, force: bool) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A branch name is required.".to_string());
        }
        if name.starts_with('-') {
            return Err(format!("Invalid branch name `{name}`."));
        }
        let flag = if force { "-D" } else { "-d" };
        self.text(&["branch", flag, name])
    }

    /// Delete a branch on its remote (`push origin --delete <name>`).
    pub fn delete_remote_branch(&mut self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A remote branch name is required.".to_string());
        }
        if name.starts_with('-') {
            return Err(format!("Invalid branch name `{name}`."));
        }
        let (remote, branch) = name
            .split_once('/')
            .ok_or_else(|| format!("`{name}` does not contain a remote (like origin/{name})."))?;
        if remote.is_empty() || branch.is_empty() {
            return Err(format!("`{name}` does not contain a remote (like origin/{name})."));
        }
        let branch = branch.trim_start_matches('/');
        self.text(&["push", remote, "--delete", branch])
    }

    /// Rename a local branch (`git branch -m old new`).
    pub fn rename_branch(&mut self, old_name: &str, new_name: &str) -> Result<String, String> {
        let old_name = old_name.trim();
        let new_name = new_name.trim();
        if old_name.is_empty() || new_name.is_empty() {
            return Err("Old and new branch names are required.".to_string());
        }
        if old_name.starts_with('-') {
            return Err(format!("Invalid branch name `{old_name}`."));
        }
        if new_name.starts_with('-') {
            return Err(format!("Invalid branch name `{new_name}`."));
        }
        let result = self.text(&["branch", "-m", old_name, new_name]);
        if result.is_ok() {
            return Ok(format!("Branch {old_name} renamed to {new_name}"));
        }
        result
    }

    /// Create a new branch and check it out, starting from `start_ref`
    /// (a commit hash, another branch or `origin/…`).
    pub fn checkout_new_branch_from(&mut self, name: &str, start_ref: &str) -> Result<String, String> {
        let name = name.trim();
        let start_ref = start_ref.trim();
        if name.is_empty() || start_ref.is_empty() {
            return Err("A branch name and a start ref are required.".to_string());
        }
        if name.starts_with('-') {
            return Err(format!("Invalid branch name `{name}`."));
        }
        if start_ref.starts_with('-') {
            return Err(format!("Invalid start ref `{start_ref}`."));
        }
        self.text(&["switch", "-c", name, start_ref])
    }

    /// Point a local branch's (or the current branch's) tracking at `upstream`.
    pub fn set_upstream(&mut self, branch: Option<&str>, upstream: &str) -> Result<String, String> {
        let upstream = upstream.trim();
        if upstream.is_empty() {
            return Err("An upstream branch is required, for example origin/main.".to_string());
        }
        if upstream.starts_with('-') || upstream.contains(' ') {
            return Err(format!("Invalid upstream branch `{upstream}`."));
        }
        let mut args = vec!["branch".to_string(), format!("--set-upstream-to={upstream}")];
        if let Some(branch) = branch.map(str::trim).filter(|value| !value.is_empty()) {
            if branch.starts_with('-') {
                return Err(format!("Invalid branch name `{branch}`."));
            }
            args.push(branch.to_string());
        }
        self.text_owned(&args)
    }

    /// Stop a local branch (or the current branch) from tracking anything.
    pub fn unset_upstream(&mut self, branch: Option<&str>) -> Result<String, String> {
        let mut args = vec!["branch".to_string(), "--unset-upstream".to_string()];
        if let Some(branch) = branch.map(str::trim).filter(|value| !value.is_empty()) {
            if branch.starts_with('-') {
                return Err(format!("Invalid branch name `{branch}`."));
            }
            args.push(branch.to_string());
        }
        self.text_owned(&args)
    }

    /// Two-way comparison with another branch: commits that would be lost
    /// (`branch..HEAD`) and commits that would be pulled (`HEAD..branch`).
    ///
    /// Returns `(ours, theirs)` with `ours` newest first.
    pub fn branch_compare(
        &mut self,
        branch: &str,
    ) -> Result<(Vec<Commit>, Vec<Commit>), String> {
        let branch = branch.trim();
        if branch.is_empty() {
            return Err("A branch name is required.".to_string());
        }
        if branch.starts_with('-') {
            return Err(format!("Invalid branch name `{branch}`."));
        }
        let ours = self.log_first_level(200, &format!("{branch}..HEAD"))?;
        let theirs = self.log_first_level(200, &format!("HEAD..{branch}"))?;
        Ok((ours, theirs))
    }

    /// All local tags, newest creation date first.
    pub fn list_tags(&mut self) -> Result<Vec<TagEntry>, String> {
        let output = self.text(&[
            "for-each-ref",
            "--sort=-creatordate",
            format!("--format=%(refname:short){}%(objectname:short){}%(creatordate:relative)",
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR).as_str(),
            "refs/tags",
        ])?;
        Ok(parse_tags(&output))
    }

    /// Create a tag at the current HEAD or an explicit ref.
    pub fn create_tag(&mut self, name: &str, rev: Option<&str>) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("A tag name is required.".to_string());
        }
        if name.starts_with('-') || name.contains(' ') {
            return Err(format!("Invalid tag name `{name}`."));
        }
        if let Some(rev) = rev.map(str::trim).filter(|value| !value.is_empty()) {
            if rev.starts_with('-') {
                return Err(format!("Invalid revision `{rev}`."));
            }
            self.text(&["tag", name, rev])
        } else {
            self.text(&["tag", name])
        }
    }

    /// Delete a local tag.
    pub fn delete_tag(&mut self, name: &str) -> Result<String, String> {
        let name = name.trim();
        if name.is_empty() || name.starts_with('-') {
            return Err(format!("Invalid tag name `{name}`."));
        }
        self.text(&["tag", "-d", name])
    }

    /// Push all local tags to the default remote.
    pub fn push_tags(&mut self) -> Result<String, String> {
        self.text(&["push", "--tags"])
    }

    /// All configured remotes with their fetch and push URLs (`git remote -v`).
    pub fn list_remotes(&mut self) -> Result<Vec<RemoteEntry>, String> {
        let output = self.text(&["remote", "--verbose"])?;
        Ok(parse_remotes(&output))
    }

    /// Register a new remote (`git remote add <name> <url>`).
    pub fn add_remote(&mut self, name: &str, url: &str) -> Result<String, String> {
        let (name, url) = validate_remote(name, url)?;
        self.text(&["remote", "add", name.as_str(), url.as_str()])
    }

    /// Replace a remote's fetch and push URL (`git remote set-url`).
    pub fn set_remote_url(&mut self, name: &str, url: &str) -> Result<String, String> {
        let (name, url) = validate_remote(name, url)?;
        self.text(&["remote", "set-url", name.as_str(), url.as_str()])
    }

    /// Unregister a remote (`git remote remove <name>`). Remote-tracking
    /// branches of that remote are removed by git as well.
    pub fn remove_remote(&mut self, name: &str) -> Result<String, String> {
        let (name, _) = validate_remote(name, "url")?;
        self.text(&["remote", "remove", name.as_str()])
    }

    /// Fetch one specific remote (`git fetch <name>`).
    pub fn fetch_remote(&mut self, name: &str) -> Result<String, String> {
        let (name, _) = validate_remote(name, "url")?;
        self.text(&["fetch", name.as_str()])
    }

    /// Commits the upstream has that the current branch does not:
    /// `HEAD..@{upstream}`. Returns `(commits, has_upstream)`.
    pub fn incoming_commits(&mut self, limit: usize) -> Result<(Vec<Commit>, bool), String> {
        if !self.has_head()? {
            return Ok((Vec::new(), false));
        }
        if !self.head_ref_exists("@{upstream}") {
            return Ok((Vec::new(), false));
        }
        let commits = self.log_first_level(limit, "HEAD..@{upstream}")?;
        Ok((commits, true))
    }

    /// Append ignore patterns to the repository `.gitignore` (created when
    /// missing). Patterns already present are skipped, so the call is
    /// idempotent.
    pub fn add_to_gitignore(&mut self, paths: &[String]) -> Result<String, String> {
        if paths.is_empty() {
            return Err("At least one path is required.".to_string());
        }
        let mut patterns = Vec::with_capacity(paths.len());
        for value in paths {
            let value = value.trim().replace('\\', "/");
            if value.is_empty() || value.starts_with('-') {
                return Err(format!("Invalid path `{value}`."));
            }
            patterns.push(escape_gitignore_pattern(&value));
        }
        let gitignore = self.worktree_path(".gitignore")?;
        let existing = std::fs::read_to_string(&gitignore).unwrap_or_default();
        let mut have: Vec<String> = existing.lines().map(|line| line.trim().to_string()).collect();
        let mut added = 0usize;
        let mut content = existing.clone();
        for pattern in patterns {
            if have.contains(&pattern) || have.contains(&format!("/{pattern}")) {
                continue;
            }
            have.push(pattern.clone());
            if !content.is_empty() && !content.ends_with('\n') {
                content.push('\n');
            }
            content.push_str(&pattern);
            content.push('\n');
            added += 1;
        }
        if added == 0 {
            return Ok(".gitignore already covers the selection.".to_string());
        }
        std::fs::write(&gitignore, content)
            .map_err(|error| format!("Could not write `.gitignore`: {error}"))?;
        Ok(format!("Added {added} line(s) to .gitignore"))
    }

    /// The full commit message (`%B`) of one commit, used to prefill
    /// "Edit commit message".
    pub fn commit_full_message(&mut self, rev: &str) -> Result<String, String> {
        let rev = self.safe_revision("show", rev)?;
        self.text_owned(&[
            "show".to_string(),
            "-s".to_string(),
            "--no-color".to_string(),
            "--format=%B".to_string(),
            rev,
        ])
    }

    /// The default message of "squash into parent": the parent's message
    /// followed by the commit's own. Also validates that the commit can be
    /// squashed at all (single parent, i.e. not a root or merge commit).
    pub fn squash_preview(&mut self, rev: &str) -> Result<String, String> {
        let sha = self.resolve_commit(rev)?;
        let (_, parents, _, message) = self.commit_raw(&sha)?;
        if parents.is_empty() {
            return Err(format!(
                "{} is a root commit — there is no parent to squash into.",
                short_sha(&sha)
            ));
        }
        if parents.len() > 1 {
            return Err(format!(
                "{} is a merge commit — squashing into a parent is ambiguous.",
                short_sha(&sha)
            ));
        }
        let (_, _, _, parent_message) = self.commit_raw(&parents[0])?;
        Ok(combined_message(&parent_message, &message))
    }

    /// Rewrite the current branch so `rev` carries `message`, keeping its
    /// tree, parents and author; all descendant commits are replayed on top.
    pub fn reword_commit(&mut self, rev: &str, message: &str) -> Result<String, String> {
        let message = normalize_message(message)?;
        let rev = self.safe_revision("reword", rev)?;
        let sha = self.resolve_commit(&rev)?;
        let short = short_sha(&sha);
        self.rewrite_history(&sha, HistoryEdit::Reword { message })?;
        Ok(format!("Reworded commit {short}"))
    }

    /// Fold `rev` into its parent: the parent is rebuilt with `rev`'s tree and
    /// a combined message, then all descendant commits are replayed on top.
    /// Without `message` the combined default is used.
    pub fn squash_commit_into_parent(
        &mut self,
        rev: &str,
        message: Option<&str>,
    ) -> Result<String, String> {
        let rev = self.safe_revision("squash", rev)?;
        let sha = self.resolve_commit(&rev)?;
        let short = short_sha(&sha);
        let default = self.squash_preview(&rev)?;
        let message = match message.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => value.to_string(),
            None => default,
        };
        let message = normalize_message(&message)?;
        self.rewrite_history(&sha, HistoryEdit::SquashIntoParent { message })?;
        Ok(format!("Squashed commit {short} into its parent"))
    }

    /// Resolves one revision to a full commit hash, rejecting anything that
    /// is not a commit.
    fn resolve_commit(&mut self, rev: &str) -> Result<String, String> {
        let rev = self.safe_revision("resolve", rev)?;
        let output = self.text(&["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")])?;
        let sha = output.trim().to_string();
        if sha.is_empty() {
            return Err(format!("`{rev}` is not a commit."));
        }
        Ok(sha)
    }

    /// Raw fields of one commit: `(tree, parents, author line, message)`.
    /// The author line is the raw object form `Name <email> unix-tz`, so it
    /// can be written back into a rebuilt commit object verbatim.
    fn commit_raw(&mut self, sha: &str) -> Result<(String, Vec<String>, String, String), String> {
        let output = self.text(&[
            "show",
            "-s",
            "--no-color",
            "--date=raw",
            "--format=%T%x1f%P%x1f%an <%ae> %ad%x1f%B",
            sha,
        ])?;
        let mut parts = output.splitn(4, '\u{1f}');
        let tree = parts.next().unwrap_or_default().trim().to_string();
        let parents = parts
            .next()
            .unwrap_or_default()
            .split_whitespace()
            .map(|value| value.to_string())
            .collect();
        let author = parts.next().unwrap_or_default().trim().to_string();
        let message = parts.next().unwrap_or_default().to_string();
        if tree.is_empty() || author.is_empty() {
            return Err(format!("Could not read the commit object of {sha}."));
        }
        Ok((tree, parents, author, message))
    }

    /// Writes a raw commit object into the object database and returns its
    /// id (`git hash-object -t commit -w --stdin`). Plumbing keeps the
    /// rewrite free of index/worktree requirements: the working tree never
    /// moves, so this also works with a dirty tree.
    fn write_commit_object(
        &mut self,
        tree: &str,
        parents: &[String],
        author_line: &str,
        committer_line: &str,
        message: &str,
    ) -> Result<String, String> {
        let mut object = String::new();
        object.push_str("tree ");
        object.push_str(tree);
        object.push('\n');
        for parent in parents {
            object.push_str("parent ");
            object.push_str(parent);
            object.push('\n');
        }
        object.push_str("author ");
        object.push_str(author_line);
        object.push('\n');
        object.push_str("committer ");
        object.push_str(committer_line);
        object.push('\n');
        object.push('\n');
        object.push_str(message.trim_end());
        object.push('\n');
        let args: Vec<String> = ["hash-object", "-t", "commit", "-w", "--stdin"]
            .iter()
            .map(|value| value.to_string())
            .collect();
        let output = self.exec_stdin_owned(&args, object.as_bytes())?;
        if !output.success() {
            return Err(git_failure(&args, &output));
        }
        let sha = output.stdout_lossy().trim().to_string();
        if sha.is_empty() {
            return Err("git hash-object returned no object id.".to_string());
        }
        Ok(sha)
    }

    /// The committer identity for rebuilt commits, as a raw object line.
    fn committer_ident(&mut self) -> Result<String, String> {
        let line = self.text(&["var", "GIT_COMMITTER_IDENT"])?.trim().to_string();
        if line.is_empty() {
            return Err(
                "Could not resolve the committer identity — set user.name and user.email."
                    .to_string(),
            );
        }
        Ok(line)
    }

    /// The checked-out branch name, `Err` on a detached HEAD.
    fn current_branch_name(&mut self) -> Result<String, String> {
        let output = self.exec(&["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        if !output.success() {
            return Err(
                "Detached HEAD — check out a branch before rewriting history.".to_string()
            );
        }
        Ok(output.stdout_lossy().trim().to_string())
    }

    /// Whether `rev` is an ancestor of (or equal to) `of`.
    fn is_ancestor(&mut self, rev: &str, of: &str) -> Result<bool, String> {
        let args: Vec<String> = ["merge-base", "--is-ancestor", rev, of]
            .iter()
            .map(|value| value.to_string())
            .collect();
        let output = self.exec_owned(&args)?;
        match output.status {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(git_failure(&args, &output)),
        }
    }

    /// One history rewrite step: replace `sha` per `edit`, then replay every
    /// descendant (`sha..HEAD`) with remapped parents and finally move the
    /// branch with a compare-and-swap `update-ref`.
    fn rewrite_history(&mut self, sha: &str, edit: HistoryEdit) -> Result<(), String> {
        if !self.has_head()? {
            return Err("The repository has no commits to rewrite.".to_string());
        }
        let branch = self.current_branch_name()?;
        if !self.is_ancestor(sha, "HEAD")? {
            return Err(format!(
                "{} is not on the current branch — only commits reachable from HEAD can be rewritten.",
                short_sha(sha)
            ));
        }
        let old_head = self
            .text(&["rev-parse", "--verify", "HEAD"])?
            .trim()
            .to_string();
        let committer = self.committer_ident()?;

        // Rebuild the target commit itself.
        let (tree, parents, author_line, _) = self.commit_raw(sha)?;
        let (new_parents, new_author, new_message) = match &edit {
            HistoryEdit::Reword { message } => (parents.clone(), author_line, message.clone()),
            HistoryEdit::SquashIntoParent { message } => {
                let parent = parents
                    .first()
                    .ok_or_else(|| format!("{} is a root commit.", short_sha(sha)))?;
                let (_, parent_parents, parent_author, _) = self.commit_raw(parent)?;
                (parent_parents, parent_author, message.clone())
            }
        };
        let new_sha = self.write_commit_object(
            &tree,
            &new_parents,
            &new_author,
            &committer,
            &new_message,
        )?;
        let mut map = std::collections::HashMap::new();
        map.insert(sha.to_string(), new_sha.clone());

        // Replay descendants in topological order, parents before children.
        let range = format!("{sha}..{old_head}");
        let descendants = self.text(&["rev-list", "--topo-order", "--reverse", range.as_str()])?;
        let mut tip = new_sha;
        for line in descendants.lines() {
            let child = line.trim();
            if child.is_empty() {
                continue;
            }
            let (child_tree, child_parents, child_author, child_message) =
                self.commit_raw(child)?;
            let mapped: Vec<String> = child_parents
                .iter()
                .map(|parent| map.get(parent).cloned().unwrap_or_else(|| parent.clone()))
                .collect();
            let new_child = self.write_commit_object(
                &child_tree,
                &mapped,
                &child_author,
                &committer,
                &child_message,
            )?;
            map.insert(child.to_string(), new_child.clone());
            tip = new_child;
        }

        // Safety net: the final tree must be identical, otherwise something
        // went deeply wrong and the branch is left untouched.
        let old_tree = self
            .text(&["rev-parse", "--verify", &format!("{old_head}^{{tree}}")])?
            .trim()
            .to_string();
        let new_tree = self
            .text(&["rev-parse", "--verify", &format!("{tip}^{{tree}}")])?
            .trim()
            .to_string();
        if old_tree != new_tree {
            return Err(
                "Internal error: the rewritten history changed the final tree; \
                 the branch was left untouched."
                    .to_string(),
            );
        }
        self.text(&[
            "update-ref",
            &format!("refs/heads/{branch}"),
            tip.as_str(),
            old_head.as_str(),
        ])?;
        Ok(())
    }

    /// Revert one non-merge commit as a new commit.
    pub fn revert_commit(&mut self, rev: &str) -> Result<String, String> {
        let rev = self.safe_revision("revert", rev)?;
        self.text(&["revert", "--no-edit", rev.as_str()])
    }

    /// Cherry-pick one non-merge commit onto the current branch.
    pub fn cherry_pick_commit(&mut self, rev: &str) -> Result<String, String> {
        let rev = self.safe_revision("cherry-pick", rev)?;
        self.text(&["cherry-pick", "--allow-empty", rev.as_str()])
    }

    /// Remove one non-merge commit from the current branch's history by
    /// replaying it out: `git rebase --onto <rev>^ <rev> HEAD`.
    pub fn drop_commit(&mut self, rev: &str) -> Result<String, String> {
        let rev = self.safe_revision("drop", rev)?;
        let parent = format!("{rev}^");
        self.text(&["rebase", "--onto", parent.as_str(), rev.as_str(), "HEAD"])
    }

    /// Undo the current HEAD commit: the commit disappears and its changes
    /// stay in the working tree as uncommitted edits (`git reset --mixed`).
    pub fn undo_head_commit(&mut self) -> Result<String, String> {
        if !self.has_head()? {
            return Err("The repository has no commits to undo.".to_string());
        }
        self.text(&["reset", "--mixed", "HEAD~1"])
    }

    /// Rejects empty or option-looking revisions before they reach git.
    fn safe_revision(&self, action: &str, rev: &str) -> Result<String, String> {
        let rev = rev.trim();
        if rev.is_empty() {
            return Err(format!("{action} requires a commit hash."));
        }
        if rev.starts_with('-') {
            return Err(format!("Invalid revision `{rev}`."));
        }
        Ok(rev.to_string())
    }

    /// Soft, mixed or hard reset of the current branch to a revision.
    pub fn reset_branch(&mut self, rev: &str, mode: &str) -> Result<String, String> {
        let mode = match mode {
            "soft" => "soft",
            "hard" => "hard",
            _ => "mixed",
        };
        let rev = self.safe_revision("reset", rev)?;
        self.text(&["reset", format!("--{mode}").as_str(), rev.as_str()])
    }

    /// Full metadata for one commit plus its changed files.
    pub fn commit_detail(&mut self, rev: &str) -> Result<CommitDetail, String> {
        let rev = self.safe_revision("show", rev)?;
        let output = self.text_owned(&[
            "show".to_string(),
            "--raw".to_string(),
            "-z".to_string(),
            "--no-color".to_string(),
            "--find-renames".to_string(),
            "--diff-merges=first-parent".to_string(),
            format!("--format=%H{}%h{}%an{}%ae{}%ar{}%ai{}%s{}%P{}%d{}",
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR,
                crate::core::log::LOG_FIELD_SEPARATOR).to_string(),
            rev.clone(),
        ])?;
        parse_commit_detail(&output)
    }

    /// The patch of one file inside a commit, with the metadata suppressed
    /// (`git show --format=`) so the frontend only sees diff text.
    pub fn commit_file_diff(&mut self, rev: &str, path: &str) -> Result<String, String> {
        let rev = self.safe_revision("show", rev)?;
        if path.trim().is_empty() {
            return Err("A file path is required.".to_string());
        }
        self.text_owned(&[
            "show".to_string(),
            "--no-color".to_string(),
            "--format=".to_string(),
            "--patch".to_string(),
            "--diff-merges=first-parent".to_string(),
            rev,
            "--".to_string(),
            path.trim().to_string(),
        ])
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// What to do with the target commit of a history rewrite.
#[derive(Debug, Clone)]
enum HistoryEdit {
    /// Keep tree, parents and author; replace the message.
    Reword { message: String },
    /// Rebuild the commit as its parent with the target's tree and a
    /// combined message, effectively folding the target into the parent.
    SquashIntoParent { message: String },
}

/// Parse `git remote --verbose` output into one entry per remote.
///
/// Each line is `name<TAB>url (fetch|push)`; the first fetch line and the
/// first push line per name win.
fn parse_remotes(output: &str) -> Vec<RemoteEntry> {
    let mut remotes: Vec<RemoteEntry> = Vec::new();
    for line in output.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }
        let Some((name, rest)) = line.split_once('\t') else {
            continue;
        };
        let (url, kind) = match rest.strip_suffix(" (fetch)") {
            Some(url) => (url, 0),
            None => match rest.strip_suffix(" (push)") {
                Some(url) => (url, 1),
                None => (rest, 2),
            },
        };
        let entry = match remotes.iter_mut().find(|entry| entry.name == name) {
            Some(entry) => entry,
            None => {
                remotes.push(RemoteEntry {
                    name: name.to_string(),
                    fetch_url: String::new(),
                    push_url: String::new(),
                });
                remotes.last_mut().unwrap()
            }
        };
        if kind == 0 && entry.fetch_url.is_empty() {
            entry.fetch_url = url.to_string();
        } else if kind == 1 && entry.push_url.is_empty() {
            entry.push_url = url.to_string();
        } else if kind == 2 && entry.fetch_url.is_empty() {
            entry.fetch_url = url.to_string();
        }
    }
    remotes
        .iter_mut()
        .for_each(|entry| {
            if entry.push_url.is_empty() {
                entry.push_url = entry.fetch_url.clone();
            }
        });
    remotes
}

/// Basic remote name/URL validation before the arguments reach git.
fn validate_remote(name: &str, url: &str) -> Result<(String, String), String> {
    let name = name.trim();
    let url = url.trim();
    if name.is_empty() {
        return Err("A remote name is required.".to_string());
    }
    if name.contains(char::is_whitespace) || name.starts_with('-') || name == "." || name == ".." {
        return Err(format!("Invalid remote name `{name}`."));
    }
    if url.is_empty() {
        return Err("A remote URL is required.".to_string());
    }
    if url.starts_with('-') {
        return Err(format!("Invalid remote URL `{url}`."));
    }
    Ok((name.to_string(), url.to_string()))
}

/// Escape the characters gitignore treats specially at the start of a line.
fn escape_gitignore_pattern(path: &str) -> String {
    if let Some(rest) = path.strip_prefix('!') {
        format!("\\!{rest}")
    } else if let Some(rest) = path.strip_prefix('#') {
        format!("\\#{rest}")
    } else {
        path.to_string()
    }
}

/// Default "squash into parent" message: the parent's message followed by the
/// folded commit's own, like `git rebase` squash does.
fn combined_message(parent_message: &str, commit_message: &str) -> String {
    format!("{}\n\n{}", parent_message.trim(), commit_message.trim())
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}

/// Validate a rewritten commit message: non-empty and free of NUL bytes
/// (commit objects cannot contain them).
fn normalize_message(message: &str) -> Result<String, String> {
    let message = message.trim();
    if message.is_empty() {
        return Err("A commit message is required.".to_string());
    }
    if message.contains('\0') {
        return Err("A commit message cannot contain NUL bytes.".to_string());
    }
    Ok(message.to_string())
}

/// Clone `url` into `parent_dir/name`. Runs outside any repository, so it is
/// a free function taking an executor instead of a client method.
pub fn clone_repository<E: GitExecutor>(
    executor: &mut E,
    parent_dir: &str,
    url: &str,
    name: &str,
) -> Result<String, String> {
    let url = url.trim();
    if url.is_empty() {
        return Err("A repository URL is required.".to_string());
    }
    if url.starts_with('-') {
        return Err(format!("Invalid repository URL `{url}`."));
    }
    let name = name.trim();
    if name.is_empty() {
        return Err("A destination folder name is required.".to_string());
    }
    let relative = std::path::Path::new(name);
    if relative.is_absolute() || name.starts_with('-') {
        return Err(format!("Invalid destination folder name `{name}`."));
    }
    for component in relative.components() {
        match component {
            std::path::Component::Prefix(_)
            | std::path::Component::RootDir
            | std::path::Component::ParentDir => {
                return Err(format!("Invalid destination folder name `{name}`."));
            }
            _ => {}
        }
    }
    let args: Vec<String> = ["clone", url, name]
        .iter()
        .map(|value| value.to_string())
        .collect();
    let output = executor.exec(parent_dir, &args)?;
    if !output.success() {
        return Err(git_failure(&args, &output));
    }
    let cloned = std::path::Path::new(parent_dir).join(name);
    Ok(format!(
        "Cloned {url} into {}",
        cloned.to_string_lossy()
    ))
}

/// Parse the combined `--format=<fields>` and `--raw -z` output of
/// [`GitClient::commit_detail`].
///
/// Layout: one NUL-terminated metadata record, then the raw entries as
/// collected by [`parse_raw_entries`]. Refs keep the ` (HEAD -> main, …)`
/// decoration form before parsing.
fn parse_commit_detail(input: &str) -> Result<CommitDetail, String> {
    let mut parts = input.splitn(2, '\0');
    let meta = parts.next().unwrap_or_default();
    let raw = parts.next().unwrap_or_default();
    let fields: Vec<&str> = meta.split('\u{1f}').collect();
    if fields.len() < 9 {
        return Err("Unexpected output for commit detail: metadata missing.".to_string());
    }
    Ok(CommitDetail {
        hash: fields[0].to_string(),
        short_hash: fields[1].to_string(),
        author: fields[2].to_string(),
        email: fields[3].to_string(),
        date: fields[4].to_string(),
        date_iso: fields[5].to_string(),
        subject: fields[6].to_string(),
        parents: fields[7].split_whitespace().map(|p| p.to_string()).collect(),
        refs: super::log::parse_decorations(fields[8]),
        files: parse_raw_entries(raw),
    })
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

/// True when a failed command means HEAD is unborn (a repository without
/// commits yet). Callers render that as an empty log/diff instead of an
/// error, which replaces an extra `rev-parse HEAD` probe per call.
fn unborn_head_error(error: &str) -> bool {
    error.contains("does not have any commits yet")
        || error.contains("bad default revision")
        || error.contains("bad revision 'HEAD'")
        // `git diff HEAD` on an unborn branch phrases it this way.
        || error.contains("ambiguous argument 'HEAD'")
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
                // `git diff HEAD` on an unborn branch fails like this.
                .with_error(128, "fatal: bad revision 'HEAD'")
                .with(0, "staged diff\n")
                .with(0, "unstaged diff\n"),
        );
        let diff = client.diff(&DiffMode::All, &[]).unwrap();
        assert_eq!(diff, "staged diff\nunstaged diff\n");
        // No `rev-parse HEAD` probe ran first — the failing diff decides.
        assert!(client
            .executor
            .calls
            .iter()
            .all(|call| call[0] != "rev-parse" || call.get(1) != Some(&"--verify".to_string())));
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["diff", "--no-color", "--find-renames", "HEAD"]));
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

    #[test]
    fn merge_builds_options_and_validates_target() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "merged\n"),
        );
        client
            .merge_branch("feature", true)
            .unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["merge", "--no-ff", "feature"]));

        let mut plain = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "merged\n"),
        );
        plain.merge_branch("origin/main", false).unwrap();
        assert!(plain
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["merge", "origin/main"]));

        let mut bad = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, ""),
        );
        assert!(bad.merge_branch("--force", false).is_err());
        assert!(!bad.executor.calls.iter().any(|call| call[0] == "merge"));
    }

    #[test]
    fn merge_abort_uses_base_subcommand() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "aborted\n"),
        );
        client.merge_abort().unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["merge", "--abort"]));
    }

    #[test]
    fn branch_operations_build_expected_commands() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "deleted\n")
                .with(0, "deleted\n")
                .with(0, "renamed\n")
                .with(0, "switched\n")
                .with(0, "tracking\n")
                .with(0, "tracking\n"),
        );
        client.delete_branch("stale", false).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["branch", "-d", "stale"]));

        client.delete_branch("stale", true).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["branch", "-D", "stale"]));

        client.delete_remote_branch("origin/stale").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "origin", "--delete", "stale"]));

        client.rename_branch("old", "new").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["branch", "-m", "old", "new"]));

        client
            .checkout_new_branch_from("work", "origin/other")
            .unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["switch", "-c", "work", "origin/other"]));

        client.set_upstream(Some("main"), "origin/main").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["branch", "--set-upstream-to=origin/main", "main"]));

        client.unset_upstream(Some("main")).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["branch", "--unset-upstream", "main"]));
    }

    #[test]
    fn delete_remote_branch_rejects_bare_names() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, ""),
        );
        assert!(client
            .delete_remote_branch("main")
            .unwrap_err()
            .contains("does not contain a remote"));
    }

    #[test]
    fn programmable_pull_push_options_are_honored() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "pulled\n")
                .with(0, "pushed\n"),
        );
        client
            .pull_with_options(Some("origin"), None, true)
            .unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["pull", "--ff-only", "origin"]));

        client.push_with_options(Some("origin"), Some("nginx"), false, true).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "-u", "origin", "nginx:nginx"]));

        // An empty remote behaves like the toolbar Push button: the current
        // branch goes to the default remote (a bare `git push HEAD` fails —
        // git would read the lone `HEAD` as a repository name).
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n") // rev-parse --show-toplevel
                .with(0, "origin\n")     // git remote
                .with(0, "main\n")       // branch --show-current
                .with(0, "pushed\n"),
        );
        client.push_with_options(None, None, true, false).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "--force-with-lease", "origin", "main:main"]));

        // A remote without a branch pushes the current branch under its own
        // name instead of erroring.
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n") // rev-parse --show-toplevel
                .with(0, "main\n"),      // branch --show-current
        );
        client.push_with_options(Some("origin"), None, false, false).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "origin", "main:main"]));

        // Detached HEAD still fails: there is no branch name to push.
        assert!(client.push_with_options(Some("origin"), None, false, false).is_err());
    }

    #[test]
    fn push_pushes_upstream_or_default_remote_by_name() {
        // With a tracked upstream: a plain push (a bare `git push -u HEAD`
        // fails because git reads `HEAD` as a repository name).
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n") // rev-parse --show-toplevel
                .with(0, "abc\n")        // rev-parse @{upstream} exists
                .with(0, "pushed\n"),
        );
        client.push().unwrap();
        assert!(client.executor.calls.iter().any(|call| call == &vec!["push"]));

        // Without an upstream: default remote + current branch by name.
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")       // rev-parse --show-toplevel
                .with_error(128, "no upstream") // rev-parse @{upstream} fails
                .with(0, "origin\n")            // git remote
                .with(0, "main\n")              // branch --show-current
                .with(0, "pushed\n"),
        );
        client.push().unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "-u", "origin", "main:main"]));
    }

    #[test]
    fn outgoing_uses_upstream_range_when_available() {
        // `has_head` succeeds, `rev-parse @{upstream}` succeeds, then `log`.
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "abc\n")
                .with(0, "")
                .with(0, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}2 hours ago\u{1f}WIP\u{1e}"),
        );
        let (commits, has_upstream) = client.outgoing_commits(200).unwrap();
        assert!(has_upstream);
        assert_eq!(commits.len(), 1);
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["log", "--no-color", "--first-parent", "-n200", "--pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1e", "@{upstream}..HEAD"]));

        let mut no_upstream = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "abc\n")
                .with(1, "")
                .with(0, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}2 hours ago\u{1f}Init\u{1e}"),
        );
        let (commits, has_upstream) = no_upstream.outgoing_commits(200).unwrap();
        assert!(!has_upstream);
        assert_eq!(commits.len(), 1);
        assert!(no_upstream
            .executor
            .calls
            .iter()
            .any(|call| call.last() == Some(&"HEAD".to_string())));
    }

    #[test]
    fn log_filters_translate_into_git_arguments() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, ""),
        );
        client
            .log_nodes(
                30,
                0,
                false,
                &LogFilter {
                    text: Some("fix".to_string()),
                    author: Some("alice".to_string()),
                    path: Some("src".to_string()),
                    ..LogFilter::default()
                },
            )
            .unwrap();
        let log = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "log")
            .unwrap();
        for expected in ["--regexp-ignore-case", "--grep=fix", "--author=alice", "--", "src"] {
            assert!(log.iter().any(|item| item == expected), "{expected} missing");
        }
        assert!(!log.iter().any(|arg| arg == "--all"));

        // Branch-filtered logs replace `--all` with the branch rev.
        let mut by_branch = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, ""),
        );
        by_branch
            .log_nodes(
                30,
                0,
                true,
                &LogFilter {
                    branch: Some("origin/main".to_string()),
                    ..LogFilter::default()
                },
            )
            .unwrap();
        let log = by_branch
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "log")
            .unwrap();
        assert!(log.iter().any(|arg| arg == "origin/main"));
        assert!(!log.iter().any(|arg| arg == "--all"));
    }

    #[test]
    fn log_pagination_skips_loaded_commits() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, ""),
        );
        client.log_nodes(100, 100, false, &LogFilter::default()).unwrap();
        let log = client
            .executor
            .calls
            .iter()
            .find(|call| call[0] == "log")
            .unwrap();
        assert!(log.iter().any(|arg| arg == "--skip=100"), "{log:?}");
        // The first page does not need a skip at all.
        assert!(!log.iter().any(|arg| arg.starts_with("--skip=0")), "{log:?}");
    }

    #[test]
    fn unborn_head_renders_an_empty_log_instead_of_an_error() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with_error(128, "fatal: your current branch 'main' does not have any commits yet"),
        );
        let nodes = client.log_nodes(30, 0, false, &LogFilter::default()).unwrap();
        assert!(nodes.is_empty());
        // No `rev-parse HEAD` probe ran first — the log result decides.
        assert!(client
            .executor
            .calls
            .iter()
            .all(|call| call[0] != "rev-parse" || call[1] != "--verify"));
    }

    #[test]
    fn merge_state_is_detected_only_with_conflicts() {
        let clean = "# branch.head main\u{0}1 .M N... 100644 100644 100644 aaaaaaa bbbbbbb a.txt\u{0}";
        let mut clean_client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, clean)
                .with(0, "stash@{0}\n")
                .with(0, ""),
        );
        let report = clean_client.status_report().unwrap();
        assert!(!report.merge_in_progress);
        // No conflicts -> no probes for MERGE_HEAD, CHERRY_PICK_HEAD or
        // REVERT_HEAD.
        assert!(clean_client
            .executor
            .calls
            .iter()
            .all(|call| call.last().is_none() || call[1] != "-q"));

        let conflicted = "# branch.head main\u{0}u UU N... 100644 100644 100644 100644 aaaaaaa bbbbbbb ccccccc c.txt\u{0}";
        let mut merge_client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, conflicted) // status
                .with(0, "")         // stash list
                .with(0, "")         // log -1
                .with(0, "abc\n")    // MERGE_HEAD exists
                .with(0, "")         // CHERRY_PICK_HEAD
                .with(0, "abc\n"),   // REVERT_HEAD exists
        );
        let report = merge_client.status_report().unwrap();
        assert!(report.merge_in_progress);
        assert!(report.revert_in_progress);
        assert_eq!(report.conflicts().len(), 1);
        assert!(merge_client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["rev-parse", "-q", "--verify", "MERGE_HEAD"]));
    }

    #[test]
    fn tag_management_builds_expected_commands() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "v1.0\u{1f}aaaaaaa\u{1f}yesterday\n")
                .with(0, "created\n")
                .with(0, "deleted\n")
                .with(0, "pushed\n"),
        );
        let tags = client.list_tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "v1.0");
        assert_eq!(tags[0].hash, "aaaaaaa");

        client.create_tag("v1.1", Some("HEAD")).unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["tag", "v1.1", "HEAD"]));

        client.delete_tag("v1.0").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["tag", "-d", "v1.0"]));

        client.push_tags().unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["push", "--tags"]));

        assert!(client.create_tag("-x", None).is_err());
        assert!(client.create_tag("", None).is_err());
    }

    #[test]
    fn commit_history_operations_build_expected_commands() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "reverted\n")
                .with(0, "picked\n")
                .with(0, "dropped\n")
                .with(0, "reset\n"),
        );
        client.revert_commit("abc").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["revert", "--no-edit", "abc"]));

        client.cherry_pick_commit("abc").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["cherry-pick", "--allow-empty", "abc"]));

        client.drop_commit("abc123").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["rebase", "--onto", "abc123^", "abc123", "HEAD"]));

        client.reset_branch("abc", "hard").unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["reset", "--hard", "abc"]));

        for bad in ["", "-q"] {
            assert!(client.revert_commit(bad).is_err());
        }
    }

    #[test]
    fn undo_commit_resets_mixed_to_parent() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "abc\n")
                .with(0, "undone\n"),
        );
        client.undo_head_commit().unwrap();
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["reset", "--mixed", "HEAD~1"]));
    }

    #[test]
    fn commit_detail_parses_metadata_and_raw_entries() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(
                    0,
                    concat!(
                        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}",
                        "alice@example.com\u{1f}2 hours ago\u{1f}2026-01-02 10:00:00 +0000\u{1f}",
                        "Rename then edit\u{1f}\u{1f} (HEAD -> main)\u{0}",
                        ":100644 100644 1111111 1111111 M\u{0}b.txt\u{0}",
                        ":100644 000000 1111111 0000000 D\u{0}a.txt\u{0}",
                        ":000000 100644 0000000 2222222 A\u{0}c.txt\u{0}",
                    ),
                ),
        );
        let detail = client.commit_detail("HEAD").unwrap();
        assert_eq!(detail.subject, "Rename then edit");
        assert_eq!(detail.author, "Alice");
        assert_eq!(detail.email, "alice@example.com");
        assert_eq!(detail.refs, vec!["HEAD -> main"]);
        assert_eq!(detail.files.len(), 3);

        let modified = detail.files.iter().find(|f| f.path == "b.txt").unwrap();
        assert_eq!(modified.status, "M");
        assert!(modified.orig_path.is_none());
        let deleted = detail.files.iter().find(|f| f.status == "D").unwrap();
        assert_eq!(deleted.path, "a.txt");
    }

    #[test]
    fn commit_file_diff_gets_searched_show() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(0, "patch body\n"),
        );
        let text = client.commit_file_diff("abc", "src/main.rs").unwrap();
        assert_eq!(text, "patch body\n");
        assert!(client
            .executor
            .calls
            .iter()
            .any(|call| call == &vec!["show", "--no-color", "--format=", "--patch", "--diff-merges=first-parent", "abc", "--", "src/main.rs"]));
    }

    #[test]
    fn branch_compare_lists_ranges_both_ways() {
        let mut client = client_with(
            MockExecutor::default()
                .with(0, "C:/project\n")
                .with(
                    0,
                    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}now\u{1f}local\u{1e}",
                )
                .with(
                    0,
                    "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\u{1f}bbbbbbb\u{1f}Bob\u{1f}now\u{1f}remote\u{1e}",
                ),
        );
        let (ours, theirs) = client.branch_compare("origin/main").unwrap();
        assert_eq!(ours.len(), 1);
        assert_eq!(ours[0].subject, "local");
        assert_eq!(theirs.len(), 1);
        assert_eq!(theirs[0].subject, "remote");
        let ranges: Vec<String> = client
            .executor
            .calls
            .iter()
            .filter(|call| call[0] == "log")
            .filter_map(|call| call.last().cloned())
            .collect();
        assert_eq!(ranges, vec!["origin/main..HEAD", "HEAD..origin/main"]);
    }

}