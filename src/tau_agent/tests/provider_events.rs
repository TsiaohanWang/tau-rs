//! Unit tests for [`crate::tau_agent::provider_events`].
//!
//! Test-only: compiled with `#[cfg(test)]` via `tau_agent::tests`.

use crate::tau_agent::messages::{AssistantContent, AssistantMessage, TextContent, ToolCall};
use crate::tau_agent::provider_events::*;
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

    let mut bad_reason = serde_json::to_value(AssistantMessageEvent::Done(AssistantDoneEvent {
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
