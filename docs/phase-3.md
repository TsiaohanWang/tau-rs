# Phase 3 实施计划 — 内置工具 + tau-coding crate

> 状态：✅ 已完成（2026-07-19）
> 目标：创建 `tau-coding` crate，实现四个核心内置工具（read/write/edit/bash），集成到 CLI 通过 harness 运行。

## 1. 范围

### 1.1 包含
- 新建 `tau-coding` crate
- 四个内置工具：`read`、`write`、`edit`、`bash`
- `create_coding_tools()` 工厂函数
- CLI 集成：通过 `AgentHarness` 运行（替换直接调用 provider）

### 1.2 不包含（防 scope creep）
- 不做 `context_window` 估算
- 不做 `AGENTS.md` 发现链
- 不做 skills/templates/resources
- 不做 `system_prompt` builder（保持 CLI 硬编码）
- 不做 session 持久化（Phase 4）
- 不做 compaction（Phase 5）

---

## 2. Crate 结构

```
crates/tau-coding/
├── Cargo.toml
├── src/
│   ├── lib.rs
│   └── tools/
│       ├── mod.rs          # create_coding_tools() -> Vec<AgentTool>
│       ├── read.rs         # read 工具
│       ├── write.rs        # write 工具
│       ├── edit.rs         # edit 工具
│       └── bash.rs         # bash 工具
└── tests/
    └── test_tools.rs
```

### 2.1 依赖

```toml
[dependencies]
tau-types = { path = "../tau-types" }
tau-agent = { path = "../tau-agent" }
tokio = { workspace = true }
tempfile = "3"
similar = "2"
anyhow = "1"
serde_json = { workspace = true }
```

### 2.2 Workspace 成员

更新 `Cargo.toml` workspace members：

```toml
[workspace]
members = [
    "crates/tau-types",
    "crates/tau-agent",
    "crates/tau-ai",
    "crates/tau-coding",  # 新增
    "crates/tau-cli",
]
```

---

## 3. 工具设计

### 3.1 `tools/mod.rs` — 工厂函数

```rust
use std::path::Path;
use tau_agent::tool::AgentTool;

pub mod read;
pub mod write;
pub mod edit;
pub mod bash;

/// 创建编码工具集
pub fn create_coding_tools(cwd: &Path) -> Vec<AgentTool> {
    vec![
        read::create_tool(cwd),
        write::create_tool(cwd),
        edit::create_tool(cwd),
        bash::create_tool(cwd),
    ]
}
```

### 3.2 `tools/read.rs` — 读取文件

**功能**: 读取文件内容，支持行范围。

**JSON Schema**:
```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "文件路径（相对于 cwd）" },
    "offset": { "type": "integer", "description": "起始行号（0-indexed）", "default": 0 },
    "limit": { "type": "integer", "description": "读取行数", "default": 2000 }
  },
  "required": ["path"]
}
```

**实现要点**:
- 路径解析：相对于 `cwd`，支持 `~` 展开
- 行号：输出带行号（`1: line content`）
- 截断：默认 2000 行，超过时提示
- 错误处理：文件不存在、权限不足、二进制文件检测

**Prompt Snippet**:
```
Use this tool to read files. Returns file contents with line numbers.
```

**Prompt Guidelines**:
```
- Always specify path relative to the working directory
- Use offset/limit for large files to avoid reading everything
- Check the first few lines before reading the entire file
```

### 3.3 `tools/write.rs` — 写入文件

**功能**: 创建或覆盖文件内容。

**JSON Schema**:
```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "文件路径" },
    "content": { "type": "string", "description": "文件内容" }
  },
  "required": ["path", "content"]
}
```

**实现要点**:
- 自动创建目录（`mkdir -p`）
- 原子写入：先写临时文件，再 `rename`
- 保留文件权限（如存在）
- 错误处理：磁盘满、权限不足

**Prompt Snippet**:
```
Use this tool to create or overwrite files. Creates parent directories automatically.
```

### 3.4 `tools/edit.rs` — 编辑文件

**功能**: 查找并替换文件中的文本。

