//! REPL slash commands.
//!
//! Parses and dispatches the built-in slash commands (`/help`, `/compact`,
//! `/clear`, `/model`, `/provider`, `/exit`, `/resume`). The manual mode
//! switching commands (`Model`, `Provider`) only update in-memory session
//! configuration in Phase 5 — persisting the change to the journal is deferred.

use std::path::Path;

use crate::session::CodingSession;

/// A parsed slash command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `/help` — print the command list.
    Help,
    /// `/compact` — force a context compaction now.
    Compact,
    /// `/clear` — drop in-memory messages (does not touch persisted journal).
    Clear,
    /// `/model <name>` — switch the active model in memory.
    Model(String),
    /// `/provider <name>` — switch the active provider in memory.
    Provider(String),
    /// `/thinking [level]` — show or set the thinking/reasoning-effort level.
    /// With no argument, prints the current level. `off` (or empty) clears it.
    Thinking(Option<String>),
    /// `/exit` — quit the REPL.
    Exit,
    /// `/resume [id|latest]` — resume another session.
    Resume(Option<String>),
}

/// Parse a REPL line into a [`Command`].
///
/// Returns `None` for any line that does not start with `/`. Returns
/// `Err(message)` for a `/`-prefixed line that is not a recognized command.
pub fn parse(line: &str) -> Option<Result<Command, String>> {
    let trimmed = line.trim();
    if !trimmed.starts_with('/') {
        return None;
    }

    let rest = trimmed[1..].trim();
    let (head, arg) = match rest.find(char::is_whitespace) {
        Some(i) => (
            rest[..i].to_string(),
            Some(rest[i + 1..].trim().to_string()),
        ),
        None => (rest.to_string(), None),
    };

    let cmd = match head.as_str() {
        "help" => Command::Help,
        "compact" => Command::Compact,
        "clear" => Command::Clear,
        "exit" | "quit" => Command::Exit,
        "model" => match arg {
            Some(model) if !model.is_empty() => Command::Model(model),
            _ => return Some(Err("usage: /model <name>".to_string())),
        },
        "provider" => match arg {
            Some(provider) if !provider.is_empty() => Command::Provider(provider),
            _ => return Some(Err("usage: /provider <name>".to_string())),
        },
        "thinking" => Command::Thinking(arg.filter(|a| !a.is_empty())),
        "resume" => Command::Resume(arg.filter(|a| !a.is_empty())),
        other => return Some(Err(format!("unknown command: /{other}"))),
    };

    Some(Ok(cmd))
}

/// The result of dispatching a [`Command`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    /// Print this message to the user.
    Handled(String),
    /// Drop all in-memory messages (the REPL then continues).
    ClearMessages,
    /// The REPL should quit.
    Quit,
}

/// Dispatch a parsed command against a live [`CodingSession`].
pub async fn dispatch(
    session: &mut CodingSession,
    cmd: Command,
    cwd: &Path,
) -> anyhow::Result<CommandOutcome> {
    match cmd {
        Command::Help => Ok(CommandOutcome::Handled(help_text())),
        Command::Exit => Ok(CommandOutcome::Quit),
        Command::Clear => {
            session.clear_messages();
            Ok(CommandOutcome::ClearMessages)
        }
        Command::Compact => match session.compact_now().await {
            Ok(true) => Ok(CommandOutcome::Handled("session compacted".to_string())),
            Ok(false) => Ok(CommandOutcome::Handled("nothing to compact".to_string())),
            Err(e) => Ok(CommandOutcome::Handled(format!("compaction failed: {e}"))),
        },
        Command::Model(model) => {
            session.set_model(model.clone());
            Ok(CommandOutcome::Handled(format!(
                "model set to {model} (in-memory)"
            )))
        }
        Command::Provider(provider) => {
            session.set_provider(provider.clone());
            Ok(CommandOutcome::Handled(format!(
                "provider set to {provider} (in-memory)"
            )))
        }
        Command::Thinking(level) => match level {
            None => Ok(CommandOutcome::Handled(format!(
                "thinking level: {}",
                session
                    .thinking_level()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "default (off)".to_string())
            ))),
            Some(lvl) => {
                if lvl == "off" {
                    session.set_thinking_level(None);
                    Ok(CommandOutcome::Handled(
                        "thinking level cleared (provider default)".to_string(),
                    ))
                } else {
                    session.set_thinking_level(Some(lvl.clone()));
                    Ok(CommandOutcome::Handled(format!(
                        "thinking level set to {lvl} (in-memory)"
                    )))
                }
            }
        },
        Command::Resume(id) => {
            let _ = (id, cwd);
            Ok(CommandOutcome::Handled(
                "use the --resume flag at startup to resume a session".to_string(),
            ))
        }
    }
}

/// The text printed by `/help`.
pub fn help_text() -> String {
    [
        "Commands:",
        "  /help              show this help",
        "  /compact           summarize and compress the conversation context",
        "  /clear             drop in-memory messages (keeps the journal)",
        "  /model <name>      switch model (in-memory)",
        "  /provider <name>   switch provider (in-memory)",
        "  /thinking [level]  show or set reasoning-effort (off = default)",
        "  /resume [id]       resume a session (use --resume at startup)",
        "  /exit              quit",
        "",
        "Shell escape:",
        "  ! <command>        run a shell command (output not sent to the agent)",
        "  !!                 repeat the last shell command",
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_compact() {
        assert_eq!(parse("/compact"), Some(Ok(Command::Compact)));
    }

    #[test]
    fn parse_model_with_arg() {
        assert_eq!(
            parse("/model gpt-4o"),
            Some(Ok(Command::Model("gpt-4o".to_string())))
        );
    }

    #[test]
    fn parse_exit() {
        assert_eq!(parse("/exit"), Some(Ok(Command::Exit)));
    }

    #[test]
    fn parse_non_command_returns_none() {
        assert_eq!(parse("hello there"), None);
    }

    #[test]
    fn parse_unknown_command_errors() {
        assert!(matches!(parse("/frobnicate"), Some(Err(_))));
    }

    #[test]
    fn parse_model_without_arg_is_unknown() {
        // `/model` alone has no argument → treated as unknown command.
        assert!(matches!(parse("/model"), Some(Err(_))));
    }

    #[test]
    fn parse_thinking_with_and_without_arg() {
        assert_eq!(parse("/thinking"), Some(Ok(Command::Thinking(None))));
        assert_eq!(
            parse("/thinking high"),
            Some(Ok(Command::Thinking(Some("high".to_string()))))
        );
        assert_eq!(
            parse("/thinking off"),
            Some(Ok(Command::Thinking(Some("off".to_string()))))
        );
    }

    #[test]
    fn help_text_lists_core_commands() {
        let h = help_text();
        assert!(h.contains("/compact"));
        assert!(h.contains("/model"));
        assert!(h.contains("/exit"));
        assert!(h.contains("!!"));
    }
}
