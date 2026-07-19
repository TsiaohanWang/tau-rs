# Phase 5 实施计划 — CodingSession + print 模式端到端

> 继承 Phase 1-4（详见 `docs/phase-1.md` / `phase-3.md` / `phase-4.md`）。
> 本阶段是**第一个用户可见里程碑**：`tau-rs -p "…"` 跑通并落盘 session，且写出的 session 可被 Python `tau` resume（双向兼容终极验证）。

---

## 1. 范围

### 1.1 包含

- **CodingSession 组合根接入 CLI** — 取代 `main.rs` 中的 `build_harness` + `persist_message`，建立 `parent_id` 链与 `LeafEntry`（收尾 `architecture-issues.md` #3）。
- **system prompt 组装器接入** — `build_system_prompt` 经由 `CodingSession::new` 间接调用，CLI 不再硬编码 `"You are a helpful assistant."`（收尾 #10）。
- **session load / resume** — `CodingSession::load`：从 JSONL via `SessionState::from_entries` 重放，重建消息与 marketing 元数据；CLI `--resume <id>`（缺省 = 当前目录最新 session）。
- **中断修复** — load 时检测孤儿 `ToolCall`（assistant 消息有 tool_call 但无对应 `ToolResultMessage`），补一条 synthetic "interrupted" error result（对齐 Python `session.py` `_repair_interrupted_tool_calls`）。
- **compaction 三触发** —
  1. 手动：`/compact` 斜杠命令
  2. 阈值：`prompt()` 前检查 `needs_compaction`（骨架已存在，需接 `harness.replace_messages`）
  3. 溢出：捕获 provider 返回的 context-overflow 错误 → compact → 重试**一次**（对齐 Python `stop_reason == error` 且匹配溢出模式时）
- **summary 生成接 LLM** — `generate_summary` 从 debug 格式占位换成调用 provider（用 `SUMMARIZATION_PROMPT` Rust 等价物），溢出场景无 LLM 可调时回退到截断式摘要。
- **自动命名** — 首条 user 消息后推断 session title（v1 用启发式：首行去空白后的前 8 字符；后置接 LLM 命名），追加 `SessionEntry::Label`。
- **斜杠命令注册表** — `/help` `/compact` `/clear` `/model <name>` `/provider <name>` `/exit` `/resume <id>`；命令是 `enum Command` + `fn dispatch`，不引入动态注册（v1 砍扩展）。
- **shell escape `!`/`!!`** — REPL 行首 `!command` 走 bash 直接执行；`!!` 重复上一条 shell 行；执行结果显示在 REPL，不进 provider 上下文。
- **三个 print 渲染器** — `Plain`（流式文本到 stdout + 工具事件到 stderr，v1 默认）、`JsonEvent`（每事件一行 JSON object，供管道处理）、`Transcript`（紧凑人类可读日志，含时间戳）；CLI `--format plain|json|transcript`。
- **工具事件用 `render_call`/`render_result`** — `AgentTool` 的自定义渲染器优先；缺省回退到当前的 `[tool: name → preview]` 格式（收尾 #11）。

### 1.2 不包含（防 scope creep）

- ❌ ratatui TUI（Phase 7）
- ❌ rustyline 行编辑/历史/补全（Phase 6）— 本阶段 REPL 仍用 `io::stdin().lines()`，但斜杠命令与 `!`-escape 先落地
- ❌ OAuth device flow / openai-codex / google / mistral 适配器（Phase 8）
- ❌ 扩展系统动态加载（Phase 8 再评估 WASM/rhai/IPC）
- ❌ session HTML 导出 / update_check（Phase 8）
- ❌ skills + AGENTS.md 发现机制（**延后到 Phase 6** — `build_system_prompt` 已留 `skills` 参数位，v1 仅组装 tools 与 user system，不扫文件）
- ❌ 分支 `branch_to_entry` / `fork`（Phase 6+ — `LeafSelector::At(id)` 重放路径已可用于只读分支预览，但写分支 UI 推迟）

