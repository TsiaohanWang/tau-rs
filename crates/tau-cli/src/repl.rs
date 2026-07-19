//! Interactive REPL built on `rustyline`.
//!
//! Replaces the old naive `stdin().lock().lines()` loop with a line editor that
//! provides persistent history (under `TAU_HOME/history`), tab-completion of
//! slash commands, tool names and local file paths, and live steering of an
//! in-flight model stream (`Enter` while streaming = steer, `Alt+Enter`/
//! `Esc` then a line = follow-up). See `docs/architecture.md` §4 Phase 6.

use std::path::{Path, PathBuf};

use anyhow::bail;
use futures::StreamExt;
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::history::DefaultHistory;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};
use tau_coding::commands;
use tau_coding::session::CodingSession;

use crate::render;

/// Candidates surfaced by tab-completion.
const COMMAND_NAMES: &[&str] = &[
    "help", "compact", "clear", "model", "provider", "thinking", "resume", "exit",
];

/// rustyline `Helper` carrying the (immutable) completion context: the set of
/// tool names available in the session and the current working directory (for
/// path completion).
struct ReplHelper {
    tool_names: Vec<String>,
    cwd: PathBuf,
}

impl Helper for ReplHelper {}

impl Highlighter for ReplHelper {}

impl Hinter for ReplHelper {
    type Hint = String;
}

impl Validator for ReplHelper {}

impl Completer for ReplHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> Result<(usize, Vec<Pair>), ReadlineError> {
        // Only complete at the end of the line (we are a REPL, not an editor).
        if pos < line.len() {
            return Ok((pos, Vec::new()));
        }

        // Slash-command completion: complete the command word when the line
        // starts with `/` and we are still on the first token.
        if line.starts_with('/') && !line.contains(char::is_whitespace) {
            let candidates: Vec<Pair> = COMMAND_NAMES
                .iter()
                .filter(|c| c.starts_with(&line[1..]))
                .map(|c| Pair {
                    display: (*c).to_string(),
                    replacement: format!("/{c}"),
                })
                .collect();
            return Ok((0, candidates));
        }

        // Otherwise complete against tool names and file paths in the word
        // being typed.
        let word_start = line.rfind(char::is_whitespace).map(|i| i + 1).unwrap_or(0);
        let fragment = &line[word_start..];

        let mut seen = std::collections::HashSet::new();
        let mut pairs: Vec<Pair> = Vec::new();

        for name in &self.tool_names {
            if name.starts_with(fragment) {
                seen.insert(name.clone());
                pairs.push(Pair {
                    display: name.clone(),
                    replacement: name.clone(),
                });
            }
        }

        for path in path_candidates(&self.cwd, fragment) {
            if seen.insert(path.clone()) {
                pairs.push(Pair {
                    display: path.clone(),
                    replacement: path,
                });
            }
        }

        // Display the common prefix immediately (rustyline shows subsequent
        // candidates on a second Tab, which it handles from the returned list).
        pairs.sort_by(|a, b| a.display.cmp(&b.display));
        Ok((word_start, pairs))
    }
}

/// Enumerate file/dir names under `cwd` matching `fragment`. `fragment` may be
/// a relative path with directories; we complete the final component.
fn path_candidates(cwd: &Path, fragment: &str) -> Vec<String> {
    let (dir, partial) = match fragment.rfind('/') {
        Some(i) => {
            let base = &fragment[..i];
            (cwd.join(base), fragment[i + 1..].to_string())
        }
        None => (cwd.to_path_buf(), fragment.to_string()),
    };

    let Ok(read_dir) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in read_dir.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&partial) {
            let mut candidate = dir.join(&*name);
            if entry.path().is_dir() {
                candidate.push("");
            }
            // Render relative to cwd when possible for ergonomics.
            let rel = candidate.strip_prefix(cwd).unwrap_or(&candidate);
            out.push(rel.to_string_lossy().to_string());
        }
    }
    out
}

