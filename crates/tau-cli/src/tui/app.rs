//! Ratatui TUI entry point.
//!
//! Architecture: the TUI depends only on `tau-types` events and on the
//! `CodingSession` read-only surface (via its `harness()`). It never reaches
//! into `tau-agent` / `tau-ai` HTTP internals. The original Python
//! `tau_coding/tui/app.py` is the port reference, but we adapt its event loop
//! to a crossterm + `tokio::select!` model.
//!
//! Streaming model: while idle the outer loop draws and processes key events.
//! When Enter fires a prompt, we enter an inner `tokio::select!` loop that
//! drives the `CodingSession::prompt` stream (which borrows `&mut session`)
//! together with **the same** key channel (for steer, cancel, scrolling) and
//! per-frame redraw.  A cloned `AgentHarness` handle provides steer/cancel/
//! queue without needing `&mut session` during the stream.

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
use crate::tui::state::{ChatItemRole, TuiState};
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

    fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
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
            ChatItemRole::System,
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
    // Single key reader — reused for both idle and streaming states.
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

/// Outer loop: idle → draw + wait for key.  When Enter fires a prompt,
/// call `dispatch_line` which runs the stream to completion in an inner
/// select! loop that also processes keys and redraws every tick.
async fn app_loop(
    app: &mut App,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    session: &mut CodingSession,
    harness: &AgentHarness,
    cwd: &Path,
    key_rx: &mut mpsc::UnboundedReceiver<CEvent>,
) -> Result<()> {
    loop {
        draw_frame(terminal, app, harness);

        tokio::select! {
            maybe_key = key_rx.recv() => {
                if let Some(CEvent::Key(key)) = maybe_key {
                    if handle_idle_key(app, key, session, harness, cwd, terminal, key_rx).await? {
                        return Ok(());
                    }
                }
            }
            _ = tokio::time::sleep(DRAW_TICK) => {}
        }
    }
}

fn draw_frame(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    app: &App,
    harness: &AgentHarness,
) {
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
    let _ = terminal.flush();
}

/// Handle a key when idle (no active stream).
/// Returns `true` if the app should quit.
async fn handle_idle_key(
    app: &mut App,
    key: crossterm::event::KeyEvent,
    session: &mut CodingSession,
    harness: &AgentHarness,
    cwd: &Path,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    key_rx: &mut mpsc::UnboundedReceiver<CEvent>,
) -> Result<bool> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    match (ctrl, key.code) {
        (true, KeyCode::Char('c')) => {
            session.clear_messages();
            app.state_mut().clear();
            app.clear_input();
            return Ok(false);
        }
        (true, KeyCode::Char('d')) => return Ok(true),
        (true, KeyCode::Char('o')) => {
            app.state_mut().toggle_tool_results();
            return Ok(false);
        }
        (true, KeyCode::Char('t')) => {
            app.state_mut().toggle_thinking();
            return Ok(false);
        }
        (true, KeyCode::Char('l')) => {
            app.state_mut().scroll_to_bottom();
            return Ok(false);
        }
        (_, KeyCode::Enter) => {
            let line = app.input.trim().to_string();
            if !line.is_empty() {
                let input = std::mem::take(&mut app.input);
                app.cursor = 0;
                app.running = true;
                app.state_mut().auto_scroll = true;

                if dispatch_line(app, session, harness, &input, cwd).await? {
                    return Ok(true);
                }

                // Back to idle — run the prompt stream with concurrent key
                // handling and redraw.
                let mut stream = Box::pin(session.prompt(&input)?);
                run_stream_loop(app, &mut stream, harness, terminal, key_rx).await?;
                app.running = false;
            }
            return Ok(false);
        }
        (_, KeyCode::PageUp) => {
            app.state_mut().page_up(10);
            return Ok(false);
        }
        (_, KeyCode::PageDown) => {
            app.state_mut().page_down(10);
            return Ok(false);
        }
        (_, KeyCode::Up) => {
            app.state_mut().scroll_up(1);
            return Ok(false);
        }
        (_, KeyCode::Down) => {
            app.state_mut().scroll_down(1);
            return Ok(false);
        }
        _ => {}
    }

    // Input editing (Unicode-safe byte cursor)
    match key.code {
        KeyCode::Char(ch) => {
            app.input.insert(app.cursor, ch);
            app.cursor += ch.len_utf8();
        }
        KeyCode::Backspace => {
            let c = app.cursor;
            app.cursor = cursor_byte_backspace(&mut app.input, c);
        }
        KeyCode::Delete => {
            cursor_byte_delete(&mut app.input, app.cursor);
        }
        KeyCode::Left => {
            app.cursor = cursor_byte_left(&app.input, app.cursor);
        }
        KeyCode::Right => {
            app.cursor = cursor_byte_right(&app.input, app.cursor);
        }
        KeyCode::Home => {
            app.cursor = 0;
        }
        KeyCode::End => {
            app.cursor = app.input.len();
        }
        _ => {}
    }

    Ok(false)
}

