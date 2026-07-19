//! Output renderers for the tau-rs CLI.
//!
//! Three formats are supported, selected by `--format`:
//! - `plain`     — assistant text to stdout, tool activity to stderr (default).
//! - `json`      — one JSON-encoded `AgentEvent` per line on stdout.
//! - `transcript`— a compact, timestamped chat transcript on stdout.
//!
//! Tool events prefer each tool's `render_call`/`render_result` renderers when
//! present, falling back to a minimal `[tool: <name> …]` form otherwise.

use std::io::{self, Write};

use tau_agent::tool::AgentTool;
use tau_types::{
    AgentEvent, AssistantMessageEvent, ToolExecutionEndEvent, ToolExecutionStartEvent,
};

/// A renderer consumes agent events and writes them somewhere.
pub trait EventRenderer {
    /// Handle a single event. `tools` is the live tool set, used to look up
    /// per-tool `render_call`/`render_result` renderers.
    fn on_event(&mut self, ev: &AgentEvent, tools: &[AgentTool]);
    /// Flush any buffered output.
    fn flush(&mut self);
}

/// Look up a tool by name.
fn find_tool<'a>(tools: &'a [AgentTool], name: &str) -> Option<&'a AgentTool> {
    tools.iter().find(|t| t.name() == name)
}

/// Render a tool-start event using the tool's `render_call` renderer, or a
/// fallback line. Returns `None` when there is nothing meaningful to print
/// (e.g. the tool has no renderer and the raw name is uninteresting).
pub fn render_tool_start(ev: &ToolExecutionStartEvent, tools: &[AgentTool]) -> Option<String> {
    if let Some(tool) = find_tool(tools, &ev.tool_name) {
        if let Some(renderer) = &tool.render_call {
            if let Some(s) = renderer(&ev.args) {
                return Some(s);
            }
        }
    }
    Some(format!("[tool: {}]", ev.tool_name))
}

/// Render a tool-end event using the tool's `render_result` renderer, or a
/// fallback preview of the result text.
pub fn render_tool_end(ev: &ToolExecutionEndEvent, tools: &[AgentTool]) -> Option<String> {
    if let Some(tool) = find_tool(tools, &ev.tool_name) {
        if let Some(renderer) = &tool.render_result {
            if let Some(s) = renderer(&ev.result, false) {
                return Some(s);
            }
        }
    }
    let preview = ev.result.text();
    let preview = if preview.len() > 200 {
        format!("{}…", &preview[..200])
    } else {
        preview
    };
    let status = if ev.is_error { " error" } else { "" };
    Some(format!("[tool: {}{} → {}]", ev.tool_name, status, preview))
}

// ---------------------------------------------------------------------------
// Plain renderer
// ---------------------------------------------------------------------------

/// Default renderer: assistant text to stdout, tool events to stderr.
pub struct PlainRenderer<W = io::Stdout> {
    out: W,
    err: io::Stderr,
}

impl PlainRenderer<io::Stdout> {
    pub fn new() -> Self {
        Self {
            out: io::stdout(),
            err: io::stderr(),
        }
    }
}

impl<W: Write> PlainRenderer<W> {
    /// Build with an explicit stdout writer (used in tests).
    #[cfg(test)]
    pub fn with_writer(out: W) -> Self {
        Self {
            out,
            err: io::stderr(),
        }
    }
}

