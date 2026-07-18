# tau-rs

> Rust rewrite of [huggingface/tau](https://github.com/huggingface/tau) — a streaming coding agent with wire-compatible `~/.tau/` session data.

[![Rust](https://img.shields.io/badge/rust-stable-orange)](https://www.rust-lang.org/)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![Tests](https://img.shields.io/badge/tests-68%20passing-brightgreen)](#testing)

---

## Overview

**tau-rs** is a from-scratch Rust rewrite of HuggingFace's Tau Python coding agent. The goal is to produce an idiomatic Rust implementation that is **byte-for-byte wire-compatible** with the existing Python agent — meaning both implementations can read and write the same `~/.tau/` session files, credentials, and provider configurations interchangeably.

The project is structured as a Cargo workspace with four crates, each corresponding to a distinct architectural layer.

### Why Rust?

| Dimension | Python (original) | Rust (tau-rs) |
|---|---|---|
| Core loop | `async for` generator | `impl Stream` (pull-based, same semantics) |
| Wire models | pydantic `Field(discriminator=...)` | serde `#[serde(tag)]` (stricter, compile-time) |
| Data compat | `~/.tau/` JSONL | Reads the same files, byte-identical wire format |
| Concurrency | GIL + `threading` | True parallelism via tokio |
| Extension system | Dynamic Python plugins | Static trait boundary (v1); dynamic loading deferred |
| TUI | Textual (Python) | ratatui (planned) |

---

## Architecture

```
tau-rs/
├── Cargo.toml                 # Workspace root
├── crates/
│   ├── tau-types/             # Wire contract — pure serde data models
│   ├── tau-agent/             # Agent brain — provider trait, tool protocol, event loop, harness
│   ├── tau-ai/                # Provider adapters — Anthropic, OpenAI-compatible, SSE, retry
│   └── tau-cli/               # CLI binary — print mode, REPL, config management
├── docs/
│   ├── architecture.md        # Full architecture design document (Chinese)
│   ├── phase-1.md             # Phase 1 implementation plan with ADRs
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
    ↑
tau-cli    (binary: clap CLI, REPL, print mode)
```

Key design principle: **`tau-agent` owns the `ModelProvider` trait**, not `tau-ai`. This inverts the naive dependency direction and ensures the core brain crate has no HTTP dependencies.

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
| `openai` | `OpenAIProvider` — OpenAI Chat Completions API (`/chat/completions`), works with any OpenAI-compatible endpoint (OpenAI, Azure, vLLM, Ollama, NVIDIA NIM, OpenCode Zen, etc.) |
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
  "opencode-zen": "sk-your-api-key-here"
}
```

Or use environment variables (loaded from `.env` or shell):
```bash
export OPENCODE_ZEN_API_KEY=sk-your-api-key-here
```

### Run

**Single-shot print mode** (non-interactive):
```bash
cargo run -p tau-cli -- --print -P opencode-zen "Explain the difference between TCP and UDP"
```

**Interactive REPL**:
```bash
cargo run -p tau-cli -- -P opencode-zen
```

**List available providers**:
```bash
cargo run -p tau-cli -- providers
```

**Show provider configuration**:
```bash
cargo run -p tau-cli -- config opencode-zen
```

---

## Configuration

tau-rs reads configuration from `~/.tau/` (same location as the Python agent):

### `~/.tau/catalog.toml`

Provider catalog — defines available providers, their endpoints, and supported models:

```toml
schema_version = 1

[[providers]]
name = "opencode-zen"
display_name = "OpenCode Zen"
kind = "openai-compatible"
base_url = "https://opencode.ai/zen/v1"
api_key_env = "OPENCODE_ZEN_API_KEY"
models = ["big-pickle", "deepseek-v4-flash-free", "glm-4.7-free"]
default_model = "big-pickle"

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
  "default_provider": "opencode-zen",
  "provider_preferences": {
    "opencode-zen": {
      "default_model": "big-pickle",
      "max_retries": 3,
      "timeout_seconds": 60
    }
  }
}
```

### `~/.tau/credentials.json`

API keys (permissions: `0600`):

```json
{
  "opencode-zen": "sk-your-api-key",
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
| **OpenCode Zen** | `openai-compatible` | `big-pickle` | Free tier available; also supports `deepseek-v4-flash-free`, `glm-4.7-free`, `minimax-m2.1-free` |
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
  -P, --provider <NAME>    Provider name (e.g., opencode-zen, nvidia-nim)
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
tau -v -p -P opencode-zen "Hello"
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
└── tau-cli/                   # ~800 lines
    ├── src/
    │   ├── main.rs            # CLI entry point, REPL, print mode
    │   └── config.rs          # Configuration loading
    ├── tests/
    │   └── test_cli.rs        # 10 integration tests
    └── Cargo.toml
```

### Build & Test

```bash
# Build all crates
cargo build --workspace

# Run all tests (including integration tests that need the "testing" feature)
cargo test --workspace --features tau-agent/testing

# Run specific crate tests
cargo test -p tau-types
cargo test -p tau-agent --features testing
cargo test -p tau-ai
cargo test -p tau-cli

# Clippy lint
cargo clippy --workspace --features tau-agent/testing

# Format check
cargo fmt --check
```

### Testing Strategy

The test suite includes **68 tests** across unit, integration, and wiremock levels:

| Crate | Unit Tests | Integration Tests | Total |
|---|---|---|---|
| `tau-types` | 10 | — | 10 |
| `tau-agent` | 5 | 11 (loop + harness) | 16 |
| `tau-ai` | — | 12 (wiremock HTTP mocks) | 12 |
| `tau-cli` | — | 10 (subprocess CLI tests) | 10 |
| **Total** | **15** | **33** | **68** |

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

## Roadmap

| Phase | Status | Description |
|---|---|---|
| Phase 0 | ✅ Done | Workspace skeleton, toolchain, CI |
| Phase 1 | ✅ Done | `tau-types` + `tau-agent` core (wire models, events, session replay, loop, harness, FakeProvider) |
| Phase 2 | ✅ Done | `tau-ai` (Anthropic + OpenAI providers, SSE, retry, HTTP) |
| Phase 3 | ✅ Done | `tau-cli` (CLI binary, print mode, REPL, config) |
| Phase 4 | 🔲 Planned | Built-in tools (read/write/edit/bash), context window, AGENTS.md discovery |
| Phase 5 | 🔲 Planned | `CodingSession` composition root, compaction, commands |
| Phase 6 | 🔲 Planned | Advanced REPL (rustyline, history, autocomplete) |
| Phase 7 | 🔲 Planned | ratatui TUI |
| Phase 8 | 🔲 Planned | OAuth, additional providers, session export |

---

## Environment Variables

| Variable | Description | Default |
|---|---|---|
| `TAU_HOME` | Override `~/.tau/` directory | `~/.tau` |
| `OPENCODE_ZEN_API_KEY` | OpenCode Zen API key | — |
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
- [OpenCode](https://opencode.ai) — OpenCode Zen provider for free model access
