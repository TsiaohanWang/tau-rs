# Phase 7 实施计划 — ratatui TUI

> 继承 Phase 1-6（详见 `docs/phase-1.md` ~ `phase-6.md`）。
> 本阶段把原版 `tau_coding/tui/`（`app.py` 6070 行 + `adapter.py` + `state.py` + `widgets.py` + `autocomplete.py`）移植为 **ratatui** 终端 UI。
> 核心约束（来自原版 `AGENTS.md` 与 `docs/architecture.md` §7）：**TUI 只依赖 `tau-types` 事件 + `CodingSession` 只读接口，绝不反向依赖 `tau-agent`/`tau-ai` 的 HTTP**。Rust 等价约束：ratatui 代码置于 `feature = "tui"`，默认不编译，且只消费 `AgentEvent` + `CodingSession` 暴露的查询方法。

---

## 0. 实施进度（2026-07-19 完成）

| 子阶段 | 状态 | 落点 |
| --- | --- | --- |
| 7.1 `TuiState` + `TuiEventAdapter`（纯，可单测） | ✅ 已完成 | `tau-cli/src/tui/{state,adapter}.rs` |
| 7.2 ratatui 渲染 + 主循环（最小布局） | ✅ 已完成 | `tau-cli/src/tui/{ui,app}.rs` |
| 7.3 输入处理 + 快捷键 | ✅ 已完成 | `app.rs` `handle_key`（Enter/Esc/Ctrl-C/Ctrl-O/Ctrl-T/Ctrl-D） |
| 7.4 `--tui` CLI 接线 | ✅ 已完成 | `main.rs` feature-gated 分支 + `--tui` flag |
| 7.5 测试与文档 | ✅ 已完成 | 5 个 `adapter` 单测；README/architecture 同步；本文件 |

约束落实：ratatui 仅依赖 `tau-types` 事件 + `CodingSession` 只读接口（经克隆 `AgentHarness` 句柄做 steer/cancel/queue 查询）；`feature = "tui"` 默认关闭，无 TUI 构建不拉 ratatui；clippy 与 `fmt` 在两种配置下均干净；全仓测试 195 全绿。运行：`cargo run --features tui -- --tui`。

---

## 1. 分析：原版 TUI 的分层与可移植边界

原版 `tui/` 实际是三层，只有其中两层需要移植，第三层（扩展/主题皮肤）可大幅裁剪：

1. **纯函数适配层 `adapter.py`（`TuiEventAdapter.apply`）** —— 输入 `CodingSessionEvent`，就地修改 `TuiState`。**这是移植的核心边界**：它是无 Textual 依赖的纯逻辑，原版注释明确 "Translate Pi-compatible session events into Textual display state"。Rust 版等价为 `fn apply(state: &mut TuiState, event: &AgentEvent)`，消费 `tau_types::AgentEvent`（已是 Pi 兼容事件），不碰 Textual/ratatui。
2. **显示状态 `state.py`（`TuiState` / `ChatItem`）** —— 一个 `items: Vec<ChatItem>` + 缓冲 + 运行标志 + 队列 + 开关。职责是"对话转录本的投影"。**完全可 1:1 翻译**为 Rust `struct TuiState { items: Vec<ChatItem>, ... }`。
3. **Textual 组件 `app.py` / `widgets.py` / `autocomplete.py`（~8800 行）** —— 这是"皮肤 + 布局 + 输入处理 + picker"，是重写成本最高、收益最外围的部分。Rust 用 ratatui 重写，但**只取最小可用子集**（消息面板 + 工具进度 + 输入条 + 状态栏），不移植 autocomplete/picker/skill 皮肤等 Phase 8 广度。

### 1.1 事件对应关系（Rust 已具备）

