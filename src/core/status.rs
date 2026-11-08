//! Parser for `git status --porcelain=v2 --branch -z`.

use super::model::{BranchInfo, EntryKind, Head, StatusEntry, StatusReport};

/// Parse NUL-separated porcelain v2 output.
pub fn parse_status_v2(bytes: &[u8]) -> Result<StatusReport, String> {
    let text = String::from_utf8_lossy(bytes);
    let records: Vec<&str> = text.split('\0').collect();

    let mut branch = BranchInfo::default();
    let mut oid: Option<String> = None;
    let mut head_name: Option<String> = None;
    let mut upstream: Option<String> = None;
    let mut ahead = 0;
    let mut behind = 0;
    let mut entries = Vec::new();

    let mut index = 0;
    while index < records.len() {
        let record = records[index];
        index += 1;
        if record.is_empty() {
            continue;
        }

        if let Some(header) = record.strip_prefix("# ") {
            if let Some(value) = header.strip_prefix("branch.oid ") {
                oid = Some(value.to_string());
            } else if let Some(value) = header.strip_prefix("branch.head ") {
                head_name = Some(value.to_string());
            } else if let Some(value) = header.strip_prefix("branch.upstream ") {
                upstream = Some(value.to_string());
            } else if let Some(value) = header.strip_prefix("branch.ab ") {
                let (parsed_ahead, parsed_behind) = parse_ahead_behind(value);
                ahead = parsed_ahead;
                behind = parsed_behind;
            }
            continue;
        }

        if let Some(path) = record.strip_prefix("? ") {
            entries.push(StatusEntry {
                path: path.to_string(),
                orig_path: None,
                index: '?',
                worktree: '?',
                kind: EntryKind::Untracked,
            });
            continue;
        }

        if let Some(path) = record.strip_prefix("! ") {
            entries.push(StatusEntry {
                path: path.to_string(),
                orig_path: None,
                index: '!',
                worktree: '!',
                kind: EntryKind::Ignored,
            });
            continue;
        }

        match record.chars().next() {
            Some('1') => {
                if let Some(entry) = parse_ordinary(record) {
                    entries.push(entry);
                }
            }
            Some('2') => {
                let orig_path = records
                    .get(index)
                    .map(|value| value.to_string())
                    .filter(|value| !value.is_empty());
                if orig_path.is_some() {
                    index += 1;
                }
                if let Some(entry) = parse_rename(record, orig_path) {
                    entries.push(entry);
                }
            }
            Some('u') => {
                if let Some(entry) = parse_unmerged(record) {
                    entries.push(entry);
                }
            }
            // Unknown record types are ignored so newer Git versions do not
            // break the whole panel.
            _ => {}
        }
    }

    let initial = oid.as_deref() == Some("(initial)");
    let detached = head_name.as_deref() == Some("(detached)");
    let head = if initial {
        Head::Initial(head_name.unwrap_or_else(|| "main".to_string()))
    } else if detached {
        Head::Detached
    } else {
        Head::Branch(head_name.unwrap_or_else(|| "HEAD".to_string()))
    };

    branch.head = head;
    branch.oid = oid.filter(|value| value != "(initial)");
    branch.upstream = upstream.filter(|value| value != "(none)");
    branch.ahead = ahead;
    branch.behind = behind;

    Ok(StatusReport {
        branch,
        entries,
        stash_count: 0,
        last_commit: None,
        merge_in_progress: false,
        cherry_pick_in_progress: false,
        revert_in_progress: false,
    })
}

fn parse_ahead_behind(value: &str) -> (usize, usize) {
    let mut ahead = 0;
    let mut behind = 0;
    for part in value.split_whitespace() {
        if let Some(number) = part.strip_prefix('+') {
            ahead = number.parse().unwrap_or(0);
        } else if let Some(number) = part.strip_prefix('-') {
            behind = number.parse().unwrap_or(0);
        }
    }
    (ahead, behind)
}

fn parse_ordinary(record: &str) -> Option<StatusEntry> {
    // 1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>
    let fields: Vec<&str> = record.splitn(9, ' ').collect();
    if fields.len() != 9 {
        return None;
    }
    let (index_status, worktree_status) = xy(fields[1])?;
    Some(StatusEntry {
        path: fields[8].to_string(),
        orig_path: None,
        index: index_status,
        worktree: worktree_status,
        kind: EntryKind::Changed,
    })
}

fn parse_rename(record: &str, orig_path: Option<String>) -> Option<StatusEntry> {
    // 2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>
    let fields: Vec<&str> = record.splitn(10, ' ').collect();
    if fields.len() != 10 {
        return None;
    }
    let (index_status, worktree_status) = xy(fields[1])?;
    let kind = if index_status == 'C' || worktree_status == 'C' {
        EntryKind::Copied
    } else {
        EntryKind::Renamed
    };
    Some(StatusEntry {
        path: fields[9].to_string(),
        orig_path,
        index: index_status,
        worktree: worktree_status,
        kind,
    })
}

fn parse_unmerged(record: &str) -> Option<StatusEntry> {
    // u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>
    let fields: Vec<&str> = record.splitn(11, ' ').collect();
    if fields.len() != 11 {
        return None;
    }
    let (index_status, worktree_status) = xy(fields[1])?;
    Some(StatusEntry {
        path: fields[10].to_string(),
        orig_path: None,
        index: index_status,
        worktree: worktree_status,
        kind: EntryKind::Unmerged,
    })
}

