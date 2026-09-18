//! Pi-compatible assistant stream events owned by the portable agent layer.
//!
//! Rust port of `tau_agent/provider_events.py`. The wire JSON is identical;
//! this header collects the places where the Rust design differs and why.
//!
//! ## Deliberate deviations
//!
//! * **The tag lives on the enum, not on each class.** Python repeats
//!   `type: Literal["..."] = "..."` on every event class so pydantic can
//!   discriminate the union. Rust has no pydantic, so
//!   [`AssistantMessageEvent`] is `#[serde(tag = "type")]` and the payload
//!   structs carry no `type` field. Serializing a variant writes the same
//!   `{"type": "...", ...}` object as Python, and each variant rename spells
//!   the Python tag (note `toolcall_start`, without the underscore).
//! * **No standalone payload deserialization.** Because the payload structs do
//!   not carry the tag, only the union can be deserialized. Python can also
//!   `model_validate` a single event class (its `type` has a default), but the
//!   port never needs that: providers construct events, and the loop matches on
//!   the union. This avoids a second, tag-carrying representation of every
//!   event just to serve validation.
//! * **`content_index` is `usize`** instead of Python's unbounded `int`. It is
//!   used to index `partial.content`, and the type rejects negative indices.
//! * **`DoneReason`/`ErrorReason` are separate enums**, mirroring the two
//!   Python `Literal` aliases, so a `done` event cannot report `aborted` and an
//!   `error` event cannot report `stop`.
//! * **The `partial()` accessor returns `Option`**, because Python's `.partial`
//!   attribute does not exist on `done`/`error` (those carry `message`/`error`).
//!
//! The snake_case aliases on multi-word fields exist because Python's
//! `WireModel` sets `validate_by_name=True`; the wire JSON always uses the
//! camelCase spelling.

use serde::{Deserialize, Serialize};

use super::messages::{AssistantMessage, ToolCall};

/// The first event of an assistant stream, carrying the initial message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantStartEvent {
    pub(crate) partial: AssistantMessage,
}

/// A text content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextStartEvent {
    // Python's unbounded `int`; `usize` matches the field's use as an index into
    // `partial.content` and rejects negative values. The alias accepts the
    // snake_case name because `WireModel` sets `validate_by_name=True`; the
    // remaining event fields follow the same pattern.
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of text was appended to a text content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A text content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct TextEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) content: String,
    pub(crate) partial: AssistantMessage,
}

/// A thinking content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingStartEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of reasoning was appended to a thinking content block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A thinking content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ThinkingEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) content: String,
    pub(crate) partial: AssistantMessage,
}

/// A tool call content block started.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallStartEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) partial: AssistantMessage,
}

/// A chunk of tool call arguments was appended to a tool call block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallDeltaEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    pub(crate) delta: String,
    pub(crate) partial: AssistantMessage,
}

/// A tool call content block finished.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct ToolCallEndEvent {
    #[serde(alias = "content_index")]
    pub(crate) content_index: usize,
    #[serde(alias = "tool_call")]
    pub(crate) tool_call: ToolCall,
    pub(crate) partial: AssistantMessage,
}

/// Why the provider stopped generating a response.
///
/// Python's `DoneReason = Literal["stop", "length", "toolUse"]`; the enum
/// keeps the same values and keeps `aborted` out of the success path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum DoneReason {
    Stop,
    Length,
    ToolUse,
}

/// Why the provider stream failed.
///
/// Python's `ErrorReason = Literal["aborted", "error"]`; the reverse of
/// [`DoneReason`], so `stop` cannot appear here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum ErrorReason {
    Aborted,
    Error,
}

/// The assistant stream finished successfully.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantDoneEvent {
    pub(crate) reason: DoneReason,
    pub(crate) message: AssistantMessage,
}

/// The assistant stream failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct AssistantErrorEvent {
    pub(crate) reason: ErrorReason,
    pub(crate) error: AssistantMessage,
}

