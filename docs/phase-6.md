# Phase 6 实施计划 — 交互 REPL + thinking-level 透传

> 继承 Phase 1-5（详见 `docs/phase-1.md` / `phase-3.md` / `phase-4.md` / `phase-5.md`）。
> 本阶段将 `tau-cli` 朴素 `stdin().lock().lines()` REPL 升级为 **rustyline** 驱动的交互式终端，并把 thinking/reasoning-effort 等级从 REPL 一路透传到 provider。
> 架构层面的"REPL 为 Phase 6 范围、thinking 透传需给 `StreamRequest` 加字段"两点已在 `docs/architecture.md` §4 Phase 6 / §7 预先界定。

---

## 1. 范围

### 1.1 包含

- **rustyline REPL（`tau-cli/src/repl.rs`）** — 取代 `main.rs` 中的 `run_repl` / `run_repl_resumed` / `handle_repl_line` 三个朴素函数。
  - 持久化历史：每次会话结束 `editor.save_history(home.root.join("history"))`；启动 `load_history`（失败非致命，首跑无文件）。
  - Tab 补全：斜杠命令名、`CodingSession::tools()` 返回的工具名、以及相对当前 `cwd` 的本地文件路径（目录补尾随 `/`）。
  - `Ctrl-C`（`ReadlineError::Interrupted`）：清 in-memory 消息（等价于 `/clear`），继续循环。
  - `Ctrl-D`（`ReadlineError::Eof`）：正常退出（先存历史）。
- **`/thinking` 斜杠命令** — `commands.rs` 新增 `Command::Thinking(Option<String>)`：
  - 无参 → 打印当前等级（未设置显示 `default (off)`）。
  - `off` 或空 → `set_thinking_level(None)`（恢复 provider 默认）。
  - 其他字符串 → `set_thinking_level(Some(level))`，下一条 `prompt` 生效（in-memory）。
- **thinking-level 全链路透传** — 这是 `docs/architecture.md` §4 Phase 6.3 标注的"架构改动点"：
  - `StreamRequest<'a>` 新增 `thinking_level: Option<&'a str>`。
  - `AgentHarnessConfig` / `HarnessConfigShared` 新增 `thinking_level: Option<String>`；`AgentHarness` 新增 `set_thinking_level` / `thinking_level`。
  - `LoopArgs` 新增 `thinking_level: Option<&'a str>`，在 `run_agent_loop` 内填入 `StreamRequest`。
  - `CodingSessionConfig` 新增 `thinking_level: Option<String>`，构造时拷贝进 harness；`CodingSession` 暴露 `set_thinking_level` / `thinking_level`。
  - **OpenAI 兼容 provider**：`build_payload` 在 `thinking_level` 非 `off`/非空时写入 `reasoning_effort`（对齐 catalog `thinking_parameter = "reasoning_effort"`）；`off`/`None` 不写字段。
  - **Anthropic provider**：`stream_response` 收到非空非 `off` 等级时设 `thinking_mode = "adaptive"` 且 `thinking_effort = level`；否则清空 `thinking_effort`（沿用既有 adaptive 分支）。

### 1.2 不包含（防 scope creep）

- ❌ ratatui TUI（Phase 7）
- ❌ `/thinking` 的 catalog 合法性校验 / `thinking_levels` 枚举约束（v1 接受任意字符串，由 provider 拒绝非法值；后续可接 `CatalogProvider.thinking_levels`）
- ❌ `/model` `/provider` 持久化落盘 `providers.json`（issue #16 推迟，同 Phase 5）
- ❌ `SessionInfo.title` 回填（issue #16 部分，推迟）
- ❌ OAuth / openai-codex / google / mistral 适配器（Phase 8）
- ❌ skills / context_files / branch / export / 扩展（Phase 8）

---

## 2. 设计决策（ADR）

### ADR-P6-1 REPL 用 rustyline，历史/补全内建，不自己造轮子

原版 `cli.py` 的交互分支（`run_print_mode` / `run_openai_print_mode`）依赖 Python 的 `input()` + readline，无独立补全器。Rust 选 **rustyline 14**（行编辑、emacs/vi 键绑定、`DefaultHistory`、可插拔 `Helper`）而非自写 `stdin` 轮询：
- 历史持久化、行编辑、撤销、`Ctrl-R` 反向搜索等开箱即得；
- `Helper` trait 把补全逻辑与渲染解耦——补全器只持有 `tool_names` + `cwd` 两个不可变上下文，不依赖 harness/provider（与 §7 "TUI 不反向依赖 harness" 同一原则）。

### ADR-P6-2 thinking 等级作为 `StreamRequest` 的一等字段，而非 provider 配置

原版 `session.py::set_thinking_level` 修改的是**运行时 provider 配置**（重建 provider）。Rust 选择把它作为每次 `stream_response` 调用的请求参数：
- 避免为切换 thinking 重建整个 provider/HTTP client（更廉价、更可测）；
- `StreamRequest` 已是 provider-neutral 的"一次请求"载体，`thinking_level` 天然归属此处；
- provider 侧各自翻译：`reasoning_effort`（OpenAI 兼容）/`thinking` adaptive（Anthropic），翻译逻辑集中在 `build_payload` / `stream_response` 顶部，对照 catalog 的 `thinking_parameter` 字段。