fn xy(value: &str) -> Option<(char, char)> {
    let mut chars = value.chars();
    let index = chars.next()?;
    let worktree = chars.next()?;
    Some((index, worktree))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn records(lines: &[&str]) -> Vec<u8> {
        let mut output = Vec::new();
        for line in lines {
            output.extend_from_slice(line.as_bytes());
            output.push(0);
        }
        output
    }

    #[test]
    fn parses_branch_headers_and_ahead_behind() {
        let input = records(&[
            "# branch.oid 065c32ba3a89e96ad103ec9677d02f1c11a14ba7",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -3",
        ]);
        let report = parse_status_v2(&input).unwrap();
        assert_eq!(report.branch.head, Head::Branch("main".to_string()));
        assert_eq!(
            report.branch.oid.as_deref(),
            Some("065c32ba3a89e96ad103ec9677d02f1c11a14ba7")
        );
        assert_eq!(report.branch.upstream.as_deref(), Some("origin/main"));
        assert_eq!(report.branch.ahead, 2);
        assert_eq!(report.branch.behind, 3);
        assert_eq!(report.branch.display(), "main \u{2191}2 \u{2193}3");
    }

    #[test]
    fn parses_initial_and_detached_heads() {
        let initial = records(&["# branch.oid (initial)", "# branch.head main"]);
        let report = parse_status_v2(&initial).unwrap();
        assert_eq!(report.branch.head, Head::Initial("main".to_string()));
        assert_eq!(report.branch.display(), "main (no commits)");
        assert_eq!(report.branch.oid, None);

        let detached = records(&[
            "# branch.oid 3f2a1b9c3d4e5f60718293a4b5c6d7e8f9012345",
            "# branch.head (detached)",
        ]);
        let report = parse_status_v2(&detached).unwrap();
        assert_eq!(report.branch.head, Head::Detached);
        assert_eq!(report.branch.display(), "detached at 3f2a1b9");
    }

    #[test]
    fn parses_every_entry_kind() {
        let input = records(&[
            "# branch.oid abc",
            "# branch.head main",
            "1 M. N... 100644 100644 100644 aaaaaaa bbbbbbb src/main.rs",
            "1 .M N... 100644 100644 100644 aaaaaaa bbbbbbb README.md",
            "1 A. N... 000000 100644 100644 0000000 bbbbbbb src/new file.rs",
            "2 R. N... 100644 100644 100644 aaaaaaa bbbbbbb R100 renamed.rs",
            "original.rs",
            "2 C. N... 100644 100644 100644 aaaaaaa bbbbbbb C075 copied.rs",
            "source.rs",
            "u UU N... 100644 100644 100644 100644 aaaaaaa bbbbbbb ccccccc conflict.rs",
            "? notes.md",
            "! target/",
        ]);
        let report = parse_status_v2(&input).unwrap();

        assert_eq!(report.staged().len(), 4);
        assert_eq!(report.unstaged().len(), 1);
        assert_eq!(report.untracked().len(), 1);
        assert_eq!(report.conflicts().len(), 1);
        assert_eq!(report.ignored().len(), 1);

        let renamed = report
            .entries
            .iter()
            .find(|entry| entry.kind == EntryKind::Renamed)
            .unwrap();
        assert_eq!(renamed.path, "renamed.rs");
        assert_eq!(renamed.orig_path.as_deref(), Some("original.rs"));
        assert_eq!(renamed.display_path(), "renamed.rs <- original.rs");
        assert_eq!(renamed.index, 'R');

        let staged_new = report
            .entries
            .iter()
            .find(|entry| entry.path == "src/new file.rs")
            .unwrap();
        assert_eq!(staged_new.index, 'A');
        assert!(staged_new.is_staged());
        assert!(!staged_new.is_unstaged());

        let unstaged = report
            .entries
            .iter()
            .find(|entry| entry.path == "README.md")
            .unwrap();
        assert!(!unstaged.is_staged());
        assert!(unstaged.is_unstaged());
    }

    #[test]
    fn handles_both_staged_and_unstaged_changes() {
        let input = records(&[
            "# branch.head main",
            "1 MM N... 100644 100644 100644 aaaaaaa bbbbbbb both.rs",
        ]);
        let report = parse_status_v2(&input).unwrap();
        assert_eq!(report.staged().len(), 1);
        assert_eq!(report.unstaged().len(), 1);
        assert_eq!(report.change_count(), 1);
        assert!(!report.is_clean());
    }

    #[test]
    fn ignores_unknown_records_and_empty_trailing_record() {
        let input = records(&["# branch.head main", "x something new"]);
        let report = parse_status_v2(&input).unwrap();
        assert!(report.entries.is_empty());
        assert!(report.is_clean());
    }

    #[test]
    fn reports_clean_repository() {
        let input = records(&["# branch.head main", "# branch.upstream origin/main"]);
        let report = parse_status_v2(&input).unwrap();
        assert!(report.is_clean());
        assert_eq!(report.change_count(), 0);
    }
}
