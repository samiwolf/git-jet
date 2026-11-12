//! Integration tests against real temporary Git repositories.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use git_jet::core::{
    clone_repository, CmdOutput, DiffMode, GitClient, GitExecutor, GitRequest, LogFilter, Rendered,
    RollbackLevel, StashAction,
};
use tempfile::TempDir;

struct StdExecutor;

impl GitExecutor for StdExecutor {
    fn exec(&mut self, root: &str, args: &[String]) -> Result<CmdOutput, String> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("--no-pager")
            .arg("-c")
            .arg("color.ui=false")
            .arg("-c")
            .arg("core.quotepath=false")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
            .env("GIT_OPTIONAL_LOCKS", "0")
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
        let mut child = Command::new("git")
            .arg("-C")
            .arg(root)
            .arg("--no-pager")
            .args(args)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_PAGER", "cat")
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

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .output()
        .expect("failed to run git");
    assert!(
        output.status.success(),
        "git {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn write(dir: &Path, relative: &str, contents: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

fn init_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Git Jet Tests"]);
    git(
        dir.path(),
        &["config", "user.email", "git-jet@example.test"],
    );
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    git(dir.path(), &["config", "tag.gpgsign", "false"]);
    git(dir.path(), &["config", "core.hooksPath", ".git/hooks"]);
    write(dir.path(), "a.txt", "old\n");
    git(dir.path(), &["add", "--all"]);
    git(dir.path(), &["commit", "-q", "-m", "initial commit"]);
    dir
}

fn empty_repo() -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["config", "user.name", "Git Jet Tests"]);
    git(
        dir.path(),
        &["config", "user.email", "git-jet@example.test"],
    );
    git(dir.path(), &["config", "core.autocrlf", "false"]);
    git(dir.path(), &["config", "commit.gpgsign", "false"]);
    dir
}

fn client(dir: &TempDir) -> GitClient<StdExecutor> {
    GitClient::new(StdExecutor, dir.path().to_string_lossy().to_string())
}

fn run(dir: &TempDir, request: &GitRequest) -> Rendered {
    client(dir).run(request).unwrap()
}

fn text(dir: &TempDir, request: &GitRequest) -> String {
    run(dir, request).text
}

#[test]
fn status_reports_branch_and_change_groups() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "changed\n");
    write(dir.path(), "staged.txt", "staged\n");
    git(dir.path(), &["add", "staged.txt"]);
    write(dir.path(), "untracked.txt", "untracked\n");

    let output = text(&dir, &GitRequest::Status);
    assert!(output.contains("main"), "{output}");
    assert!(output.contains("Staged (1)"), "{output}");
    assert!(output.contains("Unstaged (1)"), "{output}");
    assert!(output.contains("Untracked (1)"), "{output}");
    assert!(output.contains("staged.txt"), "{output}");
    assert!(output.contains("a.txt"), "{output}");
    assert!(output.contains("untracked.txt"), "{output}");
}

#[test]
fn dashboard_shows_recent_commits() {
    let dir = init_repo();
    let output = text(&dir, &GitRequest::Dashboard);
    assert!(output.contains("Git"), "{output}");
    assert!(output.contains("main"), "{output}");
    assert!(output.contains("initial commit"), "{output}");
    assert!(output.contains("/git status"), "{output}");
    assert!(run(&dir, &GitRequest::Dashboard)
        .sections
        .iter()
        .any(|section| section.label == "Recent Commits"));
}

#[test]
fn log_respects_count_and_all() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "second\n");
    git(dir.path(), &["commit", "-q", "-am", "second commit"]);
    write(dir.path(), "a.txt", "third\n");
    git(dir.path(), &["commit", "-q", "-am", "third commit"]);

    let output = text(
        &dir,
        &GitRequest::Log {
            count: 1,
            all: false,
        },
    );
    assert!(output.contains("third commit"), "{output}");
    assert!(!output.contains("initial commit"), "{output}");

    let output = text(
        &dir,
        &GitRequest::Log {
            count: 10,
            all: true,
        },
    );
    assert!(output.contains("third commit"), "{output}");
    assert!(output.contains("initial commit"), "{output}");
}

#[test]
fn diff_modes_show_the_expected_changes() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "new\n");

    let unstaged = text(
        &dir,
        &GitRequest::Diff {
            mode: DiffMode::Unstaged,
            paths: Vec::new(),
        },
    );
    assert!(unstaged.contains("-old"), "{unstaged}");
    assert!(unstaged.contains("+new"), "{unstaged}");

    git(dir.path(), &["add", "a.txt"]);
    let staged = text(
        &dir,
        &GitRequest::Diff {
            mode: DiffMode::Staged,
            paths: Vec::new(),
        },
    );
    assert!(staged.contains("-old"), "{staged}");
    assert!(staged.contains("+new"), "{staged}");

    let all = text(
        &dir,
        &GitRequest::Diff {
            mode: DiffMode::All,
            paths: Vec::new(),
        },
    );
    assert!(all.contains("+new"), "{all}");
}

#[test]
fn diff_can_be_limited_to_a_path() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "changed a\n");
    write(dir.path(), "b.txt", "new b\n");
    git(dir.path(), &["add", "b.txt"]);

    let output = text(
        &dir,
        &GitRequest::Diff {
            mode: DiffMode::Unstaged,
            paths: vec!["a.txt".to_string()],
        },
    );
    assert!(output.contains("a.txt"), "{output}");
    assert!(!output.contains("b.txt"), "{output}");
}

#[test]
fn stage_commit_and_verify_clean_tree() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "committed\n");
    write(dir.path(), "new.txt", "new file\n");

    text(&dir, &GitRequest::Stage { paths: Vec::new() });
    let staged = text(&dir, &GitRequest::Status);
    assert!(staged.contains("Staged (2)"), "{staged}");

    let commit = text(
        &dir,
        &GitRequest::Commit {
            message: "add both files".to_string(),
            amend: false,
        },
    );
    assert!(commit.contains("add both files"), "{commit}");

    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("Working tree clean."), "{status}");

    let log = text(
        &dir,
        &GitRequest::Log {
            count: 10,
            all: false,
        },
    );
    assert!(log.contains("add both files"), "{log}");
}

#[test]
fn unstage_returns_a_new_file_to_untracked() {
    let dir = init_repo();
    write(dir.path(), "new.txt", "new file\n");
    git(dir.path(), &["add", "new.txt"]);

    text(
        &dir,
        &GitRequest::Unstage {
            paths: vec!["new.txt".to_string()],
        },
    );

    let output = text(&dir, &GitRequest::Status);
    assert!(output.contains("Untracked (1)"), "{output}");
    assert!(output.contains("new.txt"), "{output}");
}

#[test]
fn branches_can_be_created_and_switched() {
    let dir = init_repo();
    let created = text(
        &dir,
        &GitRequest::CreateBranch {
            name: "feature".to_string(),
        },
    );
    assert!(created.contains("feature"), "{created}");

    let branches = text(&dir, &GitRequest::Branches);
    assert!(branches.contains("* feature"), "{branches}");
    assert!(branches.contains("main"), "{branches}");

    text(
        &dir,
        &GitRequest::Switch {
            branch: "main".to_string(),
        },
    );
    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("main"), "{status}");
}

