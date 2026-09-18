//! Unit tests for [`crate::tau_agent::messages`].
//!
//! Test-only: compiled with `#[cfg(test)]` via `tau_agent::tests`.

use crate::tau_agent::messages::*;
use crate::tau_agent::types::JsonObject;
use serde_json::json;

/// Expected strings below are produced by `tau_agent.messages` in Python.
#[test]
fn user_message_matches_python_wire_shape() {
    let message = UserMessage {
        role: MessageRole::User,
        content: "hello".into(),
        timestamp: 1,
    };

    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        r#"{"role":"user","content":"hello","timestamp":1}"#
    );
}

#[test]
fn user_message_blocks_match_python_wire_shape() {
    let message = UserMessage {
        role: MessageRole::User,
        content: UserContent::List(vec![
            UserContentItem::Text(TextContent::new("a")),
            UserContentItem::Image(ImageContent::new("b64", "image/png")),
        ]),
        timestamp: 1,
    };

    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        r#"{"role":"user","content":[{"type":"text","text":"a","textSignature":null},{"type":"image","data":"b64","mimeType":"image/png"}],"timestamp":1}"#
    );
}

#[test]
fn assistant_message_matches_python_wire_shape() {
    let message = AssistantMessage {
        role: MessageRole::Assistant,
        content: vec![
            AssistantContent::Thinking(ThinkingContent {
                redacted: true,
                ..ThinkingContent::new("t")
            }),
            AssistantContent::Text(TextContent {
                text_signature: Some("sig".into()),
                ..TextContent::new("a")
            }),
            AssistantContent::ToolCall(ToolCall {
                arguments: JsonObject::from_iter([("path".to_owned(), json!("x"))]),
                thought_signature: Some("ts".into()),
                ..ToolCall::new("c1", "read")
            }),
        ],
        api: "api".into(),
        provider: "p".into(),
        model: "m".into(),
        response_model: Some("rm".into()),
        response_provider: Some("rp".into()),
        response_id: Some("rid".into()),
        diagnostics: Some(vec![AssistantMessageDiagnostic {
            r#type: "warn".into(),
            timestamp: 1,
            error: Some(AssistantDiagnosticError {
                name: Some("E".into()),
                message: "m".into(),
                stack: Some("s".into()),
                code: Some(AssistantDiagnosticErrorCode::Int(7)),
            }),
            details: Some(JsonObject::from_iter([("k".to_owned(), json!(1))])),
        }]),
        usage: Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            cache_write_1h: Some(5),
            reasoning: Some(6),
            total_tokens: 7,
            cost: UsageCost::default(),
        },
        timing: Some(ResponseTiming {
            time_to_first_output_ms: Some(8),
            total_duration_ms: 9,
        }),
        stop_reason: StopReason::ToolUse,
        error_message: Some("err".into()),
        timestamp: 1,
    };

    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        concat!(
            r#"{"role":"assistant","content":[{"type":"thinking","thinking":"t","thinkingSignature":null,"redacted":true},"#,
            r#"{"type":"text","text":"a","textSignature":"sig"},"#,
            r#"{"type":"toolCall","id":"c1","name":"read","arguments":{"path":"x"},"thoughtSignature":"ts"}],"#,
            r#""api":"api","provider":"p","model":"m","responseModel":"rm","responseProvider":"rp","responseId":"rid","#,
            r#""diagnostics":[{"type":"warn","timestamp":1,"error":{"name":"E","message":"m","stack":"s","code":7},"details":{"k":1}}],"#,
            r#""usage":{"input":1,"output":2,"cacheRead":3,"cacheWrite":4,"cacheWrite1H":5,"reasoning":6,"totalTokens":7,"#,
            r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"#,
            r#""timing":{"timeToFirstOutputMs":8,"totalDurationMs":9},"stopReason":"toolUse","errorMessage":"err","timestamp":1}"#
        )
    );
}

#[test]
fn assistant_minimal_matches_python_wire_shape() {
    let message = AssistantMessage {
        timestamp: 1,
        ..Default::default()
    };

    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        concat!(
            r#"{"role":"assistant","content":[],"api":"unknown","provider":"unknown","model":"unknown","#,
            r#""responseModel":null,"responseProvider":null,"responseId":null,"diagnostics":null,"#,
            r#""usage":{"input":0,"output":0,"cacheRead":0,"cacheWrite":0,"cacheWrite1H":null,"reasoning":null,"totalTokens":0,"#,
            r#""cost":{"input":0.0,"output":0.0,"cacheRead":0.0,"cacheWrite":0.0,"total":0.0}},"#,
            r#""timing":null,"stopReason":"stop","errorMessage":null,"timestamp":1}"#
        )
    );
}