/// Any assistant stream event.
///
/// `#[serde(tag = "type")]` reproduces Python's discriminated union: the
/// payload structs above omit `type`, the enum writes it, and only the enum can
/// be deserialized. See the module header for why the port accepts that
/// single-entry-point trade-off.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub(crate) enum AssistantMessageEvent {
    #[serde(rename = "start")]
    AssistantStart(AssistantStartEvent),
    #[serde(rename = "text_start")]
    TextStart(TextStartEvent),
    #[serde(rename = "text_delta")]
    TextDelta(TextDeltaEvent),
    #[serde(rename = "text_end")]
    TextEnd(TextEndEvent),
    #[serde(rename = "thinking_start")]
    ThinkingStart(ThinkingStartEvent),
    #[serde(rename = "thinking_delta")]
    ThinkingDelta(ThinkingDeltaEvent),
    #[serde(rename = "thinking_end")]
    ThinkingEnd(ThinkingEndEvent),
    // Python spells these tags without an underscore between the words.
    #[serde(rename = "toolcall_start")]
    ToolCallStart(ToolCallStartEvent),
    #[serde(rename = "toolcall_delta")]
    ToolCallDelta(ToolCallDeltaEvent),
    #[serde(rename = "toolcall_end")]
    ToolCallEnd(ToolCallEndEvent),
    #[serde(rename = "done")]
    Done(AssistantDoneEvent),
    #[serde(rename = "error")]
    Error(AssistantErrorEvent),
}