#[test]
fn stash_round_trip() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "work in progress\n");

    let pushed = text(
        &dir,
        &GitRequest::Stash {
            action: StashAction::Push {
                message: Some("wip parser".to_string()),
            },
        },
    );
    assert!(pushed.contains("Working tree clean."), "{pushed}");

    let list = text(
        &dir,
        &GitRequest::Stash {
            action: StashAction::List,
        },
    );
    assert!(list.contains("wip parser"), "{list}");

    let popped = text(
        &dir,
        &GitRequest::Stash {
            action: StashAction::Pop { index: None },
        },
    );
    assert!(popped.contains("Unstaged (1)"), "{popped}");
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "work in progress\n"
    );
}

#[test]
fn rename_includes_the_original_path() {
    let dir = init_repo();
    git(dir.path(), &["mv", "a.txt", "renamed.txt"]);

    let output = text(&dir, &GitRequest::Status);
    assert!(output.contains("renamed.txt <- a.txt"), "{output}");
}

#[test]
fn paths_with_spaces_are_handled() {
    let dir = init_repo();
    write(dir.path(), "file with space.txt", "space\n");
    text(
        &dir,
        &GitRequest::Stage {
            paths: vec!["file with space.txt".to_string()],
        },
    );
    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("file with space.txt"), "{status}");
}

#[test]
fn unborn_branch_is_handled() {
    let dir = empty_repo();
    write(dir.path(), "first.txt", "first\n");
    git(dir.path(), &["add", "first.txt"]);

    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("main (no commits)"), "{status}");

    let diff = text(
        &dir,
        &GitRequest::Diff {
            mode: DiffMode::All,
            paths: Vec::new(),
        },
    );
    assert!(diff.contains("+first"), "{diff}");

    let dashboard = text(&dir, &GitRequest::Dashboard);
    assert!(dashboard.contains("No commits yet."), "{dashboard}");
}

#[test]
fn show_and_blame_work() {
    let dir = init_repo();
    let show = text(
        &dir,
        &GitRequest::Show {
            rev: "HEAD".to_string(),
        },
    );
    assert!(show.contains("initial commit"), "{show}");
    assert!(show.contains("a.txt"), "{show}");

    let blame = text(
        &dir,
        &GitRequest::Blame {
            path: "a.txt".to_string(),
        },
    );
    assert!(blame.contains("Git Jet Tests"), "{blame}");
    assert!(blame.contains("old"), "{blame}");
}

#[test]
fn show_merge_commit_includes_the_patch() {
    let dir = init_repo();
    git(dir.path(), &["checkout", "-b", "feature"]);
    write(dir.path(), "a.txt", "branch\n");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-m", "branch change"]);
    git(dir.path(), &["checkout", "main"]);
    write(dir.path(), "b.txt", "main\n");
    git(dir.path(), &["add", "b.txt"]);
    git(dir.path(), &["commit", "-m", "main change"]);
    git(dir.path(), &["merge", "--no-ff", "--no-edit", "feature"]);

    let show = text(
        &dir,
        &GitRequest::Show {
            rev: "HEAD".to_string(),
        },
    );
    assert!(show.contains("Merge branch 'feature'"), "{show}");
    // The patch against the first parent must be present, not just metadata.
    assert!(show.contains("diff --git"), "{show}");
    assert!(show.contains("+branch"), "{show}");
}

#[test]
fn non_repository_has_a_friendly_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = client(&dir).run(&GitRequest::Status).unwrap_err();
    assert!(error.contains("Not a Git repository"), "{error}");
}

#[test]
fn help_works_outside_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let rendered = client(&dir).run(&GitRequest::Help).unwrap();
    assert!(rendered.text.contains("/git status"), "{rendered:?}");
    assert!(rendered.text.contains("/git stash"), "{rendered:?}");
}

fn word_list() -> [&'static str; 20] {
    [
        "one",
        "two",
        "three",
        "four",
        "five",
        "six",
        "seven",
        "eight",
        "nine",
        "ten",
        "eleven",
        "twelve",
        "thirteen",
        "fourteen",
        "fifteen",
        "sixteen",
        "seventeen",
        "eighteen",
        "nineteen",
        "twenty",
    ]
}

fn long_file() -> String {
    word_list().iter().map(|word| format!("{word}\n")).collect()
}

/// Splits `git diff <mode> -- a.txt` into one patch fragment per hunk, in the
/// same way the frontend rebuilds a patch from the rendered diff.
fn hunk_patches(dir: &TempDir, mode: Option<&str>) -> Vec<String> {
    let mut args: Vec<&str> = vec!["diff"];
    if let Some(mode) = mode {
        args.push(mode);
    }
    args.push("--");
    let raw = git(dir.path(), &args);
    let lines: Vec<&str> = raw.trim_end().split('\n').collect();
    let hunk_starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.starts_with("@@"))
        .map(|(index, _)| index)
        .collect();
    let header: String = lines[..*hunk_starts.first().unwrap_or(&lines.len())].join("\n") + "\n";
    hunk_starts
        .iter()
        .enumerate()
        .map(|(position, &start)| {
            let end = hunk_starts
                .get(position + 1)
                .copied()
                .unwrap_or(lines.len());
            header.clone() + &lines[start..end].join("\n") + "\n"
        })
        .collect()
}

#[test]
fn amend_replaces_the_last_subject() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "amended\n");
    git(dir.path(), &["add", "a.txt"]);

    let output = text(
        &dir,
        &GitRequest::Commit {
            message: "amended subject".to_string(),
            amend: true,
        },
    );
    assert!(output.contains("amended subject"), "{output}");

    let log = text(
        &dir,
        &GitRequest::Log {
            count: 5,
            all: false,
        },
    );
    assert!(log.contains("amended subject"), "{log}");
    assert!(!log.contains("initial commit"), "{log}");
}

#[test]
fn rollback_worktree_discards_unstaged_changes_only() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "staged\n");
    git(dir.path(), &["add", "a.txt"]);
    write(dir.path(), "a.txt", "dirty\n");

    let output = text(
        &dir,
        &GitRequest::Rollback {
            paths: vec!["a.txt".to_string()],
            level: RollbackLevel::Unstaged,
        },
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "staged\n",
        "the staged change must survive a worktree rollback"
    );

    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("Staged (1)"), "{output} {status}");
    assert!(!status.contains("Unstaged (1)"), "{output} {status}");
}

#[test]
fn rollback_worktree_discards_one_hunk_via_apply() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "base\n");
    git(dir.path(), &["add", "a.txt"]);
    text(
        &dir,
        &GitRequest::Commit {
            message: "replace a.txt".to_string(),
            amend: false,
        },
    );
    // Committed baseline: "one\ntwo\n…\ntwenty\n".
    fs::write(dir.path().join("a.txt"), long_file()).unwrap();
    git(dir.path(), &["commit", "-q", "-am", "long file"]);
    // Hunk 1 changes line 1, hunk 2 changes line 20: far enough apart for
    // `git diff` to emit two separate hunks.
    let modified: String = long_file()
        .replace("one\n", "ONE\n")
        .replace("twenty\n", "TWENTY\n");
    fs::write(dir.path().join("a.txt"), &modified).unwrap();

    let hunks = hunk_patches(&dir, None);
    assert_eq!(hunks.len(), 2, "the test needs two separated hunks");

    let mut client = client(&dir);
    client.rollback_hunk(&hunks[0], false).unwrap();

    let result = fs::read_to_string(dir.path().join("a.txt")).unwrap();
    assert!(result.starts_with("one\ntwo\n"), "{result}");
    assert!(result.ends_with("TWENTY\n"), "{result}");
}