> 估计代码量：`coding_session.rs` ~181 → ~600 行，新增 `commands.rs` ~200、`render/` ~400、`naming.rs` ~80；`tau-coding` 总计 ~+1300 行；`tau-cli` `main.rs` ~+200 行。Phase 5 后测试预计 +60（→ ~190）。

---

## 2. 设计决策（ADR）

### ADR-P5-1 CodingSession 持有 `AgentHarness` 并转发关键方法

`CodingSession` 在 `new`/`load` 时构造 `AgentHarness`，保留对其 `prompt`/`replace_messages`/`cancel` 的引用转发。CLI 不再直接持有 harness，只调 `CodingSession::prompt(&self, text) -> Stream`。

**理由**：① 收敛组合根（架构 issues #3 的本质就是"CLI 越权做了 CodingSession 的活"）；② compaction 需同步改 harness 与本地消息列表，必须同 owner；③ Python 里 CodingSession 也是 harness 的所有者。

**不做**：不把 CodingSession 做成 Python 那样的 2.6k 行 god object — v1 只实现持久化 + compaction + 自动命名 + 命令分发，分支/扩展/模型热切换留给 Phase 6+。

---

### ADR-P5-2 assistant 持久化由 CodingSession 在 stream 包装层做副作用，非 CLI 负责

`CodingSession::prompt` 返回的不是裸 harness stream，而是一个**wrap stream**：下游每收一个 `MessageEnd` event 就自动 `persist_assistant` + 追加 `LeafEntry`，下游无需感知持久化。

```rust
pub fn prompt<'a>(&'a mut self, text: &str) -> Result<impl Stream<Item = AgentEvent> + 'a, SessionError> {
    // ... persist user, pre-compaction check ...
    let inner = self.harness.prompt(text)?;
    Ok(async_stream::stream! {
        pin_mut!(inner);
        while let Some(ev) = inner.next().await {
            if matches!(ev, AgentEvent::MessageEnd(_)) {
                let _ = self.persist_side_effects(&ev).await; // idempotent guard inside
            }
            yield ev;
        }
    })
}
```

**理由**：当前骨架把持久化推给 CLI（`run_repl`/`print_once` 各自 `if let Some(msg) = final_assistant { persist_message(...) }`），是 #3 的另一半病灶。包装层让"持久化副作用"成为 CodingSession 的不变量，不依赖 CLI 是否记得调。

**注意**：`async_stream!` 借用 `&'a mut self`，与 Python "CodingSession 持有自身状态 + generator 内副作用"语义一致；用户 drop stream = 取消，副作用已落盘的就落盘（与 Python 一致）。

---

### ADR-P5-3 compaction 应用：`replace_messages` + 重写本地 entry 链

`execute_compaction` 分三步，**原子性**由顺序保证（crash 时最坏多一条孤立 CompactionEntry，重放时幂等）：

1. 调用 LLM 生成 summary（`SUMMARIZATION_PROMPT` Rust 等价），填进 `CompactionPlan.summary`。
2. `storage.append(SessionEntry::Compaction(...))`。
3. **同步两处**：
   - `harness.replace_messages(rebuilt_messages)` — 新消息列表 = `[summary_user_msg, ...remaining_after_compacted_idx]`
   - 本地 `self.messages` / `self.entry_ids` 同步重写
4. 推进 `self.last_entry_id` 指向刚追加的 CompactionEntry 的 id。

**不做**：不引入 2PC / 跨 JSONL 事务 — Python 也没做，崩在中途留下孤儿 entry 由重放端的 `CompactionEntry.replaces_entry_ids` 幂等处理。

---

### ADR-P5-4 中断修复在 `CodingSession::load` 中做，不修改磁盘

load 时 reconstruct 消息列表后扫一遍：若 assistant 消息含 `ToolCall` 且后续消息里找不到匹配 `tool_call_id` 的 `ToolResultMessage`，则**在内存中**插入一条 synthetic `[interrupted]` error tool result（不 append 到 JSONL）。

