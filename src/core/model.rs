//! Shared data types for the Git Jet core.
//!
//! Everything in `core` is platform independent. The Tauri desktop
//! application and the tests supply a [`GitExecutor`] backed by
//! `std::process`.

use std::ops::Range;

/// Output of a single `git` invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CmdOutput {
    /// Exit status. `None` means the process was terminated by a signal.
    pub status: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CmdOutput {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }

    pub fn stdout_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Runs `git` in a directory.
///
/// Arguments are passed verbatim (never through a shell), so paths and commit
/// messages containing spaces or quotes are safe.
pub trait GitExecutor {
    fn exec(&mut self, root: &str, args: &[String]) -> Result<CmdOutput, String>;

    /// Runs `git` piping `stdin` into the process, e.g. a patch for
    /// `git apply`. Required so executors cannot silently drop input.
    fn exec_stdin(
        &mut self,
        root: &str,
        args: &[String],
        stdin: &[u8],
    ) -> Result<CmdOutput, String>;
}

/// A labelled byte range in the rendered output.
///
/// Zed draws these ranges as creases in the Assistant context editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedSection {
    pub range: Range<usize>,
    pub label: String,
}

/// Text plus sections returned by a slash command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Rendered {
    pub text: String,
    pub sections: Vec<RenderedSection>,
}

impl Rendered {
    pub fn message(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            sections: Vec::new(),
        }
    }

    /// The text covered by `section`.
    pub fn section_text(&self, section: &RenderedSection) -> &str {
        &self.text[section.range.clone()]
    }
}

/// The position of `HEAD`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Head {
    /// A normal local branch.
    Branch(String),
    /// Detached HEAD.
    Detached,
    /// An unborn branch, i.e. a repository without any commits.
    Initial(String),
}

impl Head {
    pub fn name(&self) -> &str {
        match self {
            Head::Branch(name) | Head::Initial(name) => name,
            Head::Detached => "HEAD",
        }
    }
}

/// Branch tracking information reported by `git status --branch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BranchInfo {
    pub head: Head,
    pub oid: Option<String>,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
}

impl Default for BranchInfo {
    fn default() -> Self {
        Self {
            head: Head::Branch("HEAD".to_string()),
            oid: None,
            upstream: None,
            ahead: 0,
            behind: 0,
        }
    }
}

impl BranchInfo {
    /// `main ↑2 ↓1`, `detached at 3f2a1b9`, or `main (no commits)`.
    pub fn display(&self) -> String {
        match &self.head {
            Head::Branch(name) => {
                let mut value = name.clone();
                if self.ahead > 0 {
                    value.push_str(&format!(" \u{2191}{}", self.ahead));
                }
                if self.behind > 0 {
                    value.push_str(&format!(" \u{2193}{}", self.behind));
                }
                value
            }
            Head::Initial(name) => format!("{name} (no commits)"),
            Head::Detached => match &self.oid {
                Some(oid) if !oid.is_empty() => format!("detached at {}", short_oid(oid)),
                _ => "detached".to_string(),
            },
        }
    }

    pub fn tracking(&self) -> String {
        match (&self.upstream, &self.head) {
            (Some(upstream), _) => upstream.clone(),
            (None, Head::Initial(_)) => "no upstream".to_string(),
            (None, _) => "no upstream".to_string(),
        }
    }
}

fn short_oid(oid: &str) -> &str {
    oid.get(..7).unwrap_or(oid)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Changed,
    Renamed,
    Copied,
    Unmerged,
    Untracked,
    Ignored,
}

/// One path from `git status --porcelain=v2`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusEntry {
    pub path: String,
    /// Original path for renames and copies.
    pub orig_path: Option<String>,
    /// Index (staged) status character, e.g. `M`, `A`, `D`, `R`, `.`.
    pub index: char,
    /// Worktree (unstaged) status character, e.g. `M`, `D`, `.`.
    pub worktree: char,
    pub kind: EntryKind,
}

impl StatusEntry {
    pub fn is_staged(&self) -> bool {
        !matches!(
            self.kind,
            EntryKind::Untracked | EntryKind::Ignored | EntryKind::Unmerged
        ) && self.index != '.'
            && self.index != ' '
    }

    pub fn is_unstaged(&self) -> bool {
        !matches!(
            self.kind,
            EntryKind::Untracked | EntryKind::Ignored | EntryKind::Unmerged
        ) && self.worktree != '.'
            && self.worktree != ' '
    }

    pub fn is_conflict(&self) -> bool {
        self.kind == EntryKind::Unmerged
    }

    pub fn is_untracked(&self) -> bool {
        self.kind == EntryKind::Untracked
    }

    pub fn is_ignored(&self) -> bool {
        self.kind == EntryKind::Ignored
    }

    pub fn display_path(&self) -> String {
        match &self.orig_path {
            Some(orig) if self.kind == EntryKind::Renamed => {
                format!("{} <- {}", self.path, orig)
            }
            Some(orig) if self.kind == EntryKind::Copied => {
                format!("{} <- {}", self.path, orig)
            }
            _ => self.path.clone(),
        }
    }
}

/// Parsed `git status --porcelain=v2 --branch`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusReport {
    pub branch: BranchInfo,
    pub entries: Vec<StatusEntry>,
    pub stash_count: usize,
    pub last_commit: Option<Commit>,
}

impl StatusReport {
    pub fn staged(&self) -> Vec<&StatusEntry> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.is_staged()).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn unstaged(&self) -> Vec<&StatusEntry> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.is_unstaged()).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn untracked(&self) -> Vec<&StatusEntry> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.is_untracked()).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn conflicts(&self) -> Vec<&StatusEntry> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.is_conflict()).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn ignored(&self) -> Vec<&StatusEntry> {
        let mut entries: Vec<_> = self.entries.iter().filter(|e| e.is_ignored()).collect();
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        entries
    }

    pub fn change_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| !e.is_ignored() && !e.is_untracked())
            .count()
            + self.untracked().len()
    }

    pub fn is_clean(&self) -> bool {
        self.entries.iter().all(|e| e.is_ignored())
    }
}

/// One commit from the custom `git log` format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub short_hash: String,
    pub author: String,
    pub date: String,
    pub subject: String,
}

/// One commit from `git log` including parents and ref decorations.
///
/// `parents` are full parent hashes ordered as in `%P`, and `refs` carries the
/// `{}` decorations without the surrounding parentheses — `HEAD -> main`,
/// `origin/main`, tags and so on. Used by the visual commit graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitNode {
    pub commit: Commit,
    pub parents: Vec<String>,
    pub refs: Vec<String>,
}

/// One local or remote branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub current: bool,
    pub upstream: Option<String>,
    pub ahead: usize,
    pub behind: usize,
    pub gone: bool,
    pub hash: String,
    pub date: String,
    pub remote: bool,
}

/// One stash entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    pub selector: String,
    pub message: String,
    pub date: String,
}