#[test]
fn rollback_hunk_staged_unstages_one_hunk() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "base\n");
    git(dir.path(), &["add", "a.txt"]);
    text(
        &dir,
        &GitRequest::Commit {
            message: "replace a.txt".to_string(),
            amend: false,
        },
    );
    // Committed baseline: "one\ntwo\n…\ntwenty\n".
    fs::write(dir.path().join("a.txt"), long_file()).unwrap();
    git(dir.path(), &["commit", "-q", "-am", "long file"]);
    let modified: String = long_file()
        .replace("one\n", "ONE\n")
        .replace("twenty\n", "TWENTY\n");
    fs::write(dir.path().join("a.txt"), &modified).unwrap();
    git(dir.path(), &["add", "a.txt"]);

    let hunks = hunk_patches(&dir, Some("--cached"));
    assert_eq!(hunks.len(), 2, "the test needs two separated hunks");

    // Roll back the second hunk (twenty → TWENTY) from the index only.
    let mut client = client(&dir);
    client.rollback_hunk(&hunks[1], true).unwrap();

    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        modified,
        "the worktree must be untouched by a staged hunk rollback"
    );
    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("Staged (1)"), "{status}");
    assert!(status.contains("Unstaged (1)"), "{status}");
}

#[test]
fn rollback_staged_reverts_to_head() {
    let dir = init_repo();
    write(dir.path(), "a.txt", "changed\n");
    git(dir.path(), &["add", "a.txt"]);

    text(
        &dir,
        &GitRequest::Rollback {
            paths: vec!["a.txt".to_string()],
            level: RollbackLevel::Staged,
        },
    );

    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "old\n"
    );
    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("Working tree clean."), "{status}");
}

#[test]
fn rollback_staged_removes_added_files() {
    let dir = init_repo();
    write(dir.path(), "new.txt", "brand new\n");
    git(dir.path(), &["add", "new.txt"]);

    text(
        &dir,
        &GitRequest::Rollback {
            paths: vec!["new.txt".to_string()],
            level: RollbackLevel::Staged,
        },
    );

    assert!(!dir.path().join("new.txt").exists());
    let status = text(&dir, &GitRequest::Status);
    assert!(status.contains("Working tree clean."), "{status}");
}

#[test]
fn rollback_untracked_deletes_the_file() {
    let dir = init_repo();
    write(dir.path(), "scratch.txt", "scratch\n");

    text(
        &dir,
        &GitRequest::Rollback {
            paths: vec!["scratch.txt".to_string()],
            level: RollbackLevel::Untracked,
        },
    );

    assert!(!dir.path().join("scratch.txt").exists());
}

/* ============================== new operations ============================ */

fn commit_change(dir: &TempDir, file: &str, contents: &str, message: &str) -> String {
    write(dir.path(), file, contents);
    git(dir.path(), &["add", "--all"]);
    git(dir.path(), &["commit", "-q", "-m", message]);
    git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string()
}

#[test]
fn merge_branches_and_reports_conflict_state() {
    let dir = init_repo();
    let mut client = client(&dir);

    // Clean merge with --no-ff.
    git(dir.path(), &["switch", "-q", "-c", "feature"]);
    commit_change(&dir, "feature.txt", "feature\n", "feature work");
    git(dir.path(), &["switch", "-q", "main"]);
    let output = client.merge_branch("feature", true).unwrap();
    assert!(output.contains("feature") || output.contains("Merge"), "{output}");
    let log = git(dir.path(), &["log", "--oneline", "-2"]);
    assert!(log.contains("Merge branch 'feature'"), "{log}");

    // Conflicting merge: abort restores the pre-merge state.
    git(dir.path(), &["switch", "-q", "-c", "conflict"]);
    commit_change(&dir, "a.txt", "branch side\n", "branch side changed");
    git(dir.path(), &["switch", "-q", "main"]);
    write(dir.path(), "a.txt", "main side\n");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "main side changed"]);

    let merge_error = client.merge_branch("conflict", false).unwrap_err();
    assert!(merge_error.contains("CONFLICT"), "{merge_error}");

    let report = client.status().unwrap();
    assert!(report.merge_in_progress, "merge should be in progress");
    assert_eq!(report.conflicts().len(), 1);

    client.merge_abort().unwrap();
    let report = client.status().unwrap();
    assert!(!report.merge_in_progress, "abort should clear the state");
    assert!(report.conflicts().is_empty());
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "main side\n"
    );
}

#[test]
fn merge_abort_without_a_started_merge_fails() {
    let dir = init_repo();
    let error = client(&dir).merge_abort().unwrap_err();
    assert!(error.to_lowercase().contains("merge"), "{error}");
}

#[test]
fn merge_editor_reads_and_resolves_a_conflicted_file() {
    let dir = init_repo();
    let mut client = client(&dir);

    git(dir.path(), &["switch", "-q", "-c", "conflict"]);
    commit_change(&dir, "a.txt", "branch side\n", "branch side changed");
    git(dir.path(), &["switch", "-q", "main"]);
    write(dir.path(), "a.txt", "main side\n");
    git(dir.path(), &["add", "a.txt"]);
    git(dir.path(), &["commit", "-q", "-m", "main side changed"]);
    client.merge_branch("conflict", false).unwrap_err();

    // The conflicted worktree file carries the merge markers.
    let content = client.worktree_file_content("a.txt").unwrap();
    assert!(content.contains("<<<<<<<"), "{content}");
    assert!(content.contains(">>>>>>>"), "{content}");

    // Absolute paths and `..` escapes stay inside the repo root.
    assert!(
        client
            .worktree_file_content("../outside.txt")
            .unwrap_err()
            .contains("Invalid"),
        "should reject parent traversal"
    );
    assert!(
        client
            .worktree_file_content(dir.path().join("a.txt").to_string_lossy().as_ref())
            .unwrap_err()
            .contains("Invalid"),
        "should reject absolute paths"
    );

    // Writing a resolution stages the file and clears the conflict.
    client.write_merge_result("a.txt", "resolved!\n").unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "resolved!\n"
    );
    let report = client.status().unwrap();
    assert!(
        report.conflicts().is_empty(),
        "resolution marks the file resolved"
    );
    assert_eq!(report.staged().len(), 1, "the resolved file is staged");
}