**理由**：① 对齐 Python `_repair_interrupted_tool_calls`；② 不污染 session 树的持久化视图（中断状态是"运行时未完成"的反映，磁盘上不该出现伪 result）；③ 后续 harness 可以正常继续。

**测试**：金手建一个含孤儿 tool_call 的 JSONL fixture，断言 load 后内存消息列表里多了一条 `ToolResultMessage { is_error: true, text: "Interrupted" }`。

---

### ADR-P5-5 LLM 调用两类：compaction summary 与（延迟的）auto-naming

- **Compaction summary**：必须调 LLM（截断式摘要丢失上下文太多）。用单独的 `SUMMARIZATION_PROMPT`（从 Python `context_window.py` 逐字翻译 Rust 常量），复用现有 provider 发一次无工具的单轮请求。溢出场景下若 provider 仍报错，回退成"截断式 debug 摘要"+ 警告 log。
- **Auto-naming**：**v1 只做启发式**（首行前 8 字符 / 文件名推断），**不调 LLM**。Python 的 LLM 命名是 quality feature，Phase 5 只保证 title 字段非空以便 `SessionManager::list` 显示，不追求语义准确。

**理由**：把 LLM 调用严格限制在 compaction，避免 print 模式每条 prompt 都隐式多一次往返；命名准确度不影响核心 milestone。

---

### ADR-P5-6 渲染器用 trait + `--format` 分派，工具事件优先用 `render_call`/`render_result`

```rust
pub trait EventRenderer {
    fn on_event(&mut self, ev: &AgentEvent, tools: &[AgentTool]);
    fn flush(&mut self);
}
pub struct PlainRenderer { /* stdout/stderr locks */ }
pub struct JsonEventRenderer { /* stdout */ }
pub struct TranscriptRenderer { /* stdout */ }
```

工具事件渲染：传入 `&[AgentTool]`，按 `event.tool_name` 查 tool；若 `tool.render_call`/`render_result` 为 `Some`，调用拿人类可读串；否则用当前 `[tool: name → preview]` 兜底。这收尾架构 issues #11。

**理由**：① render 逻辑与 provider/harness 解耦，可单测；② `JsonEvent` 是后端/CI 集成的关键，下游 `jq` 友好；③ 渲染 trait 与 Phase 6 的 TUI adapter 共享同一份事件类型。

---

### ADR-P5-7 斜杠命令是静态 `enum`，不引入插件点

```rust
enum Command { Help, Compact, Clear, Model(String), Provider(String), Exit, Resume(Option<String>) }
fn parse_command(line: &str) -> Option<Command> { /* "/…" 前缀 */ }
```

**理由**：Python commands.py 是 dict + 可扩展注册表，但扩展系统在 v1 已砍。静态 enum + `match` 分派足够 Phase 5，且编译期穷尽性检查防止漏处理。Phase 8 扩展系统再评估是否改成 trait object 注册表。

---

## 3. 实施清单（6 子阶段，按依赖顺序）

### 3.1 子阶段 5.1 — CodingSession 接入 CLI（收尾 #3 #10）

**目标**：CLI 不再有 `build_harness`/`persist_message`；`CodingSession` 成为唯一组合根；`parent_id` 链正确。

**修改**：
- `crates/tau-coding/src/session/coding_session.rs`：
  - `prompt` 改为返回 wrap stream（ADR-P5-2），在 `MessageEnd` 时内部持久化 assistant + LeafEntry；用 `async_stream::stream!` 包装 `harness.prompt` 输出。
  - 新增 `fn persist_side_effects(&mut self, ev: &AgentEvent)` — 幂等（用 `HashSet<entry_id>` 去重防重复落盘）。
  - `new` 构造时已调 `build_system_prompt`（已实现）。
- `crates/tau-cli/src/main.rs`：
  - 删 `build_harness` 函数；run_repl/print_once 改为 `let mut session = CodingSession::new(storage, CodingSessionConfig { … });`。
  - 删 `persist_message` 函数；assistant 持久化由 session.prompt 的 wrap stream 兜底。
  - 系统提示不再硬编码：`CodingSessionConfig.system` 接受 `cli.system`（None 则用默认）。
  - `open_or_create_session` 保留但只负责 SessionInfo entry；存储返回给 CodingSession。