| 原版 `CodingSessionEvent` | Rust `tau_types::AgentEvent` | 处理 |
|---|---|---|
| `AgentStartEvent` | `AgentStart` | `running = true; error = None` |
| `AgentEndEvent` | `AgentEnd` | flush + `running = false` |
| `agent_settled`（CodingSession 特有） | — | Rust `CodingSession::prompt` 不产此事件；用 `AgentEnd` 等价 |
| `QueueUpdateEvent` | — | **Rust 无此事件**；改用 `harness.subscribe` 桥接（见 §2.4） |
| `MessageStartEvent(Assistant)` | `MessageStart` | 起 `assistant_buffer`，记起始 index |
| `MessageUpdateEvent(TextDelta)` | `MessageUpdate(TextDelta)` | `assistant_buffer += delta` |
| `MessageUpdateEvent(ThinkingDelta)` | `MessageUpdate(ThinkingDelta)` | `add_thinking_delta` |
| `MessageEndEvent(User)` | `MessageEnd(UserMessage)` | `add_user_message` |
| `MessageEndEvent(Assistant)` | `MessageEnd(AssistantMessage)` | 用终态消息覆盖 buffer 行；error/aborted 特殊处理 |
| `MessageEndEvent(Custom)` | `MessageEnd(CustomMessage)` | `add_user_message(custom_type=...)` |
| `ToolExecutionStartEvent` | `ToolExecutionStart` | `add_tool_call` |
| `ToolExecutionUpdateEvent` | `ToolExecutionUpdate` | `record_tool_update` |
| `ToolExecutionEndEvent` | `ToolExecutionEnd` | `record_tool_result` |
| `AutoRetryStartEvent` | — | Rust compaction 重试**不产此事件**；可省略或后续加 |

> 结论：**`adapter.py` + `state.py` 的纯逻辑可几乎逐行翻译**；唯一需要新增的桥接是队列显示（原版靠 `QueueUpdateEvent`，Rust 靠 harness 订阅）。

### 1.2 ChatItem 角色（来自 `TranscriptRole`）

`user` / `assistant` / `tool` / `skill` / `thinking` / `error` / `custom` / `branch_summary` / `compaction_summary` / `status`。Phase 7 实现全部角色的文本投影（ratatui 渲染为带颜色的多行段落）；`skill`/`custom`/`branch_summary`/`compaction_summary` 在 Phase 7 用纯文本占位（`skills`/`context` 数据 Phase 8 才接入，见 §1.3）。

### 1.3 明确不在 Phase 7 范围（防 scope creep）

- ❌ **autocomplete / picker / 主题皮肤**（`widgets.py` 的 model/thinking/session picker、`autocomplete.py`）：Phase 8 广度，或后续独立 phase。
- ❌ **skills / context_files / AGENTS.md 发现**：原版 `state.set_skills` / `custom_renderer` 依赖 `extensions`/`skills` 运行时，Phase 8 才移植。Phase 7 的 `TuiState` 保留 `skills`/`custom_renderer` 字段位但默认空。
- ❌ **扩展系统的 `render_call`/`render_result` 在 TUI 内的懒解析**：Phase 7 复用工具已有的 `render_call`/`render_result`（`tau-agent::tool::AgentTool` 已带），但不在 TUI 内做扩展运行时桥接。
- ❌ **真实 API ratatui 交互端到端**（需 `OPENCODE_ZEN_API_KEY`）仅作冒烟，不强制 CI。

---

## 2. 设计决策（ADR）

### ADR-P7-1 TUI 消费 `TuiState`，由 `TuiEventAdapter::apply` 增量构建，复用 `EventRenderer` 的格式化

- `TuiEventAdapter`（位于 `tau-cli/src/tui/adapter.rs`）持有一个 `TuiState`，暴露 `fn apply(&mut self, &AgentEvent)`。逻辑逐行对应 `adapter.py`。
- 工具调用/结果的**格式化文本**复用 `tau-cli/src/render/mod.rs` 的 `render_tool_start` / `render_tool_end`（已存在，含 `render_call`/`render_result` 优先逻辑）。避免 TUI 与 plain/transcript 渲染各写一份格式化。
- `TuiState` 是纯数据（无 ratatui 依赖），可在 `#[cfg(test)]` 中单测 `apply` 的行为，无需起终端。

### ADR-P7-2 TUI 置于 `feature = "tui"`，默认关闭，且不反向依赖 harness/provider 的 HTTP

- `tau-cli/Cargo.toml` 增加 `ratatui` + `crossterm` 作为 `optional = true` 依赖，feature `tui` 开启它们；`main.rs` 用 `#[cfg(feature = "tui")]` 暴露 `--tui` 模式。
- TUI 模块只 `use tau_types`、`tau_coding::session::CodingSession`（只读方法）、`tau_coding::tools`（工具名/渲染器查询）。**不** `use tau_agent::harness` 内部 HTTP 逻辑（仅通过 `CodingSession`/事件流交互）。
- 编译无 TUI 时 `cargo build --workspace` 不拉 ratatui，保持二进制体积与 CI 速度（对齐 §7 约束）。

