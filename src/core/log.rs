//! Parsers for the line formats produced by `git log`, `git branch --format`
//! and `git stash list --format`.

use super::model::{Branch, ChangedFile, Commit, CommitNode, StashEntry, TagEntry};

pub const LOG_FIELD_SEPARATOR: char = '\u{1f}';
pub const LOG_RECORD_SEPARATOR: char = '\u{1e}';

/// `git log --pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1e`
pub fn parse_log_records(input: &str) -> Vec<Commit> {
    input
        .split(LOG_RECORD_SEPARATOR)
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let fields: Vec<&str> = record.split(LOG_FIELD_SEPARATOR).collect();
            if fields.len() < 5 {
                return None;
            }
            Some(Commit {
                hash: fields[0].to_string(),
                short_hash: fields[1].to_string(),
                author: fields[2].to_string(),
                date: fields[3].to_string(),
                subject: fields[4].to_string(),
            })
        })
        .collect()
}

/// `git log --pretty=format:%H%x1f%h%x1f%an%x1f%ar%x1f%s%x1f%P%x1f%d%x1e`
///
/// Returns commits with parent hashes and ref decorations for graph drawing.
pub fn parse_log_nodes(input: &str) -> Vec<CommitNode> {
    input
        .split(LOG_RECORD_SEPARATOR)
        .filter_map(|record| {
            let record = record.trim_matches('\n');
            if record.is_empty() {
                return None;
            }
            let fields: Vec<&str> = record.split(LOG_FIELD_SEPARATOR).collect();
            if fields.len() < 7 {
                return None;
            }
            Some(CommitNode {
                commit: Commit {
                    hash: fields[0].to_string(),
                    short_hash: fields[1].to_string(),
                    author: fields[2].to_string(),
                    date: fields[3].to_string(),
                    subject: fields[4].to_string(),
                },
                parents: split_oid_list(fields[5]),
                refs: parse_decorations(fields[6]),
            })
        })
        .collect()
}

/// `p1 p2` -> `["p1", "p2"]`.
fn split_oid_list(input: &str) -> Vec<String> {
    input
        .split_whitespace()
        .map(|hash| hash.to_string())
        .collect()
}

/// ` (HEAD -> main, origin/main)` -> `["HEAD -> main", "origin/main"]`.
pub fn parse_decorations(input: &str) -> Vec<String> {
    let inner = input
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    if inner.is_empty() {
        return Vec::new();
    }
    inner
        .split(", ")
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

/// Parse `for-each-ref --format=...` output for local and remote branches.
///
/// Format fields: `name`, `HEAD`, `upstream:short`, `upstream:track`,
/// `objectname:short`, `committerdate:relative`, `symref`, `refname`.
/// A branch is remote when its refname lives under `refs/remotes`.
pub fn parse_branches(input: &str) -> Vec<Branch> {
    let mut branches = Vec::new();
    for line in input.lines() {
        let line = line.trim_end_matches('\r');
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(LOG_FIELD_SEPARATOR).collect();
        if fields.len() < 6 {
            continue;
        }
        let name = fields[0];
        if name.is_empty() {
            continue;
        }
        let symref = fields.get(6).copied().unwrap_or("");
        if !symref.is_empty() {
            // Skip `origin/HEAD -> origin/main`-style symbolic refs.
            continue;
        }
        let refname = fields.get(7).copied().unwrap_or("");
        let remote = refname.starts_with("refs/remotes");
        let current = fields[1].trim() == "*";
        let upstream = fields[2].trim();
        let track = fields[3];
        let (ahead, behind, gone) = parse_track(track);
        branches.push(Branch {
            name: name.to_string(),
            current,
            upstream: if upstream.is_empty() {
                None
            } else {
                Some(upstream.to_string())
            },
            ahead,
            behind,
            gone,
            hash: fields[4].to_string(),
            date: fields[5].to_string(),
            remote,
        });
    }

    branches.sort_by(|a, b| {
        b.current
            .cmp(&a.current)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    branches
}

/// `[ahead 2, behind 1]`, `[gone]`, `[ahead 3]`, or empty.
pub fn parse_track(input: &str) -> (usize, usize, bool) {
    let mut ahead = 0;
    let mut behind = 0;
    let mut gone = false;
    let inner = input.trim().trim_start_matches('[').trim_end_matches(']');
    for part in inner.split(',') {
        let part = part.trim();
        if part == "gone" {
            gone = true;
        } else if let Some(number) = part.strip_prefix("ahead ") {
            ahead = number.trim().parse().unwrap_or(0);
        } else if let Some(number) = part.strip_prefix("behind ") {
            behind = number.trim().parse().unwrap_or(0);
        }
    }
    (ahead, behind, gone)
}

/// `git stash list --format=%gd%x1f%gs%x1f%cr`
pub fn parse_stashes(input: &str) -> Vec<StashEntry> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                return None;
            }
            let fields: Vec<&str> = line.split(LOG_FIELD_SEPARATOR).collect();
            if fields.len() < 3 {
                return None;
            }
            Some(StashEntry {
                selector: fields[0].to_string(),
                message: fields[1].to_string(),
                date: fields[2].to_string(),
            })
        })
        .collect()
}

