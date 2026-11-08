//! JetBrains-style rendering of Git data as plain text panels.
//!
//! Zed extensions cannot create native tool windows, so Git Jet renders the
//! same information as a compact, sectioned "tool window" in the Assistant
//! panel. Sections become creases, which makes the long output navigable.

use super::model::{
    Branch, Commit, Rendered, RenderedSection, StashEntry, StatusEntry, StatusReport,
};
use super::request::DiffMode;

pub const PANEL_WIDTH: usize = 74;
const CONTENT_WIDTH: usize = PANEL_WIDTH - 4;
const MAX_DIFF_LINES: usize = 500;
const MAX_DIFF_CHARS: usize = 24_000;

/// Builds text and byte ranges for sections.
#[derive(Debug, Default)]
pub struct RenderedBuilder {
    text: String,
    sections: Vec<RenderedSection>,
}

impl RenderedBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raw(&mut self, text: &str) {
        self.text.push_str(text);
    }

    pub fn line(&mut self, line: &str) {
        self.text.push_str(line);
        self.text.push('\n');
    }

    pub fn blank(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with("\n\n") {
            self.text.push('\n');
        }
    }

    /// Append `content` as a new section.
    pub fn section(&mut self, label: &str, content: &str) {
        let content = content.trim_end_matches(['\n', '\r']);
        if content.is_empty() {
            return;
        }
        self.blank();
        let start = self.text.len();
        self.text.push_str(content);
        self.text.push('\n');
        let end = self.text.len();
        self.sections.push(RenderedSection {
            range: start..end,
            label: label.to_string(),
        });
    }

    pub fn finish(mut self) -> Rendered {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
        Rendered {
            text: self.text,
            sections: self.sections,
        }
    }
}

/// A JetBrains-like panel header.
pub fn header(title: &str, subtitle: Option<&str>) -> String {
    let mut output = String::new();
    let fill = PANEL_WIDTH.saturating_sub(title.chars().count() + 5);
    output.push_str(&format!(
        "\u{256d}\u{2500} {} {}\u{256e}\n",
        title,
        "\u{2500}".repeat(fill)
    ));
    if let Some(subtitle) = subtitle.filter(|value| !value.is_empty()) {
        let subtitle = truncate_chars(subtitle, CONTENT_WIDTH);
        let padding = CONTENT_WIDTH.saturating_sub(subtitle.chars().count());
        output.push_str(&format!(
            "\u{2502} {}{} \u{2502}\n",
            subtitle,
            " ".repeat(padding)
        ));
    }
    output.push_str(&format!(
        "\u{2570}{}\u{256f}",
        "\u{2500}".repeat(PANEL_WIDTH - 2)
    ));
    output
}

fn truncate_chars(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut value: String = text.chars().take(width.saturating_sub(1)).collect();
    value.push('\u{2026}');
    value
}

/// Normalize output from Git before embedding it in a panel.
pub fn clean_output(text: &str) -> String {
    text.replace("\r\n", "\n").trim_end().to_string()
}

pub fn summary_line(report: &StatusReport) -> String {
    let mut parts = vec![report.branch.display()];
    if !matches!(report.branch.head, super::model::Head::Detached) {
        parts.push(report.branch.tracking());
    }
    let changes = report.change_count();
    parts.push(if changes == 1 {
        "1 change".to_string()
    } else {
        format!("{changes} changes")
    });
    if report.stash_count > 0 {
        parts.push(if report.stash_count == 1 {
            "1 stash".to_string()
        } else {
            format!("{} stashes", report.stash_count)
        });
    }
    parts.join(" \u{00b7} ")
}

fn status_code(entry: &StatusEntry, staged: bool) -> char {
    if entry.is_conflict() {
        return 'U';
    }
    if entry.is_untracked() {
        return '?';
    }
    let code = if staged { entry.index } else { entry.worktree };
    match code {
        '.' | ' ' => '\u{00b7}',
        other => other,
    }
}

fn status_lines(entries: &[&StatusEntry], staged: bool) -> Vec<String> {
    entries
        .iter()
        .map(|entry| format!("  {}  {}", status_code(entry, staged), entry.display_path()))
        .collect()
}

