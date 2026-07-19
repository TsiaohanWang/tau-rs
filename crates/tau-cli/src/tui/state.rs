//! Pure display state for the ratatui TUI (mirrors original `tui/state.py`).
//!
//! No ratatui/terminal dependency — this is plain data so it can be driven and
//! unit-tested through [`super::adapter::TuiEventAdapter`] without a terminal.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use tau_types::{
    AgentMessage, AssistantContent, AssistantMessage, StopReason, ToolCall, ToolResultContent,
    ToolResultMessage,
};

fn format_stop_reason(reason: &StopReason) -> Option<String> {
    match reason {
        StopReason::Stop => None,
        StopReason::Length => Some("length".to_string()),
        StopReason::ToolUse => Some("tool_use".to_string()),
        StopReason::Error => Some("error".to_string()),
        StopReason::Aborted => Some("aborted".to_string()),
    }
}

/// One rendered item in the TUI transcript (mirrors `ChatItem`).
#[derive(Debug, Clone)]
pub struct ChatItem {
    pub role: ChatItemRole,
    pub text: String,
    pub tool_call_id: Option<String>,
    pub tool_result_text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_arguments: Option<Value>,
    pub update_text: Option<String>,
    pub started_at: Option<Instant>,
    pub always_show_tool_result: bool,
    pub custom_type: Option<String>,
    pub details: Option<Value>,
    pub stop_reason: Option<String>,
}

impl ChatItem {
    fn new(role: ChatItemRole, text: String) -> Self {
        ChatItem {
            role,
            text,
            tool_call_id: None,
            tool_result_text: None,
            tool_name: None,
            tool_arguments: None,
            update_text: None,
            started_at: None,
            always_show_tool_result: false,
            custom_type: None,
            details: None,
            stop_reason: None,
        }
    }
}

/// Transcript item role (mirrors `TranscriptRole` / `state.ChatItemRole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatItemRole {
    User,
    Assistant,
    Tool,
    Skill,
    Thinking,
    Error,
    System,
    Custom,
    BranchSummary,
    CompactionSummary,
    #[allow(dead_code)]
    Status,
}

const TOOL_RESULT_PREVIEW_CHARS: usize = 2_000;

/// Mutable display state for the interactive TUI (mirrors `TuiState`).
#[derive(Debug, Clone, Default)]
pub struct TuiState {
    pub items: Vec<ChatItem>,
    pub assistant_buffer: String,
    pub running: bool,
    pub error: Option<String>,
    pub show_tool_results: bool,
    pub show_thinking: bool,
    assistant_start_index: Option<usize>,
    tool_items_by_call_id: HashMap<String, usize>,
}