/// `git for-each-ref refs/tags --format=%(refname:short)%x1f%(objectname:short)%x1f%(creatordate:relative)`
pub fn parse_tags(input: &str) -> Vec<TagEntry> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim_end_matches('\r');
            if line.is_empty() {
                return None;
            }
            let fields: Vec<&str> = line.split(LOG_FIELD_SEPARATOR).collect();
            if fields.len() < 1 || fields[0].is_empty() {
                return None;
            }
            Some(TagEntry {
                name: fields[0].to_string(),
                hash: fields.get(1).copied().unwrap_or("").to_string(),
                date: fields.get(2).copied().unwrap_or("").to_string(),
            })
        })
        .collect()
}

/// `git show --raw -z --find-renames`: the `-z` form writes every entry as
/// `:<modes>\0`, then `status\0` and one or two NUL-terminated path records.
/// Rename/copy entries list the source path before the destination path.
pub fn parse_raw_entries(input: &str) -> Vec<ChangedFile> {
    let mut files = Vec::new();
    let mut records = input.split('\0').peekable();
    while let Some(record) = records.next() {
        let record = record.trim_start_matches('\n');
        if record.is_empty() {
            continue;
        }
        if !record.starts_with(':') {
            // The leading `--format` record; everything else is an odd
            // trailing fragment, which nothing can follow up.
            continue;
        }
        // `:100644 100644 5626abf 5626abf R100`
        let status = record
            .rsplit(' ')
            .next()
            .unwrap_or_default()
            .to_string();
        if status.is_empty() {
            continue;
        }
        // Rename/copy letters start with `R` or `C`; git then writes the
        // source path first, followed by the destination path. Plain entries
        // have exactly one path record.
        let is_move = status.starts_with('R') || status.starts_with('C');
        let path = records.next().unwrap_or("");
        let second = if is_move { records.next().unwrap_or("") } else { "" };
        files.push(ChangedFile {
            status: status.clone(),
            path: if is_move { second.to_string() } else { path.to_string() },
            orig_path: if is_move { Some(path.to_string()) } else { None },
        });
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_log_records_with_separators() {
        let input = concat!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}2 hours ago\u{1f}Add parser\u{1e}\n",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\u{1f}bbbbbbb\u{1f}Bob\u{1f}3 days ago\u{1f}Fix tests\u{1e}\n",
        );
        let commits = parse_log_records(input);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].short_hash, "aaaaaaa");
        assert_eq!(commits[0].author, "Alice");
        assert_eq!(commits[0].subject, "Add parser");
        assert_eq!(commits[1].subject, "Fix tests");
    }

    #[test]
    fn ignores_malformed_log_records() {
        assert!(parse_log_records("").is_empty());
        assert!(parse_log_records("only-one-field").is_empty());
    }

    #[test]
    fn parses_log_nodes_with_parents_and_refs() {
        let input = concat!(
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\u{1f}aaaaaaa\u{1f}Alice\u{1f}2 hours ago\u{1f}Add parser\u{1f}\u{1f} (HEAD -> main, origin/main)\u{1e}\n",
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\u{1f}bbbbbbb\u{1f}Bob\u{1f}3 days ago\u{1f}Fix tests\u{1f}aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa cccccccccccccccccccccccccccccccccccccccc\u{1f} (tag: v1.0)\u{1e}\n",
        );
        let nodes = parse_log_nodes(input);
        assert_eq!(nodes.len(), 2);

        let head = &nodes[0];
        assert_eq!(head.commit.subject, "Add parser");
        assert!(head.parents.is_empty());
        assert_eq!(head.refs, vec!["HEAD -> main", "origin/main"]);

        let child = &nodes[1];
        assert_eq!(child.parents.len(), 2);
        assert_eq!(child.parents[0], "a".repeat(40));
        assert_eq!(child.refs, vec!["tag: v1.0"]);
    }

    #[test]
    fn ignores_malformed_log_nodes() {
        assert!(parse_log_nodes("").is_empty());
        assert!(parse_log_nodes("a\u{1f}b\u{1f}c\u{1f}d\u{1f}e").is_empty());
    }

    #[test]
    fn parses_branches_and_tracking() {
        let input = concat!(
            "main\u{1f}*\u{1f}origin/main\u{1f}[ahead 2, behind 1]\u{1f}aaaaaaa\u{1f}2 hours ago\u{1f}\u{1f}refs/heads/main\n",
            "feature\u{1f} \u{1f}\u{1f}\u{1f}bbbbbbb\u{1f}3 days ago\u{1f}\u{1f}refs/heads/feature\n",
            "old\u{1f} \u{1f}origin/old\u{1f}[gone]\u{1f}ccccccc\u{1f}2 weeks ago\u{1f}\u{1f}refs/heads/old\n",
        );
        let branches = parse_branches(input);
        assert_eq!(branches.len(), 3);
        assert_eq!(branches[0].name, "main");
        assert!(branches[0].current);
        assert!(!branches[0].remote);
        assert_eq!(branches[0].ahead, 2);
        assert_eq!(branches[0].behind, 1);
        assert_eq!(branches[0].upstream.as_deref(), Some("origin/main"));

        let old = branches.iter().find(|branch| branch.name == "old").unwrap();
        assert!(old.gone);
        assert_eq!(old.upstream.as_deref(), Some("origin/old"));
    }

    #[test]
    fn remote_branches_are_recognized_by_refname() {
        let input = concat!(
            "origin/main\u{1f} \u{1f}\u{1f}\u{1f}ddddddd\u{1f}1 hour ago\u{1f}\u{1f}refs/remotes/origin/main\n",
            "main\u{1f}*\u{1f}origin/main\u{1f}\u{1f}aaaaaaa\u{1f}now\u{1f}\u{1f}refs/heads/main\n",
        );
        let branches = parse_branches(input);
        assert_eq!(branches.len(), 2);
        let remote = branches.iter().find(|branch| branch.name == "origin/main").unwrap();
        assert!(remote.remote);
        assert!(branches.iter().find(|branch| branch.name == "main").unwrap().current);
        // The current branch sorts first even across the mixed input.
        assert_eq!(branches[0].name, "main");
    }

    #[test]
    fn skips_symbolic_remote_refs() {
        let input = "origin/HEAD\u{1f} \u{1f}\u{1f}\u{1f}aaaaaaa\u{1f}now\u{1f}origin/main\u{1f}refs/remotes/origin/HEAD\n";
        assert!(parse_branches(input).is_empty());
    }

    #[test]
    fn parses_track_strings() {
        assert_eq!(parse_track("[ahead 2, behind 1]"), (2, 1, false));
        assert_eq!(parse_track("[ahead 3]"), (3, 0, false));
        assert_eq!(parse_track("[behind 4]"), (0, 4, false));
        assert_eq!(parse_track("[gone]"), (0, 0, true));
        assert_eq!(parse_track(""), (0, 0, false));
    }

    #[test]
    fn parses_stashes() {
        let input = concat!(
            "stash@{0}\u{1f}WIP on main: 1234567 Add parser\u{1f}5 minutes ago\n",
            "stash@{1}\u{1f}On main: experiments\u{1f}2 days ago\n",
        );
        let stashes = parse_stashes(input);
        assert_eq!(stashes.len(), 2);
        assert_eq!(stashes[0].selector, "stash@{0}");
        assert_eq!(stashes[0].message, "WIP on main: 1234567 Add parser");
        assert_eq!(stashes[1].date, "2 days ago");
    }
}