#[test]
fn tool_result_message_matches_python_wire_shape() {
    let message = ToolResultMessage {
        role: MessageRole::ToolResult,
        tool_call_id: "c1".into(),
        tool_name: "read".into(),
        content: vec![ToolResultContent::Text(TextContent::new("out"))],
        details: Some(json!({"a": 1})),
        added_tool_names: Some(vec!["x".into()]),
        is_error: true,
        timestamp: 1,
    };

    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        r#"{"role":"toolResult","toolCallId":"c1","toolName":"read","content":[{"type":"text","text":"out","textSignature":null}],"details":{"a":1},"addedToolNames":["x"],"isError":true,"timestamp":1}"#
    );
}

#[test]
fn session_only_messages_match_python_wire_shape() {
    let bash = BashExecutionMessage {
        role: MessageRole::BashExecution,
        command: "ls".into(),
        output: "o".into(),
        exit_code: Some(0),
        cancelled: true,
        truncated: true,
        full_output_path: Some("/tmp/x".into()),
        timestamp: 1,
        exclude_from_context: true,
    };
    let custom = CustomMessage {
        role: MessageRole::Custom,
        custom_type: "ct".into(),
        content: UserContent::List(vec![UserContentItem::Text(TextContent::new("x"))]),
        display: false,
        details: None,
        timestamp: 1,
    };
    let branch = BranchSummaryMessage {
        role: MessageRole::BranchSummary,
        summary: "s".into(),
        from_id: "f".into(),
        timestamp: 1,
    };
    let compaction = CompactionSummaryMessage {
        role: MessageRole::CompactionSummary,
        summary: "s".into(),
        tokens_before: 3,
        timestamp: 1,
    };

    assert_eq!(
        serde_json::to_string(&bash).unwrap(),
        r#"{"role":"bashExecution","command":"ls","output":"o","exitCode":0,"cancelled":true,"truncated":true,"fullOutputPath":"/tmp/x","timestamp":1,"excludeFromContext":true}"#
    );
    assert_eq!(
        serde_json::to_string(&custom).unwrap(),
        r#"{"role":"custom","customType":"ct","content":[{"type":"text","text":"x","textSignature":null}],"display":false,"details":null,"timestamp":1}"#
    );
    assert_eq!(
        serde_json::to_string(&branch).unwrap(),
        r#"{"role":"branchSummary","summary":"s","fromId":"f","timestamp":1}"#
    );
    assert_eq!(
        serde_json::to_string(&compaction).unwrap(),
        r#"{"role":"compactionSummary","summary":"s","tokensBefore":3,"timestamp":1}"#
    );
}

#[test]
fn agent_message_union_dispatches_on_role() {
    let cases = [
        (
            r#"{"role":"user","content":"hello","timestamp":1}"#,
            MessageRole::User,
        ),
        (
            r#"{"role":"assistant","content":[],"api":"unknown","provider":"unknown","model":"unknown","timestamp":1}"#,
            MessageRole::Assistant,
        ),
        (
            r#"{"role":"toolResult","toolCallId":"c1","toolName":"read","timestamp":1}"#,
            MessageRole::ToolResult,
        ),
        (
            r#"{"role":"bashExecution","command":"ls","output":"o","timestamp":1}"#,
            MessageRole::BashExecution,
        ),
        (
            r#"{"role":"custom","customType":"ct","content":"x","timestamp":1}"#,
            MessageRole::Custom,
        ),
        (
            r#"{"role":"branchSummary","summary":"s","fromId":"f","timestamp":1}"#,
            MessageRole::BranchSummary,
        ),
        (
            r#"{"role":"compactionSummary","summary":"s","tokensBefore":3,"timestamp":1}"#,
            MessageRole::CompactionSummary,
        ),
    ];

    for (payload, role) in cases {
        let message: AgentMessage = serde_json::from_str(payload).unwrap();
        assert_eq!(message.role(), role);
        let round_tripped: AgentMessage =
            serde_json::from_str(&serde_json::to_string(&message).unwrap()).unwrap();
        assert_eq!(round_tripped, message);
    }
}