#[test]
fn commit_detail_lists_files_and_per_file_diffs() {
    let dir = init_repo();
    let mut client = client(&dir);

    commit_change(&dir, "b.txt", "brand\nnew\nfile\n", "add file");
    git(dir.path(), &["mv", "b.txt", "c.txt"]);
    commit_change(&dir, "extra.txt", "extra\n", "rename then add");

    let detail = client.commit_detail("HEAD").unwrap();
    assert_eq!(detail.subject, "rename then add");
    assert_eq!(detail.author, "Git Jet Tests");
    assert!(detail
        .files
        .iter()
        .any(|f| f.status.starts_with('R')
            && f.orig_path.as_deref() == Some("b.txt")
            && f.path == "c.txt"));
    assert!(detail.files.iter().any(|f| f.status == "A" && f.path == "extra.txt"));

    let patch = client.commit_file_diff("HEAD", "c.txt").unwrap();
    assert!(patch.contains("diff --git"), "{patch}");
    assert!(patch.contains("c.txt"), "{patch}");
}

#[test]
fn tags_flow_creates_and_deletes() {
    let dir = init_repo();
    let mut client = client(&dir);
    let head = commit_change(&dir, "a.txt", "tagged\n", "commit for tag");

    client.create_tag("v1.0", Some("HEAD")).unwrap();
    client.create_tag("v0.9", None).unwrap();
    let tags = client.list_tags().unwrap();
    assert!(tags.iter().any(|t| t.name == "v1.0"));
    assert!(tags.iter().any(|t| t.name == "v0.9"));
    let v1 = tags.iter().find(|t| t.name == "v1.0").unwrap();
    assert_eq!(v1.hash.len(), 7);

    // The tag shows up in the log decorations.
    assert!(git(dir.path(), &["tag", "--points-at", "HEAD"]).contains("v1.0"));

    client.delete_tag("v0.9").unwrap();
    let tags = client.list_tags().unwrap();
    assert!(!tags.iter().any(|t| t.name == "v0.9"));
    assert_eq!(git(dir.path(), &["rev-parse", "v1.0^{commit}"]).trim(), head);
}

#[test]
fn cherry_pick_revert_undo_and_drop_modify_history() {
    let dir = init_repo();
    let mut client = client(&dir);
    let base = commit_change(&dir, "a.txt", "base\n", "base commit");

    // A commit isolated on another branch is the classic cherry-pick source.
    git(dir.path(), &["switch", "-q", "-c", "source"]);
    let picked = commit_change(&dir, "picked.txt", "picked\n", "fresh pick");
    git(dir.path(), &["switch", "-q", "main"]);
    let head_before_pick = commit_change(&dir, "a.txt", "head\n", "head commit");

    // Revert removes the head commit's changes as a new commit.
    client.revert_commit(&head_before_pick).unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "base\n");

    // Cherry-pick applies the isolated commit on top.
    client.cherry_pick_commit(&picked).unwrap();
    assert_eq!(fs::read_to_string(dir.path().join("picked.txt")).unwrap(), "picked\n");
    let copied = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
    assert_ne!(copied, base);
    assert_ne!(copied, picked);

    // Drop rewrites the cherry-picked commit out again.
    client.drop_commit(&copied).unwrap();
    assert!(!dir.path().join("picked.txt").exists());

    // Undo moves HEAD back to the parent of the revert commit; the revert's
    // changes stay in the worktree.
    client.undo_head_commit().unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), head_before_pick);
    let status = client.status().unwrap();
    assert!(
        !status.unstaged().is_empty() || !status.staged().is_empty(),
        "worktree holds the undone changes"
    );
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "base\n");
}

#[test]
fn reset_branch_modes_match_git_semantics() {
    let dir = init_repo();
    let mut client = client(&dir);
    let base = commit_change(&dir, "a.txt", "base\n", "base commit");
    commit_change(&dir, "b.txt", "added\n", "addition commit");
    write(dir.path(), "c.txt", "dirty\n");
    git(dir.path(), &["add", "c.txt"]);

    // Soft keeps index and worktree exactly as they were.
    client.reset_branch(&base, "soft").unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), base);
    let staged = git(dir.path(), &["diff", "--cached", "--name-only"]);
    assert!(staged.contains("b.txt"));
    assert!(staged.contains("c.txt"));

    // Hard wipes index and worktree: staged new files are deleted.
    client.reset_branch(&base, "hard").unwrap();
    assert!(!dir.path().join("b.txt").exists());
    assert!(!dir.path().join("c.txt").exists());

    commit_change(&dir, "b.txt", "added\n", "addition commit again");

    // Mixed unstages everything; files stay on disk.
    client.reset_branch(&base, "mixed").unwrap();
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), base);
    assert!(git(dir.path(), &["diff", "--cached", "--name-only"]).trim().is_empty());
    assert!(dir.path().join("b.txt").exists());
}

#[test]
fn log_filters_narrow_the_graph() {
    let dir = init_repo();
    let mut client = client(&dir);
    commit_change(&dir, "a.txt", "one\n", "alpha work");
    commit_change(&dir, "b.txt", "two\n", "gamma work");

    let all = client.log_nodes(20, 0, false, &LogFilter::default()).unwrap();
    assert_eq!(all.len(), 3);

    // Pagination fetches only the requested window via `--skip`: skip the
    // two newest commits and the next (oldest) one is "initial commit".
    let page = client.log_nodes(1, 2, false, &LogFilter::default()).unwrap();
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].commit.subject, "initial commit");

    let by_text = client
        .log_nodes(
            20,
            0,
            false,
            &LogFilter { text: Some("alpha".to_string()), ..LogFilter::default() },
        )
        .unwrap();
    assert_eq!(by_text.len(), 1);
    assert_eq!(by_text[0].commit.subject, "alpha work");

    let by_author = client
        .log_nodes(
            20,
            0,
            false,
            &LogFilter { author: Some("nobody".to_string()), ..LogFilter::default() },
        )
        .unwrap();
    assert!(by_author.is_empty());

    let by_path = client
        .log_nodes(
            20,
            0,
            false,
            &LogFilter { path: Some("b.txt".to_string()), ..LogFilter::default() },
        )
        .unwrap();
    assert_eq!(by_path.len(), 1);
    assert_eq!(by_path[0].commit.subject, "gamma work");
}

/// Creates a bare remote over the local filesystem plus a clone to work in,
/// returning `(worktree, client)`. The file transport needs no credentials.
fn clone_of_bare_remote() -> (TempDir, TempDir) {
    let holder = tempfile::tempdir().unwrap();
    let client_dir = tempfile::tempdir().unwrap();
    let remote_path = holder.path().join("origin.git");
    assert!(Command::new("git")
        .arg("init")
        .arg("-q")
        .arg("--bare")
        .arg("--initial-branch=main")
        .arg(&remote_path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false), "creating the bare remote failed");
    // The clone needs the repository config done by `git clone` itself.
    git(
        client_dir.path(),
        &[
            "clone",
            "-q",
            "-c",
            "user.name=Git Jet Tests",
            "-c",
            "user.email=git-jet@example.test",
            remote_path.to_str().unwrap(),
            client_dir.path().join("worktree").to_str().unwrap(),
        ],
    );
    (holder, client_dir)
}

fn test_git_config(dir: &Path) {
    git(dir, &["config", "user.name", "Git Jet Tests"]);
    git(dir, &["config", "user.email", "git-jet@example.test"]);
}

