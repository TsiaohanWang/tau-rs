# Phase 1 实施计划 — tau-types + tau-agent 核心

> 状态：✅ 已完成（2026-07-18）
> 目标：建立 wire 契约 + agent 大脑，word-machine-identical 对齐 Python，trait 接缝全部固化。

## 1. 范围

### 1.1 包含
- `tau-types` crate：消息、内容块、事件、provider 流事件、会话条目、AgentToolResult、Usage（纯 serde 数据，无 async）。
- `tau-agent` crate：`ModelProvider`/`ToolExecutor` trait、`run_agent_loop`（纯函数流）、`AgentHarness`（共享状态 + Drop-guard）、session 树遍历/重放/JSONL 含 v1 迁移、`FakeProvider`（feature `testing`）。
- 测试：翻译 `test_agent_loop.py`、`test_agent_harness.py`；golden wire 逐字节对比；遍历真实 `~/.tau/sessions` 解析。

### 1.2 不包含（防 scope creep）
- 不碰 reqwest / tokio 网络。
- 不做 CLI。
- 不实现内置 read/write/edit/bash（用 stub ToolExecutor 测试 loop）。
- 不设计扩展 trait（但保留 `run_agent_loop` 的 `before_tool_call`/`after_tool_call` 参数）。
- 不做 session *文件* 存储（`JsonlSessionStorage` 的读写落到 Phase 4）；仅实现 JSONL *序列化* 与 v1 迁移（纯函数）。

## 2. 设计决策（ADR）

### ADR-1 serde 严格性：手写 Deserialize 透传 deny_unknown_fields
**背景**：Python `WireModel` 用 `extra="forbid"`；Pi 协议要求未知字段被拒绝。serde 的 internally-tagged enum（`#[serde(tag="role")]`）**不支持** `deny_unknown_fields`（serde 限制 #1547）。
**决策**：为顶层判别枚举（`AgentMessage`、`SessionEntry`、`AssistantContent`、`UserContent`、`ToolResultContent`）手写 `Deserialize`：先 `Value::deserialize`，按 tag 分派到变体结构体的 `serde_json::from_value`（此路径 enforce `deny_unknown_fields`）。`Serialize` 仍派生 internally-tagged。事件类（非持久化、仅测试需要）派生 `#[serde(tag="type")]`，不强制严格性。
**代价**：~30 行/枚举；JSON-only（仅支持 self-describing 格式，限制可接受——所有 wire 路径都是 JSON）。
**收益**：error 行为与 Python 一致；错误消息带行号。

### ADR-2 事件 `partial` 用 `Arc<AssistantMessage>`
**背景**：Python 每个 delta 事件做 `partial.model_copy(deep=True)`（O(n) 克隆/事件）。canonicalizer 累积一个 `AssistantMessage` 并发射快照。
**决策**：事件变体字段 `partial: Arc<AssistantMessage>`。canonicalizer 用 owned 累积器，每事件 `Arc::new(acc.clone())` 封装快照——与 Python 同样 O(n) 克隆/事件，但事件**本身**的克隆/传递（fan-out、notify、测试）退化为 O(1)。序列化透明（serde "rc" feature）。
**收益**：fan-out 零成本、快照语义由类型签名自解释；wire 输出逐字节不变。

### ADR-3 harness 用 `Arc<HarnessState>` 共享状态，`prompt` 返回 `'static` 流
**背景**：Python harness 对象同时做流源与控制面板；测试 `test_harness_rejects_overlap` 在 async-for 循环体内调用 `harness.follow_up("Later")`（即流挂起期间并发调用 `&mut self` 的方法）——纯 `&mut` 借用模型无法表达。
**决策**：`AgentHarness { config: HarnessConfigShared (全 Arc), state: Arc<HarnessState> }`。
  - `HarnessState` 内 `messages: Mutex<Vec<AgentMessage>>`、`steering: Mutex<VecDeque<_>>`、`follow_up: Mutex<VecDeque<_>>`、`signal: Mutex<Option<CancellationToken>>`、`running: AtomicBool`、`listeners: RwLock<Vec<Arc<dyn Fn(&AgentEvent)+Send+Sync>>>`。
  - `prompt(&self) -> Result<impl Stream + Send + 'static, HarnessError>`：返回**不借用 `&self`** 的流——所有依赖（config Arc 克隆、state Arc 克隆、prompt 消息 owned）由流拥有 → `steer()/follow_up()/cancel()`（均 `&self`）可并发调用。
  - 运行期从 `state.messages` `mem::take` 取出消息 Vec，纯 `run_agent_loop` 借用 `&mut Vec`；Drop guard 在流结束（正常或早退）时把 Vec 归还 + 重置 `running`/`signal` + 若取消则 `append_interrupted_tool_results`。