/// The "Local Changes" tree shared by the dashboard, status panel and actions.
pub fn local_changes_block(report: &StatusReport) -> String {
    let staged = report.staged();
    let unstaged = report.unstaged();
    let untracked = report.untracked();
    let conflicts = report.conflicts();

    if staged.is_empty() && unstaged.is_empty() && untracked.is_empty() && conflicts.is_empty() {
        return "Working tree clean.".to_string();
    }

    let mut lines = Vec::new();
    if !conflicts.is_empty() {
        lines.push(format!("Conflicts ({})", conflicts.len()));
        lines.extend(status_lines(&conflicts, true));
    }
    if !staged.is_empty() {
        lines.push(format!("Staged ({})", staged.len()));
        lines.extend(status_lines(&staged, true));
    }
    if !unstaged.is_empty() {
        lines.push(format!("Unstaged ({})", unstaged.len()));
        lines.extend(status_lines(&unstaged, false));
    }
    if !untracked.is_empty() {
        lines.push(format!("Untracked ({})", untracked.len()));
        lines.extend(
            untracked
                .iter()
                .map(|entry| format!("  ?  {}", entry.display_path())),
        );
    }
    lines.join("\n")
}

fn recent_commits_block(log: &str) -> String {
    let log = clean_output(log);
    if log.is_empty() {
        "No commits yet.".to_string()
    } else {
        log
    }
}

fn last_commit_block(commit: Option<&Commit>) -> String {
    match commit {
        Some(commit) => format!(
            "{}  {}\n  {} \u{00b7} {}",
            commit.short_hash, commit.subject, commit.author, commit.date
        ),
        None => "No commits yet.".to_string(),
    }
}

const COMMAND_HINTS: &str = "\
/git status             Refresh local changes
/git log [count] [all]  Browse history
/git diff [staged|all]  Inspect a diff
/git commit <message>   Commit staged changes
/git help               List every command";

fn render_dashboard(report: &StatusReport, log: &str) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section("Git", &header("Git", Some(&summary_line(report))));
    builder.section("Local Changes", &local_changes_block(report));
    builder.section("Recent Commits", &recent_commits_block(log));
    builder.section("Commands", COMMAND_HINTS);
    builder.finish()
}

fn render_status(report: &StatusReport) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Status",
        &header("Git \u{00b7} Status", Some(&summary_line(report))),
    );
    builder.section("Local Changes", &local_changes_block(report));
    builder.section(
        "Last Commit",
        &last_commit_block(report.last_commit.as_ref()),
    );
    builder.finish()
}

fn render_log(report: &StatusReport, log: &str, count: usize, all: bool) -> Rendered {
    let subtitle = format!(
        "{} \u{00b7} last {} commit{}{}",
        report.branch.display(),
        count,
        if count == 1 { "" } else { "s" },
        if all { " \u{00b7} all branches" } else { "" }
    );
    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Log",
        &header("Git \u{00b7} Log", Some(&subtitle)),
    );
    builder.section("History", &recent_commits_block(log));
    builder.finish()
}

fn branch_tracking(branch: &Branch) -> String {
    if branch.gone {
        return "gone".to_string();
    }
    match &branch.upstream {
        Some(upstream) => {
            let mut value = upstream.clone();
            if branch.ahead > 0 || branch.behind > 0 {
                value.push_str(&format!(
                    " \u{2191}{} \u{2193}{}",
                    branch.ahead, branch.behind
                ));
            }
            value
        }
        None => "no upstream".to_string(),
    }
}

fn render_branches(report: &StatusReport, local: &[Branch], remote: &[Branch]) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Branches",
        &header("Git \u{00b7} Branches", Some(&summary_line(report))),
    );

    let mut local_block = String::new();
    if local.is_empty() {
        local_block.push_str("No local branches.");
    } else {
        local_block.push_str(&format!("Local ({})\n", local.len()));
        for branch in local {
            local_block.push_str(&format!(
                "  {} {:<22} {:<24} {:<9} {}\n",
                if branch.current { '*' } else { ' ' },
                truncate_chars(&branch.name, 22),
                truncate_chars(&branch_tracking(branch), 24),
                branch.hash,
                branch.date
            ));
        }
        while local_block.ends_with('\n') {
            local_block.pop();
        }
    }
    builder.section("Local", &local_block);

    let mut remote_block = String::new();
    if remote.is_empty() {
        remote_block.push_str("No remote branches.");
    } else {
        remote_block.push_str(&format!("Remote ({})\n", remote.len()));
        for branch in remote {
            remote_block.push_str(&format!(
                "    {:<28} {:<9} {}\n",
                truncate_chars(&branch.name, 28),
                branch.hash,
                branch.date
            ));
        }
        while remote_block.ends_with('\n') {
            remote_block.pop();
        }
    }
    builder.section("Remote", &remote_block);
    builder.finish()
}