#[test]
fn outgoing_list_and_push_options_work_over_a_local_remote() {
    let (_holder, holder) = clone_of_bare_remote();
    let worktree = holder.path().join("worktree");
    test_git_config(&worktree);
    let mut client = GitClient::new(
        StdExecutor,
        worktree.to_str().unwrap().to_string(),
    );

    // The clone is empty except for config; make an initial commit.
    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);

    // Fresh clone without upstream: push lists the whole history.
    let (commits, has_upstream) = client.outgoing_commits(50).unwrap();
    assert!(!has_upstream, "fresh clone has no tracking yet");
    assert_eq!(commits.len(), 1);

    client.push().unwrap();

    // After the push nothing is ahead.
    let (commits, has_upstream) = client.outgoing_commits(50).unwrap();
    assert!(has_upstream);
    assert!(commits.is_empty());

    // One more commit, push with explicit remote + branch.
    git(&worktree, &["commit", "-q", "--allow-empty", "-m", "second"]);
    client
        .push_with_options(Some("origin"), Some("main"), false, false)
        .unwrap();
    let (commits, _) = client.outgoing_commits(50).unwrap();
    assert!(commits.is_empty());
}

#[test]
fn delete_remote_branch_removes_it_on_the_remote() {
    let (_holder, holder) = clone_of_bare_remote();
    let worktree = holder.path().join("worktree");
    test_git_config(&worktree);
    let mut client = GitClient::new(
        StdExecutor,
        worktree.to_str().unwrap().to_string(),
    );
    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    client.push().unwrap();

    git(&worktree, &["switch", "-q", "-c", "temp"]);
    client.push_with_options(Some("origin"), Some("temp"), false, true).unwrap();
    let remotes = git(&worktree, &["branch", "-r", "--format=%(refname:short)"]);
    assert!(remotes.contains("origin/temp"), "{remotes}");

    client.delete_remote_branch("origin/temp").unwrap();
    let remotes = git(&worktree, &["branch", "-r", "--format=%(refname:short)"]);
    assert!(!remotes.contains("origin/temp"), "{remotes}");
}

#[test]
fn remotes_can_be_added_edited_removed_and_fetched() {
    let dir = init_repo();
    let mut client = client(&dir);
    let holder = tempfile::tempdir().unwrap();
    let remote_url = holder.path().join("origin.git").to_string_lossy().into_owned();
    assert!(
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg("--bare")
            .arg("--initial-branch=main")
            .arg(&remote_url)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "creating the bare remote failed"
    );

    assert!(client.list_remotes().unwrap().is_empty());

    client.add_remote("origin", &remote_url).unwrap();
    let remotes = client.list_remotes().unwrap();
    assert_eq!(remotes.len(), 1);
    assert_eq!(remotes[0].name, "origin");
    assert_eq!(remotes[0].fetch_url, remote_url);
    assert_eq!(remotes[0].push_url, remote_url);

    // Validation: invalid names/URLs and duplicates never reach git.
    assert!(client.add_remote("-bad", &remote_url).is_err());
    assert!(client.add_remote("ok", "").is_err());
    assert!(client.add_remote("origin", &remote_url).is_err());

    let mirror = holder.path().join("mirror.git").to_string_lossy().into_owned();
    assert!(
        Command::new("git")
            .arg("init")
            .arg("-q")
            .arg("--bare")
            .arg("--initial-branch=main")
            .arg(&mirror)
            .status()
            .map(|s| s.success())
            .unwrap_or(false),
        "creating the bare mirror failed"
    );

    client.fetch_remote("origin").unwrap();
    client.set_remote_url("origin", &mirror).unwrap();
    let remotes = client.list_remotes().unwrap();
    assert_eq!(remotes[0].fetch_url, mirror);
    client.fetch_remote("origin").unwrap();

    client.remove_remote("origin").unwrap();
    assert!(client.list_remotes().unwrap().is_empty());
    assert!(client.remove_remote("origin").is_err());
}

#[test]
fn incoming_lists_commits_from_upstream() {
    let (bare, client_dir) = clone_of_bare_remote();
    let worktree = client_dir.path().join("worktree");
    test_git_config(&worktree);
    let remote_url = bare.path().join("origin.git").to_string_lossy().into_owned();
    let mut client = GitClient::new(StdExecutor, worktree.to_str().unwrap().to_string());

    // Nothing fetched yet: no upstream, so no incoming list.
    let (commits, has_upstream) = client.incoming_commits(50).unwrap();
    assert!(!has_upstream);
    assert!(commits.is_empty());

    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    client
        .push_with_options(Some("origin"), Some("main"), false, true)
        .unwrap();

    // Up to date with the upstream.
    let (commits, has_upstream) = client.incoming_commits(50).unwrap();
    assert!(has_upstream);
    assert!(commits.is_empty());

    // Advance the remote from a second clone, fetch, then list incoming.
    let clone2 = tempfile::tempdir().unwrap();
    git(
        clone2.path(),
        &[
            "clone",
            "-q",
            "-c",
            "user.name=Git Jet Tests",
            "-c",
            "user.email=git-jet@example.test",
            remote_url.as_str(),
            clone2.path().join("worktree").to_str().unwrap(),
        ],
    );
    let second = clone2.path().join("worktree");
    write(&second, "b.txt", "remote side\n");
    git(&second, &["add", "--all"]);
    git(&second, &["commit", "-q", "-m", "on the remote"]);
    git(&second, &["push", "-q", "origin", "main"]);

    client.fetch(true).unwrap();
    let (commits, has_upstream) = client.incoming_commits(50).unwrap();
    assert!(has_upstream);
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].subject, "on the remote");
}

#[test]
fn gitignore_adds_and_dedupes_patterns() {
    let dir = init_repo();
    let mut client = client(&dir);
    write(dir.path(), "notes.md", "scratch\n");
    write(dir.path(), "tmp/keep.txt", "nested\n");
    write(dir.path(), "!weird.md", "bang\n");

    client.add_to_gitignore(&["notes.md".to_string()]).unwrap();
    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(content, "notes.md\n");

    let report = client.status().unwrap();
    assert!(!report
        .untracked()
        .iter()
        .any(|entry| entry.path == "notes.md"));

    // Idempotent: the same pattern is not written twice.
    let again = client.add_to_gitignore(&["notes.md".to_string()]).unwrap();
    assert!(again.contains("already covers"), "{again}");
    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert_eq!(content.matches("notes.md").count(), 1);

    client.add_to_gitignore(&["tmp".to_string()]).unwrap();
    // `git check-ignore` exits 0 exactly when a path is ignored.
    git(dir.path(), &["check-ignore", "-q", "notes.md"]);
    git(dir.path(), &["check-ignore", "-q", "tmp/keep.txt"]);

    // A leading `!` is escaped so it cannot become a negation.
    client.add_to_gitignore(&["!weird.md".to_string()]).unwrap();
    let content = fs::read_to_string(dir.path().join(".gitignore")).unwrap();
    assert!(content.contains("\\!weird.md"), "{content}");

    assert!(client.add_to_gitignore(&[]).is_err());
    assert!(client.add_to_gitignore(&["-x".to_string()]).is_err());
}