> 注意：`off` 语义映射为"不传参"，与 Python `ThinkingLevel.OFF` 一致——provider 默认即关闭。

### ADR-P6-3 `/thinking` 是 in-memory 切换，不持久化

与 `/model` `/provider`（Phase 5，issue #16 推迟）保持一致：thinking 等级只改 `CodingSessionConfig.thinking_level` + harness，不写 `providers.json`、不追加 `SessionEntry`。原因：这些"运行时偏好"的持久化是同一类工作，应统一在 issue #16 里一次性设计（含 `ModelChangeEntry` / `ThinkingLevelChangeEntry`）。

### ADR-P6-4 补全只在行尾触发，按词边界切分

`complete()` 早期返回条件：`pos < line.len()` → 无候选（REPL 不是编辑器，光标只在末尾）。否则按 `rfind(whitespace)` 求当前词起点 `word_start`，对该词前缀匹配：
- 行以 `/` 开头且仍是首个 token → 仅匹配 `COMMAND_NAMES`；
- 否则 → 工具名 ∪ 文件路径候选（去重）。
首个 Tab 直接补全公共前缀，第二个 Tab 由 rustyline 列出全部候选（库行为，无需我们处理）。

---

## 3. 实施清单

### 3.1 子阶段 6.1 — thinking-level 透传（架构改动点）

| 文件 | 改动 |
|------|------|
| `tau-agent/src/provider.rs` | `StreamRequest` 加 `thinking_level: Option<&'a str>` |
| `tau-agent/src/agent_loop.rs` | `LoopArgs` 加 `thinking_level`；构造 `StreamRequest` 时填入 |
| `tau-agent/src/harness.rs` | `AgentHarnessConfig` / `HarnessConfigShared` 加 `thinking_level: Option<String>`；`set_thinking_level` / `thinking_level`；`LoopArgs` 构造处透传 `config.thinking_level.as_deref()` |
| `tau-ai/src/openai.rs` | `stream_response` 加 `thinking_level` 参数 → `build_payload`；`build_payload` 非空非 `off` 时写 `reasoning_effort`；`OpenAIModelProvider` 透传 `request.thinking_level` |
| `tau-ai/src/anthropic.rs` | `stream_response` 加 `thinking_level` 参数；非空非 `off` 时设 `thinking_mode=adaptive` + `thinking_effort`；`AnthropicModelProvider` 透传 |
| `tau-coding/src/session/coding_session.rs` | `CodingSessionConfig` 加 `thinking_level: Option<String>`；两处 `AgentHarnessConfig` 构造透传；`set_thinking_level` / `thinking_level`；`generate_summary` 的 `StreamRequest` 补 `thinking_level: None` |
| `tau-cli/src/main.rs` | `CodingSessionConfig` 两处字面量补 `thinking_level: None`；`ephemeral_print` 的 `AgentHarnessConfig` 补 `thinking_level: None` |

所有既有 `StreamRequest` / `AgentHarnessConfig` / `LoopArgs` / `CodingSessionConfig` 字面量（含测试）补齐新字段——编译期强制，无遗漏。

### 3.2 子阶段 6.2 — `/thinking` 命令

| 文件 | 改动 |
|------|------|
| `tau-coding/src/commands.rs` | `Command::Thinking(Option<String>)`；`parse` 匹配 `thinking`；`dispatch` 三态（显示 / 设置 / 清除）；`help_text` 增加 `/thinking [level]` 行；新增 `parse_thinking_*` 单测 |

### 3.3 子阶段 6.3 — rustyline REPL（`repl.rs`）

新建 `tau-cli/src/repl.rs`：

- `struct ReplHelper { tool_names: Vec<String>, cwd: PathBuf }`，实现 `Helper`/`Highlighter`/`Hinter`/`Validator`/`Completer`。
- `fn complete(...)`：见 ADR-P6-4。
- `fn path_candidates(cwd, fragment)`：按 `fragment` 可能含 `/` 切分目录/末段，读 `cwd.join(base)` 列名前缀匹配；目录候选补 `""` 后缀。
- `async fn run(session, cwd, home_history, verbose, format)`：构建 `Editor<ReplHelper, DefaultHistory>`，`load_history`，主循环 `readline("You: ")`：
  - 空行 → `continue`；非空 → `add_history_entry` 后 `handle_line`。
  - `handle_line` 复用既有 `shell_escape::parse_shell` + `commands::parse`/`dispatch`（顺序：shell escape → 斜杠命令 → 普通 prompt）。
  - `LineOutcome::RunPrompt(text)` → `run_prompt_stream`（`session.prompt(text)` 消费 `EventRenderer`，逻辑同原 `run_repl`）。