impl TuiState {
    #[allow(dead_code)]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn items(&self) -> &[ChatItem] {
        &self.items
    }

    pub fn show_tool_results(&self) -> bool {
        self.show_tool_results
    }

    pub fn show_thinking(&self) -> bool {
        self.show_thinking
    }

    /// Transcript scroll offset for the paragraph widget (top-anchored for now).
    pub fn scroll_offset(&self) -> (u16, u16) {
        (0, 0)
    }

    pub(crate) fn add_item(&mut self, role: ChatItemRole, text: String) -> &mut ChatItem {
        self.items.push(ChatItem::new(role, text));
        self.items.last_mut().unwrap()
    }

    /// Append a transcript item, optionally binding it to a tool call id.
    #[allow(clippy::too_many_arguments)]
    pub fn add_item_with(
        &mut self,
        role: ChatItemRole,
        text: String,
        tool_call_id: Option<String>,
        tool_result_text: Option<String>,
        always_show: bool,
        custom_type: Option<String>,
        details: Option<Value>,
    ) {
        let item = self.add_item(role, text);
        item.tool_call_id = tool_call_id.clone();
        item.tool_result_text = tool_result_text;
        item.always_show_tool_result = always_show;
        item.custom_type = custom_type;
        item.details = details;
        if let Some(id) = tool_call_id {
            if role == ChatItemRole::Tool || role == ChatItemRole::Skill {
                let idx = self.items.len() - 1;
                self.tool_items_by_call_id.insert(id, idx);
            }
        }
    }

    /// Append a user-authored message, compacting summary/skill messages.
    #[allow(clippy::too_many_arguments)]
    pub fn add_user_message(
        &mut self,
        content: &str,
        custom_type: Option<String>,
        details: Option<Value>,
    ) {
        if let Some(ct) = custom_type {
            self.add_item_with(
                ChatItemRole::Custom,
                content.to_string(),
                None,
                None,
                false,
                Some(ct),
                details,
            );
            return;
        }

        if let Some(summary) = parse_branch_summary(content) {
            self.add_item_with(
                ChatItemRole::BranchSummary,
                "Branch summary (Ctrl+O to expand)".to_string(),
                None,
                Some(summary),
                false,
                None,
                None,
            );
            return;
        }
        if let Some(summary) = parse_compaction_summary(content) {
            self.add_item_with(
                ChatItemRole::CompactionSummary,
                "Compaction summary (Ctrl+O to expand)".to_string(),
                None,
                Some(summary),
                false,
                None,
                None,
            );
            return;
        }
        self.add_item(ChatItemRole::User, content.to_string());
    }

    /// Append a thinking/reasoning fragment to the current (or a new) block.
    pub fn add_thinking_delta(&mut self, delta: &str) {
        if let Some(last) = self.items.last_mut() {
            if last.role == ChatItemRole::Thinking {
                last.text.push_str(delta);
                return;
            }
        }
        self.add_item(ChatItemRole::Thinking, delta.to_string());
    }

    /// Append a collapsed tool-call item.
    pub fn add_tool_call(&mut self, call: &ToolCall) {
        let idx = self.items.len();
        let item = self.add_item(ChatItemRole::Tool, format_tool_call_block(call));
        item.tool_call_id = Some(call.id.clone());
        item.tool_name = Some(call.name.clone());
        item.tool_arguments = Some(Value::Object(call.arguments.clone()));
        item.started_at = Some(Instant::now());
        self.tool_items_by_call_id.insert(call.id.clone(), idx);
    }

    /// Find the transcript item for a tool call id (O(1)).
    #[allow(dead_code)]
    pub fn find_tool_item(&self, tool_call_id: &str) -> Option<&ChatItem> {
        self.tool_items_by_call_id
            .get(tool_call_id)
            .and_then(|&idx| self.items.get(idx))
    }

    fn find_tool_item_mut(&mut self, tool_call_id: &str) -> Option<&mut ChatItem> {
        match self.tool_items_by_call_id.get(tool_call_id) {
            Some(&idx) => self.items.get_mut(idx),
            None => None,
        }
    }

    /// Attach live progress to a pending tool call; drop orphan updates.
    pub fn record_tool_update(&mut self, tool_call_id: &str, message: &str) -> Option<()> {
        let item = self.find_tool_item_mut(tool_call_id)?;
        if item.tool_result_text.is_some() {
            return None;
        }
        item.update_text = Some(message.to_string());
        Some(())
    }

    /// Attach a Pi-compatible tool result to its matching call.
    pub fn record_tool_result(
        &mut self,
        tool_call_id: &str,
        tool_name: &str,
        result_text: &str,
        is_error: bool,
    ) {
        let result_text = format_tool_result_block(tool_name, is_error, result_text);
        if let Some(item) = self.find_tool_item_mut(tool_call_id) {
            item.tool_result_text = Some(result_text);
            item.update_text = None;
            return;
        }
        let idx = self.items.len();
        self.add_item(
            ChatItemRole::Tool,
            format_tool_result_summary(tool_name, is_error),
        );
        self.items[idx].tool_call_id = Some(tool_call_id.to_string());
        self.items[idx].tool_result_text = Some(result_text);
        self.tool_items_by_call_id
            .insert(tool_call_id.to_string(), idx);
    }

    /// Toggle expanded display for tool results.
    pub fn toggle_tool_results(&mut self) -> bool {
        self.show_tool_results = !self.show_tool_results;
        self.show_tool_results
    }

    /// Toggle thinking-token display.
    pub fn toggle_thinking(&mut self) -> bool {
        self.show_thinking = !self.show_thinking;
        self.show_thinking
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.tool_items_by_call_id.clear();
        self.assistant_buffer.clear();
        self.error = None;
        self.assistant_start_index = None;
    }

    /// Project canonical assistant blocks into display state in order.
    pub fn add_assistant_message(&mut self, message: &AssistantMessage, include_tool_calls: bool) {
        let stop = format_stop_reason(&message.stop_reason);
        for block in &message.content {
            match block {
                AssistantContent::Thinking(t) => {
                    if !t.thinking.is_empty() {
                        let item = self.add_item(ChatItemRole::Thinking, t.thinking.clone());
                        item.stop_reason = stop.clone();
                    }
                }
                AssistantContent::Text(t) => {
                    if !t.text.is_empty() {
                        let item = self.add_item(ChatItemRole::Assistant, t.text.clone());
                        item.stop_reason = stop.clone();
                    }
                }
                AssistantContent::ToolCall(tc) if include_tool_calls => self.add_tool_call(tc),
                AssistantContent::ToolCall(_) => {}
            }
        }
    }

    pub fn add_assistant_error(&mut self, message: &AssistantMessage) {
        self.add_assistant_message(message, false);
        let text = message
            .error_message
            .clone()
            .unwrap_or_else(|| "Error".to_string());
        self.error = Some(text.clone());
        self.add_item(ChatItemRole::Error, format!("Error: {text}"));
    }

    fn flush_assistant_buffer(&mut self) {
        if !self.assistant_buffer.is_empty() {
            let text = std::mem::take(&mut self.assistant_buffer);
            self.add_item(ChatItemRole::Assistant, text);
        }
    }

    /// Record the start of a streaming assistant message (called on
    /// `MessageStart(Assistant)`); deltas accumulate into `assistant_buffer`
    /// until [`finalize_assistant`] swaps in the canonical message.
    pub fn begin_assistant(&mut self) {
        self.assistant_start_index = Some(self.items.len());
    }

    /// Replace provisional delta rows with the final canonical message.
    pub fn finalize_assistant(&mut self, message: &AssistantMessage) {
        if let Some(start) = self.assistant_start_index.take() {
            // Drop the provisional buffered rows (which occupy [start..]).
            self.items.truncate(start);
        }
        self.flush_assistant_buffer();
        if message.stop_reason == tau_types::StopReason::Error
            || message.stop_reason == tau_types::StopReason::Aborted
        {
            self.add_assistant_error(message);
            self.running = false;
        } else {
            self.add_assistant_message(message, true);
        }
    }

    /// Populate the transcript from restored canonical session messages (resume).
    pub fn load_messages(&mut self, messages: &[AgentMessage]) {
        for message in messages {
            match message {
                AgentMessage::User(u) => self.add_user_message(&u.text(), None, None),
                AgentMessage::Custom(c) => self.add_user_message(
                    &c.text(),
                    Some(c.custom_type.clone()),
                    if c.details.is_object() || c.details.is_array() {
                        Some(c.details.clone())
                    } else {
                        None
                    },
                ),
                AgentMessage::Assistant(a) => {
                    if a.stop_reason == tau_types::StopReason::Error
                        || a.stop_reason == tau_types::StopReason::Aborted
                    {
                        self.add_assistant_error(a);
                    } else {
                        self.add_assistant_message(a, true);
                    }
                }
                AgentMessage::ToolResult(tr) => {
                    let text = tool_result_text(tr);
                    self.record_tool_result(&tr.tool_call_id, &tr.tool_name, &text, tr.is_error);
                }
                AgentMessage::BranchSummary(b) => self.add_item_with(
                    ChatItemRole::BranchSummary,
                    "Branch summary (Ctrl+O to expand)".to_string(),
                    None,
                    Some(b.summary.clone()),
                    false,
                    None,
                    None,
                ),
                AgentMessage::CompactionSummary(c) => self.add_item_with(
                    ChatItemRole::CompactionSummary,
                    "Compaction summary (Ctrl+O to expand)".to_string(),
                    None,
                    Some(c.summary.clone()),
                    false,
                    None,
                    None,
                ),
                _ => {}
            }
        }
    }
}