**代价**：运行期间 `harness.messages()` 返回空（消息已 take 出）。无测试在此期间读；语义上"运行期观察通过事件，不通过 messages getter"是 Python 已隐含的意图。rust 化为编译期纪律。
**收益**：`follow_up`/`steer` 并发安全；`&self` API；无 `&mut self` 长期借用；早退清理由 Drop 保证。

### ADR-4 `run_agent_loop` 是纯函数（`&mut Vec<AgentMessage>`），harness 用 take-out/put-back 包装它
**背景**：Python `run_agent_loop` 接受 list 并原地 mutate；测试直接传 local list 并断言。Rust 想保留"可单独用 local Vec 测试"的纯函数 API，又想被共享状态 harness 使用。
**决策**：
  - `run_agent_loop(LoopArgs<'_>) -> impl Stream<Item=AgentEvent> + Send + '_`，`messages: &'a mut Vec<AgentMessage>`。测试直接传 local `&mut vec`。
  - harness 的 prompt 流：async-stream 块内 `let owner = MessagesOwner { state, messages: take() }; let msgs = owner.borrow_mut(); let mut s = run_agent_loop(LoopArgs { messages: msgs, ... }); while let Some(ev) = s.next().await { notify; yield ev; }` —— `owner` 与 `s` 是同一作用域兄弟局部，`s` 借用 `owner`，作用域结束按声明逆序析构（`s` 先释放借用、`owner` 后 Drop 归还），早退亦然。无自引用结构。

### ADR-5 拉取式流（async-stream），不上 channel/task
**决策**：`run_agent_loop`、harness prompt、子流（`_assistant_events`/`_execute_tool_call`）全部 `async_stream::stream!`。拉取式语义 = Python generator 背压；drop = generator close = 取消。
**拒绝**：mpsc channel + spawned task（推模型，缓冲破坏背压、abort 时机语义变化）。

### ADR-6 CancellationToken 复用 tokio_util
`tokio_util::sync::CancellationToken` 取代 Python `SimpleCancellationToken`/`CancellationToken`/`ToolCancellationToken` 三个 Protocol，统一为具体类型，`clone()` 即共享句柄。

### ADR-7 AgentTool.name 用 `Arc<str>`（非 `&'static str`）
**决策**：`name: Arc<str>` 而非 `&'static str`，使工具可由运行期数据（扩展、动态）注册；内置工具用 `Arc::<str>::from("read")`。`AgentTool` 全字段 `Clone`（Arc 克隆）。

## 3. Crate 设计

### 3.1 tau-types

依赖：`serde`、`serde_json`(preserve_order, "rc" feature)、`uuid`(v4)。无 async、无 tokio、无 thiserror。