impl AssistantMessageEvent {
    /// The partial assistant message carried by stream update events.
    ///
    /// Replaces Python's `event.partial` attribute access: `done` and `error`
    /// carry a final message instead, so the Rust accessor is an `Option`.
    pub(crate) fn partial(&self) -> Option<&AssistantMessage> {
        match self {
            Self::AssistantStart(event) => Some(&event.partial),
            Self::TextStart(event) => Some(&event.partial),
            Self::TextDelta(event) => Some(&event.partial),
            Self::TextEnd(event) => Some(&event.partial),
            Self::ThinkingStart(event) => Some(&event.partial),
            Self::ThinkingDelta(event) => Some(&event.partial),
            Self::ThinkingEnd(event) => Some(&event.partial),
            Self::ToolCallStart(event) => Some(&event.partial),
            Self::ToolCallDelta(event) => Some(&event.partial),
            Self::ToolCallEnd(event) => Some(&event.partial),
            Self::Done(_) | Self::Error(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tau_agent::messages::{AssistantContent, TextContent};
    use crate::tau_agent::types::JsonObject;
    use serde_json::json;

    fn partial() -> AssistantMessage {
        AssistantMessage {
            content: vec![AssistantContent::Text(TextContent::new("x"))],
            timestamp: 1,
            ..Default::default()
        }
    }

    fn tool_call() -> ToolCall {
        ToolCall {
            arguments: JsonObject::from_iter([("path".to_owned(), json!("x"))]),
            ..ToolCall::new("c1", "read")
        }
    }

    /// Expected shapes below are produced by `tau_agent.provider_events` in
    /// Python.
    #[test]
    fn text_delta_matches_python_wire_shape() {
        let event = AssistantMessageEvent::TextDelta(TextDeltaEvent {
            content_index: 0,
            delta: "d".into(),
            partial: partial(),
        });

        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            format!(
                r#"{{"type":"text_delta","contentIndex":0,"delta":"d","partial":{}}}"#,
                serde_json::to_string(&partial()).unwrap()
            )
        );
        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            json!({
                "type": "text_delta",
                "contentIndex": 0,
                "delta": "d",
                "partial": serde_json::to_value(partial()).unwrap(),
            })
        );
    }

    #[test]
    fn done_and_error_events_match_python_wire_shape() {
        let done = AssistantMessageEvent::Done(AssistantDoneEvent {
            reason: DoneReason::ToolUse,
            message: partial(),
        });
        assert_eq!(
            serde_json::to_value(&done).unwrap(),
            json!({
                "type": "done",
                "reason": "toolUse",
                "message": serde_json::to_value(partial()).unwrap(),
            })
        );

        let error = AssistantMessageEvent::Error(AssistantErrorEvent {
            reason: ErrorReason::Aborted,
            error: partial(),
        });
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "error",
                "reason": "aborted",
                "error": serde_json::to_value(partial()).unwrap(),
            })
        );
        // The payload key is `error`, not `message`.
        assert!(value.get("message").is_none());
    }

    #[test]
    fn tool_call_end_carries_a_tool_call_block() {
        let event = AssistantMessageEvent::ToolCallEnd(ToolCallEndEvent {
            content_index: 2,
            tool_call: tool_call(),
            partial: partial(),
        });

        assert_eq!(
            serde_json::to_value(&event).unwrap()["toolCall"],
            json!({
                "type": "toolCall",
                "id": "c1",
                "name": "read",
                "arguments": {"path": "x"},
                "thoughtSignature": null,
            })
        );
    }

    #[test]
    fn every_event_uses_python_tags_and_round_trips() {
        let events = [
            (
                AssistantMessageEvent::AssistantStart(AssistantStartEvent { partial: partial() }),
                "start",
            ),
            (
                AssistantMessageEvent::TextStart(TextStartEvent {
                    content_index: 0,
                    partial: partial(),
                }),
                "text_start",
            ),
            (
                AssistantMessageEvent::TextDelta(TextDeltaEvent {
                    content_index: 0,
                    delta: "d".into(),
                    partial: partial(),
                }),
                "text_delta",
            ),
            (
                AssistantMessageEvent::TextEnd(TextEndEvent {
                    content_index: 0,
                    content: "c".into(),
                    partial: partial(),
                }),
                "text_end",
            ),
            (
                AssistantMessageEvent::ThinkingStart(ThinkingStartEvent {
                    content_index: 1,
                    partial: partial(),
                }),
                "thinking_start",
            ),
            (
                AssistantMessageEvent::ThinkingDelta(ThinkingDeltaEvent {
                    content_index: 1,
                    delta: "d".into(),
                    partial: partial(),
                }),
                "thinking_delta",
            ),
            (
                AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
                    content_index: 1,
                    content: "c".into(),
                    partial: partial(),
                }),
                "thinking_end",
            ),
            (
                AssistantMessageEvent::ToolCallStart(ToolCallStartEvent {
                    content_index: 2,
                    partial: partial(),
                }),
                "toolcall_start",
            ),
            (
                AssistantMessageEvent::ToolCallDelta(ToolCallDeltaEvent {
                    content_index: 2,
                    delta: "d".into(),
                    partial: partial(),
                }),
                "toolcall_delta",
            ),
            (
                AssistantMessageEvent::ToolCallEnd(ToolCallEndEvent {
                    content_index: 2,
                    tool_call: tool_call(),
                    partial: partial(),
                }),
                "toolcall_end",
            ),
            (
                AssistantMessageEvent::Done(AssistantDoneEvent {
                    reason: DoneReason::Stop,
                    message: partial(),
                }),
                "done",
            ),
            (
                AssistantMessageEvent::Error(AssistantErrorEvent {
                    reason: ErrorReason::Error,
                    error: partial(),
                }),
                "error",
            ),
        ];

        for (event, tag) in events {
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["type"], tag);
            assert!(value.get("content_index").is_none());
            assert_eq!(
                serde_json::from_value::<AssistantMessageEvent>(value.clone()).unwrap(),
                event
            );

            // Python's WireModel accepts the snake_case field names too
            // (validate_by_name=True), for every event that has them.
            let mut snake = value;
            if let Some(object) = snake.as_object_mut() {
                if let Some(content_index) = object.remove("contentIndex") {
                    object.insert("content_index".to_owned(), content_index);
                }
                if let Some(tool_call) = object.remove("toolCall") {
                    object.insert("tool_call".to_owned(), tool_call);
                }
            }
            assert_eq!(
                serde_json::from_value::<AssistantMessageEvent>(snake).unwrap(),
                event
            );

            // Only `done`/`error` carry a final message instead of a partial.
            if matches!(
                &event,
                AssistantMessageEvent::Done(_) | AssistantMessageEvent::Error(_)
            ) {
                assert!(event.partial().is_none());
            } else {
                assert_eq!(event.partial(), Some(&partial()));
            }
        }
    }

    #[test]
    fn events_reject_unknown_tags_fields_and_reasons() {
        assert!(serde_json::from_str::<AssistantMessageEvent>(r#"{"type":"nope"}"#).is_err());

        let mut unknown_field =
            serde_json::to_value(AssistantMessageEvent::TextDelta(TextDeltaEvent {
                content_index: 0,
                delta: "d".into(),
                partial: partial(),
            }))
            .unwrap();
        unknown_field["extra"] = json!(1);
        assert!(serde_json::from_value::<AssistantMessageEvent>(unknown_field).is_err());

        let mut bad_reason =
            serde_json::to_value(AssistantMessageEvent::Done(AssistantDoneEvent {
                reason: DoneReason::Stop,
                message: partial(),
            }))
            .unwrap();
        bad_reason["reason"] = json!("bogus");
        assert!(serde_json::from_value::<AssistantMessageEvent>(bad_reason).is_err());

        // content_index is required in Python's TextDeltaEvent as well.
        assert!(
            serde_json::from_value::<AssistantMessageEvent>(json!({
                "type": "text_delta",
                "delta": "d",
                "partial": serde_json::to_value(partial()).unwrap(),
            }))
            .is_err()
        );
    }

    #[test]
    fn remaining_event_shapes_match_python_exactly() {
        let event = AssistantMessageEvent::TextStart(TextStartEvent {
            content_index: 0,
            partial: partial(),
        });
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            format!(
                r#"{{"type":"text_start","contentIndex":0,"partial":{}}}"#,
                serde_json::to_string(&partial()).unwrap()
            )
        );

        let event = AssistantMessageEvent::ThinkingEnd(ThinkingEndEvent {
            content_index: 1,
            content: "c".into(),
            partial: partial(),
        });
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            format!(
                r#"{{"type":"thinking_end","contentIndex":1,"content":"c","partial":{}}}"#,
                serde_json::to_string(&partial()).unwrap()
            )
        );

        let call = tool_call();
        let event = AssistantMessageEvent::ToolCallEnd(ToolCallEndEvent {
            content_index: 2,
            tool_call: call.clone(),
            partial: partial(),
        });
        assert_eq!(
            serde_json::to_string(&event).unwrap(),
            format!(
                r#"{{"type":"toolcall_end","contentIndex":2,"toolCall":{},"partial":{}}}"#,
                serde_json::to_string(&call).unwrap(),
                serde_json::to_string(&partial()).unwrap()
            )
        );
    }

    #[test]
    fn reason_enums_cover_python_literals() {
        for (reason, wire) in [
            (DoneReason::Stop, "stop"),
            (DoneReason::Length, "length"),
            (DoneReason::ToolUse, "toolUse"),
        ] {
            let event = AssistantMessageEvent::Done(AssistantDoneEvent {
                reason,
                message: partial(),
            });
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["reason"], json!(wire));
            assert_eq!(
                serde_json::from_value::<AssistantMessageEvent>(value).unwrap(),
                event
            );
        }

        for (reason, wire) in [
            (ErrorReason::Aborted, "aborted"),
            (ErrorReason::Error, "error"),
        ] {
            let event = AssistantMessageEvent::Error(AssistantErrorEvent {
                reason,
                error: partial(),
            });
            let value = serde_json::to_value(&event).unwrap();
            assert_eq!(value["reason"], json!(wire));
            assert_eq!(
                serde_json::from_value::<AssistantMessageEvent>(value).unwrap(),
                event
            );
        }

        // Python models the two reason lists as separate Literals, so a value
        // from the wrong list is rejected here as well.
        let mut value = serde_json::to_value(AssistantMessageEvent::Done(AssistantDoneEvent {
            reason: DoneReason::Stop,
            message: partial(),
        }))
        .unwrap();
        value["reason"] = json!("aborted");
        assert!(serde_json::from_value::<AssistantMessageEvent>(value).is_err());
    }

    #[test]
    fn payload_structs_are_not_standalone_wire_entry_points() {
        // ADR-004: the `type` tag belongs to the union, so a concrete event
        // struct rejects the tagged wire object as an unknown field.
        let value = serde_json::to_value(AssistantMessageEvent::TextDelta(TextDeltaEvent {
            content_index: 0,
            delta: "d".into(),
            partial: partial(),
        }))
        .unwrap();
        assert!(serde_json::from_value::<TextDeltaEvent>(value).is_err());
    }
}
