//! Ratatui TUI entry point.
//!
//! Architecture: the TUI depends only on `tau-types` events and on the
//! `CodingSession` read-only surface (via its `harness()`). It never reaches
//! into `tau-agent` / `tau-ai` HTTP internals. The original Python
//! `tau_coding/tui/app.py` is the port reference, but we adapt its event loop
//! to a crossterm + `tokio::select!` model.
//!
//! Streaming model: while a prompt is running we drive `CodingSession::prompt`
//! (which borrows `&mut session`) inside a *nested* loop. Steering / cancel /
//! queue inspection go through a *cloned* `AgentHarness` handle so they don't
//! need `&mut session` and conflict with the live stream borrow.

use std::io::{self, Stdout};
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event as CEvent, KeyCode, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use futures::StreamExt;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use tau_agent::harness::AgentHarness;
use tau_coding::commands;
use tau_coding::session::CodingSession;
use tau_coding::shell_escape;
use tokio::sync::mpsc;

use crate::tui::adapter::TuiEventAdapter;
use crate::tui::state::TuiState;
use crate::tui::ui;

const DRAW_TICK: Duration = Duration::from_millis(80);

struct App {
    adapter: TuiEventAdapter,
    input: String,
    cursor: usize,
    running: bool,
    model: String,
    thinking_level: String,
}

impl App {
    fn new(model: String, thinking_level: String) -> Self {
        Self {
            adapter: TuiEventAdapter::default(),
            input: String::new(),
            cursor: 0,
            running: false,
            model,
            thinking_level,
        }
    }

    fn state(&self) -> &TuiState {
        self.adapter.state()
    }

    fn state_mut(&mut self) -> &mut TuiState {
        self.adapter.state_mut()
    }
}

/// Run the interactive TUI.
pub async fn run(
    mut session: CodingSession,
    cwd: &Path,
    _home_history: &Path,
    verbose: bool,
    _format: &str,
) -> Result<()> {
    let model = session.model().to_string();
    let thinking_level = session.thinking_level().unwrap_or("none").to_string();
    let harness: AgentHarness = session.harness().clone();

    let mut app = App::new(model.clone(), thinking_level.clone());
    app.state_mut().load_messages(&session.messages());
    if verbose {
        app.state_mut().add_item_with(
            crate::tui::state::ChatItemRole::System,
            format!("session: {}", session.storage().path().display()),
            None,
            None,
            false,
            None,
            None,
        );
    }

    let mut terminal = setup_terminal()?;

    let (key_tx, mut key_rx) = mpsc::unbounded_channel::<CEvent>();
    let reader = tokio::spawn(async move {
        while let Ok(ev) = event::read() {
            if key_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let result = app_loop(
        &mut app,
        &mut terminal,
        &mut session,
        &harness,
        cwd,
        &mut key_rx,
    )
    .await;

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();
    reader.abort();
    result
}

#[allow(clippy::too_many_arguments)]
async fn app_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    session: &mut CodingSession,
    harness: &AgentHarness,
    cwd: &Path,
    key_rx: &mut mpsc::UnboundedReceiver<CEvent>,
) -> Result<()> {
    loop {
        {
            let mut frame = terminal.get_frame();
            ui::draw(
                &mut frame,
                app.state(),
                &app.input,
                app.cursor,
                &app.model,
                &app.thinking_level,
                app.running,
                harness,
            );
        }
        terminal.flush()?;

        tokio::select! {
            maybe_key = key_rx.recv() => {
                if let Some(CEvent::Key(key)) = maybe_key {
                    if handle_key(app, key, session, harness, cwd).await? {
                        return Ok(());
                    }
                }
            }
            _ = tokio::time::sleep(DRAW_TICK) => {}
        }
    }
}

/// Returns `true` if the app should quit.
async fn handle_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    session: &mut CodingSession,
    harness: &AgentHarness,
    cwd: &Path,
) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match (ctrl, key.code) {
        (true, KeyCode::Char('c')) => {
            if app.running {
                harness.cancel();
            } else {
                session.clear_messages();
                app.state_mut().clear();
            }
            Ok(false)
        }
        (true, KeyCode::Char('d')) => Ok(true),
        (true, KeyCode::Char('o')) => {
            app.state_mut().toggle_tool_results();
            Ok(false)
        }
        (true, KeyCode::Char('t')) => {
            app.state_mut().toggle_thinking();
            Ok(false)
        }
        (_, KeyCode::Enter) => {
            let line = app.input.trim().to_string();
            if !line.is_empty() {
                let input = std::mem::take(&mut app.input);
                app.cursor = 0;
                app.running = true;
                run_prompt(app, session, harness, &input, cwd).await?;
                app.running = false;
            }
            Ok(false)
        }
        (_, KeyCode::Esc) => {
            if app.running {
                harness.cancel();
            }
            Ok(false)
        }
        (_, KeyCode::Char(ch)) => {
            app.input.insert(app.cursor, ch);
            app.cursor += 1;
            Ok(false)
        }
        (_, KeyCode::Backspace) => {
            if app.cursor > 0 {
                app.cursor -= 1;
                app.input.remove(app.cursor);
            }
            Ok(false)
        }
        (_, KeyCode::Left) => {
            if app.cursor > 0 {
                app.cursor -= 1;
            }
            Ok(false)
        }
        (_, KeyCode::Right) => {
            if app.cursor < app.input.len() {
                app.cursor += 1;
            }
            Ok(false)
        }
        (_, KeyCode::Home) => {
            app.cursor = 0;
            Ok(false)
        }
        (_, KeyCode::End) => {
            app.cursor = app.input.len();
            Ok(false)
        }
        _ => Ok(false),
    }
}