**JSON Schema**:
```json
{
  "type": "object",
  "properties": {
    "path": { "type": "string", "description": "文件路径" },
    "old_text": { "type": "string", "description": "要替换的文本" },
    "new_text": { "type": "string", "description": "替换后的文本" }
  },
  "required": ["path", "old_text", "new_text"]
}
```

**实现要点**:
- 验证 `old_text` 在文件中存在且唯一
- 如果 `old_text` 不存在：返回错误
- 如果 `old_text` 存在多处：返回错误（要求更精确匹配）
- 使用 `similar` crate 做 diff 验证
- 原子写入

**Prompt Snippet**:
```
Use this tool to make targeted edits to files. Replaces exact text matches.
```

**Prompt Guidelines**:
```
- old_text must be unique in the file (include enough context)
- old_text must match exactly (including whitespace and indentation)
- If the edit fails, the file is NOT modified
```

### 3.5 `tools/bash.rs` — 执行命令

**功能**: 执行 shell 命令。

**JSON Schema**:
```json
{
  "type": "object",
  "properties": {
    "command": { "type": "string", "description": "要执行的命令" },
    "timeout": { "type": "integer", "description": "超时秒数", "default": 120 }
  },
  "required": ["command"]
}
```

**实现要点**:
- 使用 `tokio::process::Command`
- 设置 `process_group(0)` 以便 `killpg` 语义
- 使用 `tokio::select!` 实现超时和取消
- 输出截断：默认最大 100KB
- 捕获 stdout 和 stderr
- 返回 exit_code

**返回格式**:
```json
{
  "command": "ls -la",
  "output": "total 32\ndrwxr-xr-x...",
  "exit_code": 0,
  "truncated": false
}
```

**Prompt Snippet**:
```
Use this tool to execute shell commands. Returns output and exit code.
```

**Prompt Guidelines**:
```
- Commands run in the working directory
- Long-running commands will be killed after timeout
- Use this for building, testing, running scripts, etc.
```

---

## 4. CLI 集成

### 4.1 当前问题

当前 `tau-cli/src/main.rs` 直接调用 provider：

```rust
let stream = provider.stream_response(system, &messages, &[]);
```

这绕过了 agent loop，没有工具执行、事件生命周期、session 管理。

### 4.2 重构方案

改为通过 `AgentHarness` 运行：

```rust
use tau_agent::harness::{AgentHarness, AgentHarnessConfig, QueueMode};
use tau_agent::tool::AgentTool;
use tau_coding::tools::create_coding_tools;

// 创建工具集
let tools = create_coding_tools(&cwd);

// 创建 harness
let harness = AgentHarness::new(AgentHarnessConfig {
    provider: Arc::new(provider),
    model,
    system,
    tools: tools.into(),
    max_turns: Some(20),
    queue_mode: QueueMode::OneAtATime,
    before_tool_call: None,
    after_tool_call: None,
});

// 运行
let stream = harness.prompt(&user_input)?;
futures::pin_mut!(stream);
while let Some(event) = stream.next().await {
    match event {
        AgentEvent::MessageUpdate { assistant_message_event, .. } => {
            // 处理流式输出
        }
        AgentEvent::ToolExecutionEnd { result, .. } => {
            // 工具执行完成
        }
        _ => {}
    }
}
```

### 4.3 事件处理

| 事件 | 处理 |
|------|------|
| `MessageUpdate(TextDelta)` | 打印到 stdout |
| `ToolExecutionStart` | 打印工具名（verbose 模式） |
| `ToolExecutionEnd` | 打印结果（verbose 模式） |
| `AgentEnd` | 结束 |

---

## 5. 测试计划

### 5.1 单元测试（per tool）

每个工具至少 3 个测试：
- 正常执行
- 参数错误
- 边界情况

| 工具 | 测试用例 |
|------|----------|
| read | 正常读取、文件不存在、行范围、二进制文件 |
| write | 正常写入、创建目录、覆盖文件 |
| edit | 正常替换、old_text 不存在、多处匹配 |
| bash | 正常执行、超时、非零退出码 |

### 5.2 集成测试

