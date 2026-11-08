//! Parsing of `/git` arguments into concrete requests.
//!
//! A slash command receives its arguments already split by Zed. We keep the
//! model deliberately conservative: no shell is ever involved, and free-form
//! text (commit messages, stash messages) is joined with single spaces.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffMode {
    Unstaged,
    Staged,
    All,
}

/// Which side of a change a rollback targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RollbackLevel {
    /// Revert staged changes back to `HEAD` (index and worktree).
    Staged,
    /// Discard worktree changes, reverting the paths to their index state.
    Unstaged,
    /// Delete untracked files.
    Untracked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StashAction {
    List,
    Push { message: Option<String> },
    Pop { index: Option<String> },
    Apply { index: Option<String> },
    Drop { index: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitRequest {
    Dashboard,
    Status,
    Log {
        count: usize,
        all: bool,
    },
    Diff {
        mode: DiffMode,
        paths: Vec<String>,
    },
    Branches,
    Commit {
        message: String,
        amend: bool,
    },
    Stage {
        paths: Vec<String>,
    },
    Unstage {
        paths: Vec<String>,
    },
    Rollback {
        paths: Vec<String>,
        level: RollbackLevel,
    },
    Switch {
        branch: String,
    },
    CreateBranch {
        name: String,
    },
    Stash {
        action: StashAction,
    },
    Fetch {
        all: bool,
    },
    Pull {
        args: Vec<String>,
    },
    Push {
        args: Vec<String>,
    },
    Show {
        rev: String,
    },
    Blame {
        path: String,
    },
    Help,
}

/// A static argument completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArgumentCompletion {
    pub label: String,
    pub new_text: String,
    pub run_command: bool,
}

const DEFAULT_LOG_COUNT: usize = 30;
const MAX_LOG_COUNT: usize = 500;

pub fn parse_request(args: &[String]) -> Result<GitRequest, String> {
    let Some(subcommand) = args.first().map(|value| value.trim()) else {
        return Ok(GitRequest::Dashboard);
    };
    if subcommand.is_empty() {
        return Ok(GitRequest::Dashboard);
    }

    match subcommand.to_ascii_lowercase().as_str() {
        "status" | "st" | "changes" => Ok(GitRequest::Status),
        "log" | "history" | "hist" => parse_log(&args[1..]),
        "diff" | "d" => parse_diff(&args[1..]),
        "branches" | "branch-list" => Ok(GitRequest::Branches),
        "branch" => {
            if args.len() >= 2 {
                let name = args[1].trim();
                if name.starts_with('-') {
                    Err(format!(
                        "Invalid branch name `{name}`. Branch names cannot start with `-`."
                    ))
                } else {
                    Ok(GitRequest::CreateBranch {
                        name: name.to_string(),
                    })
                }
            } else {
                Ok(GitRequest::Branches)
            }
        }
        "commit" | "ci" => parse_commit(&args[1..]),
        "stage" | "add" => Ok(GitRequest::Stage {
            paths: clean_paths(&args[1..]),
        }),
        "unstage" | "reset" => Ok(GitRequest::Unstage {
            paths: clean_paths(&args[1..]),
        }),
        "rollback" | "discard" => parse_rollback(&args[1..]),
        "switch" | "checkout" | "co" => {
            let branch = args.get(1).map(|value| value.trim()).unwrap_or("");
            if branch.is_empty() {
                Err("Usage: `/git switch <branch>`".to_string())
            } else if branch.starts_with('-') {
                Err(format!(
                    "Invalid branch name `{branch}`. Branch names cannot start with `-`."
                ))
            } else {
                Ok(GitRequest::Switch {
                    branch: branch.to_string(),
                })
            }
        }
        "new-branch" | "create-branch" => {
            let name = args.get(1).map(|value| value.trim()).unwrap_or("");
            if name.is_empty() {
                Err("Usage: `/git new-branch <name>`".to_string())
            } else if name.starts_with('-') {
                Err(format!(
                    "Invalid branch name `{name}`. Branch names cannot start with `-`."
                ))
            } else {
                Ok(GitRequest::CreateBranch {
                    name: name.to_string(),
                })
            }
        }
        "stash" => parse_stash(&args[1..]),
        "fetch" => Ok(GitRequest::Fetch {
            all: args[1..]
                .iter()
                .any(|arg| arg == "--all" || arg.eq_ignore_ascii_case("all")),
        }),
        "pull" => Ok(GitRequest::Pull {
            args: clean_args(&args[1..]),
        }),
        "push" => Ok(GitRequest::Push {
            args: clean_args(&args[1..]),
        }),
        "show" => {
            let rev = args
                .get(1)
                .map(|value| value.trim())
                .filter(|value| !value.is_empty())
                .unwrap_or("HEAD");
            if rev.starts_with('-') {
                Err(format!(
                    "Invalid revision `{rev}`. Revisions cannot start with `-`."
                ))
            } else {
                Ok(GitRequest::Show {
                    rev: rev.to_string(),
                })
            }
        }
        "blame" | "annotate" => {
            let path = args[1..].join(" ").trim().to_string();
            if path.is_empty() {
                Err("Usage: `/git blame <path>`".to_string())
            } else {
                Ok(GitRequest::Blame { path })
            }
        }
        "help" | "?" => Ok(GitRequest::Help),
        other => Err(format!(
            "Unknown Git subcommand `{other}`. Run `/git help` to list commands."
        )),
    }
}

fn parse_log(args: &[String]) -> Result<GitRequest, String> {
    let mut count = DEFAULT_LOG_COUNT;
    let mut all = false;
    for arg in args {
        if arg.eq_ignore_ascii_case("all") {
            all = true;
        } else if let Ok(value) = arg.parse::<usize>() {
            if value == 0 || value > MAX_LOG_COUNT {
                return Err(format!("Log count must be between 1 and {MAX_LOG_COUNT}."));
            }
            count = value;
        } else if !arg.is_empty() {
            return Err(format!(
                "Unknown log option `{arg}`. Usage: `/git log [count] [all]`"
            ));
        }
    }
    Ok(GitRequest::Log { count, all })
}

fn parse_diff(args: &[String]) -> Result<GitRequest, String> {
    let mut mode = DiffMode::Unstaged;
    let mut paths = Vec::new();
    let mut index = 0;
    if let Some(first) = args.first() {
        match first.to_ascii_lowercase().as_str() {
            "staged" | "--staged" | "cached" | "--cached" => {
                mode = DiffMode::Staged;
                index = 1;
            }
            "unstaged" | "working" | "--unstaged" => {
                mode = DiffMode::Unstaged;
                index = 1;
            }
            "all" | "head" => {
                mode = DiffMode::All;
                index = 1;
            }
            _ => {}
        }
    }
    paths.extend(clean_paths(&args[index..]));
    Ok(GitRequest::Diff { mode, paths })
}

fn parse_commit(args: &[String]) -> Result<GitRequest, String> {
    let (amend, message_args) = match args.first().map(String::as_str) {
        Some("--amend") => (true, &args[1..]),
        _ => (false, args),
    };
    let message = message_args.join(" ").trim().trim_matches('"').to_string();
    if message.is_empty() {
        return Err(
            "Usage: `/git commit <message>` (add --amend to amend the last commit)".to_string(),
        );
    }
    Ok(GitRequest::Commit { message, amend })
}

fn parse_rollback(args: &[String]) -> Result<GitRequest, String> {
    let mut index = 0;
    let mut level = RollbackLevel::Unstaged;
    if let Some(first) = args.first() {
        match first.to_ascii_lowercase().as_str() {
            "staged" => {
                level = RollbackLevel::Staged;
                index = 1;
            }
            "unstaged" => {
                level = RollbackLevel::Unstaged;
                index = 1;
            }
            "untracked" => {
                level = RollbackLevel::Untracked;
                index = 1;
            }
            _ => {}
        }
    }
    let paths = clean_paths(&args[index..]);
    if paths.is_empty() {
        return Err(
            "Usage: `/git rollback [staged|unstaged|untracked] <paths>` (unstaged when omitted)"
                .to_string(),
        );
    }
    Ok(GitRequest::Rollback { paths, level })
}

fn parse_stash(args: &[String]) -> Result<GitRequest, String> {
    let Some(action) = args.first().map(|value| value.to_ascii_lowercase()) else {
        return Ok(GitRequest::Stash {
            action: StashAction::List,
        });
    };
    match action.as_str() {
        "list" | "ls" => Ok(GitRequest::Stash {
            action: StashAction::List,
        }),
        "push" | "save" => {
            let message = args[1..].join(" ");
            let message = message.trim().trim_matches('"');
            Ok(GitRequest::Stash {
                action: StashAction::Push {
                    message: if message.is_empty() {
                        None
                    } else {
                        Some(message.to_string())
                    },
                },
            })
        }
        "pop" => Ok(GitRequest::Stash {
            action: StashAction::Pop {
                index: stash_index(&args[1..])?,
            },
        }),
        "apply" => Ok(GitRequest::Stash {
            action: StashAction::Apply {
                index: stash_index(&args[1..])?,
            },
        }),
        "drop" => Ok(GitRequest::Stash {
            action: StashAction::Drop {
                index: stash_index(&args[1..])?,
            },
        }),
        other => Err(format!(
            "Unknown stash action `{other}`. Usage: `/git stash [list|push [message]|pop [index]|apply [index]|drop [index]]`"
        )),
    }
}

fn stash_index(args: &[String]) -> Result<Option<String>, String> {
    let Some(value) = args.first() else {
        return Ok(None);
    };
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    let digits = value
        .strip_prefix("stash@{")
        .and_then(|value| value.strip_suffix('}'))
        .unwrap_or(value);
    if digits.chars().all(|character| character.is_ascii_digit()) {
        Ok(Some(format!("stash@{{{digits}}}")))
    } else {
        Err(format!(
            "Invalid stash index `{value}`. Use a number, for example `/git stash pop 1`."
        ))
    }
}

fn clean_paths(args: &[String]) -> Vec<String> {
    args.iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn clean_args(args: &[String]) -> Vec<String> {
    clean_paths(args)
}

/// Static completions for the slash command argument UI.
///
/// Zed doesn't pass the worktree to `complete_slash_command_argument`, so we
/// cannot offer dynamic branch or file names here. Static subcommands and
/// common flags still make the tool window discoverable.
pub fn argument_completions(args: &[String]) -> Vec<ArgumentCompletion> {
    let mut completions = Vec::new();
    let top_level: &[(&str, &str)] = &[
        ("status", "Show local changes"),
        ("log", "Show commit history"),
        ("diff", "Show a diff"),
        ("branches", "List branches"),
        ("commit", "Commit staged changes"),
        ("stage", "Stage paths (all when omitted)"),
        ("unstage", "Unstage paths (all when omitted)"),
        ("rollback", "Roll back changes in paths"),
        ("switch", "Switch to a branch"),
        ("new-branch", "Create and switch to a branch"),
        ("stash", "Manage stashes"),
        ("fetch", "Fetch from remotes"),
        ("pull", "Pull the current branch"),
        ("push", "Push the current branch"),
        ("show", "Show a commit or object"),
        ("blame", "Annotate a file"),
        ("help", "List commands"),
    ];

    if args.is_empty() || args.iter().all(|arg| arg.trim().is_empty()) {
        for (name, description) in top_level {
            completions.push(ArgumentCompletion {
                label: format!("{name} \u{2014} {description}"),
                new_text: (*name).to_string(),
                run_command: false,
            });
        }
        return completions;
    }

    let subcommand = args[0].to_ascii_lowercase();
    let options: &[(&str, &str)] = match subcommand.as_str() {
        "stash" => &[
            ("list", "List stashes"),
            ("push", "Stash current changes"),
            ("pop", "Pop the latest stash"),
            ("apply", "Apply the latest stash"),
            ("drop", "Drop the latest stash"),
        ],
        "log" => &[
            ("10", "Last 10 commits"),
            ("25", "Last 25 commits"),
            ("50", "Last 50 commits"),
            ("all", "All branches"),
        ],
        "diff" => &[
            ("unstaged", "Working tree changes"),
            ("staged", "Staged changes"),
            ("all", "All tracked changes"),
        ],
        "rollback" => &[
            ("staged", "Revert staged changes to HEAD"),
            ("unstaged", "Discard worktree changes"),
            ("untracked", "Delete untracked files"),
        ],
        "fetch" => &[("--all", "Fetch all remotes")],
        _ => &[],
    };

    let prefix = if args.len() > 1 {
        args.last().cloned().unwrap_or_default()
    } else {
        String::new()
    };
    for (text, description) in options {
        if !prefix.is_empty() && !text.starts_with(&prefix) {
            continue;
        }
        completions.push(ArgumentCompletion {
            label: format!("{text} \u{2014} {description}"),
            new_text: (*text).to_string(),
            run_command: matches!(subcommand.as_str(), "diff" | "log" | "fetch"),
        });
    }
    completions
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(args: &[&str]) -> GitRequest {
        let args: Vec<String> = args.iter().map(|value| value.to_string()).collect();
        parse_request(&args).unwrap()
    }

    #[test]
    fn empty_arguments_open_the_dashboard() {
        assert_eq!(parse_request(&[]).unwrap(), GitRequest::Dashboard);
        assert_eq!(
            parse_request(&["".to_string()]).unwrap(),
            GitRequest::Dashboard
        );
    }

    #[test]
    fn parses_status_and_branch_aliases() {
        assert_eq!(request(&["status"]), GitRequest::Status);
        assert_eq!(request(&["st"]), GitRequest::Status);
        assert_eq!(request(&["changes"]), GitRequest::Status);
        assert_eq!(request(&["branches"]), GitRequest::Branches);
        assert_eq!(request(&["branch"]), GitRequest::Branches);
        assert_eq!(
            request(&["branch", "feature"]),
            GitRequest::CreateBranch {
                name: "feature".to_string()
            }
        );
    }

    #[test]
    fn parses_log_count_and_all() {
        assert_eq!(
            request(&["log"]),
            GitRequest::Log {
                count: 30,
                all: false
            }
        );
        assert_eq!(
            request(&["log", "50"]),
            GitRequest::Log {
                count: 50,
                all: false
            }
        );
        assert_eq!(
            request(&["log", "50", "all"]),
            GitRequest::Log {
                count: 50,
                all: true
            }
        );
        assert!(parse_request(&["log".into(), "0".into()]).is_err());
        assert!(parse_request(&["log".into(), "1000".into()]).is_err());
        assert!(parse_request(&["log".into(), "banana".into()]).is_err());
    }

    #[test]
    fn parses_diff_modes_and_paths() {
        assert_eq!(
            request(&["diff"]),
            GitRequest::Diff {
                mode: DiffMode::Unstaged,
                paths: Vec::new()
            }
        );
        assert_eq!(
            request(&["diff", "staged"]),
            GitRequest::Diff {
                mode: DiffMode::Staged,
                paths: Vec::new()
            }
        );
        assert_eq!(
            request(&["diff", "all", "src/main.rs"]),
            GitRequest::Diff {
                mode: DiffMode::All,
                paths: vec!["src/main.rs".to_string()]
            }
        );
        assert_eq!(
            request(&["diff", "src/main.rs", "README.md"]),
            GitRequest::Diff {
                mode: DiffMode::Unstaged,
                paths: vec!["src/main.rs".to_string(), "README.md".to_string()]
            }
        );
    }

    #[test]
    fn parses_commit_messages_and_amend() {
        assert_eq!(
            request(&["commit", "Fix", "the", "parser"]),
            GitRequest::Commit {
                message: "Fix the parser".to_string(),
                amend: false
            }
        );
        assert_eq!(
            request(&["commit", "--amend", "Better", "subject"]),
            GitRequest::Commit {
                message: "Better subject".to_string(),
                amend: true
            }
        );
        assert_eq!(
            request(&["commit", "fix", "--amend", "handling"]),
            GitRequest::Commit {
                message: "fix --amend handling".to_string(),
                amend: false
            }
        );
        assert!(parse_request(&["commit".into()]).is_err());
    }

    #[test]
    fn parses_stage_and_unstage_paths() {
        assert_eq!(request(&["stage"]), GitRequest::Stage { paths: Vec::new() });
        assert_eq!(
            request(&["unstage", "src/main.rs"]),
            GitRequest::Unstage {
                paths: vec!["src/main.rs".to_string()]
            }
        );
    }

    #[test]
    fn parses_rollback_levels_and_paths() {
        assert_eq!(
            request(&["rollback", "src/main.rs"]),
            GitRequest::Rollback {
                paths: vec!["src/main.rs".to_string()],
                level: RollbackLevel::Unstaged,
            }
        );
        assert_eq!(
            request(&["rollback", "staged", "src/main.rs", "README.md"]),
            GitRequest::Rollback {
                paths: vec!["src/main.rs".to_string(), "README.md".to_string()],
                level: RollbackLevel::Staged,
            }
        );
        assert_eq!(
            request(&["rollback", "untracked", "notes.md"]),
            GitRequest::Rollback {
                paths: vec!["notes.md".to_string()],
                level: RollbackLevel::Untracked,
            }
        );
        assert!(parse_request(&["rollback".into()]).is_err());
        assert!(parse_request(&["discard".into(), "".into()]).is_err());
    }

    #[test]
    fn parses_switch_and_new_branch() {
        assert_eq!(
            request(&["switch", "main"]),
            GitRequest::Switch {
                branch: "main".to_string()
            }
        );
        assert_eq!(
            request(&["new-branch", "feature"]),
            GitRequest::CreateBranch {
                name: "feature".to_string()
            }
        );
        assert!(parse_request(&["switch".into()]).is_err());
        assert!(parse_request(&["new-branch".into()]).is_err());
        assert!(parse_request(&["switch".into(), "-f".into()]).is_err());
        assert!(parse_request(&["new-branch".into(), "--force".into()]).is_err());
    }

    #[test]
    fn parses_stash_actions() {
        assert_eq!(
            request(&["stash"]),
            GitRequest::Stash {
                action: StashAction::List
            }
        );
        assert_eq!(
            request(&["stash", "push", "WIP", "parser"]),
            GitRequest::Stash {
                action: StashAction::Push {
                    message: Some("WIP parser".to_string())
                }
            }
        );
        assert_eq!(
            request(&["stash", "pop", "2"]),
            GitRequest::Stash {
                action: StashAction::Pop {
                    index: Some("stash@{2}".to_string())
                }
            }
        );
        assert_eq!(
            request(&["stash", "apply", "stash@{1}"]),
            GitRequest::Stash {
                action: StashAction::Apply {
                    index: Some("stash@{1}".to_string())
                }
            }
        );
        assert!(parse_request(&["stash".into(), "drop".into(), "abc".into()]).is_err());
    }

    #[test]
    fn parses_show_blame_help_and_unknown() {
        assert_eq!(
            request(&["show"]),
            GitRequest::Show {
                rev: "HEAD".to_string()
            }
        );
        assert_eq!(
            request(&["blame", "src/main.rs"]),
            GitRequest::Blame {
                path: "src/main.rs".to_string()
            }
        );
        assert_eq!(request(&["help"]), GitRequest::Help);
        assert!(parse_request(&["show".into(), "--help".into()]).is_err());
        assert!(parse_request(&["explode".into()]).is_err());
    }

    #[test]
    fn top_level_completions_are_described() {
        let completions = argument_completions(&[]);
        assert!(completions
            .iter()
            .any(|completion| completion.new_text == "status"
                && completion.label.contains("Show local changes")));
        assert!(!completions.iter().any(|completion| completion.run_command));

        let empty = argument_completions(&["".to_string()]);
        assert!(empty
            .iter()
            .any(|completion| completion.new_text == "status"));
    }

    #[test]
    fn subcommand_completions_are_filtered_by_prefix() {
        let args = vec!["diff".to_string(), "s".to_string()];
        let completions = argument_completions(&args);
        assert_eq!(completions.len(), 1);
        assert_eq!(completions[0].new_text, "staged");
        assert!(completions[0].run_command);

        let stash = argument_completions(&["stash".to_string()]);
        assert!(stash.iter().any(|completion| completion.new_text == "pop"));

        let rollback = argument_completions(&["rollback".to_string(), "s".to_string()]);
        assert_eq!(rollback.len(), 1);
        assert_eq!(rollback[0].new_text, "staged");
    }
}