/// Dispatch one input line, mirroring the REPL's shell-escape / slash-command /
/// chat precedence, then stream the resulting prompt into the adapter.
async fn run_prompt(
    app: &mut App,
    session: &mut CodingSession,
    harness: &AgentHarness,
    line: &str,
    cwd: &Path,
) -> Result<()> {
    harness.cancel();

    if let Some(shell) = shell_escape::parse_shell(line) {
        let output = shell_escape::run(&shell, cwd, None).await;
        app.state_mut().add_item_with(
            crate::tui::state::ChatItemRole::System,
            output,
            None,
            None,
            false,
            None,
            None,
        );
        return Ok(());
    }

    if let Some(parsed) = commands::parse(line) {
        match parsed {
            Ok(cmd) => match commands::dispatch(session, cmd, cwd).await {
                Ok(commands::CommandOutcome::Quit) => {}
                Ok(commands::CommandOutcome::ClearMessages) => app.state_mut().clear(),
                Ok(commands::CommandOutcome::Handled(msg)) => {
                    app.state_mut().add_item_with(
                        crate::tui::state::ChatItemRole::System,
                        msg,
                        None,
                        None,
                        false,
                        None,
                        None,
                    );
                }
                Err(e) => {
                    app.state_mut().add_item_with(
                        crate::tui::state::ChatItemRole::Error,
                        format!("error: {e}"),
                        None,
                        None,
                        false,
                        None,
                        None,
                    );
                }
            },
            Err(msg) => {
                app.state_mut().add_item_with(
                    crate::tui::state::ChatItemRole::Error,
                    msg,
                    None,
                    None,
                    false,
                    None,
                    None,
                );
            }
        }
        return Ok(());
    }

    app.state_mut().add_user_message(line, None, None);
    run_stream(app, session, line).await?;
    Ok(())
}

async fn run_stream(app: &mut App, session: &mut CodingSession, text: &str) -> Result<()> {
    let mut stream = Box::pin(session.prompt(text)?);
    loop {
        tokio::select! {
            maybe_ev = stream.next() => {
                match maybe_ev {
                    Some(ev) => app.adapter.apply(&ev),
                    None => break,
                }
            }
            _ = tokio::time::sleep(DRAW_TICK) => {}
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor().ok();
    terminal.clear()?;
    Ok(terminal)
}