fn diff_file_label(header_line: &str) -> String {
    let rest = header_line
        .strip_prefix("diff --git ")
        .unwrap_or(header_line);
    if let Some(index) = rest.rfind("\" \"b/") {
        return rest[index + 5..].trim_end_matches('"').to_string();
    }
    if let Some(index) = rest.rfind(" b/") {
        rest[index + 3..].trim_matches('"').to_string()
    } else {
        "diff".to_string()
    }
}

/// Split a unified diff into `(path, chunk)` pairs.
pub fn split_diff_files(diff: &str) -> Vec<(String, String)> {
    let mut files = Vec::new();
    let mut current = String::new();
    let mut current_label = String::from("diff");

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            if !current.trim().is_empty() {
                files.push((current_label.clone(), current.clone()));
            }
            current.clear();
            current_label = diff_file_label(line);
        }
        current.push_str(line);
        current.push('\n');
    }
    if !current.trim().is_empty() {
        files.push((current_label, current));
    }
    files
}

fn truncate_diff(diff: &str) -> (String, bool) {
    let mut clipped = String::new();
    let mut chars = 0;
    for (index, line) in diff.lines().enumerate() {
        if index >= MAX_DIFF_LINES || chars >= MAX_DIFF_CHARS {
            return (clipped, true);
        }
        let line_chars = line.chars().count() + 1;
        if chars + line_chars > MAX_DIFF_CHARS {
            return (clipped, true);
        }
        clipped.push_str(line);
        clipped.push('\n');
        chars += line_chars;
    }
    (clipped, false)
}

fn diff_title(mode: &DiffMode) -> &'static str {
    match mode {
        DiffMode::Unstaged => "Unstaged changes",
        DiffMode::Staged => "Staged changes",
        DiffMode::All => "All tracked changes",
    }
}

fn empty_diff_message(mode: &DiffMode) -> &'static str {
    match mode {
        DiffMode::Unstaged => "No unstaged changes.",
        DiffMode::Staged => "No staged changes.",
        DiffMode::All => "No tracked changes.",
    }
}

fn render_diff(report: &StatusReport, diff: &str, mode: &DiffMode, paths: &[String]) -> Rendered {
    let mut builder = RenderedBuilder::new();
    let (diff, truncated) = truncate_diff(diff);
    let files = split_diff_files(&diff);
    let mut subtitle = format!("{} \u{00b7} {} file", diff_title(mode), files.len());
    if files.len() != 1 {
        subtitle.push('s');
    }
    if !paths.is_empty() {
        subtitle.push_str(&format!(" \u{00b7} {}", paths.join(", ")));
    }
    builder.section(
        "Git \u{00b7} Diff",
        &header("Git \u{00b7} Diff", Some(&subtitle)),
    );

    if files.is_empty() {
        builder.section("Diff", empty_diff_message(mode));
    } else {
        for (path, content) in files {
            builder.section(&format!("Diff \u{00b7} {path}"), &content);
        }
    }
    if truncated {
        builder.section(
            "Diff \u{00b7} Truncated",
            &format!(
                "Diff truncated after {MAX_DIFF_LINES} lines / {MAX_DIFF_CHARS} characters. Narrow it with `/git diff <path>`."
            ),
        );
    }
    let _ = report;
    builder.finish()
}

fn render_commit(report: &StatusReport, output: &str) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Commit",
        &header("Git \u{00b7} Commit", Some(&summary_line(report))),
    );
    let output = clean_output(output);
    builder.section(
        "Commit",
        if output.is_empty() {
            "Commit created."
        } else {
            &output
        },
    );
    builder.section("Local Changes", &local_changes_block(report));
    builder.finish()
}