### ADR-P7-3 队列显示（steer/follow_up）经 harness 订阅桥接，而非新事件

原版 `QueueUpdateEvent` 由 CodingSession 在 `steer`/`follow_up` 时发出。Rust `CodingSession::prompt` 的流不含此类事件。方案：在 TUI 主循环里，对 `session.harness()` 调用 `subscribe` 或轮询 `queued_messages()`，将 `steering`/`follow_up` 文本投影到 `TuiState.queued_*`。Phase 7 先用**主循环每帧轮询** `CodingSession` 暴露的 `queued_message_count()` / 各队列快照（简单、无并发复杂度），后续可升级为事件订阅。

> 注意：`CodingSession` 当前未暴露 harness 的队列查询方法。需在 `CodingSession` 上加 `pub fn queued_steering(&self) -> Vec<String>` / `queued_follow_up` / `queued_message_count`（薄转发到 harness）。

### ADR-P7-4 最小布局：三区（transcript / input / status），纯键盘，无鼠标

ratatui 布局：
- 顶部主区 `TuiState.items` 滚动渲染（每行一个 `ChatItem`，按 `role` 配色）。
- 底部输入区：单行/多行编辑（Phase 7 用 `ratatui` + `crossterm` 的 `TextArea` 思路，或自写单行编辑；为减少依赖，先用 `crossterm` 原始键处理 + 一个 `String` 缓冲，`Enter` 发送）。
- 状态栏：显示 `running` 状态、队列计数、`model`、`thinking level`、历史提示。
- 快捷键：`Enter` 发送（若运行中则 steer）、`Esc` 取消（harness `cancel()`）、`Ctrl-C` 清上下文、`Ctrl-D`/`/exit` 退出、`Ctrl-O` 切换工具结果展开、`Ctrl-T` 切换 thinking 显示。

### ADR-P7-5 输入编辑用 `crossterm` 原始键事件 + 行缓冲，不引入 `tui-textarea`

为控制依赖与编译体积，Phase 7 输入条用 `crossterm::event::read()` 自管 `String` 缓冲 + 光标。若后续需要多行编辑再引入 `tui-textarea`。

---

## 3. 实施清单（子阶段，按依赖顺序）

### 3.1 子阶段 7.1 — `TuiState` + `TuiEventAdapter`（纯，先行，可单测）

新建 `tau-cli/src/tui/mod.rs`（feature 门）+ `tui/adapter.rs` + `tui/state.rs`：

- `tui/state.rs`：
  - `enum ChatItemRole { User, Assistant, Tool, Skill, Thinking, Error, Custom, BranchSummary, CompactionSummary, Status }`（对应 `TranscriptRole`）。
  - `struct ChatItem { role, text, tool_call_id, tool_name, tool_arguments, tool_result_text, started_at, update_text, custom_type, details, always_show }`。
  - `struct TuiState { items, assistant_buffer, assistant_start_index: Option<usize>, running, error, show_tool_results, show_thinking, queued_steering, queued_follow_up, skills, ... }`。
  - 方法（对应 `state.py`）：`add_item` / `add_user_message` / `add_thinking_delta` / `add_tool_call` / `find_tool_item` / `record_tool_update` / `record_tool_result` / `toggle_tool_results` / `toggle_thinking` / `update_queue` / `clear` / `load_messages`（resume 重放）。
  - 工具调用/结果格式化复用 `crate::render::{render_tool_start, render_tool_end}`。
- `tui/adapter.rs`：
  - `pub struct TuiEventAdapter { state: TuiState, assistant_start_index: Option<usize> }`。
  - `pub fn apply(&mut self, ev: &AgentEvent)`：逐分支对应 `adapter.py::apply`（见 §1.1 表）。`MessageEnd(Assistant)` 用 `stop_reason` 判 error/aborted；思考块用 `ThinkingContent` 投影（Rust 在 `MessageEnd` 已聚合，无增量 `ThinkingDelta` 单独事件——见 §3.1 注）。
  - `pub fn state(&self) -> &TuiState`。
- **注（增量 vs 聚合）**：原版 `MessageUpdate(ThinkingDelta)` 是增量流式思考；Rust `AgentEvent` 只有 `MessageUpdate(TextDelta)`/`ThinkingDelta`？需核对 `tau_types::AssistantMessageEvent` 是否有 `ThinkingDelta`。若有则同原版；若无（Rust 在 `MessageEnd` 才给完整 AssistantMessage），则 `apply` 在 `MessageEnd(Assistant)` 时把 `ThinkingContent` 块投影为 `thinking` item。Phase 7.1 实现兼容两者。