impl<W: Write> EventRenderer for PlainRenderer<W> {
    fn on_event(&mut self, ev: &AgentEvent, tools: &[AgentTool]) {
        match ev {
            AgentEvent::MessageUpdate(update) => {
                if let AssistantMessageEvent::TextDelta(delta) = &update.assistant_message_event {
                    let _ = write!(self.out, "{}", delta.delta);
                    let _ = self.out.flush();
                }
            }
            AgentEvent::ToolExecutionStart(start) => {
                if let Some(s) = render_tool_start(start, tools) {
                    let _ = writeln!(self.err, "{s}");
                }
            }
            AgentEvent::ToolExecutionUpdate(_) => {}
            AgentEvent::ToolExecutionEnd(end) => {
                if let Some(s) = render_tool_end(end, tools) {
                    let _ = writeln!(self.err, "{s}");
                }
            }
            AgentEvent::AgentEnd(_) => {
                let _ = writeln!(self.out);
            }
            _ => {}
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
        let _ = self.err.flush();
    }
}

// ---------------------------------------------------------------------------
// JSON renderer
// ---------------------------------------------------------------------------

/// Emits one JSON-encoded `AgentEvent` per line on stdout.
pub struct JsonEventRenderer<W = io::Stdout> {
    out: W,
}

impl JsonEventRenderer<io::Stdout> {
    pub fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl<W: Write> JsonEventRenderer<W> {
    #[cfg(test)]
    pub fn with_writer(out: W) -> Self {
        Self { out }
    }
}

impl<W: Write> EventRenderer for JsonEventRenderer<W> {
    fn on_event(&mut self, ev: &AgentEvent, _tools: &[AgentTool]) {
        if let Ok(line) = serde_json::to_string(ev) {
            let _ = writeln!(self.out, "{line}");
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

// ---------------------------------------------------------------------------
// Transcript renderer
// ---------------------------------------------------------------------------

/// Compact, timestamped chat transcript on stdout.
pub struct TranscriptRenderer<W = io::Stdout> {
    out: W,
}

impl TranscriptRenderer<io::Stdout> {
    pub fn new() -> Self {
        Self { out: io::stdout() }
    }
}

impl<W: Write> TranscriptRenderer<W> {
    #[cfg(test)]
    pub fn with_writer(out: W) -> Self {
        Self { out }
    }

    fn stamp() -> String {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = now.as_secs() % 86400;
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }
}

impl<W: Write> EventRenderer for TranscriptRenderer<W> {
    fn on_event(&mut self, ev: &AgentEvent, tools: &[AgentTool]) {
        let ts = Self::stamp();
        match ev {
            AgentEvent::MessageStart(_) => {
                let _ = writeln!(self.out, "[{ts}] You:");
            }
            AgentEvent::MessageUpdate(update) => {
                if let AssistantMessageEvent::TextDelta(delta) = &update.assistant_message_event {
                    let _ = write!(self.out, "{}", delta.delta);
                    let _ = self.out.flush();
                }
            }
            AgentEvent::ToolExecutionStart(start) => {
                if let Some(s) = render_tool_start(start, tools) {
                    let _ = writeln!(self.out, "[{ts}] [{tool}] {s}", tool = start.tool_name);
                }
            }
            AgentEvent::ToolExecutionUpdate(_) => {}
            AgentEvent::ToolExecutionEnd(end) => {
                if let Some(s) = render_tool_end(end, tools) {
                    let _ = writeln!(self.out, "[{ts}] [{tool}] {s}", tool = end.tool_name);
                }
            }
            AgentEvent::AgentEnd(_) => {
                let _ = writeln!(self.out);
            }
            _ => {}
        }
    }

    fn flush(&mut self) {
        let _ = self.out.flush();
    }
}

/// Build a renderer for the given format string. Falls back to `plain` for
/// unknown values.
pub fn build_renderer(format: &str) -> Box<dyn EventRenderer> {
    match format {
        "json" => Box::new(JsonEventRenderer::new()),
        "transcript" => Box::new(TranscriptRenderer::new()),
        _ => Box::new(PlainRenderer::new()),
    }
}

/// All supported format names (for `--help` / validation).
pub const FORMATS: &[&str] = &["plain", "json", "transcript"];

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;
    use std::sync::Arc;
    use tau_agent::tool::{AgentTool, ToolError, ToolExecutionMode, ToolExecutor};
    use tau_types::{AgentToolResult, AssistantMessageEvent};

    struct NoopExecutor;

    #[async_trait]
    impl ToolExecutor for NoopExecutor {
        async fn execute(
            &self,
            _id: &str,
            _args: &serde_json::Map<String, serde_json::Value>,
            _signal: Option<tokio_util::sync::CancellationToken>,
            _on_update: Option<&(dyn Fn(AgentToolResult) + Send + Sync)>,
        ) -> Result<AgentToolResult, ToolError> {
            Ok(AgentToolResult::empty())
        }
    }

    fn tool_with_renderers() -> AgentTool {
        AgentTool {
            name: "bash".into(),
            label: "Bash".into(),
            description: String::new(),
            parameters: json!({}),
            executor: Arc::new(NoopExecutor),
            prompt_snippet: None,
            prompt_guidelines: vec![],
            prepare_arguments: None,
            execution_mode: ToolExecutionMode::default(),
            render_call: Some(Arc::new(
                |args: &serde_json::Map<String, serde_json::Value>| {
                    args.get("command")
                        .and_then(|v| v.as_str())
                        .map(|c| format!("$ {c}"))
                },
            )),
            render_result: Some(Arc::new(|r: &AgentToolResult, _: bool| {
                Some(format!(
                    "exit {}",
                    r.details
                        .get("exit_code")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(-1)
                ))
            })),
        }
    }

    fn tool_without_renderers() -> AgentTool {
        let mut t = tool_with_renderers();
        t.render_call = None;
        t.render_result = None;
        t
    }

    fn start_ev() -> AgentEvent {
        AgentEvent::ToolExecutionStart(ToolExecutionStartEvent {
            tool_call_id: "c1".into(),
            tool_name: "bash".into(),
            args: serde_json::from_str(r#"{"command":"ls"}"#).unwrap(),
        })
    }

    fn end_ev() -> AgentEvent {
        let mut r = AgentToolResult::from_text("hello");
        r.details = json!({ "exit_code": 0 });
        AgentEvent::ToolExecutionEnd(ToolExecutionEndEvent {
            tool_call_id: "c1".into(),
            tool_name: "bash".into(),
            result: r,
            is_error: false,
        })
    }

    fn text_ev(text: &str) -> AgentEvent {
        AgentEvent::MessageUpdate(Box::new(tau_types::MessageUpdateEvent {
            message: tau_types::AgentMessage::Assistant(Default::default()),
            assistant_message_event: AssistantMessageEvent::TextDelta(tau_types::TextDeltaEvent {
                content_index: 0,
                delta: text.to_string(),
                partial: Default::default(),
            }),
        }))
    }

    #[test]
    fn plain_renderer_writes_text_and_tool_to_buffers() {
        let mut buf: Vec<u8> = Vec::new();
        let mut r = PlainRenderer::with_writer(&mut buf);
        let tools = vec![tool_with_renderers()];
        r.on_event(&text_ev("hi"), &tools);
        r.on_event(&start_ev(), &tools);
        r.on_event(&end_ev(), &tools);
        r.flush();
        let out = String::from_utf8(buf).unwrap();
        assert_eq!(out, "hi");
    }

    #[test]
    fn transcript_renderer_timestamps_tool_events() {
        let mut buf: Vec<u8> = Vec::new();
        let mut r = TranscriptRenderer::with_writer(&mut buf);
        let tools = vec![tool_with_renderers()];
        r.on_event(&start_ev(), &tools);
        r.on_event(&end_ev(), &tools);
        r.flush();
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("[bash] $ ls"));
        assert!(out.contains("exit 0"));
    }

    #[test]
    fn json_renderer_emits_one_line_per_event() {
        let mut buf: Vec<u8> = Vec::new();
        let mut r = JsonEventRenderer::with_writer(&mut buf);
        let tools = vec![tool_with_renderers()];
        r.on_event(&text_ev("hi"), &tools);
        r.on_event(&start_ev(), &tools);
        r.flush();
        let out = String::from_utf8(buf).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2);
        // Each line must parse back as an AgentEvent-shaped JSON object.
        for l in &lines {
            let v: serde_json::Value = serde_json::from_str(l).unwrap();
            assert!(v.get("type").is_some());
        }
    }

    #[test]
    fn render_tool_start_uses_renderer_then_fallback() {
        let with = tool_with_renderers();
        let without = tool_without_renderers();
        assert_eq!(
            render_tool_start(
                &match &start_ev() {
                    AgentEvent::ToolExecutionStart(e) => e.clone(),
                    _ => unreachable!(),
                },
                &[with]
            ),
            Some("$ ls".to_string())
        );
        assert_eq!(
            render_tool_start(
                &match &start_ev() {
                    AgentEvent::ToolExecutionStart(e) => e.clone(),
                    _ => unreachable!(),
                },
                &[without]
            ),
            Some("[tool: bash]".to_string())
        );
    }

    #[test]
    fn render_tool_end_uses_renderer_then_fallback() {
        let with = tool_with_renderers();
        let without = tool_without_renderers();
        assert_eq!(
            render_tool_end(
                &match &end_ev() {
                    AgentEvent::ToolExecutionEnd(e) => e.clone(),
                    _ => unreachable!(),
                },
                &[with]
            ),
            Some("exit 0".to_string())
        );
        let fb = render_tool_end(
            &match &end_ev() {
                AgentEvent::ToolExecutionEnd(e) => e.clone(),
                _ => unreachable!(),
            },
            &[without],
        );
        assert!(fb.unwrap().contains("[tool: bash"));
    }
}