#[test]
fn squash_folds_a_commit_into_its_parent() {
    let dir = init_repo();
    let mut client = client(&dir);
    commit_change(&dir, "a.txt", "one\n", "second commit\n\nsecond body");
    // Give the folded commit a distinct author to verify it is replaced
    // by the parent's author.
    write(dir.path(), "a.txt", "two\n");
    git(dir.path(), &["add", "--all"]);
    git(
        dir.path(),
        &["-c", "user.name=Third", "-c", "user.email=third@test", "commit", "-q", "-m", "third commit"],
    );
    let c = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();

    let tip_tree = git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    let count_before = git(dir.path(), &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count_before, "3");

    // Preview shows the combined default message and validates the commit.
    let preview = client.squash_preview(&c).unwrap();
    assert!(preview.contains("second body"), "{preview}");
    assert!(preview.contains("third commit"), "{preview}");

    client.squash_commit_into_parent(&c, None).unwrap();

    let count_after = git(dir.path(), &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count_after, "2");
    let subjects = git(dir.path(), &["log", "--format=%s"]);
    assert!(subjects.starts_with("second commit"), "{subjects}");
    assert!(subjects.contains("initial commit"), "{subjects}");
    let message = git(dir.path(), &["show", "-s", "--format=%B", "HEAD"]);
    assert!(message.contains("second body"), "{message}");
    assert!(message.contains("third commit"), "{message}");
    let author = git(dir.path(), &["show", "-s", "--format=%an", "HEAD"]).trim().to_string();
    assert_eq!(author, "Git Jet Tests", "the squashed commit keeps the parent's author");
    let tree = git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    assert_eq!(tree, tip_tree, "the final tree must not change");

    // Explicit message override.
    client.squash_commit_into_parent("HEAD", Some("manual message")).unwrap();
    let subjects = git(dir.path(), &["log", "--format=%s"]);
    assert!(subjects.starts_with("manual message"), "{subjects}");
}

#[test]
fn squash_replays_descendants_and_rejects_impossible_targets() {
    let dir = init_repo();
    let mut client = client(&dir);
    commit_change(&dir, "a.txt", "one\n", "second commit");
    let c = commit_change(&dir, "a.txt", "two\n", "third commit");
    let d = commit_change(&dir, "a.txt", "three\n", "fourth commit");
    let d_tree = git(dir.path(), &["show", "-s", "--format=%T", &d]).trim().to_string();

    // Squashing a commit below the tip replays the descendants.
    client.squash_commit_into_parent(&c, Some("folded")).unwrap();
    let count = git(dir.path(), &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count, "3");
    let subjects = git(dir.path(), &["log", "--format=%s"]);
    assert!(subjects.contains("fourth commit"), "{subjects}");
    assert!(subjects.contains("folded"), "{subjects}");
    let tree = git(dir.path(), &["show", "-s", "--format=%T", "HEAD"]).trim().to_string();
    assert_eq!(tree, d_tree, "the replayed tip keeps its tree");

    // A commit from another branch is not reachable from HEAD: `feature`
    // branches away before main advances, so "fifth" is main-only.
    git(dir.path(), &["switch", "-q", "-c", "feature"]);
    commit_change(&dir, "feature.txt", "feature\n", "feature work");
    git(dir.path(), &["switch", "-q", "main"]);
    let fifth = commit_change(&dir, "a.txt", "five\n", "fifth commit");
    git(dir.path(), &["switch", "-q", "feature"]);
    let error = client.squash_commit_into_parent(&fifth, None).unwrap_err();
    assert!(error.contains("not on the current branch"), "{error}");

    // A merge commit cannot be squashed into a parent.
    git(dir.path(), &["switch", "-q", "main"]);
    client.merge_branch("feature", true).unwrap();
    let merge_sha = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
    let error = client.squash_commit_into_parent(&merge_sha, None).unwrap_err();
    assert!(error.contains("merge"), "{error}");

    // Detached HEAD cannot rewrite history.
    git(dir.path(), &["switch", "-q", "--detach", "main"]);
    let error = client.reword_commit("HEAD", "message").unwrap_err();
    assert!(error.contains("Detached HEAD"), "{error}");
}

#[test]
fn reword_changes_only_the_message() {
    let dir = init_repo();
    let mut client = client(&dir);
    let b = commit_change(&dir, "a.txt", "one\n", "second commit");
    commit_change(&dir, "a.txt", "two\n", "third commit");

    let full = client.commit_full_message(&b).unwrap();
    assert!(full.contains("second commit"), "{full}");

    let tip_tree = git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    client.reword_commit(&b, "rewritten second\n\nnew body").unwrap();

    let count = git(dir.path(), &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count, "3");
    let subjects = git(dir.path(), &["log", "--format=%s"]);
    assert!(subjects.contains("rewritten second"), "{subjects}");
    assert!(subjects.contains("third commit"), "{subjects}");
    assert!(subjects.contains("initial commit"), "{subjects}");
    let author = git(dir.path(), &["show", "-s", "--format=%an", "HEAD"]).trim().to_string();
    assert_eq!(author, "Git Jet Tests");
    let tree = git(dir.path(), &["rev-parse", "HEAD^{tree}"]).trim().to_string();
    assert_eq!(tree, tip_tree, "a reword must not change any tree");

    assert!(client.reword_commit(&b, "   ").is_err());
    assert!(client.reword_commit("-x", "message").is_err());
}

#[test]
fn clone_repository_clones_into_the_destination() {
    let (bare, client_dir) = clone_of_bare_remote();
    let worktree = client_dir.path().join("worktree");
    test_git_config(&worktree);
    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    git(&worktree, &["push", "-q", "origin", "main"]);
    let remote_url = bare.path().join("origin.git").to_string_lossy().into_owned();

    let dest = tempfile::tempdir().unwrap();
    let mut executor = StdExecutor;
    let output =
        clone_repository(&mut executor, dest.path().to_str().unwrap(), &remote_url, "cloned").unwrap();
    assert!(output.contains("Cloned"), "{output}");

    let cloned = dest.path().join("cloned");
    let log = git(&cloned, &["log", "--oneline"]);
    assert!(log.contains("initial"), "{log}");

    assert!(clone_repository(&mut executor, dest.path().to_str().unwrap(), "", "x").is_err());
    assert!(clone_repository(&mut executor, dest.path().to_str().unwrap(), &remote_url, "..").is_err());
    assert!(clone_repository(&mut executor, dest.path().to_str().unwrap(), &remote_url, "-x").is_err());
}


#[test]
fn repo_stamp_tracks_repository_changes() {
    let dir = init_repo();
    let root = dir.path().to_str().unwrap();

    let initial = git_jet::repo_stamp(root).expect("stamp for a plain repository");
    assert!(!initial.is_empty());

    // Staging touches the index -> the stamp must move.
    write(dir.path(), "more.txt", "content\n");
    git(dir.path(), &["add", "more.txt"]);
    let after_stage = git_jet::repo_stamp(root).unwrap();
    assert_ne!(initial, after_stage, "staging must change the stamp");

    // Nothing happens -> the stamp is stable.
    let repeat = git_jet::repo_stamp(root).unwrap();
    assert_eq!(after_stage, repeat, "stamp must be stable without changes");

    // A commit moves HEAD's target ref -> the stamp moves again.
    git(dir.path(), &["commit", "-q", "-m", "second"]);
    assert_ne!(after_stage, git_jet::repo_stamp(root).unwrap());

    // A non-repository path has no stamp.
    let plain = tempfile::tempdir().unwrap();
    assert!(git_jet::repo_stamp(plain.path().to_str().unwrap()).is_none());
}