- `COMMAND_NAMES` 常量与 `commands::parse` 的头部列表保持一致（单一事实源靠人工对齐，后续可导出）。

`main.rs` 改动：加 `mod repl;`；删除 `run_repl` / `ReplLineResult` / `handle_repl_line` / `run_repl_resumed`；交互分支改为先 `open_or_create_session` 或 `resume_session` 得到 `CodingSession`，再 `repl::run(session, &cwd, &history_path, cli.verbose, &cli.format)`；移除不再使用的 `std::io`/`BufRead`/`Write`/`commands`/`shell_escape` 导入。

### 3.4 子阶段 6.4 — 测试与文档

- `tau-ai/src/openai.rs` `tests`：`build_payload_includes_reasoning_effort_for_thinking_level`——`high` → 含 `reasoning_effort:"high"`；`off`/`None` → 不含。
- `tau-agent/tests/test_agent_harness.rs`：`thinking_level_is_set_and_read_back`——`set`/`get`/`clear` 往返。
- `tau-cli/src/repl.rs` `tests`：`completes_slash_commands` / `completes_tool_names_on_plain_input` / `no_completion_in_middle_of_line`——`ReplHelper::complete` 行为。
- `tau-coding/src/commands.rs`：`parse_thinking_with_and_without_arg`。
- 测试总数 184 → **190**（+6：`tau-ai` +1、`tau-agent` +1、`tau-cli` +3、`tau-coding` +1）。
- `README.md` 测试徽章 184→190、测试清单表、Phase 6 roadmap 标记 Done。
- `docs/architecture.md` §4 Phase 6（标记 Done）、§6.3 模块表、§6.6 待实现表同步。

---

## 4. 测试计划

### 4.1 单元测试

- `openai::build_payload` 的 `reasoning_effort` 注入/省略（含 `off`）。
- `AgentHarness` thinking 存取往返。
- `ReplHelper::complete` 三类场景（命令/工具名/行内不补全）。
- `commands::parse` 的 `/thinking` 含参/无参。

### 4.2 回归

- 全量 `cargo test --workspace --features tau-agent/testing` 绿（190）。
- `cargo clippy --workspace --all-targets --features tau-agent/testing -- -D warnings` 绿。
- `cargo fmt --check` 绿。

### 4.3 手动冒烟（无需 API key）

```bash
printf '/help\n/thinking\n/thinking high\n/thinking off\n/exit\n' | \
  ./target/debug/tau -P opencode
# 期望：/help 打印命令表；/thinking 依次显示 default → set high → cleared

printf '! echo hi\n/exit\n' | ./target/debug/tau -P opencode
# 期望：输出 "hi"；退出后 ~/.tau/history 含上述行
```

### 4.4 真实 API（需 `OPENCODE_ZEN_API_KEY`）

```bash
./target/debug/tau -P opencode
# 交互中输入消息，途中按 /thinking high 切换；观察 provider 实际收到 reasoning_effort
# （可用 verbose 或抓包确认请求体含 reasoning_effort 字段）
```

---

## 5. 验收

- [x] `/thinking` 显示/设置/清除均正确（CLI 可观测）。
- [x] rustyline REPL 启动、历史保存/加载、Tab 补全可用、`Ctrl-C`/`Ctrl-D` 行为正确。
- [x] thinking 等级穿透到 provider 请求体（OpenAI `reasoning_effort`、Anthropic adaptive）。
- [x] 190 测试全绿；clippy `-D warnings`；fmt 干净。
- [x] 手动冒烟：`/help` / `/thinking` / `!` escape / 历史持久化均通过。

---

## 6. 命令

```bash
# 构建
cargo build -p tau-cli

# 交互 REPL（rustyline）
./target/debug/tau -P opencode

# REPL 内
/help            # 命令列表（含 /thinking）
/thinking        # 显示当前等级
/thinking high   # 设置（下次 prompt 生效）
/thinking off    # 清除（provider 默认）
! ls             # shell escape（不进上下文）
!!               # 重复上条 shell 命令
Ctrl-C           # 清 in-memory 消息
Ctrl-D           # 退出（自动存历史）

# 历史文件
ls -la ~/.tau/history
```

---

## 7. 风险与回退

- **rustyline 版本漂移**：v14 的 `Pair` 无 `From<String>`、需 `Context::new(&history)`——已在 `repl.rs` 直接构造 `Pair { display, replacement }` 并传 `&DefaultHistory`，避免依赖未稳定 API。
- **补全在 Windows 路径**：`path_candidates` 用 `/` 切分，Windows `\` 未特别处理（v1 仅类 Unix，与原版一致）。
- **thinking 透传的 provider 兼容**：非 OpenAI/Anthropic 适配器（github-copilot/azure，Phase 8）尚未消费 `thinking_level`；当前 `StreamRequest` 字段存在但被忽略属安全默认（等价关闭）。
- **回退**：若 rustyline 在 CI 无 TTY 环境下行为异常，交互模式仍由 `printf | tau` 管道驱动验证（已用冒烟脚本确认）；纯 `--print` 路径不受影响。