// --- formatting helpers (mirror state.py) ---------------------------------

fn tool_result_text(tr: &ToolResultMessage) -> String {
    tr.content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(t) => t.text.clone(),
            ToolResultContent::Image(_) => "<image>".to_string(),
        })
        .next()
        .unwrap_or_default()
}

fn parse_branch_summary(content: &str) -> Option<String> {
    let prefix = "The following is a summary of a branch that this conversation came back from:\n<summary>\n";
    let suffix = "\n</summary>";
    content
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix))
        .map(|inner| inner.to_string())
}

fn parse_compaction_summary(content: &str) -> Option<String> {
    let prefix = "Previous conversation summary:\n";
    content.strip_prefix(prefix).map(|rest| rest.to_string())
}

fn format_tool_call_invocation(call: &ToolCall) -> String {
    let args = Value::Object(call.arguments.clone());
    match call.name.as_str() {
        "read" => {
            let path = string_arg(&args, "path").unwrap_or_default();
            format!("read {path}{}", read_line_suffix(&args))
        }
        "edit" => {
            let path = string_arg(&args, "path").unwrap_or_default();
            format!("edit {path}")
        }
        "write" => {
            let path = string_arg(&args, "path").unwrap_or_default();
            format!("write {path}")
        }
        "bash" => {
            let command = string_arg(&args, "command").unwrap_or_default();
            format!("$ {command}")
        }
        _ => fallback_invocation(call),
    }
}