/* ============================ rebase / bisect / worktrees ================== */

#[test]
fn rebase_onto_moves_the_branch_on_top_of_the_target() {
    let dir = init_repo();
    let mut client = client(&dir);

    git(dir.path(), &["switch", "-q", "-c", "feature"]);
    let feature_tip = commit_change(&dir, "feature.txt", "feature\n", "feature work");
    git(dir.path(), &["switch", "-q", "main"]);
    commit_change(&dir, "main.txt", "main side\n", "main side commit");
    git(dir.path(), &["switch", "-q", "feature"]);

    client.rebase_onto("main").unwrap();

    let log = git(dir.path(), &["log", "--format=%s"]);
    assert!(log.contains("feature work"), "{log}");
    assert!(log.contains("main side commit"), "{log}");
    assert!(log.contains("initial commit"), "{log}");
    let merges = git(dir.path(), &["log", "--merges", "--oneline"]);
    assert!(merges.trim().is_empty(), "rebase must not create merges: {merges}");
    // Linearization: the replayed feature commit sits directly on main's tip.
    assert_eq!(
        git(dir.path(), &["rev-parse", "HEAD~1"]).trim(),
        git(dir.path(), &["rev-parse", "main"]).trim(),
        "the feature commit must replay on top of main"
    );
    // The feature file stays because the replayed commit re-applies it.
    assert_eq!(fs::read_to_string(dir.path().join("feature.txt")).unwrap(), "feature\n");

    let report = client.status().unwrap();
    assert!(!report.rebase_in_progress);
    assert!(report.conflicts().is_empty());

    // Option-like targets are rejected before git runs.
    assert!(client.rebase_onto("--force").is_err());
    assert!(client.rebase_onto("").is_err());
}

#[test]
fn rebase_conflict_can_be_resolved_continued_or_aborted() {
    let dir = init_repo();
    let mut client = client(&dir);

    git(dir.path(), &["switch", "-q", "-c", "feature"]);
    let feature_tip = commit_change(&dir, "a.txt", "feature side\n", "feature side changed");
    git(dir.path(), &["switch", "-q", "main"]);
    commit_change(&dir, "a.txt", "main side\n", "main side changed");
    git(dir.path(), &["switch", "-q", "feature"]);

    let error = client.rebase_onto("main").unwrap_err();
    assert!(error.contains("CONFLICT"), "{error}");

    let report = client.status().unwrap();
    assert!(report.rebase_in_progress, "the rebase must be detectable");
    assert_eq!(report.conflicts().len(), 1);

    // Abort restores the pre-rebase branch tip.
    client.rebase_abort().unwrap();
    let report = client.status().unwrap();
    assert!(!report.rebase_in_progress);
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), feature_tip);
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "feature side\n");

    // Retry, resolve the conflict through the merge editor and continue.
    client.rebase_onto("main").unwrap_err();
    client.write_merge_result("a.txt", "resolved\n").unwrap();
    client.rebase_continue().unwrap();

    let report = client.status().unwrap();
    assert!(!report.rebase_in_progress);
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "resolved\n");
    let log = git(dir.path(), &["log", "--format=%s"]);
    assert!(log.contains("feature side changed"), "{log}");
    assert!(log.contains("main side changed"), "{log}");
    assert!(log.contains("initial commit"), "{log}");
}

#[test]
fn pull_rebase_replays_local_commits_on_top_of_the_upstream() {
    let (holder, _client_dir) = clone_of_bare_remote();
    let remote_url = holder.path().join("origin.git").to_string_lossy().into_owned();
    let work_dir = tempfile::tempdir().unwrap();
    git(
        work_dir.path(),
        &["clone", "-q", "-c", "user.name=Git Jet Tests", "-c", "user.email=git-jet@example.test",
          remote_url.as_str(), work_dir.path().join("worktree").to_str().unwrap()],
    );
    let worktree = work_dir.path().join("worktree");
    test_git_config(&worktree);
    let mut client = GitClient::new(StdExecutor, worktree.to_str().unwrap().to_string());

    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    client.push().unwrap();

    // The remote advances from a second clone.
    let second_dir = tempfile::tempdir().unwrap();
    git(
        second_dir.path(),
        &["clone", "-q", "-c", "user.name=Git Jet Tests", "-c", "user.email=git-jet@example.test",
          remote_url.as_str(), second_dir.path().join("worktree").to_str().unwrap()],
    );
    let second = second_dir.path().join("worktree");
    write(&second, "b.txt", "remote side\n");
    git(&second, &["add", "--all"]);
    git(&second, &["commit", "-q", "-m", "on the remote"]);
    git(&second, &["push", "-q", "origin", "main"]);

    // A local commit that diverges from the upstream.
    write(&worktree, "c.txt", "local side\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "local side"]);

    client.pull_with_options(None, None, false, true).unwrap();

    let log = git(&worktree, &["log", "--format=%s"]);
    assert!(log.starts_with("local side"), "{log}");
    assert!(log.contains("on the remote"), "{log}");
    let merges = git(&worktree, &["log", "--merges", "--oneline"]);
    assert!(merges.trim().is_empty(), "pull --rebase must not merge: {merges}");
}

#[test]
fn cherry_pick_conflict_can_be_aborted() {
    let dir = init_repo();
    let mut client = client(&dir);

    git(dir.path(), &["switch", "-q", "-c", "source"]);
    let picked = commit_change(&dir, "a.txt", "source side\n", "source side changed");
    git(dir.path(), &["switch", "-q", "main"]);
    let head_before = commit_change(&dir, "a.txt", "main side\n", "main side changed");

    let error = client.cherry_pick_commit(&picked).unwrap_err();
    assert!(error.contains("CONFLICT"), "{error}");
    let report = client.status().unwrap();
    assert!(report.cherry_pick_in_progress);

    client.cherry_pick_abort().unwrap();
    let report = client.status().unwrap();
    assert!(!report.cherry_pick_in_progress);
    assert!(report.conflicts().is_empty());
    assert_eq!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), head_before);
}

