//! Platform-independent Git Jet core.
//!
//! The Tauri commands and the host tests adapt the process APIs to the
//! [`GitExecutor`] trait defined here. Keeping the rest in this module means
//! the same parsing, rendering and command orchestration can be tested on the
//! host.

pub mod client;
pub mod log;
pub mod model;
pub mod oplog;
pub mod render;
pub mod request;
pub mod status;

pub use client::{clone_repository, GitClient};
pub use log::parse_log_nodes;
pub use model::{
    Branch, BranchInfo, ChangedFile, CmdOutput, Commit, CommitDetail, CommitNode, EntryKind,
    GitExecutor, Head, LogFilter, RemoteEntry, Rendered, RenderedSection, StashEntry, StatusEntry,
    StatusReport, TagEntry, WorktreeEntry,
};
pub use request::{
    argument_completions, parse_request, ArgumentCompletion, DiffMode, GitRequest, RollbackLevel,
    StashAction,
};
