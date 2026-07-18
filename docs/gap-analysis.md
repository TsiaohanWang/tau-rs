# Functional Gap Analysis: tau-rs vs Python tau

Generated from line-by-line comparison of all agent source files.

## Bug (must fix)

### GAP-1: Duplicate `append_interrupted_tool_results` in `drive_stream`

**Location:** `harness.rs:348` vs `harness.py:148,187`

In Python, `_append_interrupted_tool_results()` is called exactly once before the loop starts (in `prompt_message`/`continue_`) and once in `_run`'s `finally` block only if cancelled.

In Rust, `drive_stream` calls `append_interrupted_tool_results_on_vec(&mut messages)` at line 348 **after** `prompt_message` already called `self.append_interrupted_tool_results()` at line 316. This double-processes the messages unnecessarily.

Worse: on cancellation, `MessagesGuard::drop` (line 128) calls `append_interrupted_tool_results_on_vec` a **third** time on the already-repaired messages, potentially creating duplicate synthetic entries.

**Fix:** Remove the redundant call at `harness.rs:348`. The `prompt_message`/`continue_` entry points already handle the pre-loop repair. `MessagesGuard::drop` handles the post-cancellation repair.

---

## Functional Gaps

### GAP-2: No async subscriber support

**Location:** `harness.rs:94` vs `harness.py:192-196`

Python `EventListener` is `Callable[[AgentEvent], Awaitable[None] | None]` — subscribers can be async functions. The `_notify` method checks with `isawaitable()` and `await`s if needed.

Rust `listeners` stores `Arc<dyn Fn(&AgentEvent) + Send + Sync>` — only sync closures. Async subscribers are not supported.

**Impact:** If any frontend or test needs async event processing (e.g., writing to an async file, calling an async API), it cannot use the subscriber system directly.

**Fix:** This is a design decision for Phase 2+. Async subscribers could be added by having listeners return a `BoxFuture` and spawning them, or by keeping sync-only and documenting the constraint.

---

### GAP-3: No `accepting` guard in tool update collection

**Location:** `agent_loop.rs:305-321` vs `loop.py:272-287`

Python's `_run_tool` uses an `accepting` flag set to `False` in the `finally` block. The `on_update` callback checks `if accepting` before appending, preventing late-arriving updates after the tool executor returns.

Rust's `run_tool` collects all `on_update` calls unconditionally. If a misbehaving executor calls the callback after returning `Ok`/`Err`, those stale updates would be emitted as `ToolExecutionUpdateEvent`s.

**Impact:** Low in practice — well-behaved executors don't call back after returning. But it's a correctness gap vs Python's defensive design.

**Fix:** Add an `Arc<AtomicBool>` accept flag, set it to `false` after the execute call returns, and check it in the update callback.

---

### GAP-4: Unconditional signal clearing in `MessagesGuard::drop`

**Location:** `harness.rs:140` vs `harness.py:188-189`

Python only clears `_current_signal` if it still belongs to the current invocation (`if self._current_signal is signal`). This prevents clobbering a newer run's signal.

Rust unconditionally clears: `*self.state.signal.lock().unwrap() = None`. In practice this is safe because `start_run` uses `compare_exchange` to prevent concurrent runs. But it's a deviation from the Python defensive pattern.

**Impact:** None in practice (the compare_exchange guard prevents the race). But it's a deviation.

**Fix:** Store the `CancellationToken` in `MessagesGuard` and only clear if it matches. Low priority.

---

## Wire Compatibility (Serialization)

### GAP-5: `details` field null handling diverges on edge case

**Location:** `tool_result.rs:18` vs `tools.py:25`

Python `AgentToolResult.details: JSONValue = None` with `exclude_none=True` — when `details` is `None`, the field is **omitted** from JSON.

Rust `AgentToolResult.details: Value` with `skip_serializing_if = "Value::is_null"` — same: when `Value::Null`, the field is **omitted**.

These match in the default case. The divergence only happens if a Rust consumer explicitly sets `details = Value::Null` (which gets skipped) vs Python where `details = None` (also skipped). **No actual gap** — both omit on null.

### GAP-6: `ToolResultMessage.details` default differs

**Location:** `message.rs:480` vs `messages.py:175`

Python: `details: JSONValue = None` → serialized as omitted (via `exclude_none`).
Rust: `details: Value` with `#[serde(default, skip_serializing_if = "Value::is_null")]` → `Value::Null` default, serialized as omitted.

These match. **No gap.**

---

## Missing Edge-Case Handling (Low Priority)

### GAP-7: `before_tool_call` not called for unknown tools in Rust

**Location:** `agent_loop.rs:189-214` vs `loop.py:220-234`

Python calls `before_tool_call` **before** checking if the tool exists. This allows the hook to block or intercept unknown tools.

Rust calls `before_tool_call` first too (line 189-193), so this actually **matches**. ✅ No gap.

### GAP-8: `after_tool_call` not called for blocked/cancelled/unknown-tool paths

**Location:** `agent_loop.rs:194-221` vs `loop.py:246-247`

Both Python and Rust call `after_tool_call` unconditionally after the result is determined (whether from `before_tool_call` block, cancellation, unknown tool, or actual execution). ✅ No gap.

---

## Summary

| ID | Severity | Description | Fix Complexity |
|----|----------|-------------|----------------|
| GAP-1 | **Bug** | Duplicate `append_interrupted_tool_results` in `drive_stream` | Trivial — delete one line |
| GAP-2 | Medium | No async subscriber support | Design decision — defer to Phase 2 |
| GAP-3 | Low | No `accepting` guard in tool update callback | Small — add AtomicBool flag |
| GAP-4 | None | Unconditional signal clearing (safe due to compare_exchange) | Trivial — optional |

**GAP-1 is the only item requiring immediate action.** The rest are either design decisions, defensive improvements, or non-issues.
