//! Hand-written SSE line parser for streaming model responses.
//!
//! SSE format: lines prefixed with `data: ` carry JSON payloads.
//! An empty line separates events. `data: [DONE]` signals stream end.
//! This parser yields parsed `serde_json::Value` payloads, leaving
//! the JSON interpretation to provider-specific code.

use serde_json::Value;

/// A single parsed SSE event payload.
#[derive(Debug, Clone)]
pub enum SseEvent {
    /// A JSON data payload from a `data: <json>` line.
    Data(Value),
    /// The stream-end sentinel `data: [DONE]`.
    Done,
}

/// Parse a single SSE `data:` line.
///
/// Returns `Some(SseEvent::Data(...))` for valid JSON payloads,
/// `Some(SseEvent::Done)` for `[DONE]`, and `None` for non-data
/// lines (comments, event types, empty lines, etc.).
pub fn parse_sse_line(line: &str) -> Option<SseEvent> {
    let line = line.trim();
    if line.is_empty() || line.starts_with(':') {
        return None;
    }
    // SSE spec: "data:" followed by optional space, then payload
    let payload = if let Some(rest) = line.strip_prefix("data:") {
        rest.strip_prefix(' ').unwrap_or(rest)
    } else {
        return None;
    };
    if payload == "[DONE]" {
        return Some(SseEvent::Done);
    }
    match serde_json::from_str::<Value>(payload) {
        Ok(v) => Some(SseEvent::Data(v)),
        Err(_) => None,
    }
}

/// Accumulate SSE lines into complete JSON events.
///
/// Some providers send multi-line JSON (though most don't). This
/// accumulator collects consecutive `data:` lines until an empty
/// line signals event boundary. For the common single-line case,
/// it yields immediately.
pub struct SseAccumulator {
    buffer: String,
}

impl Default for SseAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl SseAccumulator {
    pub fn new() -> Self {
        SseAccumulator {
            buffer: String::new(),
        }
    }

    /// Feed a line. Returns `Some(SseEvent)` when a complete event is ready.
    pub fn feed(&mut self, line: &str) -> Option<SseEvent> {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            if !self.buffer.is_empty() {
                let event = parse_sse_line(&self.buffer);
                self.buffer.clear();
                return event;
            }
            return None;
        }
        if self.buffer.is_empty() {
            // First line of a new event — try to parse directly
            if let Some(event) = parse_sse_line(trimmed) {
                return Some(event);
            }
            // Not a complete data line — buffer it for multi-line accumulation
            self.buffer = trimmed.to_string();
            return None;
        }
        // Continuation line for multi-line data
        self.buffer.push('\n');
        self.buffer.push_str(trimmed);
        None
    }

    /// Flush any remaining buffered data as an event.
    pub fn flush(&mut self) -> Option<SseEvent> {
        if self.buffer.is_empty() {
            return None;
        }
        let event = parse_sse_line(&self.buffer);
        self.buffer.clear();
        event
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_line() {
        let ev = parse_sse_line(r#"data: {"type":"delta","text":"hi"}"#).unwrap();
        match ev {
            SseEvent::Data(v) => assert_eq!(v["type"], "delta"),
            _ => panic!("expected Data"),
        }
    }

    #[test]
    fn parses_done() {
        let ev = parse_sse_line("data: [DONE]").unwrap();
        assert!(matches!(ev, SseEvent::Done));
    }

    #[test]
    fn ignores_comments_and_empty() {
        assert!(parse_sse_line(": a comment").is_none());
        assert!(parse_sse_line("").is_none());
        assert!(parse_sse_line("event: message").is_none());
    }

    #[test]
    fn accumulator_yields_immediately_for_single_line() {
        let mut acc = SseAccumulator::new();
        let ev = acc.feed(r#"data: {"ok":true}"#);
        assert!(ev.is_some());
        assert!(matches!(ev.unwrap(), SseEvent::Data(_)));
    }

    #[test]
    fn accumulator_handles_multi_line() {
        let mut acc = SseAccumulator::new();
        assert!(acc.feed("data: {").is_none());
        assert!(acc.feed(r#"  "type":"delta""#).is_none());
        assert!(acc.feed("}").is_none());
        let ev = acc.feed("").unwrap();
        match ev {
            SseEvent::Data(v) => assert_eq!(v["type"], "delta"),
            _ => panic!("expected Data"),
        }
    }
}
