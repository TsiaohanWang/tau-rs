//! `!`-prefixed shell escape for the REPL.
//!
//! Lets the user run a shell command without it ever reaching the agent
//! provider, so shell output does not pollute the conversation context.

use std::path::Path;
use std::process::Command;

/// A parsed shell escape line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellLine {
    /// `! <command>` — run this command once.
    Once(String),
    /// `!!` — repeat the most recently run shell command.
    Repeat,
}

/// Parse a single REPL line into a shell escape.
///
/// Returns `Some` only when the line begins with `!`:
/// - `"! echo hi"`   → `Some(Once("echo hi"))`
/// - `"!!"`          → `Some(Repeat)`
/// - `"! "` (only a bang, empty command) → `Some(Once(""))`
/// - a line not starting with `!` → `None` (treat as a normal prompt).
///
/// A `!!` that is not a standalone repeat (e.g. `"!!not shell"`) is *not* a
/// valid escape and returns `None`, so it falls through to the prompt path.
pub fn parse_shell(line: &str) -> Option<ShellLine> {
    let trimmed = line.trim();
    if trimmed == "!!" || trimmed.starts_with("!! ") {
        return Some(ShellLine::Repeat);
    }
    if trimmed.starts_with("!!") {
        // Ambiguous `!!`-prefixed text that is not a clean repeat.
        return None;
    }
    if trimmed.starts_with('!') {
        let rest = trimmed.strip_prefix('!').unwrap().trim_start();
        return Some(ShellLine::Once(rest.to_string()));
    }
    None
}

/// Run a shell escape. `prev` (the most recent `Once` command) is replayed when
/// `shell` is [`ShellLine::Repeat`]. Returns the captured stdout+stderr,
/// truncated to the first 4 KiB.
pub async fn run(shell: &ShellLine, cwd: &Path, prev: Option<&str>) -> String {
    let command = match shell {
        ShellLine::Once(cmd) => cmd.clone(),
        ShellLine::Repeat => match prev {
            Some(cmd) => cmd.to_string(),
            None => return "(no previous shell command)".to_string(),
        },
    };

    if command.trim().is_empty() {
        return "(empty shell command)".to_string();
    }

    let output = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .current_dir(cwd)
        .output();

    match output {
        Ok(out) => {
            let mut combined = Vec::with_capacity(out.stdout.len() + out.stderr.len());
            combined.extend_from_slice(&out.stdout);
            combined.extend_from_slice(&out.stderr);
            let mut s = String::from_utf8_lossy(&combined).to_string();
            let limit = 4 * 1024;
            if s.len() > limit {
                s.truncate(limit);
                s.push_str("\n…(truncated)");
            }
            if s.is_empty() {
                format!("(exit code {})", out.status.code().unwrap_or(-1))
            } else {
                s
            }
        }
        Err(e) => format!("(failed to spawn shell: {e})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_once() {
        assert_eq!(
            parse_shell("! echo hi"),
            Some(ShellLine::Once("echo hi".to_string()))
        );
    }

    #[test]
    fn parse_repeat() {
        assert_eq!(parse_shell("!!"), Some(ShellLine::Repeat));
    }

    #[test]
    fn parse_bang_then_space_is_empty_once() {
        assert_eq!(parse_shell("! "), Some(ShellLine::Once("".to_string())));
    }

    #[test]
    fn parse_non_shell_returns_none() {
        assert_eq!(parse_shell("echo hi"), None);
        assert_eq!(parse_shell("!!not shell"), None);
    }

    #[tokio::test]
    async fn run_once_captures_output() {
        let out = run(
            &ShellLine::Once("echo hi".to_string()),
            Path::new("."),
            None,
        )
        .await;
        assert_eq!(out, "hi\n");
    }

    #[tokio::test]
    async fn run_repeat_replays_prev() {
        let out = run(&ShellLine::Repeat, Path::new("."), Some("echo hi")).await;
        assert_eq!(out, "hi\n");
    }

    #[tokio::test]
    async fn run_repeat_without_prev() {
        let out = run(&ShellLine::Repeat, Path::new("."), None).await;
        assert_eq!(out, "(no previous shell command)");
    }
}