/// Dispatch one input line (shell/command).  Returns `true` if the app
/// should quit.  For chat messages this is a no-op (the calller handles
/// stream initiation).
async fn dispatch_line(
    app: &mut App,
    session: &mut CodingSession,
    harness: &AgentHarness,
    line: &str,
    cwd: &Path,
) -> Result<bool> {
    harness.cancel();

    if let Some(shell) = shell_escape::parse_shell(line) {
        let output = shell_escape::run(&shell, cwd, None).await;
        app.state_mut()
            .add_item_with(ChatItemRole::System, output, None, None, false, None, None);
        return Ok(false);
    }

    if let Some(parsed) = commands::parse(line) {
        match parsed {
            Ok(cmd) => match commands::dispatch(session, cmd, cwd).await {
                Ok(commands::CommandOutcome::Quit) => return Ok(true),
                Ok(commands::CommandOutcome::ClearMessages) => {
                    app.state_mut().clear();
                    app.clear_input();
                }
                Ok(commands::CommandOutcome::Handled(msg)) => {
                    app.state_mut().add_item_with(
                        ChatItemRole::System,
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
                        ChatItemRole::Error,
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
                    ChatItemRole::Error,
                    msg,
                    None,
                    None,
                    false,
                    None,
                    None,
                );
            }
        }
        return Ok(false);
    }

    // Chat message: the adapter adds User from the stream's MessageEnd(User)
    // event — do NOT call add_user_message here or it doubles.
    Ok(false)
}

/// Inner loop: drive the prompt stream with concurrent key handling and
/// per-frame redraw, using the **same** key channel as the outer loop.
async fn run_stream_loop(
    app: &mut App,
    stream: &mut (impl futures::Stream<Item = tau_types::AgentEvent> + std::marker::Unpin + Send),
    harness: &AgentHarness,
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    key_rx: &mut mpsc::UnboundedReceiver<CEvent>,
) -> Result<()> {
    loop {
        draw_frame(terminal, app, harness);

        tokio::select! {
            maybe_key = key_rx.recv() => {
                if let Some(CEvent::Key(key)) = maybe_key {
                    handle_streaming_key(app, key, harness);
                }
            }
            maybe_ev = stream.next() => {
                match maybe_ev {
                    Some(ev) => {
                        app.adapter.apply(&ev);
                        app.state_mut().scroll_to_bottom();
                    }
                    None => {
                        app.state_mut().scroll_to_bottom();
                        return Ok(());
                    }
                }
            }
            _ = tokio::time::sleep(DRAW_TICK) => {}
        }
    }
}

/// Key handler during active streaming — only a subset of keys is meaningful.
fn handle_streaming_key(app: &mut App, key: crossterm::event::KeyEvent, harness: &AgentHarness) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Cancel / steer
    match key.code {
        KeyCode::Esc => {
            harness.cancel();
            return;
        }
        KeyCode::Enter => {
            let input = std::mem::take(&mut app.input);
            let trimmed = input.trim().to_string();
            if !trimmed.is_empty() {
                harness.steer(trimmed);
            }
            app.cursor = 0;
            return;
        }
        _ => {}
    }

    match (ctrl, key.code) {
        (true, KeyCode::Char('c')) => {
            harness.cancel();
            return;
        }
        (true, KeyCode::Char('o')) => {
            app.state_mut().toggle_tool_results();
            return;
        }
        (true, KeyCode::Char('t')) => {
            app.state_mut().toggle_thinking();
            return;
        }
        (true, KeyCode::Char('l')) => {
            app.state_mut().scroll_to_bottom();
            return;
        }
        _ => {}
    }

    // Scroll
    match key.code {
        KeyCode::PageUp => {
            app.state_mut().page_up(10);
            return;
        }
        KeyCode::PageDown => {
            app.state_mut().page_down(10);
            return;
        }
        KeyCode::Up => {
            app.state_mut().scroll_up(1);
            return;
        }
        KeyCode::Down => {
            app.state_mut().scroll_down(1);
            return;
        }
        _ => {}
    }

    // Input editing (for steer text, Unicode-safe)
    match key.code {
        KeyCode::Char(ch) => {
            app.input.insert(app.cursor, ch);
            app.cursor += ch.len_utf8();
        }
        KeyCode::Backspace => {
            let c = app.cursor;
            app.cursor = cursor_byte_backspace(&mut app.input, c);
        }
        KeyCode::Delete => {
            cursor_byte_delete(&mut app.input, app.cursor);
        }
        KeyCode::Left => {
            app.cursor = cursor_byte_left(&app.input, app.cursor);
        }
        KeyCode::Right => {
            app.cursor = cursor_byte_right(&app.input, app.cursor);
        }
        KeyCode::Home => {
            app.cursor = 0;
        }
        KeyCode::End => {
            app.cursor = app.input.len();
        }
        _ => {}
    }
}

