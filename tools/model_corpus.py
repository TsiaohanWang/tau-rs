"""Source of truth for the Python <-> Rust differential corpus.

`tools/gen_fixtures.py` turns this into two committed artifacts:

* `tests/fixtures/model_corpus.jsonl`  - the inputs the Rust test replays
* `tests/fixtures/model_expected.jsonl` - what Python/pydantic validates them to

Rules for cases:

* Every accepted payload must be deterministic: always pass `timestamp`
  explicitly, otherwise both pydantic and serde fill in the current clock.
* `group` is one of `normal`, `extreme`, `edge`; the Rust test asserts that
  every `kind` covers all three groups.
* `divergence` marks an intentional Python/Rust behavior difference; it must
  come with `rust` (the expected Rust verdict). The most common reason is
  pydantic's lax coercion (`true -> 1`, `"1" -> 1`, `1 -> true`), which the
  Rust port rejects on purpose (AGENTS ADR-007).
"""

TS = 1


def case(case_id, kind, payload, group="normal", note=None, divergence=None, rust=None):
    record = {"id": case_id, "kind": kind, "group": group, "input": payload}
    if note:
        record["note"] = note
    if divergence is not None:
        if rust is None:
            raise ValueError(f"{case_id}: divergence cases need a rust expectation")
        record["divergence"] = divergence
        record["rust"] = rust
    return record


# --- payload builders ------------------------------------------------------

def user(content, **extra):
    return {"role": "user", "content": content, "timestamp": TS, **extra}


def assistant(content=None, **extra):
    payload = {"role": "assistant", "timestamp": TS}
    if content is not None:
        payload["content"] = content
    payload.update(extra)
    return payload


def tool_result(content=None, **extra):
    payload = {
        "role": "toolResult",
        "toolCallId": "call-1",
        "toolName": "read",
        "timestamp": TS,
    }
    if content is not None:
        payload["content"] = content
    payload.update(extra)
    return payload


def bash(command="ls", output="out", **extra):
    return {"role": "bashExecution", "command": command, "output": output, "timestamp": TS, **extra}


def custom(content="note", **extra):
    return {"role": "custom", "customType": "note", "content": content, "timestamp": TS, **extra}


def branch(**extra):
    return {"role": "branchSummary", "summary": "branch", "fromId": "f1", "timestamp": TS, **extra}


def compaction(**extra):
    return {"role": "compactionSummary", "summary": "compact", "tokensBefore": 3, "timestamp": TS, **extra}


def text(value, **extra):
    return {"type": "text", "text": value, **extra}


def thinking(value, **extra):
    return {"type": "thinking", "thinking": value, **extra}


def image(data="ZGF0YQ==", mime="image/png"):
    return {"type": "image", "data": data, "mimeType": mime}


def call(**extra):
    payload = {"type": "toolCall", "id": "call-1", "name": "read", "arguments": {}}
    payload.update(extra)
    return payload


def partial(**extra):
    return {"role": "assistant", "content": [], "timestamp": TS, **extra}


def event(tag, **fields):
    return {"type": tag, **fields}


def tool_result_model(**extra):
    payload = {"content": [], "details": None, "addedToolNames": None, "terminate": None}
    payload.update(extra)
    return payload


U64_MAX = 18446744073709551615
LONG = "a" * 10000
UNICODE = "中文🙂 café e\u0301 \u0000-ish"


def deep(levels):
    value = {"leaf": True}
    for index in range(levels):
        value = {f"l{index}": value}
    return value