- `crates/tau-coding/src/session/mod.rs`：导出 `CodingSession`、`CodingSessionConfig`。

**验收**：
- `cargo test --workspace` 全绿（130 既有的 + 5.1 新增 ≥ 5，新计数发布时同步更新文档）。
- `SESSION_PATH=$(mktemp -d)/s.jsonl` 下跑 `tau-rs -p "say hi"`，`od -c` JSONL 确认每条 MessageEntry 的 `parentId` 字段非空且指向上一条；末尾有 LeafEntry。

---

### 3.2 子阶段 5.2 — Session load / resume + 中断修复

**目标**：`CodingSession::load` 能从已有 JSONL 重建并继续；孤儿 tool_call 修复；CLI `--resume`。

**新增**：
- `crates/tau-coding/src/session/coding_session.rs`：
  - `pub async fn load(storage: JsonlSessionStorage, config: CodingSessionConfig) -> Result<Self, SessionError>`
    1. `storage.read_all()` → `Vec<SessionEntry>`
    2. `SessionState::from_entries(&entries, LeafSelector::Linear)` → 拿 `messages` / `active_leaf_id` / `session_info` / `context_entry_ids`
    3. `repair_interrupted_tool_calls(&mut messages)` —— 扫孤儿 tool_call，插入 synthetic interrupted `ToolResultMessage`
    4. 重建 `AgentHarness` 后 `replace_messages(messages.clone())` 让 harness 从正确状态继续
    5. `self.last_entry_id = active_leaf_id`
- `crates/tau-coding/src/session/repair.rs`（新模块）：`pub fn repair_interrupted_tool_calls(messages: &mut Vec<AgentMessage>) -> Vec<String>` 返回修复的 tool_call_id 列表（供测试断言）。
- `crates/tau-cli/src/main.rs`：新增 `--resume <id>`（默认行为：无此 flag 时调 `SessionManager` 找当前目录最新 session；找不到则 fallback 到 `create`，等于现有 `open_or_create_session`）。`--continue` 作为 `--resume latest` 别名。
- `crates/tau-cli/src/config.rs`：`TauHome::sessions_dir()` 辅助 + `SessionManager::latest_for_project(project_dir) -> Option<session_id>`。

**验收**：
- 单测 `repair.rs`：含孤儿 tool_call 的消息列表 → 修复后多一条 error result。
- 集成测试：写一个 fixture JSONL（Python 风格）→ `CodingSession::load` → 消息数与 `messages.len()` 吻合，无孤儿残留。
- 手动：先 `tau-rs -p "test"`，然后 `tau-rs --resume <id> -p "continue"`，断言两次 prompt 都在同一 session 文件中产生 entry。

---

### 3.3 子阶段 5.3 — compaction 三触发（收尾 #8）✅ Done

**目标**：手动 / 阈值 / 溢出三条路径全通；接 harness。

**修改**：
- `crates/tau-coding/src/compaction_prompts.rs`（新）：`SUMMARIZATION_PROMPT` 与 `UPDATE_SUMMARIZATION_PROMPT`（从 Python `context_window.py` Rust 字面量翻译）。
- `crates/tau-coding/src/session/compaction.rs`：新增 `build_compaction_summary_prompt()`、`serialize_messages_for_compaction()`、`summarization_system_prompt()` 及消息序列化辅助，完整移植 Python 的 `build_compaction_summary_prompt` / `serialize_messages_for_compaction`。
- `crates/tau-coding/src/session/coding_session.rs`：
  - `generate_summary` 改为 async：构造单个 user message 含 `build_compaction_summary_prompt` 输出，调 `provider.stream_response(...)` 拿到 summary 文本。溢出退化路径：若 summary 调用本身又 fail 或返回空，用 debug 格式 + 警告。
  - `execute_compaction`：补 `harness.replace_messages(rebuilt)` 调用（ADR-P5-3 第三步，已落地）。
  - `prompt`：pre-prompt 阈值检查已骨架，补 `context_window` 字段从 catalog `context_windows[model]` 自动读取（目前 config 由外部喂，5.3 时在 `CodingSessionConfig::from_catalog` 构造助手）。
  - 溢出重试：`prompt` 的 wrap stream 在收到 `MessageEnd(stop_reason=Error, message matches overflow)` 时，若**未在重试中**则触发一次 compaction 后再发同一 prompt；用 `self.is_retrying_compaction: bool` 防止无限循环。重试前先 drain 当前 harness stream 释放 `running` 锁。