#[test]
fn agent_message_validates_roles_and_unknown_fields() {
    // A minimal assistant payload must not fall into the user variant.
    let message: AgentMessage =
        serde_json::from_str(r#"{"role":"assistant","content":[{"type":"text","text":"x"}]}"#)
            .unwrap();
    assert!(matches!(message, AgentMessage::AssistantMessage(_)));

    // A minimal user payload must not fall into the assistant variant.
    let message: AgentMessage =
        serde_json::from_str(r#"{"role":"user","content":[{"type":"text","text":"x"}]}"#).unwrap();
    assert!(matches!(message, AgentMessage::UserMessage(_)));

    assert!(serde_json::from_str::<AgentMessage>(r#"{"role":"nope","content":"x"}"#).is_err());
    assert!(
        serde_json::from_str::<AgentMessage>(r#"{"role":"user","content":"x","unexpected":true}"#)
            .is_err()
    );
    assert!(serde_json::from_str::<UserMessage>(r#"{"role":"assistant","content":"x"}"#).is_err());
}

#[test]
fn content_items_validate_the_type_tag() {
    assert!(serde_json::from_str::<UserContentItem>(r#"{"type":"image","text":"x"}"#).is_err());
    assert!(
        serde_json::from_str::<UserContentItem>(
            r#"{"type":"text","data":"x","mimeType":"image/png"}"#
        )
        .is_err()
    );
    assert!(serde_json::from_str::<UserContentItem>(r#"{"type":"other"}"#).is_err());
}

#[test]
fn assistant_deserialization_uses_python_defaults() {
    let message: AssistantMessage =
        serde_json::from_str(r#"{"role":"assistant","timestamp":1}"#).unwrap();
    assert_eq!(message.api, "unknown");
    assert_eq!(message.provider, "unknown");
    assert_eq!(message.model, "unknown");
    assert!(message.content.is_empty());
    assert_eq!(message.usage, Usage::default());
    assert_eq!(message.stop_reason, StopReason::Stop);

    // Python's before-validator turns an explicit null usage into Usage().
    let message: AssistantMessage =
        serde_json::from_str(r#"{"role":"assistant","usage":null,"timestamp":1}"#).unwrap();
    assert_eq!(message.usage, Usage::default());

    // stop reason uses the camelCase wire spelling.
    let message: AssistantMessage =
        serde_json::from_str(r#"{"role":"assistant","stopReason":"toolUse"}"#).unwrap();
    assert_eq!(message.stop_reason, StopReason::ToolUse);
}

#[test]
fn sum_usage_matches_python_optional_semantics() {
    let usages = [
        Usage {
            input: 1,
            cache_write_1h: Some(2),
            ..Usage::default()
        },
        Usage {
            output: 3,
            reasoning: Some(4),
            ..Usage::default()
        },
    ];
    let total = sum_usage(&usages);

    assert_eq!(total.input, 1);
    assert_eq!(total.output, 3);
    assert_eq!(total.cache_write_1h, Some(2));
    assert_eq!(total.reasoning, Some(4));

    assert_eq!(sum_usage(&[]).cache_write_1h, None);
    assert_eq!(sum_usage(&[Usage::default()]).cache_write_1h, None);
}

#[test]
fn assistant_accessors_mirror_python_properties() {
    let call = ToolCall::new("c1", "read");
    let message = AssistantMessage {
        content: assistant_content("answer", [call.clone()]),
        timestamp: 1,
        ..Default::default()
    };

    assert_eq!(message.text(), "answer");
    assert_eq!(message.thinking_text(), "");
    assert_eq!(message.tool_calls(), vec![&call]);

    let message = AssistantMessage {
        content: vec![
            AssistantContent::Thinking(ThinkingContent::new("why")),
            AssistantContent::Text(TextContent::new("answer")),
        ],
        timestamp: 1,
        ..Default::default()
    };
    assert_eq!(message.thinking_text(), "why");
    assert_eq!(message.text(), "answer");
}

#[test]
fn content_text_skips_images_and_message_text_dispatches() {
    let mixed: Vec<ToolResultContent> = vec![
        ToolResultContent::Text(TextContent::new("a")),
        ToolResultContent::Image(ImageContent::new("b64", "image/png")),
    ];
    let message = ToolResultMessage {
        role: MessageRole::ToolResult,
        tool_call_id: "c1".into(),
        tool_name: "read".into(),
        content: mixed,
        details: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 1,
    };
    assert_eq!(message.text(), "a");

    assert_eq!(
        content_text(&UserContent::List(vec![
            UserContentItem::Text(TextContent::new("a")),
            UserContentItem::Image(ImageContent::new("b64", "image/png")),
        ])),
        "a"
    );

    let bash = BashExecutionMessage {
        role: MessageRole::BashExecution,
        command: "ls".into(),
        output: "out".into(),
        exit_code: None,
        cancelled: false,
        truncated: false,
        full_output_path: None,
        timestamp: 7,
        exclude_from_context: false,
    };
    let message = AgentMessage::BashExecutionMessage(bash);
    assert_eq!(message_text(&message), "out");
    assert_eq!(message.text(), "out");
    assert_eq!(message.timestamp(), 7);
    assert_eq!(message.role(), MessageRole::BashExecution);
}

#[test]
fn message_to_user_preserves_text_and_timestamp() {
    let custom = CustomMessage {
        role: MessageRole::Custom,
        custom_type: "note".into(),
        content: "remember".into(),
        display: true,
        details: None,
        timestamp: 42,
    };
    let user = message_to_user(&custom.into());

    assert_eq!(user.role, MessageRole::User);
    assert_eq!(user.timestamp, 42);
    assert_eq!(user.text(), "remember");
    assert_eq!(
        serde_json::to_string(&user).unwrap(),
        r#"{"role":"user","content":"remember","timestamp":42}"#
    );
}

#[test]
fn assistant_content_builds_ordered_blocks() {
    let call = ToolCall::new("c1", "read");

    assert_eq!(
        assistant_content("hello", [call.clone()]),
        vec![
            AssistantContent::Text(TextContent::new("hello")),
            AssistantContent::ToolCall(call),
        ]
    );
    assert!(assistant_content("", Vec::<ToolCall>::new()).is_empty());
}

#[test]
fn agent_message_requires_a_role() {
    // Python's discriminated union rejects a missing discriminator; the
    // untagged Rust union must not fall back to the assistant variant.
    assert!(serde_json::from_str::<AgentMessage>(r#"{"content":[]}"#).is_err());
    assert!(serde_json::from_str::<AgentMessage>(r#"{"content":"x"}"#).is_err());
}

#[test]
fn content_blocks_default_their_type() {
    let message: AssistantMessage = serde_json::from_str(
        r#"{"role":"assistant","content":[{"text":"a"},{"thinking":"t"},{"id":"c1","name":"read"}]}"#,
    )
    .unwrap();
    assert_eq!(
        message.content,
        vec![
            AssistantContent::Text(TextContent::new("a")),
            AssistantContent::Thinking(ThinkingContent::new("t")),
            AssistantContent::ToolCall(ToolCall::new("c1", "read")),
        ]
    );

    let message: UserMessage = serde_json::from_str(
        r#"{"role":"user","content":[{"data":"b64","mimeType":"image/png"}]}"#,
    )
    .unwrap();
    assert_eq!(
        message.content,
        UserContent::List(vec![UserContentItem::Image(ImageContent::new(
            "b64",
            "image/png"
        ))])
    );
}

#[test]
fn python_snake_case_field_names_are_accepted() {
    // Python's WireModel sets validate_by_name=True; the legacy session
    // loader relies on it (for example usage.cache_read).
    let usage: Usage = serde_json::from_str(
        r#"{"cache_read":1,"cache_write":2,"cache_write_1h":3,"total_tokens":4}"#,
    )
    .unwrap();
    assert_eq!(usage.cache_read, 1);
    assert_eq!(usage.cache_write, 2);
    assert_eq!(usage.cache_write_1h, Some(3));
    assert_eq!(usage.total_tokens, 4);

    let assistant: AssistantMessage = serde_json::from_str(
        r#"{"role":"assistant","stop_reason":"length","response_model":"m","error_message":"e"}"#,
    )
    .unwrap();
    assert_eq!(assistant.stop_reason, StopReason::Length);
    assert_eq!(assistant.response_model.as_deref(), Some("m"));
    assert_eq!(assistant.error_message.as_deref(), Some("e"));

    let tool_result: ToolResultMessage = serde_json::from_str(
        r#"{"role":"toolResult","tool_call_id":"c1","tool_name":"read","is_error":true,"added_tool_names":["x"]}"#,
    )
    .unwrap();
    assert_eq!(tool_result.tool_call_id, "c1");
    assert_eq!(tool_result.tool_name, "read");
    assert!(tool_result.is_error);
    assert_eq!(tool_result.added_tool_names, Some(vec!["x".to_owned()]));

    let text: TextContent = serde_json::from_str(r#"{"text":"a","text_signature":"sig"}"#).unwrap();
    assert_eq!(text.text_signature.as_deref(), Some("sig"));

    // Duplicate spellings: pydantic's `validate_python` path (json.loads +
    // validate_python, which the session loader uses) rejects them just
    // like serde; only pydantic's `validate_json` path prefers the alias.
    assert!(serde_json::from_str::<Usage>(r#"{"cacheRead":1,"cache_read":2}"#).is_err());
}

#[test]
fn agent_message_matches_python_acceptance_corpus() {
    // Verdicts produced by TypeAdapter(AgentMessage).validate_python in
    // Python. This pins the accept/reject contract of the union.
    let cases: [(&str, bool); 28] = [
        (r#"{"role":"user","content":"x"}"#, true),
        (r#"{"role":"user","content":[]}"#, true),
        (
            r#"{"role":"user","content":[{"type":"text","text":"x"}]}"#,
            true,
        ),
        (r#"{"role":"user","content":[{"text":"x"}]}"#, true),
        (
            r#"{"role":"user","content":[{"type":"image","text":"x"}]}"#,
            false,
        ),
        (
            r#"{"role":"user","content":[{"type":"zzz","text":"x"}]}"#,
            false,
        ),
        (r#"{"role":"user"}"#, false),
        (r#"{"role":"user","content":"x","bogus":1}"#, false),
        (r#"{"role":"assistant"}"#, true),
        (
            r#"{"role":"assistant","content":[{"id":"c","name":"n"}]}"#,
            true,
        ),
        (
            r#"{"role":"assistant","content":[{"data":"x","mimeType":"m"}]}"#,
            false,
        ),
        (r#"{"role":"assistant","usage":null}"#, true),
        (
            r#"{"role":"assistant","usage":{"cache_read":1,"total_tokens":2}}"#,
            true,
        ),
        (r#"{"role":"assistant","stopReason":"toolUse"}"#, true),
        (r#"{"role":"assistant","stopReason":"nope"}"#, false),
        (r#"{"role":"assistant","stop_reason":"toolUse"}"#, true),
        (
            r#"{"role":"toolResult","toolCallId":"c","toolName":"t"}"#,
            true,
        ),
        (
            r#"{"role":"toolResult","tool_call_id":"c","tool_name":"t"}"#,
            true,
        ),
        (
            r#"{"role":"toolResult","toolCallId":"c","toolName":"t","isError":true}"#,
            true,
        ),
        (
            r#"{"role":"bashExecution","command":"ls","output":"o"}"#,
            true,
        ),
        (r#"{"role":"bashExecution","command":"ls"}"#, false),
        (r#"{"role":"custom","customType":"ct","content":"x"}"#, true),
        (r#"{"role":"custom","content":"x"}"#, false),
        (
            r#"{"role":"branchSummary","summary":"s","fromId":"f"}"#,
            true,
        ),
        (
            r#"{"role":"compactionSummary","summary":"s","tokensBefore":3}"#,
            true,
        ),
        (r#"{"content":"x"}"#, false),
        (r#"{"role":"nope","content":"x"}"#, false),
        (
            r#"{"role":"assistant","content":[{"thinking":"t","redacted":true}]}"#,
            true,
        ),
    ];

    for (payload, accepted) in cases {
        assert_eq!(
            serde_json::from_str::<AgentMessage>(payload).is_ok(),
            accepted,
            "payload: {payload}"
        );
    }
}

#[test]
fn display_and_serde_use_the_same_spelling() {
    for role in [
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::ToolResult,
        MessageRole::BashExecution,
        MessageRole::Custom,
        MessageRole::BranchSummary,
        MessageRole::CompactionSummary,
    ] {
        assert_eq!(serde_json::to_string(&role).unwrap(), format!("\"{role}\""));
    }

    for content_type in [
        ContentType::Text,
        ContentType::Thinking,
        ContentType::Image,
        ContentType::ToolCall,
    ] {
        assert_eq!(
            serde_json::to_string(&content_type).unwrap(),
            format!("\"{content_type}\"")
        );
    }
}

#[test]
fn json_objects_preserve_insertion_order() {
    // The serde_json `preserve_order` feature keeps `arguments` in the
    // order a provider or a Python dict wrote it.
    let mut arguments = JsonObject::new();
    arguments.insert("z".to_owned(), json!(1));
    arguments.insert("a".to_owned(), json!(2));
    let call = ToolCall {
        arguments,
        ..ToolCall::new("c1", "read")
    };

    assert!(
        serde_json::to_string(&call)
            .unwrap()
            .contains(r#""arguments":{"z":1,"a":2}"#)
    );
}

/// One value of every message variant, shared by the coverage tests below.
fn sample_messages() -> Vec<AgentMessage> {
    vec![
        UserMessage {
            role: MessageRole::User,
            content: "prompt".into(),
            timestamp: 9,
        }
        .into(),
        AssistantMessage {
            content: vec![
                AssistantContent::Text(TextContent::new("part")),
                AssistantContent::ToolCall(ToolCall::new("c1", "read")),
            ],
            timestamp: 9,
            ..Default::default()
        }
        .into(),
        ToolResultMessage {
            role: MessageRole::ToolResult,
            tool_call_id: "c1".into(),
            tool_name: "read".into(),
            content: vec![ToolResultContent::Text(TextContent::new("out"))],
            details: None,
            added_tool_names: None,
            is_error: false,
            timestamp: 9,
        }
        .into(),
        BashExecutionMessage {
            role: MessageRole::BashExecution,
            command: "ls".into(),
            output: "o".into(),
            exit_code: Some(0),
            cancelled: false,
            truncated: false,
            full_output_path: None,
            timestamp: 9,
            exclude_from_context: false,
        }
        .into(),
        CustomMessage {
            role: MessageRole::Custom,
            custom_type: "note".into(),
            content: "x".into(),
            display: true,
            details: None,
            timestamp: 9,
        }
        .into(),
        BranchSummaryMessage {
            role: MessageRole::BranchSummary,
            summary: "branch summary".into(),
            from_id: "f".into(),
            timestamp: 9,
        }
        .into(),
        CompactionSummaryMessage {
            role: MessageRole::CompactionSummary,
            summary: "compaction summary".into(),
            tokens_before: 3,
            timestamp: 9,
        }
        .into(),
    ]
}

#[test]
fn every_message_variant_round_trips_through_the_union() {
    let expected_roles = [
        MessageRole::User,
        MessageRole::Assistant,
        MessageRole::ToolResult,
        MessageRole::BashExecution,
        MessageRole::Custom,
        MessageRole::BranchSummary,
        MessageRole::CompactionSummary,
    ];

    for (message, role) in sample_messages().iter().zip(expected_roles) {
        assert_eq!(message.role(), role);
        let json = serde_json::to_string(message).unwrap();
        let parsed: AgentMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(&parsed, message);
    }
}

#[test]
fn message_text_and_timestamp_cover_every_message_role() {
    let expected_texts = [
        "prompt",
        "part",
        "out",
        "o",
        "x",
        "branch summary",
        "compaction summary",
    ];

    for (message, text) in sample_messages().iter().zip(expected_texts) {
        assert_eq!(message_text(message), text);
        assert_eq!(message.text(), text);
        assert_eq!(message.timestamp(), 9);
    }
}

#[test]
fn message_to_user_covers_every_message_role() {
    let expected_texts = [
        "prompt",
        "part",
        "out",
        "o",
        "x",
        "branch summary",
        "compaction summary",
    ];

    for (message, text) in sample_messages().iter().zip(expected_texts) {
        let user = message_to_user(message);
        assert_eq!(user.role, MessageRole::User);
        assert_eq!(user.timestamp, 9);
        assert_eq!(user.text(), text);
    }
}

#[test]
fn standalone_content_blocks_keep_their_type_tag() {
    // Locks the ADR-004 decision: a block serialized outside a content list
    // still writes its `type`, exactly like the Python class.
    assert_eq!(
        serde_json::to_string(&TextContent::new("a")).unwrap(),
        r#"{"type":"text","text":"a","textSignature":null}"#
    );
    assert_eq!(
        serde_json::to_string(&ThinkingContent::new("t")).unwrap(),
        r#"{"type":"thinking","thinking":"t","thinkingSignature":null,"redacted":false}"#
    );
    assert_eq!(
        serde_json::to_string(&ImageContent::new("d", "image/png")).unwrap(),
        r#"{"type":"image","data":"d","mimeType":"image/png"}"#
    );
    assert_eq!(
        serde_json::to_string(&ToolCall::new("c", "n")).unwrap(),
        r#"{"type":"toolCall","id":"c","name":"n","arguments":{},"thoughtSignature":null}"#
    );
}

#[test]
fn remaining_snake_case_aliases_are_accepted() {
    let assistant: AssistantMessage = serde_json::from_str(
        r#"{"role":"assistant","content":[{"type":"thinking","thinking":"t","thinking_signature":"sig"},{"type":"toolCall","id":"c","name":"n","thought_signature":"ts"}],"timing":{"time_to_first_output_ms":1,"total_duration_ms":2},"usage":{"cost":{"cache_read":0.5,"cache_write":0.25}}}"#,
    )
    .unwrap();
    let AssistantContent::Thinking(thinking) = &assistant.content[0] else {
        panic!("expected thinking block");
    };
    assert_eq!(thinking.thinking_signature.as_deref(), Some("sig"));
    let AssistantContent::ToolCall(call) = &assistant.content[1] else {
        panic!("expected tool call block");
    };
    assert_eq!(call.thought_signature.as_deref(), Some("ts"));
    let timing = assistant.timing.unwrap();
    assert_eq!(timing.time_to_first_output_ms, Some(1));
    assert_eq!(timing.total_duration_ms, 2);
    assert_eq!(assistant.usage.cost.cache_read, 0.5);
    assert_eq!(assistant.usage.cost.cache_write, 0.25);

    let user: UserMessage = serde_json::from_str(
        r#"{"role":"user","content":[{"type":"image","data":"d","mime_type":"image/png"}]}"#,
    )
    .unwrap();
    let UserContent::List(items) = &user.content else {
        panic!("expected an image list");
    };
    let UserContentItem::Image(image) = &items[0] else {
        panic!("expected an image block");
    };
    assert_eq!(image.mime_type, "image/png");

    let bash: BashExecutionMessage = serde_json::from_str(
        r#"{"role":"bashExecution","command":"ls","output":"o","exit_code":-1,"full_output_path":"/tmp/x","exclude_from_context":true}"#,
    )
    .unwrap();
    assert_eq!(bash.exit_code, Some(-1));
    assert_eq!(bash.full_output_path.as_deref(), Some("/tmp/x"));
    assert!(bash.exclude_from_context);

    let custom: CustomMessage =
        serde_json::from_str(r#"{"role":"custom","custom_type":"ct","content":"x"}"#).unwrap();
    assert_eq!(custom.custom_type, "ct");

    let branch: BranchSummaryMessage =
        serde_json::from_str(r#"{"role":"branchSummary","summary":"s","from_id":"f"}"#).unwrap();
    assert_eq!(branch.from_id, "f");

    let compaction: CompactionSummaryMessage =
        serde_json::from_str(r#"{"role":"compactionSummary","summary":"s","tokens_before":3}"#)
            .unwrap();
    assert_eq!(compaction.tokens_before, 3);
}

#[test]
fn diagnostic_error_code_accepts_strings_and_integers() {
    let message: AssistantMessage = serde_json::from_str(
        r#"{"role":"assistant","diagnostics":[{"type":"w","error":{"message":"m","code":"E1"}},{"type":"w","error":{"message":"m","code":-7}}]}"#,
    )
    .unwrap();
    let diagnostics = message.diagnostics.unwrap();
    assert_eq!(
        diagnostics[0].error.as_ref().unwrap().code,
        Some(AssistantDiagnosticErrorCode::Str("E1".into()))
    );
    assert_eq!(
        diagnostics[1].error.as_ref().unwrap().code,
        Some(AssistantDiagnosticErrorCode::Int(-7))
    );

    // Objects and booleans belong to neither variant.
    assert!(
        serde_json::from_str::<AssistantMessage>(
            r#"{"role":"assistant","diagnostics":[{"type":"w","error":{"message":"m","code":{}}}]}"#
        )
        .is_err()
    );
}

#[test]
fn stop_reason_covers_python_literals() {
    let cases = [
        ("stop", StopReason::Stop),
        ("length", StopReason::Length),
        ("toolUse", StopReason::ToolUse),
        ("error", StopReason::Error),
        ("aborted", StopReason::Aborted),
    ];

    for (wire, expected) in cases {
        let message: AssistantMessage =
            serde_json::from_str(&format!(r#"{{"role":"assistant","stopReason":"{wire}"}}"#))
                .unwrap();
        assert_eq!(message.stop_reason, expected);
        assert_eq!(serde_json::to_value(expected).unwrap(), json!(wire));
    }

    assert!(
        serde_json::from_str::<AssistantMessage>(r#"{"role":"assistant","stopReason":"nope"}"#)
            .is_err()
    );
}

#[test]
fn unsigned_fields_reject_negative_values_and_exit_code_is_signed() {
    assert!(
        serde_json::from_str::<AgentMessage>(r#"{"role":"user","content":"x","timestamp":-1}"#)
            .is_err()
    );
    assert!(
        serde_json::from_str::<AgentMessage>(
            r#"{"role":"compactionSummary","summary":"s","tokensBefore":-1}"#
        )
        .is_err()
    );
    assert!(
        serde_json::from_str::<AgentMessage>(r#"{"role":"assistant","usage":{"input":-1}}"#)
            .is_err()
    );

    let bash: BashExecutionMessage = serde_json::from_str(
        r#"{"role":"bashExecution","command":"c","output":"o","exitCode":-9}"#,
    )
    .unwrap();
    assert_eq!(bash.exit_code, Some(-9));
}

#[test]
fn string_content_is_rejected_and_left_to_the_migration_layer() {
    assert!(
        serde_json::from_str::<AssistantMessage>(r#"{"role":"assistant","content":"hi"}"#).is_err()
    );
    assert!(
        serde_json::from_str::<ToolResultMessage>(
            r#"{"role":"toolResult","toolCallId":"c","toolName":"t","content":"hi"}"#
        )
        .is_err()
    );
}

#[test]
fn sum_usage_sums_cost_fields() {
    let total = sum_usage(&[
        Usage {
            input: 1,
            output: 2,
            cache_read: 3,
            cache_write: 4,
            total_tokens: 5,
            cost: UsageCost {
                input: 0.5,
                output: 0.25,
                cache_read: 0.125,
                cache_write: 0.0625,
                total: 1.0,
            },
            ..Usage::default()
        },
        Usage {
            input: 10,
            cost: UsageCost {
                input: 1.5,
                output: 0.75,
                cache_read: 0.25,
                cache_write: 0.125,
                total: 2.0,
            },
            ..Usage::default()
        },
    ]);

    assert_eq!(total.input, 11);
    assert_eq!(total.output, 2);
    assert_eq!(total.cache_read, 3);
    assert_eq!(total.cache_write, 4);
    assert_eq!(total.total_tokens, 5);
    assert_eq!(total.cost.input, 2.0);
    assert_eq!(total.cost.output, 1.0);
    assert_eq!(total.cost.cache_read, 0.375);
    assert_eq!(total.cost.cache_write, 0.1875);
    assert_eq!(total.cost.total, 3.0);
}

#[test]
fn assistant_accessors_preserve_content_order() {
    let message = AssistantMessage {
        content: vec![
            AssistantContent::ToolCall(ToolCall::new("c1", "read")),
            AssistantContent::Text(TextContent::new("a")),
            AssistantContent::Thinking(ThinkingContent::new("why")),
            AssistantContent::ToolCall(ToolCall::new("c2", "write")),
            AssistantContent::Text(TextContent::new("b")),
        ],
        timestamp: 1,
        ..Default::default()
    };

    assert_eq!(message.text(), "ab");
    assert_eq!(message.thinking_text(), "why");
    assert_eq!(
        message
            .tool_calls()
            .iter()
            .map(|call| call.id.as_str())
            .collect::<Vec<_>>(),
        vec!["c1", "c2"]
    );
}

#[test]
fn user_content_conversions_and_empty_list() {
    assert_eq!(UserContent::from("x"), UserContent::Str("x".to_owned()));
    assert_eq!(
        UserContent::from("x".to_owned()),
        UserContent::Str("x".to_owned())
    );

    let empty: UserContent = Vec::<UserContentItem>::new().into();
    let message = UserMessage {
        role: MessageRole::User,
        content: empty,
        timestamp: 1,
    };
    assert_eq!(
        serde_json::to_string(&message).unwrap(),
        r#"{"role":"user","content":[],"timestamp":1}"#
    );

    let blocks: UserContent =
        vec![UserContentItem::Image(ImageContent::new("d", "image/png"))].into();
    assert_eq!(
        serde_json::to_string(&blocks).unwrap(),
        r#"[{"type":"image","data":"d","mimeType":"image/png"}]"#
    );
}
