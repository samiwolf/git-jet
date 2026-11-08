//! Integration tests against real temporary Git repositories.

use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use git_jet::core::{
    CmdOutput, DiffMode, GitClient, GitExecutor, GitRequest, Rendered, RollbackLevel, StashAction,
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