- `crates/tau-coding/src/commands.rs`（新，沿用 5.4 的 commander）：`Command::Compact` 调 `session.execute_compaction(plan_compaction(...))`。

**验收**：
- ✅ 单测 `generate_summary`：用 `FakeProvider` 返回预定 summary 文本（`generate_summary_uses_llm_provider`）。
- ✅ 单测溢出重试：`FakeProvider` 第一次返回 `stop_reason=Error` 带溢出关键字，第二次返回正常 → 断言调用 3 次（原始 + compaction + retry）且成功（`overflow_retry_compacts_and_retries_once`）。
- ✅ 单测 compaction 后 `messages.len()` 比之前少（`execute_compaction_reduces_harness_messages`）。
- ✅ 退化路径单测：LLM 返回空 → 回退 debug 格式（`generate_summary_falls_back_when_llm_empty`）。

---

### 3.4 子阶段 5.4 — 自动命名 + 斜杠命令 + `!`/`!!` shell escape

**目标**：REPL 可用基础命令集；session 有非空 title；shell escape 可直接跑命令不污染 provider 上下文。

**新增**：
- `crates/tau-coding/src/naming.rs`：`pub fn auto_title(first_user: &str, cwd: &Path) -> String` — 启发式：首行去前后空白，前 8 字符；空则用 `cwd.file_name()` 兜底；末尾省略号。
- `crates/tau-coding/src/commands.rs`（正式落地）：
  - `enum Command { Help, Compact, Clear, Model(String), Provider(String), Exit, Resume(Option<String>) }`
  - `pub fn parse(line: &str) -> Option<Command>` — `"/…"` 前缀解析；非法命令报错。
  - `pub async fn dispatch(session: &mut CodingSession, cmd: Command) -> CommandOutcome` — `CommandOutcome::ClearMessages` / `Handled(String)` / `Quit`。
- `crates/tau-coding/src/shell_escape.rs`：`pub enum ShellLine { Once(String), Repeat }`，`pub fn parse_shell(line: &str) -> Option<ShellLine>`；`pub async fn run(shell: &ShellLine, cwd: &Path) -> String`（直接 spawn `sh -c`，截 stdout+stderr 前 4KB）。
- `crates/tau-coding/src/session/coding_session.rs`：`prompt` 首次调用后若 `self.title.is_none()` 则 `auto_title(...) → storage.append(SessionEntry::Label(...))`。
- `crates/tau-cli/src/main.rs`：REPL 循环顶部依次试 `parse_shell` → `Command::parse` → 否则当普通 prompt 喂给 `session.prompt`。

**验收**：
- 单测 `commands::parse`：`/compact` / `/model gpt-4o` / `/exit` 三个分支。
- 单测 `naming::auto_title`：`"  Fix bug in parser\nmore"` → `"Fix bug"`；空字符串 + `/tmp/foo` → `"foo"`。
- 单测 shell escape：`"! echo hi"` → 触发 spawn，输出 `"hi\n"`；`"!!"` 在上一条 shell 行为 `"echo hi"` 时返回 `"hi\n"`。
- 手动： REPL 输 `/help` 列出命令，`/model deepseek-v4-flash-free` 后续 prompt 走新模型（Phase 5 不实现热切换 persist，5.4 只在内存切）。

---

### 3.5 子阶段 5.5 — 三个渲染器 + 工具事件用 `render_call`/`render_result`（收尾 #11）

**目标**：CLI `--format plain|json|transcript`；工具事件优先用 tool 的自定义渲染器。

