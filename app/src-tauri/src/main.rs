//! Tauri backend for the Git Jet desktop application.
//!
//! Commands adapt the engine crate (`git_jet`) to the frontend: JSON DTOs in,
//! raw git output or engine errors out. Mutating commands let the frontend
//! refresh its views afterwards.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Mutex;

use git_jet::core::{
    Branch, CommitNode, DiffMode, EntryKind, GitClient, Head, RollbackLevel, StashEntry,
    StatusEntry, StatusReport,
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
    state: State<AppState>,
) -> Result<Vec<CommitNodeDto>, String> {
    let count = count.unwrap_or(100).max(1).min(500) as usize;
    let all = all.unwrap_or(false);
    with_engine(&state, |client| {
        let nodes = client.log_nodes(count, all)?;
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
            push,
            checkout_commit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Git Jet");
}
