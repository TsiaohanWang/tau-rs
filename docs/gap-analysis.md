# Functional Gap Analysis: tau-rs vs Python tau

> 更新时间: 2026-07-18
> 状态: Phase 1-3 已完成

## 已修复的问题

### GAP-1: Duplicate `append_interrupted_tool_results` in `drive_stream` — ✅ 已修复

**原始位置:** `harness.rs:348` vs `harness.py:148,187`

**问题**: `append_interrupted_tool_results` 被多次调用，可能导致重复的 synthetic entries。

**修复**: `MessagesGuard::drop` 现在仅在取消时调用 `append_interrupted_tool_results_on_vec`（第133-134行），且第392行有注释明确说明此处不调用。

---

## 当前设计决策（by design）

### GAP-2: No async subscriber support — 设计决策

**位置:** `harness.rs:94` vs `harness.py:192-196`

**说明**: Python `EventListener` 支持 async 函数。Rust `listeners` 仅支持 sync closures。

**影响**: 如果前端或测试需要异步事件处理（如写入异步文件、调用异步 API），不能直接使用 subscriber 系统。

**状态**: 这是 Phase 2+ 的设计决策。可以通过让 listeners 返回 `BoxFuture` 并 spawn 来添加异步支持，或保持 sync-only 并记录约束。

**优先级**: 中（设计决策，非 bug）

---

### GAP-3: No `accepting` guard in tool update collection — 低优先级

**位置:** `agent_loop.rs:305-321` vs `loop.py:272-287`

**说明**: Python 的 `_run_tool` 使用 `accepting` 标志，在 `finally` 块中设置为 `False`。`on_update` 回调检查 `if accepting` 后才追加，防止工具执行器返回后到达的 late-arriving updates。

Rust 的 `run_tool` 无条件收集所有 `on_update` 调用。如果执行器在返回 `Ok`/`Err` 后调用回调，这些 stale updates 会被发出为 `ToolExecutionUpdateEvent`。

**影响**: 实际影响低 — 良好行为的执行器不会在返回后调用回调。

**修复**: 添加 `Arc<AtomicBool>` accept 标志，在 execute 调用返回后设置为 `false`，并在 update 回调中检查。

**优先级**: 低

---

### GAP-4: Unconditional signal clearing in `MessagesGuard::drop` — 安全偏差

**位置:** `harness.rs:140` vs `harness.py:188-189`

**说明**: Python 仅在 `_current_signal` 仍属于当前调用时清除（`if self._current_signal is signal`）。Rust 无条件清除。

**影响**: 实际无影响（`compare_exchange` 保护防止并发运行）。但这是一个偏差。

**修复**: 在 `MessagesGuard` 中存储 `CancellationToken`，仅在匹配时清除。低优先级。

**优先级**: 无（安全偏差）

---

## 已确认无差距的问题

### GAP-5: `details` field null handling — ✅ 无差距

**位置:** `tool_result.rs:18` vs `tools.py:25`

Python 和 Rust 都在 `details` 为 `None`/`Null` 时省略该字段。

### GAP-6: `ToolResultMessage.details` default — ✅ 无差距

**位置:** `message.rs:480` vs `messages.py:175`

Python 和 Rust 都使用 `None`/`Null` 默认值，并在序列化时省略。

### GAP-7: `before_tool_call` not called for unknown tools — ✅ 无差距

**位置:** `agent_loop.rs:189-214` vs `loop.py:220-234`

Python 和 Rust 都在检查工具是否存在之前调用 `before_tool_call`。

### GAP-8: `after_tool_call` not called for blocked/cancelled/unknown-tool paths — ✅ 无差距

**位置:** `agent_loop.rs:194-221` vs `loop.py:246-247`

Python 和 Rust 都在结果确定后无条件调用 `after_tool_call`。

---

## 总结

| ID | 严重性 | 描述 | 状态 | 修复复杂度 |
|----|--------|------|------|-----------|
| GAP-1 | Bug | Duplicate `append_interrupted_tool_results` | ✅ 已修复 | — |
| GAP-2 | Medium | No async subscriber support | ⚠️ 设计决策 | 设计决策 |
| GAP-3 | Low | No `accepting` guard in tool update callback | ⚠️ 待修复 | 小 |
| GAP-4 | None | Unconditional signal clearing | ⚠️ 安全偏差 | 可选 |

**结论**: Phase 1-3 的所有 bug 级别问题已修复。剩余问题均为设计决策或低优先级改进。
