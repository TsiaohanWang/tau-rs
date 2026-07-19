# tau-rs

> Rust rewrite of [huggingface/tau](https://github.com/huggingface/tau) — a streaming coding agent with wire-compatible `~/.tau/` session data.

[![Rust](https://img.shields.io/badge/rust-stable-orange)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-200%20passing-brightgreen)](#testing)

---

## Overview

**tau-rs** is a from-scratch Rust rewrite of HuggingFace's Tau Python coding agent. The goal is to produce an idiomatic Rust implementation that is **byte-for-byte wire-compatible** with the existing Python agent — meaning both implementations can read and write the same `~/.tau/` session files, credentials, and provider configurations interchangeably.

The project is structured as a Cargo workspace with five crates, each corresponding to a distinct architectural layer.

### Why Rust?

| Dimension | Python (original) | Rust (tau-rs) |
|---|---|---|
| Core loop | `async for` generator | `impl Stream` (pull-based, same semantics) |
| Wire models | pydantic `Field(discriminator=...)` | serde `#[serde(tag)]` (stricter, compile-time) |
| Data compat | `~/.tau/` JSONL | Reads the same files, byte-identical wire format |
| Concurrency | GIL + `threading` | True parallelism via tokio |
| Extension system | Dynamic Python plugins | Static trait boundary (v1); dynamic loading deferred (see §7.7) |
| TUI | Textual (Python) | Plain + 3 renderers ready; ratatui TUI available behind `--features tui` (Phase 7) |

---

## Architecture

```
tau-rs/
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── tau-types/             # Wire contract — pure serde data models
│   ├── tau-agent/             # Agent brain — provider trait, tool protocol, event loop, harness
│   ├── tau-ai/                # Provider adapters — Anthropic, OpenAI-compatible, SSE, retry
│   ├── tau-coding/            # Coding domain — built-in tools (read/write/edit/bash), session storage, catalog merge
│   └── tau-cli/               # CLI binary — print mode, REPL, config management
├── docs/
│   ├── architecture.md        # Full architecture design document (Chinese)
│   ├── phase-1.md             # Phase 1 implementation plan with ADRs
│   ├── phase-3.md             # Phase 3 implementation plan
│   ├── phase-4.md            # Phase 4 implementation plan
│   ├── phase-6.md            # Phase 6 implementation plan (rustyline REPL + thinking)
│   └── gap-analysis.md        # Gap analysis vs Python original
└── rust-toolchain.toml        # Rust stable + rustfmt + clippy
```

### Dependency Graph

```
tau-types  (no async, no HTTP — pure serde)
    ↑
tau-agent  (provider trait, tool trait, loop, harness, session)
    ↑
tau-ai     (Anthropic/OpenAI adapters, SSE, retry, HTTP)
    ↑                                  ↑
tau-coding (tools, session storage,    │ catalog merge)
    ↑
tau-cli    (binary: clap CLI, REPL, print mode)
```

Key design principle: **`tau-agent` owns the `ModelProvider` trait**, not `tau-ai`. This inverts the naive dependency direction and ensures the core brain crate has no HTTP dependencies. `tau-coding` builds on top of `tau-agent` + `tau-ai`, providing the coding-specific layer (tools, on-disk session storage, and catalog merging) consumed by `tau-cli`.

> For a deep architecture-level comparison against the original Python implementation (layering, stream semantics, the `CodingSession` breadth gap, provider convergence, and the two real-API bugs caught during validation), see [`docs/architecture.md` §7](docs/architecture.md).

---

## Crate Descriptions

### `tau-types` — Wire Contract

The foundational crate containing all serde data models that cross provider/agent/application boundaries. **Zero async dependencies.**

| Module | Contents |
|---|---|
| `message` | 7 message types (`UserMessage`, `AssistantMessage`, `ToolResultMessage`, etc.), 4 content block types (`TextContent`, `ThinkingContent`, `ImageContent`, `ToolCall`), `Usage`, `StopReason` |
| `event` | 10 `AgentEvent` variants (agent start/end, turn start/end, message start/update/end, tool execution start/update/end) |
| `provider_event` | 12 `AssistantMessageEvent` variants (text/thinking/tool_call start/delta/end, done, error) with `Arc<AssistantMessage>` partial snapshots |
| `session` | 9 `SessionEntry` variants for append-only session logs (messages, compaction, branching, labels, leaves) |
| `tool_result` | `AgentToolResult` — structured tool execution results |

**Wire compatibility**: All serde models use `#[serde(rename_all = "camelCase")]` aliases and hand-written `Deserialize` implementations to enforce `deny_unknown_fields` — matching Python's pydantic `extra="forbid"` behavior.

### `tau-agent` — Agent Brain

The portable agent layer containing the core abstractions and logic.

| Module | Contents |
|---|---|
| `provider` | `ModelProvider` trait — `stream_response(&StreamRequest) -> BoxStream<AssistantMessageEvent>`. Pull-based (drop = cancel). |
| `tool` | `AgentTool` struct (schema + async executor + hooks), `ToolExecutor` trait, `BeforeToolCall`/`AfterToolCall` hook traits |
| `agent_loop` | `run_agent_loop(LoopArgs) -> impl Stream<Item=AgentEvent>` — the core event loop: stream assistant → execute tools → emit events → repeat |
| `harness` | `AgentHarness` — stateful wrapper with `Arc<HarnessState>` sharing, message queues, steering/follow-up, cancellation, listener subscriptions |
| `session` | Session tree traversal, state replay from entries, JSONL serialization with v1 migration |
| `testing` | `FakeProvider` — deterministic test provider (feature-gated behind `testing`) |

**Key ADRs (Architecture Decision Records)**:
- **ADR-1**: Hand-written `Deserialize` for strict `deny_unknown_fields` on tagged enums
- **ADR-2**: `Arc<AssistantMessage>` for O(1) event cloning with O(n) snapshot semantics
- **ADR-3**: `Arc<HarnessState>` shared state for concurrent `steer()/follow_up()/cancel()` during streaming
- **ADR-5**: Pull-based `async-stream` (not push-based channels) to preserve Python generator backpressure semantics

### `tau-ai` — Provider Adapters

HTTP layer implementing the actual API communication.

| Module | Contents |
|---|---|
| `anthropic` | `AnthropicProvider` — Anthropic Messages API (`/v1/messages`), SSE streaming with `message_start`→`content_block_delta`→`message_stop` lifecycle |
| `openai` | `OpenAIProvider` — OpenAI Chat Completions API (`/chat/completions`), works with any OpenAI-compatible endpoint (OpenAI, Azure, vLLM, Ollama, NVIDIA NIM, OpenCode, etc.) |
| `sse` | Hand-written SSE line parser — extracts `data:` payloads from streaming HTTP responses |
| `stream` | `canonicalize_provider_stream()` — normalizes raw provider events into Pi-compatible `AssistantMessageEvent`s |
| `retry` | Exponential backoff with jitter — retries on 408/429/5xx, network errors, and SSE-wrapped errors |
| `http` | HTTP client builder with configurable timeout, proxy support |

**Provider wire protocols**:

- **Anthropic**: POST `/v1/messages`, SSE: `event: message_start` → `event: content_block_start` → `event: content_block_delta` → `event: content_block_stop` → `event: message_delta` → `event: message_stop`
- **OpenAI**: POST `/chat/completions`, SSE: `data: {"choices":[{"delta":{...}}]}` → `data: [DONE]`

### `tau-cli` — CLI Binary

The user-facing application entry point.

| Module | Contents |
|---|---|
| `main` | clap CLI with `--print`/`-p`, `--provider`/`-P`, `--model`/`-m`, `--system`/`-S`, `--max-tokens`/`-M`, `--verbose`/`-v`; `Providers` and `Config` subcommands |
| `config` | `TauHome` (with `TAU_HOME` env override), `ProvidersConfig`, `CredentialsConfig`, `CatalogConfig`, `resolve_api_key()`, `ProviderKind` |

### `tau-coding` — Coding Domain

The coding-specific layer that wires `tau-agent` + `tau-ai` into a usable coding agent: built-in file tools, on-disk session storage, and catalog merging.

| Module | Contents |
|---|---|
| `tools` | `create_coding_tools()` — built-in tools: `read` (read file, optional offset/limit), `write` (atomic write via tempfile+rename), `edit` (similar diff + LF/BOM normalization), `bash` (shell command w/ optional timeout). Each implements `tau_agent::tool::AgentTool` |
| `session/storage` | `JsonlSessionStorage` — atomic read/append over JSONL session files (tokio::sync::Mutex for concurrent safety, v1 migration on read) |
| `session/manager` | `SessionManager` — async per-project directory hashing, create/load/list, index.jsonl append |
| `session/coding_session` | `CodingSession` — composition root: owns persistence, harness, system-prompt assembly, context-window estimation, and compaction |
| `session/context_window` | `estimate_context_usage()` — chars/4 token heuristic, `needs_compaction()` threshold check |
| `session/compaction` | `plan_compaction()` + `create_compaction_entry()` — plan which messages to compact and create compaction entries |
| `config/catalog` | `CatalogConfig`/`CatalogProvider`/`ProviderKind`, `merge_catalogs()` (overlay-replaces-base on provider name), built-in catalog embedded via `include_str!` |
| `prompt` | `build_system_prompt()` — assembles tool descriptions and guidelines into the system prompt |

**Phase 3 scope**: built-in `read`/`write`/`edit`/`bash` tools (no context-window / AGENTS.md / skills in v1 — deferred).

**Phase 4 scope**: `JsonlSessionStorage` + `SessionManager` (session persistence) and `merge_catalogs` (catalog merge) integrated into the CLI — print and REPL modes now persist `SessionInfo` + `MessageEntry` + `LeafEntry` rows per turn.

**Architecture audit (15 fixes)**: CatalogConfig deduplication, SSE true streaming, CodingSession foundation, file locking, atomic writes, similar-based edit diffs, context-window estimation, compaction basics, tool event display, async SessionManager, system prompt assembler, and more. CodingSession is now a complete composition root (Phase 5.1–5.8); see `docs/architecture.md` §7 for the full comparison vs the original Python `CodingSession`.

---

## Quick Start

### Prerequisites

- Rust stable (1.85+)
- An API key for one of the supported providers

### Build

```bash
git clone git@github.com:TsiaohanWang/tau-rs.git
cd tau-rs
cargo build --workspace
```

### Set up credentials

Create `~/.tau/credentials.json`:
```json
{
  "opencode": "sk-your-api-key-here"
}
```

Or use environment variables (loaded from `.env` or shell):
```bash
export OPENCODE_API_KEY=sk-your-api-key-here
```

### Run

**Single-shot print mode** (non-interactive):
```bash
cargo run -p tau-cli -- --print -P opencode "Explain the difference between TCP and UDP"
```

**Interactive REPL**:
```bash
cargo run -p tau-cli -- -P opencode
```

**List available providers**:
```bash
cargo run -p tau-cli -- providers
```

**Show provider configuration**:
```bash
cargo run -p tau-cli -- config opencode
```

---

## Configuration

tau-rs reads configuration from `~/.tau/` (same location as the Python agent):

### `~/.tau/catalog.toml`

Provider catalog — defines available providers, their endpoints, and supported models:

```toml
schema_version = 1

[[providers]]
name = "opencode"
display_name = "OpenCode"
kind = "openai-compatible"
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_ZEN_API_KEY"
models = ["nemotron-3-ultra-free", "north-mini-code-free", "deepseek-v4-flash-free", "mimo-v2.5-free"]
default_model = "nemotron-3-ultra-free"

[[providers]]
name = "nvidia-nim"
display_name = "NVIDIA NIM"
kind = "openai-compatible"
base_url = "https://integrate.api.nvidia.com/v1"
api_key_env = "NVIDIA_NIM_API_KEY"
models = ["deepseek-ai/deepseek-v4-flash", "deepseek-ai/deepseek-r1"]
default_model = "deepseek-ai/deepseek-v4-flash"
```

### `~/.tau/providers.json`

Per-provider preferences (default model, retries, timeout):

```json
{
  "default_provider": "opencode",
  "provider_preferences": {
    "opencode": {
      "default_model": "nemotron-3-ultra-free",
      "max_retries": 5,
      "timeout_seconds": 60
    }
  }
}
```

### `~/.tau/credentials.json`

API keys (permissions: `0600`):

```json
{
  "opencode": "sk-your-api-key",
  "nvidia-nim": "nvapi-your-api-key"
}
```

### API Key Resolution Order

1. `--provider` CLI flag → look up provider in catalog
2. `~/.tau/credentials.json` → `credential_name` field
3. Environment variable → `api_key_env` field in catalog
4. `.env` file (auto-loaded by `dotenvy`)

---

## Supported Providers

| Provider | Kind | Default Model | Notes |
|---|---|---|---|
| **OpenCode** | `openai-compatible` | `nemotron-3-ultra-free` | Free tier only (4 models) |
| **NVIDIA NIM** | `openai-compatible` | `deepseek-ai/deepseek-v4-flash` | Free tier with rate limits |
| **DeepSeek** | `openai-compatible` | `deepseek-v4-flash` | Official DeepSeek API |
| **OpenAI** | `openai` | `gpt-4o` | Official OpenAI API |
| **Anthropic** | `anthropic` | `claude-sonnet-4` | Official Anthropic API |

Any provider implementing the OpenAI-compatible `/chat/completions` endpoint can be added to the catalog.

---

## CLI Reference

```
tau-rs [OPTIONS] [PROMPT]

OPTIONS:
  -p, --print              Print response and exit (non-interactive)
  -P, --provider <NAME>    Provider name (e.g., opencode, nvidia-nim)
  -m, --model <MODEL>      Model override
  -S, --system <SYSTEM>    System prompt
  -M, --max-tokens <N>     Maximum tokens for response
  -v, --verbose            Enable verbose logging
  -h, --help               Print help

SUBCOMMANDS:
  providers    List available providers from catalog
  config       Show resolved configuration for a provider
```

### Examples

```bash
# Basic query
tau -p "Write a Python function to check if a string is a palindrome"

# Use specific provider and model
tau -p -P nvidia-nim -m deepseek-ai/deepseek-v4-pro "Explain quantum computing"

# Custom system prompt
tau -p -S "You are a Rust expert" "Write a safe concurrent queue"

# Verbose logging (for debugging)
tau -v -p -P opencode "Hello"
```

---

## Development

### Project Structure

```
crates/
├── tau-types/                 # ~1,200 lines
│   ├── src/
│   │   ├── lib.rs
│   │   ├── message.rs         # Wire models: messages, content blocks, usage
│   │   ├── event.rs           # Agent events (10 variants)
│   │   ├── provider_event.rs  # Provider stream events (12 variants)
│   │   ├── session.rs         # Session entries (9 variants)
│   │   └── tool_result.rs     # Tool execution results
│   └── Cargo.toml
├── tau-agent/                 # ~1,800 lines
│   ├── src/
│   │   ├── lib.rs
│   │   ├── provider.rs        # ModelProvider trait
│   │   ├── tool.rs            # AgentTool, ToolExecutor, hooks
│   │   ├── agent_loop.rs      # run_agent_loop (core event loop)
│   │   ├── harness.rs         # AgentHarness (stateful wrapper)
│   │   ├── session/           # Session tree, state replay, JSONL
│   │   └── testing.rs         # FakeProvider (feature-gated)
│   ├── tests/
│   │   ├── test_agent_loop.rs
│   │   └── test_agent_harness.rs
│   └── Cargo.toml
├── tau-ai/                    # ~2,200 lines
│   ├── src/
│   │   ├── lib.rs
│   │   ├── anthropic.rs       # Anthropic Messages API adapter
│   │   ├── openai.rs          # OpenAI Chat Completions adapter
│   │   ├── sse.rs             # SSE line parser
│   │   ├── stream.rs          # Provider event canonicalizer
│   │   ├── retry.rs           # Exponential backoff with jitter
│   │   └── http.rs            # HTTP client builder
│   ├── tests/
│   │   ├── test_anthropic.rs  # 6 wiremock tests
│   │   └── test_openai.rs     # 6 wiremock tests
│   └── Cargo.toml
├── tau-coding/                # Phase 3+4: tools + session storage + catalog + coding session
│   ├── src/
│   │   ├── lib.rs
│   │   ├── tools/             # read / write / edit / bash + factory
│   │   ├── session/           # storage, manager, coding_session, context_window, compaction
│   │   ├── config/catalog.rs  # merge_catalogs + embedded built-in catalog
│   │   └── prompt.rs          # system prompt assembler
│   ├── data/
│   │   └── catalog.toml       # Built-in provider catalog (embedded via include_str!)
│   └── Cargo.toml
└── tau-cli/                   # ~900 lines
    ├── src/
    │   ├── main.rs            # CLI entry point, REPL, print mode, session persistence
    │   └── config.rs          # Configuration loading
    ├── tests/
    │   └── test_cli.rs        # 10 integration tests
    └── Cargo.toml
```

### Build & Test

#### Prerequisites

- Rust toolchain **1.85+** (uses edition 2024、`async fn in trait`)
- Optional for real API testing: an API key for a supported provider

```bash
# Check toolchain version
rustc --version   # must be ≥ 1.85
```

#### Quick Start (Free Tier)

```bash
# 1. Get a free OpenCode Zen API key from https://opencode.ai/docs
# 2. Set it as an environment variable (or use a .env file)
export OPENCODE_ZEN_API_KEY="sk-..."
# 3. Build and run
cargo run --release -p tau-cli -- -P opencode -p "Write a Rust fibonacci function"
```

The project includes a `.env` file for local development (not committed to git).  
`main.rs` calls `dotenvy::dotenv()` on startup to load `OPENCODE_ZEN_API_KEY` and other keys.

#### Build

```bash
# Debug build (fast compile, with debug info)
cargo build --workspace

# Release build (optimized)
cargo build --workspace --release

# ===== TUI (ratatui, feature-gated) =====

# Build with TUI
cargo build -p tau-cli --features tui --release

# Verify TUI binary
./target/release/tau --help | grep tui
# Should show: --tui  Use the interactive ratatui TUI

# Without tui feature, ratatui/crossterm are NOT compiled
cargo tree --workspace 2>/dev/null | grep -i ratatui
# (empty output = ratatui not pulled)
```

#### Run All Tests

```bash
# Default (193 tests — TUI module excluded without "tui" feature)
cargo test --workspace

# With TUI feature (198 tests — includes 5 TUI adapter tests)
cargo test --workspace --features tui

# ===== Run specific crate tests =====
cargo test -p tau-types
cargo test -p tau-agent --features testing     # FakeProvider requires "testing" feature
cargo test -p tau-ai                           # SSE + retry + wiremock integration
cargo test -p tau-coding
cargo test -p tau-cli
```

#### Linting & Formatting

```bash
# Format check
cargo fmt --check

# Format auto-fix
cargo fmt

# Clippy lint (default features)
cargo clippy --workspace --all-targets

# Clippy with TUI feature
cargo clippy --workspace --all-targets --features tui
```

> The project passes clippy with **zero warnings** in both configurations.
> No `-D warnings` gate is enforced in CI; manual discipline ensures cleanliness.

#### Run the Agent

##### 1. Print mode (one-shot, non-interactive)

```bash
# With free OpenCode Zen key
cargo run --release -p tau-cli -- -P opencode -p "Your prompt here"

# Using environment variable or .env
./target/release/tau -P opencode -p "List files in the current directory"

# With verbose logging
./target/release/tau -P opencode -v -p "Create a Rust hello world"

# Specify output format
./target/release/tau -P opencode --format json -p "..."
./target/release/tau -P opencode --format transcript -p "..."

# Resume a previous session
./target/release/tau -P opencode --resume latest -p "Continue..."
# or by session ID
./target/release/tau -P opencode --resume <session-id>
```

##### 2. REPL mode (interactive line editor)

```bash
# Start interactive REPL
./target/release/tau -P opencode

# In REPL:
#   /help              — list commands
#   /model <name>      — switch model (in-memory)
#   /provider <name>   — switch provider (in-memory)
#   /thinking [level]  — view/set thinking level (off/minimal/low/medium/high/xhigh)
#   /compact           — manually compact context
#   /clear             — clear in-memory messages
#   /exit              — quit
#   ! command          — run shell command
#   !!                 — repeat last shell command
#   Ctrl-C             — clear context
#   Ctrl-D             — exit
#   Tab                — auto-complete (slash commands / tool names / file paths)
#   Enter during run   — steer (send follow-up to running agent)
```

##### 3. TUI mode (ratatui, requires `--features tui`)

```bash
# Build with TUI
cargo build -p tau-cli --features tui --release

# Run TUI (must be in a REAL terminal: gnome-terminal / wezterm / tmux / etc.)
./target/release/tau -P opencode --tui

# TUI Key Bindings:
# =================  Idle  =================
#   Enter              send prompt
#   Esc / Ctrl-C       (no-op when idle)
#   Ctrl-D             quit
#   Ctrl-O             toggle tool results expanded/collapsed
#   Ctrl-T             toggle thinking blocks visible/hidden
#   Ctrl-L             scroll to latest (auto-scroll re-enable)
#   PageUp / ↑         scroll up
#   PageDown / ↓       scroll down
#   Backspace / ← / →  edit input line
#
# ==========  During Streaming  ============
#   Enter              steer (send typed text as follow-up)
#   Esc / Ctrl-C       cancel current stream
#   Ctrl-O             toggle tool results
#   Ctrl-T             toggle thinking
#   Ctrl-L             scroll to latest
#   PageUp / ↑         scroll up (auto-scroll disables)
#   PageDown / ↓       scroll down
#   (text input)       edit steer message; Enter to send
```

##### 4. List available providers

```bash
./target/release/tau providers
# Shows all providers from the built-in catalog (opencode, anthropic, openai,
# deepseek, nvidia, xai, xiaomi, github-copilot, ...)
```

---

### TUI 使用指南

TUI 模式是 Phase 7 的核心成果，基于 **ratatui + crossterm** 重写了原版 Python Textual 前端（6070 行 `app.py`），对齐原版 `tui/adapter.py`/`state.py` 分层。

#### 特性

| 功能 | 说明 | 状态 |
|------|------|------|
| Transcript 面板 | 滚动显示 User/Assistant/Tool/Thinking/Error 消息，role 配色区分 | ✅ |
| Input 输入条 | 行编辑（光标移动、退格）、发送/steer 一体化 | ✅ |
| Status 状态栏 | 运行状态（● idle / ● running）、模型名、thinking 级别、队列数 | ✅ |
| 工具结果折叠/展开 | `Ctrl-O` 切换；默认折叠显示摘要，展开预览截断 2000 字符 | ✅ |
| Thinking 块显示/隐藏 | `Ctrl-T` 切换；默认隐藏，打开后 ThinkingContent 以灰色斜体显示 | ✅ |
| 自动滚动 | 新消息到达时自动跳到底部；PageUp/↑ 浏览历史时暂停自动滚动 | ✅ |
| 流式输出 | 实时显示 assistant 文字增量（TextDelta）+ thinking 增量（ThinkingDelta） | ✅ |
| 取消 / Steer | `Esc` 取消当前流，`Enter` 在运行中发送 steer 消息 | ✅ |
| Resume 恢复 | `--resume latest` 从 `~/.tau/sessions/` 重新加载历史消息到 transcript | ✅ |
| 斜杠命令 | `/help` `/model` `/provider` `/thinking` `/clear` `/compact` `/exit` | ✅ |
| Shell 转义 | `! command` 执行 bash 命令，输出以 System 角色显示 | ✅ |

#### 架构约束

TUI crate（`tau-cli/src/tui/`）**仅依赖 `tau-types` 事件 + `CodingSession` 只读接口**，绝不反向依赖 `tau-agent`/`tau-ai` 的 HTTP 实现。steer/cancel 通过克隆 `AgentHarness` 句柄实现，避免 `&mut session` 与 live stream 的借用冲突。

> 完整架构说明见 `docs/architecture.md` §4 Phase 7。

#### 编译与运行

```bash
# ===== 在 Cargo.toml 中启用 tui feature =====
# crates/tau-cli/Cargo.toml 已经配置：
#   [features]
#   tui = ["dep:ratatui", "dep:crossterm"]
# 无需手动修改

# ===== 编译（默认不含 tui）=====
cargo build --workspace --release
# 二进制不包含 ratatui，体积更小

# ===== 编译（含 tui）=====
cargo build -p tau-cli --features tui --release

# ===== 运行 =====
# 必须在真实终端中运行（不支持 pipe / redirect / IDE 内置终端可能不兼容）
./target/release/tau -P opencode --tui

# ===== 常用组合 =====
# 指定模型
./target/release/tau -P opencode -m deepseek-v4-flash-free --tui
# Resume 上一个 session
./target/release/tau -P opencode --resume latest --tui
# Verbose 模式（显示 session 路径）
./target/release/tau -P opencode -v --tui
```

#### 界面布局

```
┌─────────────────────────────────────────────┐
│  Transcript                                 │ ⬆ 消息面板（自动滚动）
│                                             │
│  You                                        │
│  写一个斐波那契函数                           │
│                                             │
│  Assistant                                  │
│  以下是 Rust 实现：                           │
│  fn fib(n: u64) -> u64 { ... }             │  ← PageUp/↓ 浏览
│    [stop]                                   │
│                                             │
│  Tool → write src/main.rs                   │  ← Ctrl-O 展开详情
│    (result hidden; Ctrl-O to toggle)        │
│                                             │
│  Tool ✓ bash                                │
│    $ cargo run                              │
│    output: 55                               │
│                                             │
├─────────────────────────────────────────────┤
│  Input                             │  › 你好的 │ 输入条
├─────────────────────────────────────────────┤
│  ● running | model: nemotron-3-ultra-free  │ 状态栏
│  | think: off | queued: 0                   │
└─────────────────────────────────────────────┘
```

#### 滚动操作

- **自动滚动**：新消息到达时自动跳到底部（默认行为）
- **手动向上滚动**：按 `PageUp` 或 `↑` 浏览历史消息，自动滚动暂停
- **手动向下滚动**：按 `PageDown` 或 `↓` 向下翻
- **恢复自动滚动**：按 `Ctrl-L` 跳到底部并恢复自动滚动

> 提示：在长对话中，如果想查看之前的输出，按 `PageUp` 向上滚动；完成后按 `Ctrl-L` 回到最新消息。

#### Cargo.toml 配置参考

```toml
# crates/tau-cli/Cargo.toml
[dependencies]
ratatui = { version = "0.29", optional = true }
crossterm = { version = "0.28", optional = true }

[features]
default = []
tui = ["dep:ratatui", "dep:crossterm"]
```

#### 已知限制

| 限制 | 说明 | 计划 |
|------|------|------|
| 无 autocomplete | TUI 输入条暂不支持 Tab 补全（REPL 有） | Phase 8 |
| 无 file tree panel | 原版有 sidebar 显示项目文件树 | Phase 8 |
| 无 session tree picker | resume 需通过 `--resume <id>` CLI 传入 | Phase 8 |
| TUI 需要真实 TTY | `crossterm::enable_raw_mode()` 需要终端 ioctl | 无法绕过 |

#### API Key Configuration

```bash
# Option A: environment variable (highest priority)
export OPENCODE_ZEN_API_KEY="sk-..."

# Option B: .env file in project root (loaded by dotenvy on startup)
echo 'OPENCODE_ZEN_API_KEY=sk-...' > .env

# Option C: credentials file at ~/.tau/credentials.json
mkdir -p ~/.tau
echo '{"opencode": "sk-..."}' > ~/.tau/credentials.json

# Option D: for other providers
export ANTHROPIC_API_KEY="sk-ant-..."
export OPENAI_API_KEY="sk-..."
export DEEPSEEK_API_KEY="sk-..."
```

#### Troubleshooting

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| `Error: No such device` on `--tui` | Not running in a real terminal | Use gnome-terminal / wezterm / tmux / Windows Terminal |
| `HTTP 401/403` | API key not set or invalid | Check `OPENCODE_ZEN_API_KEY` env var or `.env` file |
| `HTTP 429 Too Many Requests` | Rate limited on free tier | The client has built-in retry (5 attempts, `Retry-After` respect) |
| `SSE-wrapped error, retrying` | Transient provider error | Auto-retry with exponential backoff; if persists, try a different provider |
| TUI shows garbled characters | Terminal doesn't support truecolor | WezTerm, iTerm2, Windows Terminal, or gnome-terminal recommended |

### Testing Strategy

The test suite includes **200 tests** (default) / **205** with `--features tui` across unit, integration, and wiremock levels:

| Crate | Unit Tests | Integration Tests | Total |
|---|---|---|---|
| `tau-types` | 4 | — | 4 |
| `tau-agent` | 10 | 11 (loop + harness) | 21 |
| `tau-ai` | 26 (incl. retry/backoff + reasoning_effort + SSE proptest) | 10 (wiremock HTTP mocks) | 36 |
| `tau-coding` | 100 (tools + session + catalog + context_window + compaction + compaction_prompts + naming + commands + shell_escape + prompt + repair + render) | 10 (coding session e2e + compat) | 110 |
| `tau-cli` | 11 (render module + subprocess CLI tests + REPL completion + TUI) | 9 (subprocess CLI tests) | 20 |
| **Total** | **151** | **41** | **192** |

> 测试总数以 `cargo test --workspace` 实时结果为准（默认 **200** / `--features tui` **205**，含 `tau-types` 新增 7 个 hand-written `Deserialize` proptest 性质测试）；上表为分类快照。

**Integration test patterns**:
- `tau-ai` tests use [wiremock](https://github.com/LukeMathWalker/wiremock-rs) to mock HTTP responses and verify SSE parsing + retry behavior
- `tau-cli` tests use [assert_cmd](https://github.com/assert-rs/assert_cmd) to run the binary as a subprocess and verify output
- `tau-agent` tests use `FakeProvider` to drive the event loop deterministically

### Key Design Decisions

See `docs/phase-1.md` for detailed ADRs. Summary:

| ADR | Decision | Rationale |
|---|---|---|
| ADR-1 | Hand-written `Deserialize` for tagged enums | serde's `internally-tagged` doesn't support `deny_unknown_fields`; hand-written ensures wire compatibility |
| ADR-2 | `Arc<AssistantMessage>` for event partials | O(1) clone for fan-out; O(1) snapshot for events; wire output unchanged |
| ADR-3 | `Arc<HarnessState>` shared state | Enables concurrent `steer()/follow_up()/cancel()` during streaming without `&mut self` conflicts |
| ADR-5 | Pull-based `async-stream` | Preserves Python generator backpressure semantics; drop = cancel |
| ADR-7 | `AgentTool.name: Arc<str>` | Enables dynamic tool names from runtime data; `Clone` cheap |

---

## Data Compatibility

tau-rs is designed to be **fully compatible** with existing `~/.tau/` data from the Python agent:

| Artifact | Format | Status |
|---|---|---|
| `~/.tau/catalog.toml` | TOML | ✅ Read/write compatible |
| `~/.tau/providers.json` | JSON | ✅ Read/write compatible |
| `~/.tau/credentials.json` | JSON | ✅ Read/write compatible |
| `~/.tau/sessions/*.jsonl` | JSONL (append-only) | ✅ Read compatible (v1 migration included) |
| `~/.tau/sessions/index.jsonl` | JSONL | ✅ Read compatible |

**Wire format alignment**:
- camelCase field aliases (`toolCallId`, `isError`, `stopReason`, etc.)
- `role`/`type` discriminated unions for message/content/event types
- `skip_serializing_if = "Option::is_none"` for optional fields
- `preserve_order` feature for deterministic JSON key ordering
- Timestamps default to current time (matching Python's `default_factory`)

---

## Real-World Validation

tau-rs has been exercised end-to-end against the **OpenCode free tier** (`-P opencode`, real `OPENCODE_ZEN_API_KEY`) with multi-turn coding tasks that drive the real file-system tools and `cargo`:

- **Thread-safe LRU cache** (`nemotron-3-ultra-free`): scaffolded a Rust crate, ran `cargo test`, hit a compile error, **auto-fixed it via the `edit` tool**, re-ran, and finished **10/10 tests passing** (~72s).
- **Resume across an incomplete edit** (`north-mini-code-free` left a type-annotation error): `--resume latest` loaded the half-finished session and completed it to **18/18 tests passing** (~39s) — validates session persistence + continuation.
- **`--format json`** emits the full agent event stream; the final `message_end` / `turn_end` now correctly carry the resolved `model` (previously `"unknown"` — fixed in 5.7, see `docs/architecture-issues.md` #17).

The OpenCode free models rotate (`deepseek-v4-flash-free`, `mimo-v2.5-free`, `nemotron-3-ultra-free`, `north-mini-code-free`); some are rate-limited on cold start. tau-rs applies a dedicated **429 backoff** (base 2s, honors the server `Retry-After` header, capped at 60s) and retries up to 5 times by default — so transient rate limits are absorbed automatically. A hard account-level limit (the API may return `Retry-After` of many hours) is reported as a graceful failure rather than an infinite wait.

---

## Roadmap

| Phase | Status | Description |
|---|---|---|
| Phase 0 | ✅ Done | Workspace skeleton, toolchain, CI |
| Phase 1 | ✅ Done | `tau-types` + `tau-agent` core (wire models, events, session replay, loop, harness, FakeProvider) |
| Phase 2 | ✅ Done | `tau-ai` (Anthropic + OpenAI providers, SSE, retry, HTTP) |
| Phase 3 | ✅ Done | Built-in tools (read/write/edit/bash) + `tau-cli` harness integration (print mode, REPL, config) |
| Phase 4 | ✅ Done | Session persistence (`JsonlSessionStorage` + `SessionManager`) and catalog merge (`merge_catalogs` + embedded built-in catalog) integrated into CLI |
| Phase 5 | ✅ Done | `CodingSession` 组合根、load/resume、compaction（三触发+LLM 摘要）、自动命名/斜杠命令/`!` shell escape、三渲染器（plain/json/transcript）、双向兼容 golden 验证、真实 API 端到端验证（5.1–5.7）、429 限流专用退避（5.8） |
| Phase 6 | ✅ Done | rustyline REPL（持久化历史、斜杠命令/工具名/路径补全、/thinking 切换、Ctrl-C 清上下文）；thinking_level 穿过 StreamRequest 由 provider 翻译为 reasoning_effort / Anthropic adaptive effort |
| Phase 7 | ✅ Done | ratatui TUI（`feature = "tui"`，默认不编译）：纯 `TuiEventAdapter.apply(&AgentEvent)` 对齐原版 `tui/adapter.py` 分层；`TuiState` 数据模型对齐 `tui/state.py`；crossterm 终端 + `tokio::select!` 事件循环；仅依赖 `tau-types` 事件 + `CodingSession` 只读接口，绝不反向依赖 harness HTTP。运行 `cargo run --features tui -- --tui` |
| Phase 8 | 🔲 Planned | OAuth 流、openai-codex/google/mistral 适配器、skills/context 文件、session 导出、扩展再评估 |

---

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `TAU_HOME` | Override `~/.tau/` directory | `~/.tau` |
| `OPENCODE_API_KEY` | OpenCode API key | — |
| `NVIDIA_NIM_API_KEY` | NVIDIA NIM API key | — |
| `DEEPSEEK_API_KEY` | DeepSeek API key | — |
| `OPENAI_API_KEY` | OpenAI API key | — |
| `ANTHROPIC_API_KEY` | Anthropic API key | — |

Environment variables can be placed in a `.env` file in the project root (auto-loaded by `dotenvy`).

---

## License

MIT — see [LICENSE](LICENSE).

---

## Acknowledgments

- [huggingface/tau](https://github.com/huggingface/tau) — the original Python implementation
- [OpenCode](https://opencode.ai) — OpenCode provider for free model access