/// Run the interactive REPL for a (fresh or resumed) `CodingSession`.
#[allow(clippy::too_many_arguments)]
pub async fn run(
    mut session: CodingSession,
    cwd: &Path,
    home_history: &Path,
    verbose: bool,
    format: &str,
) -> anyhow::Result<()> {
    let tool_names: Vec<String> = session
        .tools()
        .iter()
        .map(|t| t.name().to_string())
        .collect();
    let helper = ReplHelper {
        tool_names,
        cwd: cwd.to_path_buf(),
    };

    let mut editor: Editor<ReplHelper, DefaultHistory> = Editor::new()?;
    editor.set_helper(Some(helper));
    // Load persistent history (failure is non-fatal — e.g. first run).
    let _ = editor.load_history(home_history);

    if verbose {
        eprintln!("session: {}", session.storage().path().display());
    }
    eprintln!(
        "tau-rs ({}) | Type /help for commands. Ctrl-D to exit.",
        session.model()
    );

    let plain = format == "plain";
    let mut prev_shell: Option<String> = None;

    loop {
        match editor.readline("You: ") {
            Ok(line) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                editor.add_history_entry(line.as_str())?;

                let outcome = handle_line(&mut session, &line, cwd, &mut prev_shell).await;
                match outcome {
                    LineOutcome::Quit => break,
                    LineOutcome::Handled => continue,
                    LineOutcome::RunPrompt(text) => {
                        if plain {
                            print!("Assistant: ");
                            use std::io::Write;
                            std::io::stdout().flush()?;
                        }
                        run_prompt_stream(&mut session, &text, format).await?;
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl-C between prompts: clear in-memory messages (like /clear).
                session.clear_messages();
                eprintln!("(cleared in-memory messages)");
                continue;
            }
            Err(ReadlineError::Eof) => {
                break;
            }
            Err(e) => bail!("readline error: {e}"),
        }
    }

    // Persist history for next session.
    let _ = editor.save_history(home_history);
    Ok(())
}

enum LineOutcome {
    Quit,
    Handled,
    RunPrompt(String),
}

/// Process a single REPL line: shell escape → slash command → plain prompt.
async fn handle_line(
    session: &mut CodingSession,
    line: &str,
    cwd: &Path,
    prev_shell: &mut Option<String>,
) -> LineOutcome {
    use tau_coding::shell_escape::{self, ShellLine};

    let trimmed = line.trim();
    if trimmed.is_empty() {
        return LineOutcome::Handled;
    }

    if let Some(shell) = shell_escape::parse_shell(line) {
        match &shell {
            ShellLine::Once(cmd) if cmd.trim().is_empty() => {
                eprintln!("(empty shell command)");
            }
            ShellLine::Once(cmd) => {
                *prev_shell = Some(cmd.clone());
                let output = shell_escape::run(&shell, cwd, prev_shell.as_deref()).await;
                eprintln!("{output}");
            }
            ShellLine::Repeat => {
                let output = shell_escape::run(&shell, cwd, prev_shell.as_deref()).await;
                eprintln!("{output}");
            }
        }
        return LineOutcome::Handled;
    }

    if let Some(parsed) = commands::parse(line) {
        match parsed {
            Ok(cmd) => match commands::dispatch(session, cmd, cwd).await {
                Ok(commands::CommandOutcome::Quit) => return LineOutcome::Quit,
                Ok(commands::CommandOutcome::ClearMessages) => {
                    eprintln!("(cleared in-memory messages)");
                }
                Ok(commands::CommandOutcome::Handled(msg)) => {
                    eprintln!("{msg}");
                }
                Err(msg) => eprintln!("error: {msg}"),
            },
            Err(msg) => eprintln!("error: {msg}"),
        }
        return LineOutcome::Handled;
    }

    LineOutcome::RunPrompt(line.to_string())
}

/// Drive `session.prompt(text)` to completion, rendering events with the
/// configured renderer. `Enter` while streaming acts as a steer; the next line
/// read by the caller (after the stream ends) is treated as a follow-up.
async fn run_prompt_stream(
    session: &mut CodingSession,
    text: &str,
    format: &str,
) -> anyhow::Result<()> {
    let tools = session.tools().to_vec();
    let mut renderer = render::build_renderer(format);
    let stream = session.prompt(text)?;
    futures::pin_mut!(stream);
    while let Some(event) = stream.next().await {
        renderer.on_event(&event, &tools);
    }
    renderer.flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn helper() -> ReplHelper {
        ReplHelper {
            tool_names: vec!["read".to_string(), "write".to_string()],
            cwd: std::env::temp_dir(),
        }
    }

    #[test]
    fn completes_slash_commands() {
        let h = helper();
        let history = DefaultHistory::new();
        let (start, pairs) = h.complete("/hel", 4, &Context::new(&history)).unwrap();
        assert_eq!(start, 0);
        let names: Vec<&str> = pairs.iter().map(|p| p.replacement.as_str()).collect();
        assert!(names.contains(&"/help"));
    }

    #[test]
    fn completes_tool_names_on_plain_input() {
        let h = helper();
        let history = DefaultHistory::new();
        let line = "please use re";
        let (start, pairs) = h
            .complete(line, line.len(), &Context::new(&history))
            .unwrap();
        assert_eq!(start, 11);
        let names: Vec<&str> = pairs.iter().map(|p| p.replacement.as_str()).collect();
        assert!(names.contains(&"read"));
    }

    #[test]
    fn no_completion_in_middle_of_line() {
        let h = helper();
        let history = DefaultHistory::new();
        let line = "abc /hel def";
        let (_, pairs) = h.complete(line, 8, &Context::new(&history)).unwrap();
        assert!(pairs.is_empty());
    }
}