### 3.2 子阶段 7.2 — ratatui 渲染 + 主循环（最小布局）

- `tui/app.rs`：
  - `pub async fn run(session: CodingSession, cwd, home_history, verbose, format)`（feature 门）。
  - 初始化 `crossterm::terminal::enable_raw_mode` + `EnterAlternateScreen` + `ratatui::Terminal<CrosstermBackend>`。
  - 主循环：`event::poll` 超时 → 若有键事件则处理输入（见 ADR-P7-4/5）；每帧 `terminal.draw(|f| ui(f, &adapter.state(), &input_buf, &status))`；并每帧把 `session` 的新事件喂给 `adapter.apply`。
  - **事件喂入**：TUI 不自己驱动 `session.prompt`（那是 REPL 职责）。Phase 7 的 TUI 是"前端"，它需要在用户发送后调用 `session.prompt` 并**在 `prompt` 流进行中，把每个 `AgentEvent` 实时 `apply` 到 `TuiState`**，同时主循环渲染。由于 `session.prompt` 是 `Stream`，TUI 主循环用 `tokio::select!` 在"键事件"与"流 next"之间多路：流产出事件→`adapter.apply`；键事件→输入/取消/发送（发送时 spawn 一个新的 prompt 流消费任务，或复用单个 in-flight 流）。
  - **并发模型（关键）**：用 `tokio::select!` 在 `crossterm 键事件`（需 `block_on`/channel 桥接到 async）与 `session.prompt` 流之间切换。crossterm 是同步的，需一个任务 `tokio::task::spawn_blocking` 读键并 `mpsc::unbounded_channel` 发给主循环。主循环 `select!` 该 channel 与 prompt 流。
- `tui/ui.rs`：`fn ui(frame, state, input, status)` 用 `Layout` 分三区并渲染。工具结果按 `state.show_tool_results` 展开。

### 3.3 子阶段 7.3 — 输入处理 + 快捷键

- `Enter`：若 `!`/`/` 开头走 shell escape / 斜杠命令（复用 `tau_coding::shell_escape` / `commands`）；否则若 `session` 正在 `running` 则 `harness.steer(text)`，否则启动新 `prompt` 流（spawn 消费任务）。
- `Esc`：`session.harness().cancel()`（需暴露 `cancel` 转发）。
- `Ctrl-C`：`session.clear_messages()`（同 REPL）。
- `Ctrl-O` / `Ctrl-T`：`state.toggle_tool_results()` / `toggle_thinking()`。
- `Ctrl-D` 或 `/exit`：退出。

### 3.4 子阶段 7.4 — `CodingSession` 薄转发（支持 ADR-P7-3/7.3）

在 `coding_session.rs` 加：
- `pub fn queued_steering(&self) -> Vec<String>` / `queued_follow_up(&self) -> Vec<String>` / `pub fn queued_message_count(&self) -> usize`（转发 `harness`）。
- `pub fn harness_cancel(&self)`（转发 `harness.cancel`）。
- `pub fn harness(&self) -> &AgentHarness`（只读借用，供 TUI 订阅/查询；不暴露可写 HTTP）。

### 3.5 子阶段 7.5 — 测试与文档

- `tui/adapter.rs` `#[cfg(test)]`：对照 `test_tui_adapter.py` 关键用例——`assistant` 增量→`MessageEnd` 覆盖、`error`/`aborted` 投影、`tool` start/update/end 绑定、thinking 块、`custom`/`branch_summary`/`compaction_summary` 文本投影、`queue` 更新。
- `tui/state.rs` `#[cfg(test)]`：`add_item` / `record_tool_result` O(1) 查找 / `toggle_*` / `load_messages` 重放。
- 编译验证：`cargo build --workspace`（无 tui feature）仍不拉 ratatui；`cargo build -p tau-cli --features tui` 通过；`cargo clippy -p tau-cli --features tui -- -D warnings`。
- 手动冒烟：`cargo run -p tau-cli --features tui -- -P opencode --tui`（需 API key），观察 transcript/工具/状态栏。
- 文档：`docs/phase-7.md`（本文件）、`README.md` roadmap Phase 7 标记 Done、`architecture.md` §4/§6.3/§6.6 同步。