#[test]
fn revert_conflict_can_be_resolved_and_continued_or_aborted() {
    let dir = init_repo();
    let mut client = client(&dir);
    commit_change(&dir, "a.txt", "one\n", "first change");
    let reverted = commit_change(&dir, "a.txt", "two\n", "second change");
    commit_change(&dir, "a.txt", "three\n", "third change");

    // Reverting the middle commit while HEAD carries "three" conflicts.
    let error = client.revert_commit(&reverted).unwrap_err();
    assert!(error.contains("CONFLICT"), "{error}");
    let report = client.status().unwrap();
    assert!(report.revert_in_progress);

    client.revert_abort().unwrap();
    let report = client.status().unwrap();
    assert!(!report.revert_in_progress);
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "three\n");
    let head_before = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();

    // Retry and resolve: the reverted content is the pre-change state.
    client.revert_commit(&reverted).unwrap_err();
    client.write_merge_result("a.txt", "one\n").unwrap();
    client.revert_continue().unwrap();
    let report = client.status().unwrap();
    assert!(!report.revert_in_progress);
    assert_eq!(fs::read_to_string(dir.path().join("a.txt")).unwrap(), "one\n");
    let count = git(dir.path(), &["rev-list", "--count", "HEAD"]).trim().to_string();
    assert_eq!(count, "5", "the revert creates one extra commit");
    assert_ne!(git(dir.path(), &["rev-parse", "HEAD"]).trim(), head_before);
}

#[test]
fn fetch_prune_removes_gone_remote_branches() {
    let (holder, client_dir) = clone_of_bare_remote();
    let worktree = client_dir.path().join("worktree");
    test_git_config(&worktree);
    let mut client = GitClient::new(StdExecutor, worktree.to_str().unwrap().to_string());

    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    client.push().unwrap();

    // Push a second branch, then delete it on the remote side directly.
    // The local remote-tracking ref origin/temp survives until a pruned
    // fetch (a `push --delete` would clean it up immediately).
    git(&worktree, &["switch", "-q", "-c", "temp"]);
    git(&worktree, &["commit", "-q", "--allow-empty", "-m", "temp work"]);
    client
        .push_with_options(Some("origin"), Some("temp"), false, true)
        .unwrap();
    git(&holder.path().join("origin.git"), &["branch", "-D", "temp"]);
    let remotes = git(&worktree, &["branch", "-r", "--format=%(refname:short)"]);
    assert!(remotes.contains("origin/temp"), "{remotes}");

    client.fetch_with_prune(true, true).unwrap();
    let remotes = git(&worktree, &["branch", "-r", "--format=%(refname:short)"]);
    assert!(!remotes.contains("origin/temp"), "{remotes}");
    assert!(remotes.contains("origin/main"), "{remotes}");
}

#[test]
fn bisect_marks_commits_and_reports_the_bad_one() {
    let dir = init_repo();
    let mut client = client(&dir);
    let good = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
    commit_change(&dir, "a.txt", "broken\n", "introduce the bug");
    let bad = commit_change(&dir, "a.txt", "more broken\n", "worsen the bug");

    client.bisect_start(Some("HEAD"), Some(&good)).unwrap();

    let report = client.status().unwrap();
    assert!(report.bisect_in_progress);
    assert!(
        matches!(report.branch.head, git_jet::core::Head::Detached),
        "bisect checks out candidate commits"
    );

    // Two bad commits after the good one: git checks out the middle one.
    let middle = git(dir.path(), &["rev-parse", "HEAD"]).trim().to_string();
    assert_ne!(middle, bad, "bisect must test the middle, not the tip");
    assert_ne!(middle, good, "bisect must test a candidate, not the known-good");

    client.bisect_mark("good", None).unwrap();
    // The final mark names the culprit; modern git resets automatically.
    if client.status().unwrap().bisect_in_progress {
        client.bisect_reset().unwrap();
    }
    let report = client.status().unwrap();
    assert!(!report.bisect_in_progress);

    // The transcript keeps the whole session; reset returned to the branch.
    let log = git(dir.path(), &["log", "--format=%s", "-1"]);
    assert!(log.contains("worsen the bug"), "{log}");

    // Validation: unknown kinds and option-like revisions never reach git.
    assert!(client.bisect_mark("banana", None).is_err());
    assert!(client.bisect_mark("good", Some("-f")).is_err());
    assert!(client.bisect_start(Some("--rebase"), None).is_err());
}

#[test]
fn worktrees_can_be_added_listed_and_removed() {
    let dir = init_repo();
    let mut client = client(&dir);

    let linked = client.list_worktrees().unwrap();
    assert_eq!(linked.len(), 1);
    assert!(linked[0].branch.as_deref() == Some("main"), "{linked:?}");
    assert!(!linked[0].bare);

    // Add a linked worktree with a new branch.
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("linked").to_string_lossy().replace('\\', "/");
    client.add_worktree(&path, Some("topic"), None).unwrap();

    let worktrees = client.list_worktrees().unwrap();
    assert_eq!(worktrees.len(), 2);
    let topic = worktrees
        .iter()
        .find(|worktree| worktree.path.ends_with("/linked"))
        .expect("the new worktree is listed");
    assert_eq!(topic.branch.as_deref(), Some("topic"));
    let branches = git(dir.path(), &["branch", "--format=%(refname:short)"]);
    assert!(branches.contains("topic"), "{branches}");

    // Remove it again; prune is a no-op afterwards.
    client.remove_worktree(&path, false).unwrap();
    let worktrees = client.list_worktrees().unwrap();
    assert_eq!(worktrees.len(), 1);
    client.prune_worktrees().unwrap();

    // Validation: empty and option-like paths never reach git.
    assert!(client.add_worktree("", None, None).is_err());
    assert!(client.add_worktree("-f", None, None).is_err());
    assert!(client.remove_worktree("--all", false).is_err());
}

#[test]
fn rebase_in_progress_is_detected_for_pull_rebase_conflicts_too() {
    let (holder, _client_dir) = clone_of_bare_remote();
    let remote_url = holder.path().join("origin.git").to_string_lossy().into_owned();
    let work_dir = tempfile::tempdir().unwrap();
    git(
        work_dir.path(),
        &["clone", "-q", "-c", "user.name=Git Jet Tests", "-c", "user.email=git-jet@example.test",
          remote_url.as_str(), work_dir.path().join("worktree").to_str().unwrap()],
    );
    let worktree = work_dir.path().join("worktree");
    test_git_config(&worktree);
    let mut pull_client = GitClient::new(StdExecutor, worktree.to_str().unwrap().to_string());

    write(&worktree, "a.txt", "one\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "initial"]);
    pull_client.push().unwrap();

    let second_dir = tempfile::tempdir().unwrap();
    git(
        second_dir.path(),
        &["clone", "-q", "-c", "user.name=Git Jet Tests", "-c", "user.email=git-jet@example.test",
          remote_url.as_str(), second_dir.path().join("worktree").to_str().unwrap()],
    );
    let second = second_dir.path().join("worktree");
    write(&second, "a.txt", "remote side\n");
    git(&second, &["add", "--all"]);
    git(&second, &["commit", "-q", "-m", "on the remote"]);
    git(&second, &["push", "-q", "origin", "main"]);

    write(&worktree, "a.txt", "local side\n");
    git(&worktree, &["add", "--all"]);
    git(&worktree, &["commit", "-q", "-m", "local side"]);

    let error = pull_client.pull_with_options(None, None, false, true).unwrap_err();
    assert!(error.contains("CONFLICT"), "{error}");
    let report = pull_client.status().unwrap();
    assert!(report.rebase_in_progress, "pull --rebase conflicts must show the banner: {report:?}");

    pull_client.rebase_abort().unwrap();
    let report = pull_client.status().unwrap();
    assert!(!report.rebase_in_progress);
}
