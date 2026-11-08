//! Tauri backend for the Git Jet desktop application.
//!
//! Commands adapt the engine crate (`git_jet`) to the frontend: JSON DTOs in,
//! raw git output or engine errors out. Mutating commands let the frontend
//! refresh its views afterwards.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use git_jet::core::{
    Branch, ChangedFile, CommitDetail, CommitNode, DiffMode, EntryKind, GitClient, Head, LogFilter,
    RollbackLevel, StashEntry, StatusEntry, StatusReport, TagEntry,
};
use git_jet::{Engine, StdExecutor};
use serde::Serialize;
use tauri::State;

/// The currently opened repository, if any.
struct AppState {
    root: Mutex<Option<String>>,
}

impl AppState {
    fn path(&self) -> Result<String, String> {
        self.root
            .lock()
            .map_err(|_| "State poisoned".to_string())?
            .clone()
            .ok_or_else(|| "No repository is open.".to_string())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoInfo {
    root: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FileDto {
    path: String,
    display_path: String,
    index: char,
    worktree: char,
    kind: String,
}

impl FileDto {
    fn new(entry: &StatusEntry) -> Self {
        Self {
            path: entry.path.clone(),
            display_path: entry.display_path(),
            index: entry.index,
            worktree: entry.worktree,
            kind: kind_name(entry.kind),
        }
    }
}

fn kind_name(kind: EntryKind) -> String {
    match kind {
        EntryKind::Changed => "changed",
        EntryKind::Renamed => "renamed",
        EntryKind::Copied => "copied",
        EntryKind::Unmerged => "unmerged",
        EntryKind::Untracked => "untracked",
        EntryKind::Ignored => "ignored",
    }
    .to_string()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchHeadDto {
    name: String,
    display: String,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    detached: bool,
    initial: bool,
    oid: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitDto {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    subject: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StatusDto {
    branch: BranchHeadDto,
    staged: Vec<FileDto>,
    unstaged: Vec<FileDto>,
    untracked: Vec<FileDto>,
    conflicts: Vec<FileDto>,
    stash_count: usize,
    last_commit: Option<CommitDto>,
    merge_in_progress: bool,
    cherry_pick_in_progress: bool,
    revert_in_progress: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitNodeDto {
    hash: String,
    short_hash: String,
    author: String,
    date: String,
    subject: String,
    parents: Vec<String>,
    refs: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchDto {
    name: String,
    current: bool,
    upstream: Option<String>,
    ahead: usize,
    behind: usize,
    gone: bool,
    hash: String,
    date: String,
    remote: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchesDto {
    local: Vec<BranchDto>,
    remote: Vec<BranchDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StashDto {
    selector: String,
    message: String,
    date: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChangedFileDto {
    status: String,
    path: String,
    display_path: String,
    orig_path: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CommitDetailDto {
    hash: String,
    short_hash: String,
    author: String,
    email: String,
    date: String,
    date_iso: String,
    subject: String,
    parents: Vec<String>,
    refs: Vec<String>,
    files: Vec<ChangedFileDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct TagDto {
    name: String,
    hash: String,
    date: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct OutgoingDto {
    commits: Vec<CommitDto>,
    has_upstream: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BranchCompareDto {
    /// Commits on HEAD that the other branch does not have.
    ours: Vec<CommitDto>,
    /// Commits on the other branch that HEAD does not have.
    theirs: Vec<CommitDto>,
}

fn map_branch(branch: &Branch) -> BranchDto {
    BranchDto {
        name: branch.name.clone(),
        current: branch.current,
        upstream: branch.upstream.clone(),
        ahead: branch.ahead,
        behind: branch.behind,
        gone: branch.gone,
        hash: branch.hash.clone(),
        date: branch.date.clone(),
        remote: branch.remote,
    }
}

fn map_commit(commit: &git_jet::core::Commit) -> CommitDto {
    CommitDto {
        hash: commit.hash.clone(),
        short_hash: commit.short_hash.clone(),
        author: commit.author.clone(),
        date: commit.date.clone(),
        subject: commit.subject.clone(),
    }
}

fn map_status(report: &StatusReport) -> StatusDto {
    let head = &report.branch;
    let (detached, initial) = match head.head {
        Head::Branch(_) => (false, false),
        Head::Detached => (true, false),
        Head::Initial(_) => (false, true),
    };
    StatusDto {
        branch: BranchHeadDto {
            name: head.head.name().to_string(),
            display: head.display(),
            upstream: head.upstream.clone(),
            ahead: head.ahead,
            behind: head.behind,
            detached,
            initial,
            oid: head.oid.clone(),
        },
        staged: report.staged().iter().map(|e| FileDto::new(e)).collect(),
        unstaged: report.unstaged().iter().map(|e| FileDto::new(e)).collect(),
        untracked: report.untracked().iter().map(|e| FileDto::new(e)).collect(),
        conflicts: report.conflicts().iter().map(|e| FileDto::new(e)).collect(),
        stash_count: report.stash_count,
        last_commit: report.last_commit.as_ref().map(map_commit),
        merge_in_progress: report.merge_in_progress,
        cherry_pick_in_progress: report.cherry_pick_in_progress,
        revert_in_progress: report.revert_in_progress,
    }
}

fn with_engine<T>(
    state: &State<AppState>,
    run: impl FnOnce(&mut GitClient<StdExecutor>) -> Result<T, String>,
) -> Result<T, String> {
    let path = state.path()?;
    let mut engine = Engine::new(path);
    run(engine.client())
}

fn parse_mode(mode: &str) -> DiffMode {
    match mode {
        "staged" => DiffMode::Staged,
        "all" => DiffMode::All,
        _ => DiffMode::Unstaged,
    }
}

#[tauri::command]
fn open_repository(path: Option<String>, state: State<AppState>) -> Result<RepoInfo, String> {
    let picked = match path {
        Some(path) => path,
        None => {
            return Err("A directory path is required.".to_string());
        }
    };
    if picked.trim().is_empty() {
        return Err("Empty path.".to_string());
    }
    let mut engine = Engine::new(picked);
    let root = engine.client().root()?;
    *state.root.lock().map_err(|_| "State poisoned")? = Some(root.clone());
    let name = std::path::Path::new(&root)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| root.clone());
    Ok(RepoInfo { root, name })
}

#[tauri::command]
fn current_repository(state: State<AppState>) -> Result<Option<String>, String> {
    Ok(state.root.lock().map_err(|_| "State poisoned")?.clone())
}

#[tauri::command]
fn close_repository(state: State<AppState>) -> Result<(), String> {
    *state.root.lock().map_err(|_| "State poisoned")? = None;
    Ok(())
}

#[tauri::command]
fn get_status(state: State<AppState>) -> Result<StatusDto, String> {
    with_engine(&state, |client| {
        let report = client.status()?;
        Ok(map_status(&report))
    })
}

#[tauri::command]
fn get_log(
    count: Option<u32>,
    all: Option<bool>,
    text: Option<String>,
    author: Option<String>,
    path: Option<String>,
    state: State<AppState>,
) -> Result<Vec<CommitNodeDto>, String> {
    let count = count.unwrap_or(100).max(1).min(500) as usize;
    let all = all.unwrap_or(false);
    let text = text.map(|value| value.trim().to_string()).filter(|v| !v.is_empty());
    let author = author.map(|value| value.trim().to_string()).filter(|v| !v.is_empty());
    let path = path.map(|value| value.trim().to_string()).filter(|v| !v.is_empty());
    let filter = LogFilter {
        text,
        author,
        path,
        branch: None,
    };
    with_engine(&state, |client| {
        let nodes = client.log_nodes(count, all, &filter)?;
        Ok(nodes
            .into_iter()
            .map(|node: CommitNode| CommitNodeDto {
                hash: node.commit.hash.clone(),
                short_hash: node.commit.short_hash.clone(),
                author: node.commit.author.clone(),
                date: node.commit.date.clone(),
                subject: node.commit.subject.clone(),
                parents: node.parents,
                refs: node.refs,
            })
            .collect())
    })
}

#[tauri::command]
fn get_branches(state: State<AppState>) -> Result<BranchesDto, String> {
    with_engine(&state, |client| {
        let (local, remote) = client.list_branches()?;
        Ok(BranchesDto {
            local: local.iter().map(map_branch).collect(),
            remote: remote.iter().map(map_branch).collect(),
        })
    })
}

#[tauri::command]
fn get_stashes(state: State<AppState>) -> Result<Vec<StashDto>, String> {
    with_engine(&state, |client| {
        let stashes: Vec<StashEntry> = client.stash_list()?;
        Ok(stashes
            .into_iter()
            .map(|stash| StashDto {
                selector: stash.selector,
                message: stash.message,
                date: stash.date,
            })
            .collect())
    })
}

#[tauri::command]
fn get_diff(
    mode: Option<String>,
    paths: Option<Vec<String>>,
    state: State<AppState>,
) -> Result<String, String> {
    let mode = parse_mode(mode.as_deref().unwrap_or("unstaged"));
    let paths = paths.unwrap_or_default();
    with_engine(&state, |client| client.diff_text(&mode, &paths))
}

#[tauri::command]
fn get_commit(sha: Option<String>, state: State<AppState>) -> Result<String, String> {
    let sha = sha.unwrap_or_else(|| "HEAD".to_string());
    with_engine(&state, |client| client.show_text(&sha))
}

#[tauri::command]
fn get_blame(path: Option<String>, state: State<AppState>) -> Result<String, String> {
    let path = path.ok_or_else(|| "A file path is required.".to_string())?;
    with_engine(&state, |client| client.blame_text(&path))
}

#[tauri::command]
fn stage(paths: Option<Vec<String>>, state: State<AppState>) -> Result<String, String> {
    let paths = paths.unwrap_or_default();
    with_engine(&state, |client| client.stage(&paths))
}

#[tauri::command]
fn unstage(paths: Option<Vec<String>>, state: State<AppState>) -> Result<String, String> {
    let paths = paths.unwrap_or_default();
    with_engine(&state, |client| client.unstage(&paths))
}

#[tauri::command]
fn rollback_files(
    paths: Option<Vec<String>>,
    level: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let paths = paths.unwrap_or_default();
    if paths.is_empty() {
        return Err("No files selected for rollback.".to_string());
    }
    let level = match level.as_deref() {
        Some("staged") => RollbackLevel::Staged,
        Some("untracked") => RollbackLevel::Untracked,
        _ => RollbackLevel::Unstaged,
    };
    with_engine(&state, |client| client.rollback_files(&paths, level))
}

#[tauri::command]
fn discard_hunk(
    patch: Option<String>,
    staged: Option<bool>,
    state: State<AppState>,
) -> Result<String, String> {
    let patch = patch.unwrap_or_default();
    if patch.trim().is_empty() {
        return Err("The hunk patch is empty.".to_string());
    }
    with_engine(&state, |client| {
        client.rollback_hunk(&patch, staged.unwrap_or(false))
    })
}

#[tauri::command]
fn get_worktree_file(file: Option<String>, state: State<AppState>) -> Result<String, String> {
    let file = file.ok_or_else(|| "A file path is required.".to_string())?;
    with_engine(&state, |client| client.worktree_file_content(&file))
}

#[tauri::command]
fn write_merge_result(
    file: Option<String>,
    content: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let file = file.ok_or_else(|| "A file path is required.".to_string())?;
    let content = content.ok_or_else(|| "The merged content is required.".to_string())?;
    with_engine(&state, |client| client.write_merge_result(&file, &content))
}

#[tauri::command]
fn commit(
    message: Option<String>,
    amend: Option<bool>,
    state: State<AppState>,
) -> Result<String, String> {
    let message = message
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if message.is_empty() {
        return Err("Commit message is empty.".to_string());
    }
    let amend = amend.unwrap_or(false);
    with_engine(&state, |client| client.commit(&message, amend))
}

#[tauri::command]
fn switch_branch(branch: Option<String>, state: State<AppState>) -> Result<String, String> {
    let branch = branch.ok_or_else(|| "A branch name is required.".to_string())?;
    if branch.trim().is_empty() {
        return Err("Branch name is empty.".to_string());
    }
    with_engine(&state, |client| client.switch(&branch))
}

#[tauri::command]
fn create_branch(name: Option<String>, state: State<AppState>) -> Result<String, String> {
    let name = name.ok_or_else(|| "A branch name is required.".to_string())?;
    if name.trim().is_empty() {
        return Err("Branch name is empty.".to_string());
    }
    with_engine(&state, |client| client.create_branch(name.trim()))
}

#[tauri::command]
fn stash_push(message: Option<String>, state: State<AppState>) -> Result<String, String> {
    let message = message
        .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty());
    with_engine(&state, |client| client.stash_push(message.as_deref()))
}

#[tauri::command]
fn stash_apply(
    action: Option<String>,
    selector: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let command = match action.as_deref().unwrap_or("apply") {
        "pop" => "pop",
        "drop" => "drop",
        _ => "apply",
    };
    with_engine(&state, |client| {
        client.stash_apply(command, selector.as_deref())
    })
}

#[tauri::command]
fn fetch(all: Option<bool>, state: State<AppState>) -> Result<String, String> {
    let all = all.unwrap_or(false);
    with_engine(&state, |client| client.fetch(all))
}

#[tauri::command]
fn pull(state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.pull())
}

#[tauri::command]
fn checkout_commit(sha: String, state: State<AppState>) -> Result<String, String> {
    if sha.is_empty() || sha.starts_with('-') {
        return Err(format!("Invalid revision `{sha}`."));
    }
    with_engine(&state, |client| client.checkout_commit(&sha))
}

#[tauri::command]
fn push(state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.push())
}

#[tauri::command]
fn pull_options(
    remote: Option<String>,
    branch: Option<String>,
    ff_only: Option<bool>,
    state: State<AppState>,
) -> Result<String, String> {
    with_engine(&state, |client| {
        client.pull_with_options(
            remote.as_deref(),
            branch.as_deref(),
            ff_only.unwrap_or(false),
        )
    })
}

#[tauri::command]
fn push_options(
    remote: Option<String>,
    branch: Option<String>,
    force: Option<bool>,
    set_upstream: Option<bool>,
    state: State<AppState>,
) -> Result<String, String> {
    with_engine(&state, |client| {
        client.push_with_options(
            remote.as_deref(),
            branch.as_deref(),
            force.unwrap_or(false),
            set_upstream.unwrap_or(false),
        )
    })
}

#[tauri::command]
fn outgoing(state: State<AppState>) -> Result<OutgoingDto, String> {
    with_engine(&state, |client| {
        let (commits, has_upstream) = client.outgoing_commits(200)?;
        Ok(OutgoingDto {
            commits: commits.iter().map(map_commit).collect(),
            has_upstream,
        })
    })
}

#[tauri::command]
fn merge_branch(
    branch: Option<String>,
    no_ff: Option<bool>,
    state: State<AppState>,
) -> Result<String, String> {
    let branch = branch.ok_or_else(|| "A branch name is required.".to_string())?;
    with_engine(&state, |client| client.merge_branch(&branch, no_ff.unwrap_or(false)))
}

#[tauri::command]
fn merge_abort(state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.merge_abort())
}

#[tauri::command]
fn delete_branch(name: Option<String>, force: Option<bool>, state: State<AppState>) -> Result<String, String> {
    let name = name.ok_or_else(|| "A branch name is required.".to_string())?;
    with_engine(&state, |client| client.delete_branch(&name, force.unwrap_or(false)))
}

#[tauri::command]
fn delete_remote_branch(name: Option<String>, state: State<AppState>) -> Result<String, String> {
    let name = name.ok_or_else(|| "A remote branch name is required.".to_string())?;
    with_engine(&state, |client| client.delete_remote_branch(&name))
}

#[tauri::command]
fn rename_branch(
    old_name: Option<String>,
    new_name: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let old_name = old_name.ok_or_else(|| "The old branch name is required.".to_string())?;
    let new_name = new_name.ok_or_else(|| "The new branch name is required.".to_string())?;
    with_engine(&state, |client| client.rename_branch(&old_name, &new_name))
}

#[tauri::command]
fn checkout_new_branch(
    name: Option<String>,
    from_ref: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let name = name.ok_or_else(|| "A branch name is required.".to_string())?;
    let from_ref = from_ref.ok_or_else(|| "A start ref is required.".to_string())?;
    with_engine(&state, |client| client.checkout_new_branch_from(&name, &from_ref))
}

#[tauri::command]
fn set_upstream(
    branch: Option<String>,
    upstream: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let upstream = upstream.ok_or_else(|| "An upstream branch is required.".to_string())?;
    with_engine(&state, |client| client.set_upstream(branch.as_deref(), &upstream))
}

#[tauri::command]
fn unset_upstream(branch: Option<String>, state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.unset_upstream(branch.as_deref()))
}

#[tauri::command]
fn compare_branch(branch: Option<String>, state: State<AppState>) -> Result<BranchCompareDto, String> {
    let branch = branch.ok_or_else(|| "A branch name is required.".to_string())?;
    with_engine(&state, |client| {
        let (ours, theirs) = client.branch_compare(&branch)?;
        Ok(BranchCompareDto {
            ours: ours.iter().map(map_commit).collect(),
            theirs: theirs.iter().map(map_commit).collect(),
        })
    })
}

#[tauri::command]
fn get_tags(state: State<AppState>) -> Result<Vec<TagDto>, String> {
    with_engine(&state, |client| {
        let tags: Vec<TagEntry> = client.list_tags()?;
        Ok(tags
            .into_iter()
            .map(|tag| TagDto {
                name: tag.name,
                hash: tag.hash,
                date: tag.date,
            })
            .collect())
    })
}

#[tauri::command]
fn create_tag(
    name: Option<String>,
    rev: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let name = name.ok_or_else(|| "A tag name is required.".to_string())?;
    with_engine(&state, |client| client.create_tag(&name, rev.as_deref()))
}

#[tauri::command]
fn delete_tag(name: Option<String>, state: State<AppState>) -> Result<String, String> {
    let name = name.ok_or_else(|| "A tag name is required.".to_string())?;
    with_engine(&state, |client| client.delete_tag(&name))
}

#[tauri::command]
fn push_tags(state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.push_tags())
}

#[tauri::command]
fn revert_commit(sha: Option<String>, state: State<AppState>) -> Result<String, String> {
    let sha = sha.ok_or_else(|| "A commit hash is required.".to_string())?;
    with_engine(&state, |client| client.revert_commit(&sha))
}

#[tauri::command]
fn cherry_pick(sha: Option<String>, state: State<AppState>) -> Result<String, String> {
    let sha = sha.ok_or_else(|| "A commit hash is required.".to_string())?;
    with_engine(&state, |client| client.cherry_pick_commit(&sha))
}

#[tauri::command]
fn drop_commit(sha: Option<String>, state: State<AppState>) -> Result<String, String> {
    let sha = sha.ok_or_else(|| "A commit hash is required.".to_string())?;
    with_engine(&state, |client| client.drop_commit(&sha))
}

#[tauri::command]
fn undo_commit(state: State<AppState>) -> Result<String, String> {
    with_engine(&state, |client| client.undo_head_commit())
}

#[tauri::command]
fn reset_branch(
    sha: Option<String>,
    mode: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let sha = sha.ok_or_else(|| "A commit hash is required.".to_string())?;
    if sha.is_empty() || sha.starts_with('-') {
        return Err(format!("Invalid revision `{sha}`."));
    }
    with_engine(&state, |client| {
        client.reset_branch(&sha, mode.as_deref().unwrap_or("mixed"))
    })
}

#[tauri::command]
fn commit_detail(sha: Option<String>, state: State<AppState>) -> Result<CommitDetailDto, String> {
    let sha = sha.unwrap_or_else(|| "HEAD".to_string());
    with_engine(&state, |client| {
        let detail: CommitDetail = client.commit_detail(&sha)?;
        Ok(CommitDetailDto {
            hash: detail.hash.clone(),
            short_hash: detail.short_hash.clone(),
            author: detail.author.clone(),
            email: detail.email.clone(),
            date: detail.date.clone(),
            date_iso: detail.date_iso.clone(),
            subject: detail.subject.clone(),
            parents: detail.parents.clone(),
            refs: detail.refs.clone(),
            files: detail
                .files
                .iter()
                .map(|file: &ChangedFile| ChangedFileDto {
                    status: file.status.clone(),
                    path: file.path.clone(),
                    display_path: file.display(),
                    orig_path: file.orig_path.clone(),
                })
                .collect(),
        })
    })
}

#[tauri::command]
fn commit_file_diff(
    sha: Option<String>,
    path: Option<String>,
    state: State<AppState>,
) -> Result<String, String> {
    let sha = sha.ok_or_else(|| "A commit hash is required.".to_string())?;
    let path = path.ok_or_else(|| "A file path is required.".to_string())?;
    with_engine(&state, |client| client.commit_file_diff(&sha, &path))
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState {
            root: Mutex::new(None),
        })
        .invoke_handler(tauri::generate_handler![
            open_repository,
            current_repository,
            close_repository,
            get_status,
            get_log,
            get_branches,
            get_stashes,
            get_diff,
            get_commit,
            get_blame,
            stage,
            unstage,
            rollback_files,
            discard_hunk,
            commit,
            switch_branch,
            create_branch,
            stash_push,
            stash_apply,
            fetch,
            pull,
            pull_options,
            push,
            push_options,
            outgoing,
            merge_branch,
            merge_abort,
            get_worktree_file,
            write_merge_result,
            delete_branch,
            delete_remote_branch,
            rename_branch,
            checkout_new_branch,
            set_upstream,
            unset_upstream,
            compare_branch,
            get_tags,
            create_tag,
            delete_tag,
            push_tags,
            revert_commit,
            cherry_pick,
            drop_commit,
            undo_commit,
            reset_branch,
            commit_detail,
            commit_file_diff,
            checkout_commit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Git Jet");
}