| 模块 | 内容 |
|---|---|
| `message.rs` | 内容块 `TextContent`/`ThinkingContent`/`ImageContent`/`ToolCall`；联合 `AssistantContent`(tag type, 3)、`UserContent`(untagged)、`ToolResultContent`(tag type, 2)、`UserBlock`；消息 `UserMessage`/`AssistantMessage`/`ToolResultMessage`/`BashExecutionMessage`/`CustomMessage`/`BranchSummaryMessage`/`CompactionSummaryMessage`；`AgentMessage`(tag role, 7)；`Usage`/`UsageCost`/`StopReason`；`AssistantMessageDiagnostic`/`AssistantDiagnosticError`；辅助 `content_text`/`message_text`/`message_to_user`/`assistant_content`；时间戳 `current_timestamp_ms()` + 提供 `new_entry_id()` 供 entry 复用（或放 session.rs）。**默认 timestamps 用 serde `default = "current_timestamp_ms"`**（对齐 pydantic default_factory，缺失时填充 now——保留 Python quirk）。 |
| `event.rs` | `AgentEvent`(tag type, 10 变体，rename_all="snake_case")：`agent_start`/`agent_end`(messages)/`turn_start`/`turn_end`(message, tool_results)/`message_start`/`message_update`(message, assistant_message_event)/`message_end`/`tool_execution_start`(tool_call_id, tool_name, args)/`tool_execution_update`(+partial_result)/`tool_execution_end`(+result, is_error)。派生 ser/de（非持久化，跳过严格性）。 |
| `provider_event.rs` | `AssistantMessageEvent`(tag type, 12 变体)：`Start`/`Text`{Start,Delta,End}/`Thinking`{Start,Delta,End}/`ToolCall`{Start,Delta,End}/`Done`(reason, message)/`Error`(reason, error)；`DoneReason`/`ErrorReason`。`partial: Arc<AssistantMessage>`。派生 ser/de。 |
| `tool_result.rs` | `AgentToolResult`(content: Vec<ToolResultContent>, details: Value(null-skip), added_tool_names: Option, terminate: Option<bool>)；含 `from_text()` 构造器、`text()` 方法。 |
| `session.rs` | `SessionEntry`(tag type, 9 变体)：`MessageEntry`(message)/`ModelChangeEntry`(model)/`ThinkingLevelChangeEntry`(thinking_level: Option)/`CompactionEntry`(summary, replaces_entry_ids)/`BranchSummaryEntry`(summary, branch_root_id: Option)/`LabelEntry`(label)/`LeafEntry`(entry_id: Option)/`SessionInfoEntry`(created_at, cwd: Option, title: Option)/`CustomEntry`(namespace, data: Map)。共享字段 `id`(default new_entry_id)、`parent_id`(skip)、`timestamp`(f64 seconds default now)。字段名**snake_case**（entry 层无 camelCase！），但嵌套 `message` 用 camelCase（来自 message.rs）。 |
| `lib.rs` | 模块导出 + `prelude`。 |

**关键字段映射细节**（避免回归）：
- camelCase alias（`toolCallId`/`toolName`/`addedToolNames`/`isError`/`responseModel`/`responseId`/`errorMessage`/`totalTokens`/`cacheWrite1h`/`textSignature`/`thinkingSignature`/`fullOutputPath`/`excludeFromContext`/`tokensBefore`/`fromId`/`customType`/`cacheRead`/`cacheWrite`）。
- 始终序列化（非 None 默认）的 bool 字段：`ThinkingContent.redacted=false`、`ToolResultMessage.is_error=false`、`BashExecutionMessage.cancelled=false`/`truncated=false`/`exclude_from_context=false`、`CustomMessage.display=true`、`AssistantMessage.stop_reason="stop"`、`AssistantMessage.usage`=默认对象、`AssistantMessage.api`/`provider`/`model`="unknown"。
- `Option` 字段 skip-if-none：`text_signature`/`thinking_signature`/`thought_signature`/`cache_write_1h`/`reasoning`/`response_model`/`response_id`/`error_message`/`diagnostics`/`added_tool_names`/`exit_code`/`full_output_path`/`branch_root_id`/`thinking_level`/`cwd`/`title`(entry)/`parent_id`/`entry_id`(leaf)/`terminate`。
- `Value` 字段 null-skip：`ToolResultMessage.details`、`CustomMessage.details`、`AssistantMessageDiagnostic.details`。
- `AssistantDiagnosticError.code`：untagged 枚举 `String`/`i64`。

### 3.2 tau-agent