/// Byte-safe cursor operations — all positions are byte offsets guaranteed
/// to land on character boundaries.
/// Move cursor left one character. Returns the new byte position.
fn cursor_byte_left(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    // Find the start of the character before `cursor`
    let prev = s[..cursor].char_indices().next_back();
    match prev {
        Some((i, _)) => i,
        None => 0,
    }
}

/// Move cursor right one character. Returns the new byte position.
fn cursor_byte_right(s: &str, cursor: usize) -> usize {
    if cursor >= s.len() {
        return s.len();
    }
    // Find the start of the character at or after `cursor`
    s[cursor..]
        .chars()
        .next()
        .map(|ch| cursor + ch.len_utf8())
        .unwrap_or(s.len())
}

/// Backspace: remove the character before the cursor and return the new
/// byte position.
fn cursor_byte_backspace(s: &mut String, cursor: usize) -> usize {
    if cursor == 0 || s.is_empty() {
        return 0;
    }
    let prev = s[..cursor].char_indices().next_back();
    if let Some((byte_pos, ch)) = prev {
        let len = ch.len_utf8();
        s.drain(byte_pos..byte_pos + len);
        byte_pos
    } else {
        cursor
    }
}

/// Delete: remove the character at/after the cursor. Cursor position
/// stays the same (the next character slides into its place).
fn cursor_byte_delete(s: &mut String, cursor: usize) {
    if cursor >= s.len() {
        return;
    }
    if let Some((_, ch)) = s[cursor..].char_indices().next() {
        s.drain(cursor..cursor + ch.len_utf8());
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    // Hide terminal cursor — we render our own cursor block in draw_input
    terminal.hide_cursor()?;
    terminal.clear()?;
    Ok(terminal)
}