- `create_coding_tools()` 返回 4 个工具
- 工具名称正确（`read`、`write`、`edit`、`bash`）
- 工具 JSON schema 正确

### 5.3 CLI 测试

- `--print` 模式使用 harness
- 工具执行结果正确显示

---

## 6. 验收

- [x] `cargo build --workspace` 零警告
- [x] `cargo test --workspace --features tau-agent/testing` 全绿（撰写时 130 测试；截至 2026-07-19 全仓已 200+ 测试）
- [x] `tau-coding` 单元测试通过（43 测试）
- [x] CLI `--print` 模式通过 harness 集成
- [x] `cargo clippy --workspace --all-targets --features tau-agent/testing -- -D warnings` 通过
- [x] `cargo fmt --check` 通过

## 9. 实施完成记录（2026-07-19）

### 已完成文件

| 文件 | 内容 |
|------|------|
| `crates/tau-coding/Cargo.toml` | crate 配置（依赖 tau-types, tau-agent, tokio, tempfile, similar, dirs, libc） |
| `crates/tau-coding/src/lib.rs` | 模块导出（tools, session, config） |
| `crates/tau-coding/src/tools/mod.rs` | `create_coding_tools()` 工厂函数 |
| `crates/tau-coding/src/tools/read.rs` | ReadExecutor + create_tool（20 测试） |
| `crates/tau-coding/src/tools/write.rs` | WriteExecutor + create_tool |
| `crates/tau-coding/src/tools/edit.rs` | EditExecutor + create_tool |
| `crates/tau-coding/src/tools/bash.rs` | BashExecutor + create_tool（process_group + libc::killpg） |
| `crates/tau-coding/src/session/mod.rs` | Phase 4 占位 |
| `crates/tau-coding/src/config/mod.rs` | Phase 4 占位 |

### 关键实现决策

1. **workspace Cargo.toml**: tokio 添加 `fs`, `process`, `io-util` features 支持文件和进程操作
2. **路径解析**: 使用 `strip_prefix('~')` 避免 clippy `manual_strip` 警告，支持 `~`、绝对路径、相对路径
3. **bash 工具**: 使用 `tokio::process::Command::process_group(0)` + `libc::killpg` 实现进程组取消
4. **ModelProvider 集成**: 创建 `AnthropicModelProvider` / `OpenAIModelProvider` 包装器实现 `ModelProvider` trait（避免生命周期冲突）
5. **provider Clone**: 为 `AnthropicProvider` 和 `OpenAIProvider` 添加 `#[derive(Clone)]` 以支持 `Arc::new(provider.clone())`
6. **CLI 重构**: print 模式和 REPL 全部通过 `AgentHarness` 运行，处理 `AgentEvent::MessageUpdate` 和 `AgentEvent::AgentEnd`

### Provider 流改造

- 将 `stream_response` 返回类型从 `impl Stream + '_` 改为 `impl Stream + Send + 'static`（内部已 clone config/client，可安全返回 'static）
- 新增 `AnthropicModelProvider` / `OpenAIModelProvider` 包装器，内部调用 `canonicalize_provider_stream` 转换 `ProviderEvent` → `AssistantMessageEvent`

### 测试统计

| 测试套件 | 数量 |
|----------|------|
| tau-types 单元测试 | 4 |
| tau-agent 单元测试 | 10 |
| tau-agent 集成测试 | 11 |
| tau-ai 单元测试 | 19 |
| tau-ai 集成测试 | 10 |
| tau-cli 单元测试 | 4 |
| tau-cli 集成测试 | 10 |
| **tau-coding 单元测试** | **43** |
| **总计** | **130** |

---

## 7. 实施顺序

1. 创建 `tau-coding` crate 骨架
2. 实现 `tools/read.rs` + 测试
3. 实现 `tools/write.rs` + 测试
4. 实现 `tools/edit.rs` + 测试
5. 实现 `tools/bash.rs` + 测试
6. 实现 `tools/mod.rs` 工厂函数
7. 重构 `tau-cli` 集成 harness
8. 更新 workspace Cargo.toml
9. 全量测试
10. 文档更新