依赖：`tau-types` + `futures`（Stream, StreamExt, BoxStream）+ `async-stream` + `async-trait` + `tokio-util`（CancellationToken）+ `tokio`（sync 锁、 AtomicBool; 实际只用 `tokio::sync::Mutex`/`RwLock` 与 `std::sync::atomic`?——选用 tokio::sync::Mutex 因可能跨 await 持守?此处不跨 await 持守；用 `std::sync::Mutex` 足够且更快，配 `AtomicBool` running、`parking_lot`? 不引依赖，用 `std::sync::Mutex`/`RwLock`）+ `thiserror`。

> 决策：用 `std::sync::Mutex`/`RwLock`（锁不跨 await 持守——每次操作 lock-take-unlock）。`running` 用 `std::sync::atomic::AtomicBool`。

| 模块 | 内容 |
|---|---|
| `provider.rs` | `trait ModelProvider: Send + Sync`、`StreamRequest<'a>`、re-export `CancellationToken`。 |
| `tool.rs` | `trait ToolExecutor`（async_trait）、`ToolError`（thiserror 单字段 `message`，`From<String>`）、`AgentTool`（结构体，全 Arc 字段，`Clone`）、`ToolExecutionMode { Sequential, Parallel }`、`BeforeToolCall`/`AfterToolCall` 类型别名（HRTB）、`ToolUpdateCallback`、`ToolCallRenderer`/`ToolResultRenderer` trait。 |
| `agent_loop.rs` | `LoopArgs<'a>`、`run_agent_loop` 返回 `impl Stream + Send + '_`；私有 `assistant_events`/`execute_tool_call` 子流（async-stream）。`_error_result`/`_error_message` 辅助。 |
| `harness.rs` | `AgentHarness`、`HarnessConfig`（或 `HarnessConfigShared`）、`HarnessError`(AlreadyRunning)、`QueuedMessages`、`QueueMode`、`Unsubscribe`、`HarnessState`(私有)、`RunGuard`(私有 Drop)、`prompt`/`prompt_message`/`continue_`/`steer`/`steer_message`/`follow_up`/`follow_up_message`/`cancel`/`subscribe`/`subscribe_async`/`unsubscribe`/`queued_messages`/`pending_message_count`/`has_queued_messages`/`pop_latest_steering`/`pop_latest_follow_up`/`clear_queues`/`append_message`/`replace_messages`/`messages`/`append_interrupted_tool_results`。 |
| `session/tree.rs` | `entries_by_id`/`path_to_entry`/`SessionTreeError`。 |
| `session/state.rs` | `LeafSelector<'a>`、`SessionState`、`from_entries`、`apply_compaction`、分支/compaction 摘要格式化。 |
| `session/jsonl.rs` | `entry_to_json_line`/`entry_from_json_line`/`entries_from_json_lines`/`SessionJsonlError`/`migrate_session_entry`/`migrate_message`。 |
| `testing.rs` | `FakeProvider`、`ProviderCall`（feature `testing`，also `#[cfg(any(test, feature="testing"))]`）。 |
| `lib.rs` | 模块导出。 |

## 4. 实施清单

### 4.1 tau-types
- [x] `Cargo.toml`
- [x] `message.rs`：内容块 4 + 联合 3 + 消息 7 + usage/stop + diagnostics + 手写 Deserialize（AgentMessage/AssistantContent/UserContent/ToolResultContent/UserBlock 5 枚举）+ 辅助函数
- [x] `event.rs`：AgentEvent 派生
- [x] `provider_event.rs`：AssistantMessageEvent 派生 + DoneReason/ErrorReason
- [x] `tool_result.rs`：AgentToolResult
- [x] `session.rs`：SessionEntry 9 + 手写 Deserialize
- [x] `lib.rs`
- [ ] golden 测试（fixtures 由 Python 生成）— 待实现

### 4.2 tau-agent
- [x] `Cargo.toml`
- [x] `provider.rs`
- [x] `tool.rs`
- [x] `agent_loop.rs`
- [x] `harness.rs`
- [x] `session/tree.rs`
- [x] `session/state.rs`
- [x] `session/jsonl.rs`
- [x] `testing.rs`（feature `testing`）
- [x] `lib.rs`
- [x] 翻译 `test_agent_loop.py`（8 测试）
- [x] 翻译 `test_agent_harness.py`（6 测试）
- [x] session replay/jsonl 测试
- [x] 真实 `~/.tau/sessions` 解析测试（env/skip 守卫）