fn read_line_suffix(args: &Value) -> String {
    let offset = int_arg(args, "offset");
    let limit = int_arg(args, "limit");
    if offset.is_none() && limit.is_none() {
        return String::new();
    }
    let start = offset.unwrap_or(1).max(1);
    match limit {
        None => format!(":{start}-"),
        Some(l) => format!(":{start}-{}", start + l.max(1) - 1),
    }
}

fn fallback_invocation(call: &ToolCall) -> String {
    let args = Value::Object(call.arguments.clone());
    if args.as_object().map(|o| o.is_empty()).unwrap_or(true) {
        return call.name.clone();
    }
    let rendered = args.to_string();
    if rendered.len() > 160 {
        format!("{} {}", call.name, &rendered[..160].trim_end())
    } else {
        format!("{} {rendered}", call.name)
    }
}

fn format_tool_call_block(call: &ToolCall) -> String {
    let invocation = format_tool_call_invocation(call);
    if call.name == "bash" {
        invocation
    } else {
        format!("→ {invocation}")
    }
}

fn format_tool_result_summary(name: &str, ok: bool) -> String {
    let status = if ok { "✓" } else { "✗" };
    format!("{status} {name}")
}

fn format_tool_result_block(name: &str, ok: bool, content: &str) -> String {
    let status = if ok { "✓" } else { "✗" };
    let mut lines = vec![format!("{status} {name}")];
    if !content.is_empty() {
        let preview = preview_text(content, 8);
        lines.push(preview);
    }
    lines.join("\n")
}

fn preview_text(text: &str, max_lines: usize) -> String {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines.is_empty() {
        return text.chars().take(TOOL_RESULT_PREVIEW_CHARS).collect();
    }
    let preview_lines = &lines[..lines.len().min(max_lines)];
    let mut preview = preview_lines.join("\n");
    let hidden = lines.len().saturating_sub(preview_lines.len());
    let truncated = preview.chars().count() > TOOL_RESULT_PREVIEW_CHARS;
    if truncated {
        preview = preview.chars().take(TOOL_RESULT_PREVIEW_CHARS).collect();
    }
    if hidden > 0 || truncated {
        let mut detail = Vec::new();
        if hidden > 0 {
            detail.push(format!(
                "{hidden} more line{}",
                if hidden == 1 { "" } else { "s" }
            ));
        }
        if truncated {
            detail.push("additional text".to_string());
        }
        preview.push_str(&format!(
            "\n\n[Preview only: {} hidden from the TUI.]",
            detail.join(", ")
        ));
    }
    preview
}

fn string_arg(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(|x| x.as_str()).map(|s| s.to_string())
}

fn int_arg(v: &Value, key: &str) -> Option<i64> {
    v.get(key).and_then(|x| x.as_i64())
}

/// Format elapsed duration tersely: 23s, 1m 23s, 1h 2m.
#[allow(dead_code)]
pub fn format_elapsed(d: Duration) -> String {
    let total = d.as_secs() as i64;
    if total < 60 {
        return format!("{total}s");
    }
    let (minutes, secs) = (total / 60, total % 60);
    if minutes < 60 {
        return format!("{minutes}m {secs}s");
    }
    let (hours, minutes) = (minutes / 60, minutes % 60);
    format!("{hours}h {minutes}m")
}
