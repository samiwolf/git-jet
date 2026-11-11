//! In-memory log of the git operations Git Jet runs.
//!
//! Every command the [`GitClient`](super::client::GitClient) executes is
//! recorded here (arguments, outcome, duration) in a bounded ring buffer
//! that powers the in-app "Op Log" viewer. The log lives for the current
//! app session only — nothing is ever written to disk.

use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::model::CmdOutput;

/// Maximum number of operation lines kept (and shown in the viewer).
const MAX_LINES: usize = 1000;

/// Longest error excerpt attached to a failed operation.
const MAX_ERROR_CHARS: usize = 160;

static BUFFER: OnceLock<Mutex<VecDeque<String>>> = OnceLock::new();

fn buffer() -> &'static Mutex<VecDeque<String>> {
    BUFFER.get_or_init(|| Mutex::new(VecDeque::with_capacity(MAX_LINES)))
}

/// Records one completed git invocation as a single log line.
///
/// Failures are recorded too — with the first stderr line as detail — so
/// the viewer explains why an operation broke. `stdin_bytes` marks commands
/// that received content on standard input.
pub fn record(
    args: &[String],
    result: &Result<CmdOutput, String>,
    duration: Duration,
    stdin_bytes: Option<usize>,
) {
    let line = format_line(args, result, duration, stdin_bytes);
    if let Ok(mut buffer) = buffer().lock() {
        if buffer.len() >= MAX_LINES {
            buffer.pop_front();
        }
        buffer.push_back(line);
    }
}

/// The recorded lines, oldest first (at most [`MAX_LINES`]).
pub fn lines() -> Vec<String> {
    match buffer().lock() {
        Ok(buffer) => buffer.iter().cloned().collect(),
        Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
    }
}

/// Deletes every recorded line.
pub fn clear() {
    if let Ok(mut buffer) = buffer().lock() {
        buffer.clear();
    }
}

/// `2026-11-11 09:41:23  git status --porcelain=v2 --branch -z  →  ok (12 ms)`
fn format_line(
    args: &[String],
    result: &Result<CmdOutput, String>,
    duration: Duration,
    stdin_bytes: Option<usize>,
) -> String {
    let mut line = String::with_capacity(128);
    line.push_str(&utc_timestamp());
    line.push_str("  git");
    for arg in args {
        line.push(' ');
        line.push_str(&quote(arg));
    }
    if let Some(bytes) = stdin_bytes {
        line.push_str("  [stdin: ");
        line.push_str(&format_size(bytes));
        line.push(']');
    }
    line.push_str("  →  ");
    match result {
        Ok(output) if output.success() => line.push_str("ok"),
        Ok(output) => {
            let code = output
                .status
                .map(|code| code.to_string())
                .unwrap_or_else(|| "signal".to_string());
            line.push_str("exit ");
            line.push_str(&code);
            let detail = first_error_line(&output.stderr_lossy());
            if !detail.is_empty() {
                line.push_str(": ");
                line.push_str(&detail);
            }
        }
        Err(error) => {
            line.push_str("error: ");
            line.push_str(&first_error_line(error));
        }
    }
    line.push_str(" (");
    line.push_str(&format_duration(duration));
    line.push(')');
    line
}

/// Quotes a single argument for display the way a shell would.
fn quote(arg: &str) -> String {
    if arg.is_empty() || arg.contains([' ', '\t', '"']) {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        arg.to_string()
    }
}

/// First non-empty line of an error message, trimmed and clipped.
fn first_error_line(detail: &str) -> String {
    let line = detail
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut excerpt: String = line.chars().take(MAX_ERROR_CHARS).collect();
    if line.chars().count() > MAX_ERROR_CHARS {
        excerpt.push('…');
    }
    excerpt
}

fn format_duration(duration: Duration) -> String {
    let millis = duration.as_millis();
    if millis < 10_000 {
        format!("{millis} ms")
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

fn format_size(bytes: usize) -> String {
    if bytes < 1024 {
        format!("{bytes} B")
    } else if bytes < 1024 * 1024 {
        format!("{:.1} kB", bytes as f64 / 1024.0)
    } else {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    }
}

/// Current UTC time as `YYYY-MM-DD HH:MM:SS` (no external time crate).
fn utc_timestamp() -> String {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let (year, month, day) = civil_from_days((since_epoch.as_secs() / 86_400) as i64);
    let second_of_day = since_epoch.as_secs() % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02}",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60,
    )
}

/// Days since 1970-01-01 → (year, month, day) in the proleptic Gregorian
/// calendar (Howard Hinnant's `civil_from_days` algorithm).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = (z - era * 146_097) as i64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let mp = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_keeps_the_last_thousand_lines() {
        clear();
        let total = MAX_LINES + 25;
        for index in 0..total {
            record(
                &["log".to_string(), format!("-n{index}")],
                &Ok(CmdOutput {
                    status: Some(0),
                    stdout: Vec::new(),
                    stderr: Vec::new(),
                }),
                Duration::from_millis(3),
                None,
            );
        }
        let recorded = lines();
        assert!(recorded.len() <= MAX_LINES);
        // The oldest lines were trimmed away, the newest are all present.
        assert!(!recorded.iter().any(|line| line.contains("-n0 ") || line.ends_with("-n0")));
        assert!(recorded.last().unwrap().contains(&format!("-n{} ", total - 1)));
        clear();
        assert!(lines().is_empty());
    }

    #[test]
    fn lines_show_command_outcome_and_duration() {
        let ok = record_line(&["status", "--porcelain=v2"], Ok(()));
        assert!(ok.contains(" git status --porcelain=v2 "), "{ok}");
        assert!(ok.contains(" →  ok ("), "{ok}");

        let failed = record_line(&["push"], Err("fatal: rejected".to_string()));
        assert!(failed.contains(" git push  →  error: fatal: rejected"), "{failed}");
    }

    #[test]
    fn stdin_commands_are_marked() {
        let line = record_line_stdin(&["hash-object", "-w", "--stdin"], 2400);
        assert!(line.contains("[stdin: 2.3 kB]"), "{line}");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(20_089), (2025, 1, 1));
    }

    fn record_line(args: &[&str], result: Result<(), String>) -> String {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let output = result.map(|()| CmdOutput {
            status: Some(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        });
        let line = format_line(&args, &output, Duration::from_millis(5), None);
        clear();
        line
    }

    fn record_line_stdin(args: &[&str], stdin_bytes: usize) -> String {
        let args: Vec<String> = args.iter().map(|arg| (*arg).to_string()).collect();
        let line = format_line(
            &args,
            &Ok(CmdOutput {
                status: Some(0),
                stdout: Vec::new(),
                stderr: Vec::new(),
            }),
            Duration::from_millis(5),
            Some(stdin_bytes),
        );
        clear();
        line
    }
}