---

## 4. 测试计划

### 4.1 单元测试（纯逻辑，无终端）

- `TuiEventAdapter::apply` 逐事件行为（对照 `test_tui_adapter.py`）。
- `TuiState` 方法（对照 `state.py` 行为）。
- `CodingSession` 队列转发 / cancel 转发。

### 4.2 编译/特性门

- `cargo build --workspace` 不含 ratatui。
- `cargo build -p tau-cli --features tui` 成功。
- clippy/fmt 在两种配置下均干净。

### 4.3 手动冒烟（需 `OPENCODE_ZEN_API_KEY`）

```bash
cargo run -p tau-cli --features tui -- -P opencode --tui
# 输入消息 → transcript 滚动显示 assistant 文本 + 工具调用/结果
# Ctrl-O 展开工具结果；Ctrl-T 切换 thinking；Esc 取消；Ctrl-D 退出
```

---

## 5. 验收

- [x] `TuiEventAdapter::apply` 行为与原版 `adapter.py` 对齐（单测覆盖）。
- [x] ratatui 三区布局渲染 `TuiState`：transcript 滚动、输入条、状态栏。
- [x] 工具调用/结果经 `render_tool_start`/`render_tool_end` 格式化（折叠/展开 + 预览截断）。
- [x] 快捷键：Enter 发送、Esc 取消、Ctrl-C 清上下文、Ctrl-O/Ctrl-T 切换、Ctrl-D 退出。
- [x] 队列（steer/follow_up）经 `harness.queued_messages().count()` 显示在状态栏。
- [x] `feature = "tui"` 默认关闭；无 TUI 构建不拉 ratatui（已用 `cargo tree` 目检）。
- [x] clippy + fmt 干净；全仓测试 195 全绿。

---

## 6. 命令

```bash
# 默认构建（无 TUI 依赖）
cargo build --workspace

# 带 TUI 构建
cargo build -p tau-cli --features tui

# 运行 TUI
./target/debug/tau -P opencode --tui

# TUI 内
Enter           发送（运行中 = steer）
Esc             取消当前流
Ctrl-C          清 in-memory 消息
Ctrl-O          展开/收起工具结果
Ctrl-T          显示/隐藏思考块
Ctrl-D / /exit  退出
```

---

## 7. 风险与回退

- **事件增量差异**：若 Rust `AgentEvent` 缺少 `ThinkingDelta` 增量（只在 `MessageEnd` 给完整 AssistantMessage），`apply` 改为在 `MessageEnd(Assistant)` 投影 `ThinkingContent` 块；行为等价（原版也只是把 ThinkingContent 块投影成 thinking item）。
- **crossterm 同步键事件与 tokio async 的桥接**：用 `spawn_blocking` + `mpsc` channel，主循环 `select!` 键 channel 与 prompt 流；回退方案是直接用 `crossterm` 在单线程 `block_on` 内轮询（功能等价，仅并发度低）。
- **ratatui 版本漂移**：锁定 `ratatui` 0.29 / `crossterm` 0.28（当前稳定），API 相对稳定；若破坏性变更，pin 版本。
- **TUI 不反向依赖 harness HTTP**：通过仅暴露 `CodingSession` 只读方法 + `AgentEvent` 流保证；CI 可加 `cargo tree --features tui` 目检不引入非预期 HTTP 抽象（ratatui 本身无网络依赖）。

---

## 实施后修复记录（2026-07-19）

初始实现中发现的 6 个 bug 已全部修复，详见 [`docs/tui-fixes.md`](tui-fixes.md)：

| # | 问题 | 修复提交 |
|---|------|----------|
| 1 | 输入框不清空/退格无效/重复发送/无法滚动 | `b1e36ad` |
| 2 | 光标不可见 + 光标位置偏差（Unicode prompt） | `347a723` |
| 3 | 光标狂晃（终端 cursor vs ratatui）+ 退格 Unicode panic | `b169503` |
| 4 | Assistant 消息重复 + Delete 键缺失 | `53e154b` |
| 4.5 | 发送后输入框不清空（命令/shell 后错误启动流） | `262bc93` |
| 5 | 彻底修复重复 + 光标残留 + 输入不清空（空 span 渲染问题） | `7f8ddf2` |

最终状态：200（默认）/ 205（--features tui）测试全绿，TUI 可用。