## 5. 测试计划

### 5.1 翻译测试（行为对齐）
| Python 测试 | Rust 翻译 | 关键断言 |
|---|---|---|
| `test_agent_loop_streams_canonical_nested_events` | `agent_loop::streams_canonical_nested_events` | 事件 type 序列 + delta 序列 + messages 终态 |
| `test_agent_loop_nests_thinking_events_without_losing_final_message` | `..._thinking` | thinking delta 序列 + final assistant |
| `test_agent_loop_executes_tool_and_emits_tool_result_message_lifecycle` | `..._executes_tool` | ToolResult 消息 + 3 个 message_start + provider.calls[1].messages |
| `test_agent_loop_passes_call_id_signal_and_progress_to_tool` | `..._passes_call_id` | observed (call_id, signal) + update event 文本 |
| `test_agent_loop_records_unknown_tool_as_canonical_error_result` | `..._unknown_tool` | is_error + result text "Tool {n} not found" |
| `test_agent_loop_converts_provider_error_to_assistant_error_message` | `..._provider_error` | 事件序列 + stop_reason=error + error_message |
| `test_agent_loop_injects_steering_and_follow_up_messages` | `..._steering_follow_up` | user 文本序列 + 3 次 provider 调用 |
| `test_agent_loop_stops_with_assistant_error_after_max_turns` | `..._max_turns` | stop_reason=error + errorMessage 字串 + 1 次调用 |
| `test_prompt_appends_user_and_assistant_with_pi_lifecycle` | harness | 事件序列 + messages 终态 |
| `test_subscribers_receive_nested_message_updates_and_unsubscribe` | harness | seen 含 message_update + agent_end + 2 次调用 |
| `test_harness_rejects_overlap_and_drains_followups` | harness | 同时调用 prompt 时 AlreadyRunning 错误 + follow_up + 终态 |
| `test_harness_queue_mode_all_drains_messages_together` | harness | queue_mode=All 时两条 follow_up 一起排空 |
| `test_harness_passes_canonical_tools_to_loop` | harness | tool 执行 + provider.calls[0].tools |
| `test_queue_mutators_return_canonical_snapshots` | harness | pop_latest_steering/follow_up + queued_messages 快照 + clear_queues |
| `test_harness_repairs_interrupted_tool_calls` | harness | append_interrupted_tool_results 计数 + 修复消息 |

### 5.2 Golden wire 测试
由 `scripts/gen_fixtures.py`（uv run）在 `~/.tau` 旁生成 `tests/fixtures/*.json`：每种消息、事件、entry 的序列化。Rust 测试 `parse -> reserialize -> assert_eq`（中文字符串走 UTF-8，serde_json 不转义！）。

### 5.3 真实数据测试
`tests/parse_real_sessions.rs`：glob `~/.tau/sessions/**/*.jsonl`；每行 `entry_from_json_line` 必须成功；遇缺失目录 `ignored` 跳过。这验证 v1 迁移与 camelCase。

### 5.4 Golden 反向（开发期手工）
`scripts/check_roundtrip.py`：读取 Rust 序列化输出（由 test 写入 fixtures）→ 用 Python pydantic 解析 → assert 等价。开发期运行一次，不进 CI。

## 6. 验收

- [x] `cargo build --workspace` 零警告（`-D warnings` 由 CI 强制，本地 `cargo clippy -- -D warnings`）
- [x] `cargo test --workspace` 全绿
- [x] `cargo test --features testing`（FakeProvider 用例）全绿
- [x] 真实 `~/.tau/sessions` 全量解析（test 5.3）通过或 ignored
- [ ] golden 逐字节对比（test 5.2）通过 — 待实现
- [x] `cargo fmt --check` 通过

## 7. 命令

```bash
cd ~/Codespace/tau-rs
cargo build --workspace
cargo test --workspace --features testing
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
# 生成 golden fixtures（planned）：
# cd ~/Codespace/tau && uv run python ~/Codespace/tau-rs/scripts/gen_fixtures.py
```