fn render_action(
    title: &str,
    subtitle: &str,
    output: &str,
    report: Option<&StatusReport>,
) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section(
        &format!("Git \u{00b7} {title}"),
        &header(&format!("Git \u{00b7} {title}"), Some(subtitle)),
    );
    let output = clean_output(output);
    builder.section("Result", if output.is_empty() { "Done." } else { &output });
    if let Some(report) = report {
        builder.section("Local Changes", &local_changes_block(report));
    }
    builder.finish()
}

fn render_stashes(stashes: &[StashEntry]) -> Rendered {
    let mut builder = RenderedBuilder::new();
    let subtitle = format!(
        "{} stash{}",
        stashes.len(),
        if stashes.len() == 1 { "" } else { "es" }
    );
    builder.section(
        "Git \u{00b7} Stashes",
        &header("Git \u{00b7} Stashes", Some(&subtitle)),
    );
    if stashes.is_empty() {
        builder.section("Stashes", "No stashes.");
    } else {
        let block = stashes
            .iter()
            .map(|stash| {
                format!(
                    "  {:<12} {}  \u{00b7} {}",
                    stash.selector, stash.message, stash.date
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        builder.section("Stashes", &block);
    }
    builder.finish()
}

fn render_show(rev: &str, output: &str) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section("Git \u{00b7} Show", &header("Git \u{00b7} Show", Some(rev)));
    let (output, truncated) = truncate_diff(&clean_output(output));
    builder.section("Object", &output);
    if truncated {
        builder.section("Show \u{00b7} Truncated", "Output was truncated.");
    }
    builder.finish()
}

fn render_blame(path: &str, output: &str) -> Rendered {
    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Blame",
        &header("Git \u{00b7} Blame", Some(path)),
    );
    let (output, truncated) = truncate_diff(&clean_output(output));
    builder.section("Blame", &output);
    if truncated {
        builder.section("Blame \u{00b7} Truncated", "Output was truncated.");
    }
    builder.finish()
}

fn render_help() -> Rendered {
    let commands = "\
/git                        Open the Git dashboard
/git status                 Local changes and last commit
/git log [count] [all]      Commit graph (default 30, max 500)
/git diff [mode] [path]     Diff: unstaged (default), staged, all
/git branches               Local and remote branches
/git commit <message>       Commit staged changes
/git commit --amend <text>  Amend the last commit
/git stage [path...]        Stage all or the given paths
/git unstage [path...]      Unstage all or the given paths
/git switch <branch>        Switch branches
/git new-branch <name>      Create and switch to a branch
/git stash [action]         list, push, pop, apply, drop
/git fetch [--all]          Fetch remotes
/git pull [args]            Pull the current branch
/git push [args]            Push the current branch
/git show [rev]             Show a commit (default HEAD)
/git blame <path>           Blame a file
/git help                   This list";

    let notes = "\
Run these from the Assistant panel (for example `git: /git status`).
Paths and messages are passed to Git directly; no shell is involved.
Staging, committing, switching, and stashing change the worktree.";

    let mut builder = RenderedBuilder::new();
    builder.section(
        "Git \u{00b7} Help",
        &header("Git \u{00b7} Help", Some("JetBrains-style Git tool window")),
    );
    builder.section("Commands", commands);
    builder.section("Notes", notes);
    builder.finish()
}

/// Rendering entry points used by `GitClient`.
pub(crate) fn dashboard(report: &StatusReport, log: &str) -> Rendered {
    render_dashboard(report, log)
}

pub(crate) fn status(report: &StatusReport) -> Rendered {
    render_status(report)
}

pub(crate) fn log(report: &StatusReport, log: &str, count: usize, all: bool) -> Rendered {
    render_log(report, log, count, all)
}

pub(crate) fn branches(report: &StatusReport, local: &[Branch], remote: &[Branch]) -> Rendered {
    render_branches(report, local, remote)
}

pub(crate) fn diff(
    report: &StatusReport,
    diff: &str,
    mode: &DiffMode,
    paths: &[String],
) -> Rendered {
    render_diff(report, diff, mode, paths)
}

pub(crate) fn commit(report: &StatusReport, output: &str) -> Rendered {
    render_commit(report, output)
}

pub(crate) fn action(
    title: &str,
    subtitle: &str,
    output: &str,
    report: Option<&StatusReport>,
) -> Rendered {
    render_action(title, subtitle, output, report)
}

pub(crate) fn stashes(stashes: &[StashEntry]) -> Rendered {
    render_stashes(stashes)
}

pub(crate) fn show(rev: &str, output: &str) -> Rendered {
    render_show(rev, output)
}

pub(crate) fn blame(path: &str, output: &str) -> Rendered {
    render_blame(path, output)
}

pub(crate) fn help() -> Rendered {
    render_help()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{BranchInfo, EntryKind, Head, StatusEntry};

    fn entry(path: &str, index: char, worktree: char) -> StatusEntry {
        StatusEntry {
            path: path.to_string(),
            orig_path: None,
            index,
            worktree,
            kind: EntryKind::Changed,
        }
    }

    fn clean_report() -> StatusReport {
        StatusReport {
            branch: BranchInfo {
                head: Head::Branch("main".to_string()),
                oid: Some("aaaaaaa".to_string()),
                upstream: Some("origin/main".to_string()),
                ahead: 0,
                behind: 0,
            },
            ..StatusReport::default()
        }
    }

    #[test]
    fn header_has_consistent_width() {
        let top = header("Git", Some("main \u{00b7} 3 changes"));
        let lines: Vec<&str> = top.lines().collect();
        assert_eq!(lines.len(), 3);
        for line in lines {
            assert_eq!(line.chars().count(), PANEL_WIDTH);
        }
    }

    #[test]
    fn header_without_subtitle_is_two_lines() {
        let top = header("Git", None);
        let lines: Vec<&str> = top.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            assert_eq!(line.chars().count(), PANEL_WIDTH);
        }
    }

    #[test]
    fn local_changes_groups_and_sorts_entries() {
        let mut report = clean_report();
        report.entries = vec![
            entry("z.rs", 'M', '.'),
            entry("a.rs", 'M', '.'),
            entry("b.rs", '.', 'M'),
            StatusEntry {
                path: "new.rs".to_string(),
                orig_path: None,
                index: '?',
                worktree: '?',
                kind: EntryKind::Untracked,
            },
            StatusEntry {
                path: "conflict.rs".to_string(),
                orig_path: None,
                index: 'U',
                worktree: 'U',
                kind: EntryKind::Unmerged,
            },
        ];
        let block = local_changes_block(&report);
        assert!(block.contains("Conflicts (1)\n  U  conflict.rs"));
        assert!(block.contains("Staged (2)\n  M  a.rs\n  M  z.rs"));
        assert!(block.contains("Unstaged (1)\n  M  b.rs"));
        assert!(block.contains("Untracked (1)\n  ?  new.rs"));
        let staged_at = block.find("Staged (2)").unwrap();
        let unstaged_at = block.find("Unstaged (1)").unwrap();
        let untracked_at = block.find("Untracked (1)").unwrap();
        assert!(staged_at < unstaged_at && unstaged_at < untracked_at);
    }

    #[test]
    fn clean_report_renders_clean_message() {
        let report = clean_report();
        assert_eq!(local_changes_block(&report), "Working tree clean.");
        let rendered = render_status(&report);
        assert!(rendered.text.contains("Working tree clean."));
        assert!(rendered.text.contains("No commits yet."));
    }

    #[test]
    fn dashboard_sections_cover_their_text() {
        let mut report = clean_report();
        report.entries = vec![entry("src/main.rs", '.', 'M')];
        report.stash_count = 1;
        let rendered = render_dashboard(
            &report,
            "* aaaaaaa (HEAD -> main) Add parser (Alice, 2 hours ago)",
        );
        assert!(rendered.text.contains("Unstaged (1)"));
        assert!(rendered.text.contains("1 stash"));
        assert!(rendered.text.contains("/git status"));
        assert!(rendered.sections.len() >= 4);
        for section in &rendered.sections {
            assert!(!rendered.section_text(section).trim().is_empty());
        }
        let labels: Vec<&str> = rendered
            .sections
            .iter()
            .map(|section| section.label.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["Git", "Local Changes", "Recent Commits", "Commands"]
        );
    }

    #[test]
    fn unicode_paths_preserve_valid_section_ranges() {
        let mut report = clean_report();
        report.entries = vec![entry("src/na\u{00ef}ve.rs", '.', 'M')];
        let rendered = render_dashboard(&report, "");
        for section in &rendered.sections {
            let text = rendered.section_text(section);
            assert!(text.contains("na\u{00ef}ve.rs") || !text.contains("na"));
        }
    }

    #[test]
    fn diff_is_split_per_file_with_sections() {
        let diff = "\
diff --git a/src/main.rs b/src/main.rs
index 111..222 100644
--- a/src/main.rs
+++ b/src/main.rs
@@ -1 +1 @@
-old
+new
diff --git a/README.md b/README.md
--- a/README.md
+++ b/README.md
@@ -1 +1 @@
-before
+after
";
        let files = split_diff_files(diff);
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].0, "src/main.rs");
        assert_eq!(files[1].0, "README.md");
        let report = clean_report();
        let rendered = render_diff(&report, diff, &DiffMode::Unstaged, &[]);
        assert!(rendered
            .sections
            .iter()
            .any(|section| section.label == "Diff \u{00b7} src/main.rs"));
        assert!(rendered.text.contains("+new"));
    }

    #[test]
    fn empty_diff_has_friendly_message() {
        let report = clean_report();
        let rendered = render_diff(&report, "", &DiffMode::Staged, &[]);
        assert!(rendered.text.contains("No staged changes."));

        let rendered = render_diff(&report, "", &DiffMode::All, &[]);
        assert!(rendered.text.contains("No tracked changes."));
    }

    #[test]
    fn quoted_diff_paths_are_labelled() {
        assert_eq!(
            diff_file_label("diff --git \"a/file with space.rs\" \"b/file with space.rs\""),
            "file with space.rs"
        );
        assert_eq!(
            diff_file_label("diff --git a/src/main.rs b/src/main.rs"),
            "src/main.rs"
        );
    }

    #[test]
    fn long_diff_is_truncated() {
        let mut diff = String::new();
        for index in 0..(MAX_DIFF_LINES + 50) {
            diff.push_str(&format!("+line {index}\n"));
        }
        let (clipped, truncated) = truncate_diff(&diff);
        assert!(truncated);
        assert_eq!(clipped.lines().count(), MAX_DIFF_LINES);
    }

    #[test]
    fn branches_show_current_and_tracking() {
        let report = clean_report();
        let local = vec![
            Branch {
                name: "main".to_string(),
                current: true,
                upstream: Some("origin/main".to_string()),
                ahead: 2,
                behind: 1,
                gone: false,
                hash: "aaaaaaa".to_string(),
                date: "2 hours ago".to_string(),
                remote: false,
            },
            Branch {
                name: "feature".to_string(),
                current: false,
                upstream: None,
                ahead: 0,
                behind: 0,
                gone: false,
                hash: "bbbbbbb".to_string(),
                date: "3 days ago".to_string(),
                remote: false,
            },
        ];
        let rendered = render_branches(&report, &local, &[]);
        assert!(rendered.text.contains("* main"));
        assert!(rendered.text.contains("origin/main \u{2191}2 \u{2193}1"));
        assert!(rendered.text.contains("feature"));
        assert!(rendered.text.contains("No remote branches."));
    }

    #[test]
    fn help_lists_commands() {
        let rendered = render_help();
        for command in [
            "/git status",
            "/git log",
            "/git diff",
            "/git commit",
            "/git stash",
        ] {
            assert!(rendered.text.contains(command));
        }
    }

    #[test]
    fn section_ranges_are_exact() {
        let mut builder = RenderedBuilder::new();
        builder.section("one", "first\nsecond");
        builder.section("two", "third");
        let rendered = builder.finish();
        assert_eq!(rendered.sections.len(), 2);
        assert_eq!(
            rendered.section_text(&rendered.sections[0]),
            "first\nsecond\n"
        );
        assert_eq!(rendered.section_text(&rendered.sections[1]), "third\n");
    }
}