**新增**：
- `crates/tau-cli/src/render/mod.rs`（新模块）：
  - `pub trait EventRenderer: Send { fn on_event(&mut self, ev: &AgentEvent, tools: &[AgentTool]); fn flush(&mut self); }`
  - `PlainRenderer`：stdout 文本流，stderr 工具事件（用 `render_call`/`render_result`，兜底当前 `[tool: …]`）。
  - `JsonEventRenderer`：每 event 一行 `serde_json::to_string(&AgentEvent)` 到 stdout。
  - `TranscriptRenderer`：紧凑多人对话格式（`[HH:MM:SS] You: …` / `[HH:MM:SS] Assistant: …` / `[HH:MM:SS] [bash] …`）。
  - 工具事件渲染辅助 `fn render_tool_event(ev, tools) -> Option<ToolRender>`，查询 `tools.iter().find(|t| t.name() == ev.tool_name)`。
- `crates/tau-cli/src/main.rs`：`--format <plain|json|transcript>`（默认 plain）；run_repl/print_once 把 match-event 巨块替换为 `renderer.on_event(&ev, &tools)`。
- `AgentTool` 的 `render_call`/`render_result` 字段在四个 coding tool (`read/write/edit/bash`) 里填上基础实现：bash 显示命令、read 显示路径、write/edit 显示目标文件 + 行数。

**验收**：
- 单测三 renderer 对一条固定 `AgentEvent` 序列的输出快照。
- 单测 `render_tool_event`：有 `render_call` 时用它；无时回退兜底。
- 手动：`tau-rs --format json -p "list files"` 输出逐行 `jq .` 不报错；`--format transcript` 看上去像聊天记录。

---

### 3.6 子阶段 5.6 — 双向兼容验证

**目标**：tau-rs 与 Python tau 写出的 session 文件可互相 resume。

**步骤**：
1. 用 `tau-rs -p "create a hello.txt with 'hi'" --provider opencode -m deepseek-v4-flash-free` 跑一次 → 得到 `<id>.jsonl`。
2. 在 Python tau 仓存在时，跑 `tau --resume <id> -p "now append 'world' to it"`，断言成功；若 Python 仓不可用，用 Rust 写一个 golden 文件，逐字节 diff 与 Python 风格 fixture。
3. 反向：构造 Python 风格 JSONL → `tau-rs --resume <id>` → 继续一轮。
4. 加 golden 测试：`crates/tau-coding/tests/test_compat.rs`，含两个 fixture（一短对话 + 一含 tool_call+result 的），断言 `entry_to_json_line` 输出与 fixture 逐字节相等（含 camelCase / 隐藏 None 字段）。

**验收**：
- 双向任一方向都不报 JSON 解析错；session 文件每一行都是合法 JSON 对象。
- golden diff 通过 CI（`cargo test --workspace --features tau-agent/testing`）。
- 在 architecture-issues.md 把 #3 #10 #11 从 🚧 Partial 改为 ✅ Fixed（自动同步 architecture.md 状态、README badge 测试计数）。

---

## 4. 测试计划

### 4.1 单元测试（per module）

| 模块 | 测试 | 数量 |
|---|---|---|
| `coding_session.rs`（扩展） | wrap stream 持久化幂等、parent_id 链、溢出重试一次、load 重建 + 中断修复、auto_title 触发 | 8 |
| `session/repair.rs`（新） | 无孤儿原样通过、检测孤儿 + 插 synthetic、多重孤儿 | 3 |
| `session/compaction.rs`（扩展） | summary LLM 调用、溢出回退、`replace_messages` 同步 | 3 |
| `naming.rs`（新） | 首行前 8、空白、空回退 cwd、UTF-8 字符边界 | 4 |
| `commands.rs`（新） | `/help`/`/compact`/`/clear`/`/model x`/`/provider y`/`/exit`/`/resume`/非法串 | 9 |
| `shell_escape.rs`（新） | `!cmd` 触发、`!!` 重复、非 `!` 前缀不匹配 | 3 |
| `render/`（新） | 三 renderer 各对 5 event 序列快照、`render_tool_event` 优先用 tool renderer | 7 |
| `compaction_prompts.rs`（新） | 字面量与 Python golden 字符串逐字节相等 | 2 |