CASES = [
    # =====================================================================
    # agent_message / normal
    # =====================================================================
    case("user-string", "agent_message", user("hello")),
    case("user-empty-string", "agent_message", user("")),
    case("user-empty-list", "agent_message", user([])),
    case("user-text-block", "agent_message", user([text("a")])),
    case("user-image-block", "agent_message", user([image()])),
    case("user-mixed-blocks", "agent_message", user([text("a"), image("Zg==", "image/jpeg")])),
    case("user-unicode", "agent_message", user(UNICODE)),
    case("user-timestamp-zero", "agent_message", user("x", timestamp=0)),
    case("assistant-minimal", "agent_message", assistant()),
    case("assistant-text", "agent_message", assistant([text("answer")])),
    case("assistant-thinking", "agent_message", assistant([thinking("why", redacted=True)])),
    case("assistant-tool-call", "agent_message", assistant([call(arguments={"path": "README.md"})])),
    case(
        "assistant-all-blocks",
        "agent_message",
        assistant([thinking("t", thinkingSignature="ts"), text("a", textSignature="sig"), call()]),
    ),
    case(
        "assistant-full-meta",
        "agent_message",
        assistant(
            [text("answer")],
            api="anthropic",
            provider="anthropic",
            model="claude",
            responseModel="claude-3",
            responseProvider="bedrock",
            responseId="req-1",
            diagnostics=[
                {"type": "warn", "timestamp": TS, "error": {"message": "m", "code": "E1"}, "details": {"k": 1}}
            ],
            usage={
                "input": 1,
                "output": 2,
                "cacheRead": 3,
                "cacheWrite": 4,
                "cacheWrite1H": 5,
                "reasoning": 6,
                "totalTokens": 7,
                "cost": {"input": 0.5, "output": 0.25, "cacheRead": 0.125, "cacheWrite": 0.0625, "total": 1.0},
            },
            timing={"timeToFirstOutputMs": 8, "totalDurationMs": 9},
            stopReason="toolUse",
            errorMessage="err",
        ),
    ),
    case("assistant-stop-aborted", "agent_message", assistant([], stopReason="aborted")),
    case("assistant-usage-null", "agent_message", assistant([], usage=None)),
    case("tool-result-minimal", "agent_message", tool_result()),
    case("tool-result-full", "agent_message", tool_result(
        [text("out")],
        details={"bytes": 3},
        addedToolNames=["read", "write"],
        isError=True,
    )),
    case("tool-result-image", "agent_message", tool_result([image(), text("caption")])),
    case("bash-minimal", "agent_message", bash()),
    case("bash-full", "agent_message", bash(
        command="echo hi",
        output="hi\n",
        exitCode=0,
        cancelled=False,
        truncated=True,
        fullOutputPath="/tmp/out.txt",
        excludeFromContext=True,
    )),
    case("custom-string", "agent_message", custom("remember")),
    case("custom-blocks", "agent_message", custom([text("remember")], display=False)),
    case("branch", "agent_message", branch()),
    case("compaction", "agent_message", compaction()),
    case("usage-snake-aliases", "agent_message", assistant([], usage={
        "cache_read": 1, "cache_write": 2, "cache_write_1h": 3, "total_tokens": 4,
        "cost": {"cache_read": 0.1, "cache_write": 0.2},
    })),
    case("tool-result-snake-aliases", "agent_message", {
        "role": "toolResult", "tool_call_id": "call-1", "tool_name": "read",
        "content": [{"text": "out"}], "added_tool_names": ["read"], "is_error": True, "timestamp": TS,
    }),
    case("custom-snake-type", "agent_message", {"role": "custom", "custom_type": "note", "content": "x", "timestamp": TS}),
    case("branch-snake-from", "agent_message", {"role": "branchSummary", "summary": "s", "from_id": "f", "timestamp": TS}),
    case("compaction-snake-tokens", "agent_message", {"role": "compactionSummary", "summary": "s", "tokens_before": 3, "timestamp": TS}),
    case("assistant-snake-meta", "agent_message", assistant(
        [], stop_reason="length", response_model="m", response_provider="p", response_id="r", error_message="e",
        timing={"time_to_first_output_ms": 1, "total_duration_ms": 2},
    )),
    case("tool-call-snake-signature", "agent_message", assistant([
        {"type": "toolCall", "id": "c", "name": "n", "thought_signature": "ts"},
        {"type": "thinking", "thinking": "t", "thinking_signature": "s"},
    ])),
    case("diagnostic-code-int", "agent_message", assistant([], diagnostics=[
        {"type": "warn", "timestamp": TS, "error": {"message": "m", "code": -7}},
    ])),
    case("text-block-default-type", "agent_message", assistant([{"text": "a"}])),
    case("tool-result-details-scalar", "agent_message", tool_result([], details=7)),
    case("custom-display-false", "agent_message", custom("x", display=False)),

    # =====================================================================
    # agent_message / extreme
    # =====================================================================
    case("user-timestamp-max", "agent_message", user("x", timestamp=U64_MAX), group="extreme"),
    case("user-long-string", "agent_message", user(LONG), group="extreme"),
    case("user-many-blocks", "agent_message", user([text(f"t{i}") for i in range(64)]), group="extreme"),
    case("assistant-tokens-max", "agent_message", assistant([], usage={
        "input": U64_MAX, "output": U64_MAX, "cacheRead": U64_MAX, "cacheWrite": U64_MAX,
        "cacheWrite1H": U64_MAX, "reasoning": U64_MAX, "totalTokens": U64_MAX,
        "cost": {"input": 1e-7, "output": 1e20, "cacheRead": 2.5e-8, "cacheWrite": -0.0, "total": 1e21},
    }), group="extreme"),
    case("assistant-long-arguments", "agent_message", assistant([
        call(arguments={f"k{i:02d}": i for i in range(50)} | {"text": LONG}),
    ]), group="extreme"),
    case("assistant-deep-details", "agent_message", assistant([], diagnostics=[
        {"type": "warn", "timestamp": TS, "details": deep(8)},
    ]), group="extreme"),
    case("assistant-unicode", "agent_message", assistant([text(UNICODE), thinking(UNICODE)]), group="extreme"),
    case("assistant-empty-strings", "agent_message", assistant([], api="", provider="", model="", errorMessage=""), group="extreme"),
    case("bash-exit-min", "agent_message", bash(exitCode=-2147483648), group="extreme"),
    case("bash-exit-max", "agent_message", bash(exitCode=2147483647), group="extreme"),
    case("compaction-tokens-max", "agent_message", compaction(tokensBefore=U64_MAX), group="extreme"),
    case("assistant-many-diagnostics", "agent_message", assistant([], diagnostics=[
        {"type": "warn", "timestamp": TS, "error": {"message": str(i)}} for i in range(10)
    ]), group="extreme"),
    case("tool-result-long-image", "agent_message", tool_result([image(LONG[:2048], "image/png")]), group="extreme"),
    case("user-empty-content-list-plus-meta", "agent_message", user([], timestamp=U64_MAX), group="extreme"),

    # =====================================================================
    # agent_message / edge
    # =====================================================================
    case("missing-role", "agent_message", {"content": "x"}, group="edge"),
    case("unknown-role", "agent_message", {"role": "nope", "content": "x"}, group="edge"),
    case("empty-role", "agent_message", {"role": "", "content": "x"}, group="edge"),
    case("role-number", "agent_message", {"role": 1, "content": "x"}, group="edge"),
    case("user-missing-content", "agent_message", {"role": "user", "timestamp": TS}, group="edge"),
    case("user-content-number", "agent_message", user(7), group="edge"),
    case("user-content-null", "agent_message", {"role": "user", "content": None, "timestamp": TS}, group="edge"),
    case("user-content-bool", "agent_message", user(True), group="edge"),
    case("user-block-wrong-type", "agent_message", user([{"type": "image", "text": "a"}]), group="edge"),
    case("user-block-unknown-type", "agent_message", user([{"type": "zzz", "text": "a"}]), group="edge"),
    case("user-block-empty-object", "agent_message", user([{}]), group="edge"),
    case("user-unknown-field", "agent_message", user("x", bogus=1), group="edge"),
    case("assistant-unknown-field", "agent_message", assistant([], bogus=1), group="edge"),
    case("assistant-bad-stop", "agent_message", assistant([], stopReason="nope"), group="edge"),
    case("assistant-usage-cost-null", "agent_message", assistant([], usage={"cost": None}), group="edge"),
    case("assistant-content-number", "agent_message", assistant(7), group="edge"),
    case("assistant-content-null", "agent_message", {"role": "assistant", "content": None, "timestamp": TS}, group="edge"),
    case("assistant-content-empty-object", "agent_message", assistant({}), group="edge"),
    case("tool-result-missing-id", "agent_message", {"role": "toolResult", "toolName": "t", "timestamp": TS}, group="edge"),
    case("tool-result-missing-name", "agent_message", {"role": "toolResult", "toolCallId": "c", "timestamp": TS}, group="edge"),
    case("tool-result-arguments-number", "agent_message", assistant([call(arguments=5)]), group="edge"),
    case("bash-missing-output", "agent_message", {"role": "bashExecution", "command": "ls", "timestamp": TS}, group="edge"),
    case("custom-missing-type", "agent_message", {"role": "custom", "content": "x", "timestamp": TS}, group="edge"),
    case("branch-missing-from", "agent_message", {"role": "branchSummary", "summary": "s", "timestamp": TS}, group="edge"),
    case("compaction-missing-tokens", "agent_message", {"role": "compactionSummary", "summary": "s", "timestamp": TS}, group="edge"),
    case("added-names-string", "agent_message", tool_result([], addedToolNames="read"), group="edge"),
    case("diagnostic-code-object", "agent_message", assistant([], diagnostics=[
        {"type": "warn", "timestamp": TS, "error": {"message": "m", "code": {}}},
    ]), group="edge"),
    case("diagnostic-missing-message", "agent_message", assistant([], diagnostics=[{"type": "warn", "timestamp": TS}]), group="edge"),
    case("input-null", "agent_message", None, group="edge"),
    case("input-array", "agent_message", [], group="edge"),
    case("input-empty-object", "agent_message", {}, group="edge"),
    case("alias-duplicate-camel-snake", "agent_message", assistant([], usage={
        "cacheRead": 1, "cache_read": 2,
    }), group="edge",
         note="pydantic's validate_python (the session loader path) rejects duplicates; only validate_json prefers the alias"),
    # Intentional strictness deltas: pydantic lax coercion vs Rust typed deserialization.
    case("timestamp-float-integral", "agent_message", user("x", timestamp=1.0), group="edge",
         divergence="pydantic lax int accepts 1.0; Rust u64 rejects floats", rust={"accepted": False}),
    case("timestamp-bool", "agent_message", user("x", timestamp=True), group="edge",
         divergence="pydantic lax int accepts bool; Rust u64 rejects", rust={"accepted": False}),
    case("timestamp-numeric-string", "agent_message", user("x", timestamp="1"), group="edge",
         divergence="pydantic lax int accepts numeric strings; Rust u64 rejects", rust={"accepted": False}),
    case("timestamp-negative", "agent_message", user("x", timestamp=-1), group="edge",
         divergence="Python int is unbounded/signed; Rust u64 rejects", rust={"accepted": False}),
    case("timestamp-over-u64", "agent_message", user("x", timestamp=2**64), group="edge",
         divergence="Python int unbounded; Rust u64 rejects", rust={"accepted": False}),
    case("tokens-negative", "agent_message", assistant([], usage={"input": -1}), group="edge",
         divergence="Python int signed; Rust u64 rejects", rust={"accepted": False}),
    case("tokens-float-integral", "agent_message", assistant([], usage={"input": 1.0}), group="edge",
         divergence="pydantic lax int accepts 1.0; Rust u64 rejects", rust={"accepted": False}),
    case("is-error-int", "agent_message", tool_result([], isError=1), group="edge",
         divergence="pydantic lax bool accepts 1; Rust bool rejects", rust={"accepted": False}),
    case("is-error-string", "agent_message", tool_result([], isError="true"), group="edge",
         divergence="pydantic lax bool accepts 'true'; Rust bool rejects", rust={"accepted": False}),
    case("display-int", "agent_message", custom("x", display=1), group="edge",
         divergence="pydantic lax bool accepts 1; Rust bool rejects", rust={"accepted": False}),
    case("exit-code-numeric-string", "agent_message", bash(exitCode="9"), group="edge",
         divergence="pydantic lax int accepts numeric strings; Rust i32 rejects", rust={"accepted": False}),
    case("exit-code-float-integral", "agent_message", bash(exitCode=9.0), group="edge",
         divergence="pydantic lax int accepts 9.0; Rust i32 rejects", rust={"accepted": False}),
    case("exit-code-over-i32", "agent_message", bash(exitCode=2**31), group="edge",
         divergence="Python int unbounded; Rust i32 rejects", rust={"accepted": False}),
    case("compaction-tokens-negative", "agent_message", compaction(tokensBefore=-1), group="edge",
         divergence="Python int signed; Rust u64 rejects", rust={"accepted": False}),
    case("assistant-content-string", "agent_message", assistant("hi"), group="edge",
         divergence="ADR-006: Rust rejects bare string content; session migration converts legacy strings",
         rust={"accepted": False}),
    case("tool-result-content-string", "agent_message", tool_result("hi"), group="edge",
         divergence="ADR-006: Rust rejects bare string content; session migration converts legacy strings",
         rust={"accepted": False}),
    case("cache-write-1h-wrong-case", "agent_message", assistant([], usage={"cacheWrite1h": 5}), group="edge",
         note="Python alias is cacheWrite1H (str.title), so cacheWrite1h is unknown on both ends"),

    # =====================================================================
    # assistant_message_event / normal
    # =====================================================================
    case("event-start", "assistant_message_event", event("start", partial=partial())),
    case("event-text-start", "assistant_message_event", event("text_start", contentIndex=0, partial=partial())),
    case("event-text-delta", "assistant_message_event", event("text_delta", contentIndex=0, delta="d", partial=partial())),
    case("event-text-end", "assistant_message_event", event("text_end", contentIndex=0, content="c", partial=partial())),
    case("event-thinking-start", "assistant_message_event", event("thinking_start", contentIndex=1, partial=partial())),
    case("event-thinking-delta", "assistant_message_event", event("thinking_delta", contentIndex=1, delta="d", partial=partial())),
    case("event-thinking-end", "assistant_message_event", event("thinking_end", contentIndex=1, content="c", partial=partial())),
    case("event-toolcall-start", "assistant_message_event", event("toolcall_start", contentIndex=2, partial=partial())),
    case("event-toolcall-delta", "assistant_message_event", event("toolcall_delta", contentIndex=2, delta="d", partial=partial())),
    case("event-toolcall-end", "assistant_message_event", event(
        "toolcall_end", contentIndex=2, toolCall={"type": "toolCall", "id": "c", "name": "n", "arguments": {"a": 1}},
        partial=partial(),
    )),
    case("event-done-stop", "assistant_message_event", event("done", reason="stop", message=partial())),
    case("event-done-length", "assistant_message_event", event("done", reason="length", message=partial())),
    case("event-done-tooluse", "assistant_message_event", event("done", reason="toolUse", message=partial())),
    case("event-error-aborted", "assistant_message_event", event("error", reason="aborted", error=partial())),
    case("event-error-error", "assistant_message_event", event("error", reason="error", error=partial())),
    case("event-snake-content-index", "assistant_message_event", event("text_delta", content_index=0, delta="d", partial=partial())),
    case("event-snake-tool-call", "assistant_message_event", event(
        "toolcall_end", content_index=0, tool_call={"id": "c", "name": "n"}, partial=partial(),
    )),

    # =====================================================================
    # assistant_message_event / extreme
    # =====================================================================
    case("event-partial-timestamp-max", "assistant_message_event", event(
        "start", partial=partial(timestamp=U64_MAX),
    ), group="extreme"),
    case("event-long-delta", "assistant_message_event", event(
        "text_delta", contentIndex=0, delta=LONG, partial=partial(),
    ), group="extreme"),
    case("event-unicode-delta", "assistant_message_event", event(
        "thinking_delta", contentIndex=0, delta=UNICODE, partial=partial(),
    ), group="extreme"),
    case("event-partial-many-blocks", "assistant_message_event", event(
        "text_start", contentIndex=63, partial=partial(content=[text(f"t{i}") for i in range(64)]),
    ), group="extreme"),
    case("event-toolcall-arguments-extreme", "assistant_message_event", event(
        "toolcall_end", contentIndex=0,
        toolCall={"type": "toolCall", "id": "c", "name": "n", "arguments": {"big": 1e20, "small": 1e-7, "neg": -0.0}},
        partial=partial(),
    ), group="extreme"),

    # =====================================================================
    # assistant_message_event / edge
    # =====================================================================
    case("event-missing-type", "assistant_message_event", {"partial": partial()}, group="edge"),
    case("event-unknown-type", "assistant_message_event", event("nope", partial=partial()), group="edge"),
    case("event-camel-tag", "assistant_message_event", event("toolCall_start", contentIndex=0, partial=partial()), group="edge"),
    case("event-missing-content-index", "assistant_message_event", event("text_delta", delta="d", partial=partial()), group="edge"),
    case("event-missing-delta", "assistant_message_event", event("text_delta", contentIndex=0, partial=partial()), group="edge"),
    case("event-unknown-field", "assistant_message_event", event("text_delta", contentIndex=0, delta="d", partial=partial(), extra=1), group="edge"),
    case("event-done-bad-reason", "assistant_message_event", event("done", reason="bogus", message=partial()), group="edge"),
    case("event-error-cross-reason", "assistant_message_event", event("error", reason="stop", error=partial()), group="edge"),
    case("event-done-cross-reason", "assistant_message_event", event("done", reason="aborted", message=partial()), group="edge"),
    case("event-partial-missing-role", "assistant_message_event", event("start", partial={"timestamp": TS}), group="edge",
         divergence="ADR-003: pydantic defaults a standalone message role, Rust requires it",
         rust={"accepted": False}),
    case("event-partial-bad-content", "assistant_message_event", event("start", partial={"role": "assistant", "content": 7, "timestamp": TS}), group="edge"),
    case("event-toolcall-string-call", "assistant_message_event", event(
        "toolcall_end", contentIndex=0, toolCall="c", partial=partial(),
    ), group="edge"),
    case("event-done-with-partial", "assistant_message_event", event("done", reason="stop", partial=partial()), group="edge"),
    case("event-content-index-string", "assistant_message_event", event(
        "text_delta", contentIndex="0", delta="d", partial=partial(),
    ), group="edge",
         divergence="pydantic lax int accepts numeric strings; Rust usize rejects", rust={"accepted": False}),
    case("event-content-index-negative", "assistant_message_event", event(
        "text_delta", contentIndex=-1, delta="d", partial=partial(),
    ), group="edge",
         divergence="Python int signed; Rust usize rejects", rust={"accepted": False}),
    case("event-content-index-float", "assistant_message_event", event(
        "text_delta", contentIndex=1.0, delta="d", partial=partial(),
    ), group="edge",
         divergence="pydantic lax int accepts 1.0; Rust usize rejects", rust={"accepted": False}),

    # =====================================================================
    # agent_tool_result / normal
    # =====================================================================
    case("tool-result-model-empty", "agent_tool_result", {}),
    case("tool-result-model-text", "agent_tool_result", tool_result_model(content=[text("x")])),
    case("tool-result-model-image", "agent_tool_result", tool_result_model(content=[image()])),
    case("tool-result-model-mixed", "agent_tool_result", tool_result_model(content=[text("a"), image(), text("b")])),
    case("tool-result-model-details-object", "agent_tool_result", tool_result_model(details={"a": 1})),
    case("tool-result-model-details-scalar", "agent_tool_result", tool_result_model(details=7)),
    case("tool-result-model-added", "agent_tool_result", tool_result_model(addedToolNames=["read"], terminate=True)),
    case("tool-result-model-terminate-false", "agent_tool_result", tool_result_model(terminate=False)),
    case("tool-result-model-snake-alias", "agent_tool_result", {"added_tool_names": ["x"], "content": [{"text": "hi"}]}),

    # =====================================================================
    # agent_tool_result / extreme
    # =====================================================================
    case("tool-result-model-many-blocks", "agent_tool_result", tool_result_model(
        content=[text(f"t{i}") for i in range(64)],
    ), group="extreme"),
    case("tool-result-model-long-text", "agent_tool_result", tool_result_model(content=[text(LONG)]), group="extreme"),
    case("tool-result-model-deep-details", "agent_tool_result", tool_result_model(details=deep(8)), group="extreme"),
    case("tool-result-model-float-details", "agent_tool_result", tool_result_model(
        details={"small": 1e-7, "big": 1e20, "neg": -0.0, "plain": 0.1},
    ), group="extreme"),
    case("tool-result-model-unicode", "agent_tool_result", tool_result_model(content=[text(UNICODE)]), group="extreme"),
    case("tool-result-model-many-added", "agent_tool_result", tool_result_model(addedToolNames=[f"t{i}" for i in range(32)]), group="extreme"),

    # =====================================================================
    # agent_tool_result / edge
    # =====================================================================
    case("tool-result-model-content-string", "agent_tool_result", {"content": "hi"}, group="edge",
         divergence="ADR-006: Rust rejects bare string content; session migration converts legacy strings",
         rust={"accepted": False}),
    case("tool-result-model-content-number", "agent_tool_result", {"content": 7}, group="edge"),
    case("tool-result-model-unknown-field", "agent_tool_result", {"nope": 1}, group="edge"),
    case("tool-result-model-added-string", "agent_tool_result", {"addedToolNames": "x"}, group="edge"),
    case("tool-result-model-terminate-int", "agent_tool_result", {"terminate": 1}, group="edge",
         divergence="pydantic lax bool accepts 1; Rust bool rejects", rust={"accepted": False}),
    case("tool-result-model-terminate-string", "agent_tool_result", {"terminate": "yes"}, group="edge",
         divergence="pydantic lax bool accepts strings; Rust bool rejects", rust={"accepted": False}),
    case("tool-result-model-input-null", "agent_tool_result", None, group="edge"),
    case("tool-result-model-content-object", "agent_tool_result", {"content": {}}, group="edge"),

    # =====================================================================
    # standalone models / normal + edge
    # =====================================================================
    case("usage-default", "usage", {}),
    case("usage-camel", "usage", {"input": 1, "cacheWrite1H": 2}),
    case("usage-snake", "usage", {"cache_read": 1, "total_tokens": 2}),
    case("usage-negative", "usage", {"input": -1}, group="edge",
         divergence="Python int signed; Rust u64 rejects", rust={"accepted": False}),
    case("usage-over-u64", "usage", {"input": 2**64}, group="edge",
         divergence="Python int unbounded; Rust u64 rejects", rust={"accepted": False}),
    case("usage-cost-null", "usage", {"cost": None}, group="edge"),
    case("usage-unknown-field", "usage", {"nope": 1}, group="edge"),
    case("usage-cost-default", "usage_cost", {}),
    case("usage-cost-floats", "usage_cost", {"input": 1e-7, "output": 1e20, "cache_read": 0.1, "cache_write": -0.0, "total": 1e21}),
    case("usage-cost-snake", "usage_cost", {"cache_read": 0.5, "cache_write": 0.25}),
    case("usage-cost-extreme", "usage_cost", {
        "input": 1e308, "output": 5e-324, "cache_read": -1.7976931348623157e308,
        "cache_write": 0.1, "total": 1e21,
    }, group="extreme"),
    case("usage-cost-bad-type", "usage_cost", {"input": "x"}, group="edge"),
    case("usage-cost-unknown-field", "usage_cost", {"nope": 1}, group="edge"),
    case("usage-extreme", "usage", {
        "input": U64_MAX, "output": U64_MAX, "cacheRead": U64_MAX, "cacheWrite": U64_MAX,
        "cacheWrite1H": U64_MAX, "reasoning": U64_MAX, "totalTokens": U64_MAX,
        "cost": {"input": 1e-7, "output": 1e20},
    }, group="extreme"),
    case("usage-empty-strings-ok", "usage", {"input": 0, "totalTokens": 0}),
    case("response-timing", "response_timing", {"timeToFirstOutputMs": 8, "totalDurationMs": 9}),
    case("response-timing-null-first", "response_timing", {"timeToFirstOutputMs": None, "totalDurationMs": 9}),
    case("response-timing-snake", "response_timing", {"time_to_first_output_ms": 8, "total_duration_ms": 9}),
    case("response-timing-extreme", "response_timing", {"timeToFirstOutputMs": U64_MAX, "totalDurationMs": U64_MAX}, group="extreme"),
    case("response-timing-missing-total", "response_timing", {"timeToFirstOutputMs": 8}, group="edge"),
    case("text-content", "text_content", {"text": "a"}),
    case("text-content-default-type", "text_content", {"text": "a", "textSignature": "s"}),
    case("text-content-snake-signature", "text_content", {"text": "a", "text_signature": "s"}),
    case("text-content-missing-text", "text_content", {"type": "text"}, group="edge"),
    case("text-content-unknown-field", "text_content", {"text": "a", "nope": 1}, group="edge"),
    case("text-content-wrong-type-tag", "text_content", {"type": "thinking", "text": "a"}, group="edge"),
    case("text-content-number", "text_content", {"text": 7}, group="edge"),
    case("text-content-extreme", "text_content", {"text": LONG, "textSignature": UNICODE}, group="extreme"),
    case("thinking-content", "thinking_content", {"thinking": "t", "redacted": True}),
    case("thinking-content-snake", "thinking_content", {"thinking": "t", "thinking_signature": "s"}),
    case("thinking-content-wrong-tag", "thinking_content", {"type": "text", "thinking": "t"}, group="edge"),
    case("thinking-content-extreme", "thinking_content", {"thinking": LONG, "redacted": True}, group="extreme"),
    case("image-content", "image_content", {"data": "Zg==", "mimeType": "image/png"}),
    case("image-content-snake", "image_content", {"data": "Zg==", "mime_type": "image/png"}),
    case("image-content-missing-mime", "image_content", {"data": "Zg=="}, group="edge"),
    case("image-content-extreme", "image_content", {"data": LONG, "mimeType": "image/png"}, group="extreme"),
    case("tool-call", "tool_call", {"id": "c", "name": "n", "arguments": {"a": 1}}),
    case("tool-call-snake", "tool_call", {"id": "c", "name": "n", "thought_signature": "ts"}),
    case("tool-call-missing-name", "tool_call", {"id": "c"}, group="edge"),
    case("tool-call-wrong-tag", "tool_call", {"type": "text", "id": "c", "name": "n"}, group="edge"),
    case("tool-call-extreme", "tool_call", {
        "id": "c", "name": "n", "arguments": {f"k{i:02d}": i for i in range(50)} | {"text": LONG},
        "thoughtSignature": UNICODE,
    }, group="extreme"),
    case("diagnostic-error", "assistant_diagnostic_error", {"message": "m"}),
    case("diagnostic-error-code-str", "assistant_diagnostic_error", {"message": "m", "code": "E1"}),
    case("diagnostic-error-code-int", "assistant_diagnostic_error", {"message": "m", "code": -7}),
    case("diagnostic-error-code-object", "assistant_diagnostic_error", {"message": "m", "code": {}}, group="edge"),
    case("diagnostic-error-missing-message", "assistant_diagnostic_error", {"code": 1}, group="edge"),
    case("diagnostic-error-extreme", "assistant_diagnostic_error", {"message": LONG, "stack": UNICODE, "code": 9223372036854775807}, group="extreme"),
    case("diagnostic-code-over-i64", "assistant_diagnostic_error", {"message": "m", "code": 2**63}, group="edge",
         divergence="Python int unbounded; Rust stores the code as i64",
         rust={"accepted": False}),
    case("diagnostic", "assistant_message_diagnostic", {"type": "warn", "timestamp": TS}),
    case("diagnostic-details", "assistant_message_diagnostic", {"type": "warn", "timestamp": TS, "details": {"k": 1}}),
    case("diagnostic-bad-details", "assistant_message_diagnostic", {"type": "warn", "details": 7}, group="edge"),
    case("diagnostic-extreme", "assistant_message_diagnostic", {
        "type": "warn", "timestamp": U64_MAX, "details": deep(8),
        "error": {"message": LONG, "code": -7},
    }, group="extreme"),
]
