//! Rendering for the TUI. Pure ratatui; depends only on `TuiState` plus the
//! read-only `AgentHarness` handle (for the queued-message count). No agent /
//! provider internals are reachable here.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState, Wrap,
};
use tau_agent::harness::AgentHarness;

use crate::tui::state::{ChatItem, ChatItemRole, TuiState};

/// Draw a single frame.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    frame: &mut ratatui::Frame<'_>,
    state: &TuiState,
    input: &str,
    cursor: usize,
    model: &str,
    thinking_level: &str,
    running: bool,
    harness: &AgentHarness,
) {
    let size = frame.area();
    if size.height < 4 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(size);

    draw_transcript(frame, state, chunks[0]);
    draw_input(frame, input, cursor, chunks[1]);
    draw_status(
        frame,
        state,
        model,
        thinking_level,
        running,
        harness,
        chunks[2],
    );
}

fn draw_transcript(frame: &mut ratatui::Frame<'_>, state: &TuiState, area: Rect) {
    let mut lines: Vec<Line<'static>> = Vec::new();
    for item in state.items() {
        render_item(
            item,
            &mut lines,
            state.show_tool_results(),
            state.show_thinking(),
        );
    }

    let total = lines.len();
    let visible = area.height as usize;
    // Compute actual scroll offset: when auto-scrolling, stick to bottom
    let scroll_row = if state.auto_scroll {
        total.saturating_sub(visible) as u16
    } else {
        state
            .scroll_offset_rows
            .min(total.saturating_sub(visible) as u16)
    };
    let para = Paragraph::new(Text::from(lines))
        .wrap(Wrap { trim: false })
        .scroll((scroll_row, 0));
    frame.render_widget(para, area);

    if total > visible {
        let mut scrollbar = ScrollbarState::new(total).position(scroll_row as usize);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            area,
            &mut scrollbar,
        );
    }
}

fn render_item(
    item: &ChatItem,
    out: &mut Vec<Line<'static>>,
    show_tool_results: bool,
    show_thinking: bool,
) {
    match item.role {
        ChatItemRole::User => {
            out.push(Line::from(Span::styled(
                "You",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            for l in item.text.lines() {
                out.push(Line::from(l.to_string()));
            }
        }
        ChatItemRole::Assistant => {
            out.push(Line::from(Span::styled(
                "Assistant",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            let body = item.text.trim();
            if !body.is_empty() {
                for l in body.lines() {
                    out.push(Line::from(l.to_string()));
                }
            }
            if let Some(stop) = item.stop_reason.as_ref() {
                out.push(Line::from(Span::styled(
                    format!("  [{}]", stop),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        ChatItemRole::Thinking => {
            let t = item.text.trim();
            if show_thinking && !t.is_empty() {
                out.push(Line::from(Span::styled(
                    "  thinking:",
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                )));
                for l in t.lines() {
                    out.push(Line::from(Span::styled(
                        format!("  {}", l),
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                }
            }
        }
        ChatItemRole::Tool => {
            out.push(Line::from(Span::styled(
                format!("Tool {}", item.text.trim()),
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )));
            if show_tool_results {
                if let Some(result) = item.tool_result_text.as_ref() {
                    let r = result.trim();
                    if !r.is_empty() {
                        for l in r.lines() {
                            out.push(Line::from(Span::styled(
                                format!("  {}", l),
                                Style::default().fg(Color::DarkGray),
                            )));
                        }
                    }
                }
            } else {
                out.push(Line::from(Span::styled(
                    "  (result hidden; Ctrl-O to toggle)",
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        ChatItemRole::Error => {
            out.push(Line::from(Span::styled(
                format!("Error: {}", item.text.trim()),
                Style::default().fg(Color::Red),
            )));
        }
        _ => {
            let t = item.text.trim();
            if !t.is_empty() {
                out.push(Line::from(Span::styled(
                    t.to_string(),
                    Style::default().fg(Color::Yellow),
                )));
            }
        }
    }
    out.push(Line::from(""));
}

fn draw_input(frame: &mut ratatui::Frame<'_>, input: &str, cursor: usize, area: Rect) {
    let prompt = "› ";
    let text = format!("{}{}", prompt, input);
    let para = Paragraph::new(text)
        .style(Style::default().fg(Color::White))
        .block(Block::default().borders(Borders::ALL).title(" Input "));
    frame.render_widget(para, area);

    let cx = area.x + 1 + prompt.len() as u16 + cursor as u16;
    if cx < area.x + area.width.saturating_sub(1) {
        frame.set_cursor_position((cx, area.y + 1));
    }
}

fn draw_status(
    frame: &mut ratatui::Frame<'_>,
    _state: &TuiState,
    model: &str,
    thinking_level: &str,
    running: bool,
    harness: &AgentHarness,
    area: Rect,
) {
    let queued = harness.queued_messages().count();
    let running_str = if running { "● running" } else { "○ idle" };
    let text = format!(
        " {} | model: {} | think: {} | queued: {} | Ctrl-O tools | Ctrl-T thinking | Ctrl-D quit",
        running_str, model, thinking_level, queued
    );
    let para = Paragraph::new(text).style(Style::default().fg(Color::DarkGray));
    frame.render_widget(para, area);
}