### 4.2 集成测试

- `crates/tau-coding/tests/test_coding_session_e2e.rs`：FakeProvider 模拟多轮对话 + 一次 compaction 触发，断言全部 JSONL entry 形成 parent_id 链。
- `crates/tau-cli/tests/test_cli.rs`（扩展）：`--format json` 输出可被 `serde_json` 解析每行；`--resume` 找到现有 session；`/exit` 干净退出；`!echo hi` 输出 `hi\n`。

### 4.3 双向兼容 / golden

- `crates/tau-coding/tests/test_compat.rs`（新）：两个 Python 风格 fixture，断言（i）Rust 读取不报错；（ii）Rust 反向序列化与原字节相等；（iii）`CodingSession::load` 重建后 `messages.len()` 与预期一致。

### 4.4 行为对齐（翻译自 Python `test_session.py` 关键用例）

| Python 测试 | Rust 等价 | 关键断言 |
|---|---|---|
| `test_repair_interrupted_tool_calls` | `repair.rs` 测试 | 孤儿补 synthetic error result |
| `test_auto_compact_on_threshold` | 5.3 阈值单测 | 阈值触发 plan + apply + harness 消息变少 |
| `test_compaction_round_trip` | 5.6 golden | write → read → write 字节相等 |
| `test_persist_message_chain` | 5.1 e2e | parent_id 串联、LeafEntry 在尾部 |

---

## 5. 验收

- [ ] `cargo test --workspace --features tau-agent/testing` 全绿，测试计数更新到当前值（预计 ~190）。
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` 0 警告。
- [ ] `cargo fmt --check` 通过。
- [ ] `tau-rs -p "…"` 单次运行成功，落盘 JSONL 每行合法 JSON。
- [ ] `tau-rs --resume <id> -p "more"` 在同一 session 文件追加 entry。
- [ ] `tau-rs --format json -p "…"` 输出每行 `jq .` 不报错。
- [ ] REPL `/help` `/compact` `/clear` `/model x` `/exit` 均可执行；`!ls` 输出目录列表。
- [ ] **双向兼容**：tau-rs 写的 session 文件可被 Python tau（或等价 fixture 比对）resume；golden 字节相等。
- [ ] architecture-issues.md 中 #3、#8、#10、#11 改为 ✅ Fixed，architecture.md 状态与测试计数同步更新。

---

## 6. 命令

```bash
# 全量验证
cargo test --workspace --features tau-agent/testing
cargo clippy --workspace --all-targets --features tau-agent/testing -- -D warnings
cargo fmt --check

# 本地 end-to-end（需 OPENCODE_API_KEY）
cargo run -p tau-cli -- -P opencode -m deepseek-v4-flash-free -p "say hi"

# resume
cargo run -p tau-cli -- --resume <id> -p "now say bye"

# 渲染模式
cargo run -p tau-cli -- --format json -p "list files in cwd"
cargo run -p tau-cli -- --format transcript -p "explain what you can do"
```

---

## 7. 风险与回退

| 风险 | 缓解 |
|---|---|
| wrap stream 借用 `&mut self` 与 `async_stream` 生命周期难调 | 退路：把持久化副作用移到 CLI 侧但提供 `persist_event` 公共方法，#3 仍收尾但代码不够干净 |
| LLM compaction summary 在 OpenCode 免费模型上可能再 overflow | 已规划溢出回退路径（截断式 + log）；若免费模型连"读两条消息再总结"都挂，降级到**纯截断式摘要**（不调 LLM） |
| 双向兼容 fixture 缺 Python tau 实物对比 | 用手工构造的 Python 风格 JSONL + 严格 serde 字段断言；Phase 8 补真实 Python 联调 |
| 工具 `render_call`/`render_result` 扩展到四个 tool 增加改动面 | 5.5 是最后一阶段，前面已稳；每 tool 一处改动，可独立回退 |