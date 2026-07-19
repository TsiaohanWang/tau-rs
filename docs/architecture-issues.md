# Architecture Issues Report — tau-rs

> Date: 2026-07-19
> Scope: Full-project audit of tau-rs (Phases 1–4) against huggingface/tau (Python), focusing on architectural correctness, robustness, and parity with the rewrite target.

---

## Issue Index

| # | Priority | Title | Status |
|---|----------|-------|--------|
| 1 | 🔴 P0 | CatalogConfig 类型双份定义 — CLI 与 tau-coding 各自持有独立的 `CatalogConfig`/`ProviderKind` | ✅ Fixed |
| 2 | 🔴 P0 | SSE 伪流式 — `resp.text().await` 缓冲完整响应体再逐行解析，破坏取消语义 | ✅ Fixed |
| 3 | 🔴 P0 | 缺 CodingSession 组合根 — CLI 直接驱动 harness，无持久化树的 parent_id 链 | ✅ Fixed (5.1) |
| 4 | 🟠 P1 | JsonlSessionStorage 无文件锁 — 并发追加会交错行 | ✅ Fixed |
| 5 | 🟠 P1 | write.rs 非原子写 — 崩溃即文件损坏 | ✅ Fixed |
| 6 | 🟡 P2 | edit.rs 未用 `similar` — 依赖已引入但无调用；缺 LF/BOM 处理 | ✅ Fixed |
| 7 | 🟡 P2 | 无 context-window token 估算 — `context_windows` 字段存在但零读取 | ✅ Fixed |
| 8 | 🟡 P2 | 无 compaction — `apply_compaction` 存在但无生成代码 | ✅ Fixed |
| 9 | 🟡 P2 | main.rs 重复 provider 构造块 — print/REPL 两段近乎相同的 match | ✅ Fixed |
| 10 | 🟡 P2 | 硬编码 system prompt + 未组装 tool prompt 片段 | ✅ Fixed (5.1) |
| 11 | 🟡 P2 | REPL 忽略工具事件 — 用户看不到 agent 在做什么 | 🚧 Partial |
| 12 | 🟡 P2 | `cli_verbose()` 读 TAU_VERBOSE 环境变量反模式 | ✅ Fixed |
| 13 | 🟡 P2 | IO 风格混用 — `SessionManager` 同步 `std::fs` + `JsonlSessionStorage` async tokio + `block_on` 嵌套 | ✅ Fixed |
| 14 | 🟢 P3 | 死字段 `ToolExecutionMode` — 定义但从未读取 | ✅ Fixed |
| 15 | 🟢 P3 | 文档测试计数不一致（architecture.md 85 vs phase-4.md 104） | ✅ Fixed |

---

## Detailed Analysis

### Issue #1: CatalogConfig 类型双份定义

**Location**:
- `crates/tau-cli/src/config.rs:115-154` — `CatalogConfig`, `CatalogProvider` (no `api` field), `ProviderKind` (2-variant: `Anthropic | OpenaiCompatible`)
- `crates/tau-coding/src/config/catalog.rs:14-54` — `CatalogConfig`, `CatalogProvider` (has `api` field), `ProviderKind` (3-variant: `Anthropic | OpenaiCompatible | OpenaiResponses`)

**Problem**:
- The CLI's `CatalogConfig` is loaded independently via `CatalogConfig::load()` (no merge with builtin catalog), while tau-coding uses `load_user_or_default()` (merge with embedded builtin). These two will disagree on provider lists.
- The CLI's `ProviderKind::from_catalog` (`config.rs:229`) only matches `kind == "anthropic"`; it has no `api` field awareness, so providers with `api = "openai-responses"` are silently misclassified as `OpenaiCompatible`.
- The two `CatalogConfig` types are independently deserialized and will drift as fields are added.
- `resolve_api_key` in `config.rs:183` accepts the CLI's `CatalogConfig`, not tau-coding's.

**Criticism**: This is a textbook "parallel type hierarchy" smell. The CLI duplicated a type that already existed in `tau-coding` (and the Python original), leading to divergent `ProviderKind` semantics. The `api` field was added in tau-coding to support OpenAI Responses API, but the CLI copy was never updated. This means the binary literally cannot dispatch to a responses-style provider even if one appears in the catalog.

**Impact**: Any provider using `api = "openai-responses"` in catalog.toml will be silently routed to the chat-completions adapter, producing wrong API calls or 404s.

**Fix**: Delete the duplicate types from `config.rs`; have the CLI use `tau_coding::config::{CatalogConfig, CatalogProvider, ProviderKind}` with the `load_user_or_default` path. `resolve_api_key` and `cmd_config`/`cmd_providers` accept the tau-coding types.

---

### Issue #2: SSE 伪流式

**Location**:
- `crates/tau-ai/src/anthropic.rs:147` — `let text = resp.text().await.unwrap_or_default();`
- `crates/tau-ai/src/openai.rs:145` — `let text = resp.text().await.unwrap_or_default();`

**Problem**: Both providers buffer the *entire* HTTP response body into a `String` before iterating `text.lines()`. The `ModelProvider` trait returns a `BoxStream` that advertises pull-based, drop-to-cancel semantics. In practice, cancellation only kicks in at the retry/send boundary, not while consuming SSE lines mid-body. For long responses (multi-KB), this means:
1. Memory spike for large responses.
2. Cancellation latency proportional to body size.
3. The "stream" is actually "buffer-then-iterate", not a true byte-at-a-time pipeline.

**Criticism**: This defeats the primary architectural advantage Rust offers over Python's `async for` — true streaming without buffering. reqwest with the `"stream"` feature (already enabled in Cargo.toml) provides `bytes_stream()` which yields `Result<Bytes>` chunks. The `SseAccumulator` in `sse.rs` already supports incremental feeding via its `feed()` method but is currently unused by the providers.

**Impact**: In practice, for typical assistant responses (<100KB), the latency difference is negligible. But for very long tool-call outputs or multi-turn compaction summaries, the buffer-then-parse pattern adds latency and prevents mid-stream cancellation.

**Fix**: Replace `resp.text().await` with `resp.bytes_stream()` + incremental `SseAccumulator::feed()`. Handle `CancellationToken` between chunks via `tokio::select!`. The existing `parse_sse_line` / `SseAccumulator` infrastructure is already in place.

---

### Issue #3: 缺 CodingSession 组合根

**Location**:
- `crates/tau-cli/src/main.rs` — directly constructs harness, calls `persist_message`, drives stream
- `crates/tau-coding/src/session/coding_session.rs` — **new: skeleton exists but not wired into CLI**

**Problem**: The Python `CodingSession` (in `session.py`, 2662 lines) is the composition root that owns persistence, tools, system-prompt assembly, model switching, compaction, branching, and context-window management. In tau-rs, the CLI `main.rs` directly:
1. Constructs the harness via `build_harness()`.
2. Calls `persist_message()` with `parent_id: None` for every message — so the tree has no parent linkage.
3. Never generates a `LeafEntry` pointing at the last entry in the tree.
4. Has no compaction trigger, no context-window awareness, no slash-command dispatch.

**Criticism**: Without `parent_id` linkage, the session tree cannot be replayed from an arbitrary leaf. The `SessionState::from_entries` code in `tau-agent/src/session/state.rs` supports `LeafSelector::At(id)` but since all entries are roots, this always replays the entire journal linearly. Branch/resume (Phase 5) is structurally blocked.

**Impact**: Sessions written by tau-rs cannot be meaningfully replayed by the tree-walking code; branching creates orphaned entries; long sessions grow unbounded (no compaction trigger).

**Fix**: Introduce `tau_coding::session::CodingSession` (or `tau_coding::coding_session`) as a composition root that:
- Maintains a `last_entry_id` and passes it as `parent_id` to new messages.
- Generates `LeafEntry` with correct `entry_id` pointing at the last assistant message.
- Provides `prompt()` / `continue_()` / `compact()` methods.
- Owns context-window estimation and auto-compaction threshold.
- The CLI delegates to `CodingSession` instead of driving the harness directly.

**Partial fix (2026-07-19)**: `CodingSession` skeleton implemented in `session/coding_session.rs` with parent_id linkage, compaction trigger, and context-window check. However, `main.rs` still uses `build_harness()` + `persist_message()` directly — `CodingSession` is not yet wired into the CLI. Remaining: swap CLI to use `CodingSession`.

**Complete fix (2026-07-19, Phase 5.1)**: `main.rs` now constructs `CodingSession` via `open_or_create_session` and drives `session.prompt(text)`; the `build_harness`/`persist_message` helpers are deleted. `CodingSession::prompt` returns a wrapped `async-stream` stream that:
1. runs the pre-prompt compaction threshold check on the in-memory message list;
2. starts the harness (impossible-to-fail path → silent early-return);
3. persists every `MessageEnd` event — both the user-prompt echo and the assistant reply — via `persist_with_parent`, advancing `last_entry_id` so `parent_id` chains correctly.

Verified with live `--print` against `opencode/deepseek-v4-flash-free` (entries chain user → assistant → …) and two integration tests (`test_coding_session_e2e.rs`: `<user, None>`, `<assistant, Some(user_id)>`). Single-user / single-assistant file has 5 entries; multi-turn has 9, all with correct `parent_id`.

---

### Issue #4: JsonlSessionStorage 无文件锁

**Location**: `crates/tau-coding/src/session/storage.rs:81` — bare `OpenOptions::append`

**Problem**: Phase 4 design doc (`phase-4.md:82`) promised "并发安全：使用文件锁". The implementation uses plain append without any locking. If two processes (or threads) concurrently append to the same session file, lines can interleave at the OS level.

**Criticism**: The Python implementation uses `index.jsonl` for metadata and separate JSONL files for transcripts, with single-writer semantics. In Rust, tokio's cooperative scheduling means two tasks calling `append` concurrently can interleave their `write_all` calls. While the OS typically handles small appends atomically on local filesystems, this is not guaranteed for writes > `PIPE_BUF` (4096 bytes on Linux, but variable).

**Impact**: Corrupted session files under concurrent access. Low probability in single-user mode but real in multi-threaded harness scenarios.

**Fix**: Use `std::sync::Mutex` (or `fs2::FileExt::lock_exclusive`) wrapping the file handle. Since JSONL append is a simple sequential write, a `Mutex<()>` per-storage or per-file is sufficient. The simpler approach: keep an in-process `Mutex` (one per `JsonlSessionStorage` instance) and document that cross-process locking is not yet supported.

---

### Issue #5: write.rs 非原子写

**Location**: `crates/tau-coding/src/tools/write.rs:61` — `tokio::fs::write(&file_path, content)`

**Problem**: Phase 3 design doc (`phase-3.md:148`) promised "atomic write (write to tempfile, rename)". The implementation uses direct `tokio::fs::write`, which truncates and overwrites in place. If the process crashes mid-write (OOM kill, power loss, SIGKILL), the file is left in a partially-written state.

**Criticism**: This is a standard reliability concern for any tool that writes to user files. The Python `write` tool uses a lock (`_file_locks` dict in `tools.py:113`) but also does direct write. However, Rust's faster execution and the agent's ability to write large files makes the window larger.

**Impact**: File corruption on crash. The agent could overwrite a critical source file with partial content.

**Fix**: Write to a tempfile in the same directory (`tempfile::NamedTempFile`), then `rename` to the target path. `rename` is atomic on POSIX when source and target are on the same filesystem. Handle the cross-device case by falling back to `std::fs::copy` + `rename`.

---

### Issue #6: edit.rs 未用 similar + 缺 LF/BOM 处理

**Location**: `crates/tau-coding/src/tools/edit.rs:81` — `content.replace(old_text, new_text)`

**Problem**: The `similar` crate (version 2, in Cargo.toml) is never used. The edit tool does a plain `str::replace`. Python's edit tool (`tools.py:331`) has:
- LF-only normalization (no CRLF in old_text matching)
- BOM preservation (strips BOM before matching, re-adds after)
- Diff generation for result text
- Uniqueness/overlap validation beyond simple count

**Criticism**: The `similar` crate is specifically designed for text diffing and is listed as a dependency. Not using it means the edit tool can't generate a human-readable diff of what changed, which is valuable for verbose-mode output and session diagnostics. CRLF/BOM handling matters on Windows-mixed codebases.

**Impact**: On Windows-mixed repos, edits may fail or produce incorrect results due to CRLF mismatches. No diff output for user feedback.

**Fix**: (a) Normalize `\r\n` → `\n` in both `old_text` and file content before matching. (b) Detect and preserve BOM (strip, edit, re-add). (c) Use `similar::TextDiff::from_lines` to generate a unified diff and include it in the result text. (d) Use `similar::TextDiff::new()` to also support `replace_all` (multiple occurrences) as an optional mode.

---

### Issue #7: 无 context-window token 估算

**Location**: `crates/tau-coding/src/config/catalog.rs:53` — `context_windows: Option<HashMap<String, u64>>`

**Problem**: The catalog defines per-model context window sizes (in tokens), but the field is never read anywhere in the workspace. There is no token estimation, no truncation of the message list before sending to the provider, and no compaction trigger based on context usage.

**Criticism**: Python's `context_window.py` uses a `CHARS_PER_TOKEN = 4` heuristic to estimate token usage and triggers auto-compaction when usage exceeds `context_window - 16384` reserve. Without this, long sessions will silently hit the provider's context limit, causing either truncation (if the provider handles it) or hard errors.

**Impact**: Long-running REPL sessions will eventually fail when the provider rejects the payload for exceeding context length. The user sees an unhelpful HTTP error rather than a graceful compaction.

**Fix**: Implement `estimate_context_usage(messages, tools) -> ContextUsageEstimate` in `tau-coding` (or `tau-agent`). Use `chars / 4` heuristic (matching Python). Wire it into the harness or a pre-prompt hook. Auto-compact when estimate exceeds `context_window - RESERVE` (configurable, default 16384 tokens). Read `context_windows` from the merged catalog.

---

### Issue #8: 无 compaction

**Location**: `crates/tau-agent/src/session/state.rs:108` — `apply_compaction` (replay side exists); no creation-side code anywhere.

**Problem**: `SessionState::from_entries` correctly handles `CompactionEntry` during replay (replacing referenced entry IDs with a summary `UserMessage`). But no code ever *creates* a `CompactionEntry`. Python's `CodingSession` has three compaction triggers:
1. Manual: user-initiated `/compact` command
2. Threshold: auto-compact when `context_usage > auto_compact_token_threshold`
3. Overflow: catch context-overflow error, compact, retry exactly once

**Criticism**: This is the single biggest functional gap. Without compaction, sessions grow unbounded. The replay machinery for compaction is already implemented and tested — only the creation side is missing. The `context_window.py` prompts (`SUMMARIZATION_PROMPT`, `UPDATE_SUMMARIZATION_PROMPT`) need Rust equivalents.

**Impact**: Long sessions will either crash (context overflow) or silently lose early messages (if the provider truncates). Users cannot recover from context pressure.

**Fix**: Implement `Compactor` in `tau-coding`:
- `estimate_context_usage(messages, tools) -> ContextUsageEstimate`
- `plan_compaction(messages, target_tokens) -> CompactionPlan`
- `generate_compaction_summary(provider, plan) -> String` (calls the model)
- `apply_and_persist(session, plan, summary)` (appends `CompactionEntry`)
- Auto-trigger: before each `prompt()`, check threshold; on context overflow error, compact + retry once.

---

### Issue #9: main.rs 重复 provider 构造块

**Location**: `crates/tau-cli/src/main.rs:152-178` (print) and `185-211` (REPL)

**Problem**: The print and REPL branches contain near-identical ~25-line match blocks that construct `Arc<dyn ModelProvider>` from the same inputs. The only difference is the call site (`print_once` vs `run_repl`). This violates DRY and means any new provider kind or configuration change must be duplicated.

**Criticism**: This is a mechanical duplication that should never have shipped. The `kind` match on `config::ProviderKind` should be a single function: `build_provider(kind, api_key, base_url, model, max_tokens, max_retries, timeout) -> Arc<dyn ModelProvider + Send + Sync>`.

**Impact**: Maintenance burden. Risk of drift between the two blocks (e.g., one adding a field the other forgets).

**Fix**: Extract `build_provider(...)` helper function. Both `print_once` and `run_repl` call it.

---

### Issue #10: 硬编码 system prompt + 未组装 tool prompt 片段

**Location**: `crates/tau-cli/src/main.rs:125` — `let system = cli.system.unwrap_or_else(|| "You are a helpful assistant.".to_string());`

**Problem**: The system prompt is hardcoded to a generic placeholder. Each `AgentTool` has `prompt_snippet` and `prompt_guidelines` fields (filled by the four coding tools), but these are never assembled into the system prompt. Python's `system_prompt.py` builds a rich system prompt including:
- Tool descriptions and usage guidelines
- Skill instructions
- AGENTS.md project context
- Model-specific guidelines

**Criticism**: Without tool descriptions in the system prompt, the model has no guidance on *how* to use the tools, only their JSON schemas. The `prompt_snippet` and `prompt_guidelines` fields are populated but dead data.

**Impact**: The model may not use tools correctly (wrong argument format, missing context). The system prompt is unhelpfully generic.

**Fix**: Implement `build_system_prompt(tools, skills, context, model, user_system) -> String` in `tau-coding`. Assemble tool `prompt_snippet` + `prompt_guidelines` into a structured system prompt. Wire it into `build_harness` or `CodingSession`.

**Partial fix (2026-07-19)**: `build_system_prompt(tools, user_system)` implemented in `tau-coding/src/prompt.rs` with tool description/ guideline assembly. Called from `CodingSession::new()`, but `main.rs` still uses the hardcoded string. Remaining: swap CLI `build_harness` to use `build_system_prompt` or `CodingSession`.

**Complete fix (2026-07-19, Phase 5.1)**: The hardcoded `"You are a helpful assistant."` fallback in `main.rs` is dead code (deleted); `cli.system` passes through to `CodingSessionConfig` as `Option<String>`, and `CodingSession::new` calls `build_system_prompt(&tools, user_system.unwrap_or(""))`, which assembles tool snippets + guidelines on top of the user system (or the default placeholder `prompt::DEFAULT_SYSTEM_PROMPT`). Tool `prompt_snippet`/`prompt_guidelines` are no longer dead data.

---

### Issue #11: REPL 忽略工具事件

**Location**: `crates/tau-cli/src/main.rs` — event loop in `run_repl` and `print_once`

**Problem**: The REPL silently ignores `ToolExecutionStart`, `ToolExecutionUpdate`, `ToolExecutionEnd`, and `TurnStart`/`TurnEnd`. The user sees only text deltas and never sees which tools are called or their results. Python's `rendering/plain.py` prints tool calls and results to stderr in print mode and to the TUI in interactive mode.

**Criticism**: This makes the agent a black box. Even with `--verbose`, there's no tool visibility. The `AgentTool.render_call` and `render_result` fields exist specifically for this purpose but are never invoked.

**Impact**: Users cannot tell what the agent is doing during tool execution. Debugging is impossible without the verbose log.

**Fix**: In the REPL event loop, render `ToolExecutionStart` (show tool name + call ID), `ToolExecutionUpdate` (show partial progress), and `ToolExecutionEnd` (show result summary). Use `tool.render_call` / `tool.render_result` when available. Print to stderr so stdout stays clean for piped output.

**Partial fix (2026-07-19)**: Both `run_repl` and `print_once` now handle `ToolExecutionStart` and `ToolExecutionEnd` via `eprintln!`, showing tool name and truncated result on stderr. However, `render_call`/`render_result` from `AgentTool` are not yet used — the format is hardcoded. Remaining: use tool's custom renderers when available.

---

### Issue #12: cli_verbose() 反模式

**Location**: `crates/tau-cli/src/main.rs:520-521`

```rust
fn cli_verbose() -> bool {
    std::env::var("TAU_VERBOSE").is_ok()
}
```

**Problem**: Reads a global environment variable instead of taking the `cli.verbose` flag as a parameter. This is a workaround for not threading the CLI options through the call chain. The `TAU_VERBOSE` env var is undocumented and separate from the `-v` flag.

**Criticism**: This creates two independent verbosity controls that can disagree. The `-v` flag controls `tracing_subscriber` filter level, while `TAU_VERBOSE` controls session-path display in the REPL. A user passing `-v` would expect verbose output but wouldn't see the session path.

**Impact**: Confusing UX. Undocumented env var behavior.

**Fix**: Thread `verbose: bool` through `run_repl` and `print_once` as a parameter. Remove `cli_verbose()`.

---

### Issue #13: IO 风格混用

**Location**:
- `crates/tau-coding/src/session/manager.rs` — all methods use `std::fs` (sync)
- `crates/tau-coding/src/session/storage.rs` — all methods use `tokio::fs` (async)
- `crates/tau-cli/src/main.rs:319` — `rt.block_on(storage.append(&info))` inside an async fn

**Problem**: `SessionManager` is entirely synchronous (`std::fs::create_dir_all`, `std::fs::OpenOptions`), while `JsonlSessionStorage` is entirely async (`tokio::fs`). In `open_or_create_session` (line 295), which is an async fn, `rt.block_on(storage.append(&info))` is used to call the async storage from a synchronous context. This is a nested `block_on` anti-pattern — it works on tokio's current-thread runtime but can deadlock on a multi-threaded runtime if called from within an already-running task.

**Criticism**: The mixed sync/async boundary is a code smell. `SessionManager::create` calls `std::fs::create_dir_all` + `std::fs::OpenOptions` for index.jsonl, while `JsonlSessionStorage` uses tokio equivalents. This inconsistency means one path blocks the tokio executor thread.

**Impact**: Potential executor stalls under load. The `block_on` in an async fn is technically wrong (it blocks the executor thread for the duration of the I/O).

**Fix**: Make `SessionManager` async (`tokio::fs` throughout), or use `tokio::task::spawn_blocking` for the sync parts. Eliminate the `block_on` in `open_or_create_session`.

---

### Issue #14: 死字段 ToolExecutionMode

**Location**: `crates/tau-agent/src/tool.rs:59-66`

```rust
pub enum ToolExecutionMode {
    #[default]
    Parallel,
    Sequential,
}
```

**Problem**: Defined as "reserved for future use" (doc comment), with `Parallel` as default. The agent loop (`agent_loop.rs`) always runs tool calls sequentially regardless of this field. All four coding tools set it to `Default` (= `Parallel`) but the loop ignores it.

**Criticism**: A default of `Parallel` is misleading — it implies concurrent execution is the norm, but the loop doesn't support it. The field is set by every tool creator but never read.

**Impact**: Dead code. Misleading for contributors reading the tool implementations.

**Fix**: Remove `ToolExecutionMode` entirely until parallel execution is implemented. If keeping it for future use, change the default to `Sequential` (matching current behavior) and add a doc comment noting the loop doesn't yet honor it.

---

### Issue #15: 文档测试计数不一致

**Location**:
- `docs/architecture.md` — now 130 tests (fixed)
- `docs/phase-4.md` — now 149 total including Phase 4 extras (fixed)
- `README.md` — badge says 130 (fixed)

**Problem**: `architecture.md` was not updated when Phase 4 landed. The Phase 3 commit updated README but not architecture.md's test count.

**Impact**: Misleading for contributors evaluating project health.

**Fix**: Updated all docs to current 130 count.

---

## Cross-Cutting Observations

### Architectural Strengths (things to preserve)

1. **Clean 5-crate layering** (`tau-types` → `tau-agent` → `tau-ai` → `tau-coding` → `tau-cli`) with well-defined dependency directions. `tau-agent` owning the `ModelProvider` trait (not `tau-ai`) is a strong design choice.
2. **Pull-based `BoxStream`** contract in `ModelProvider` — drop = cancel. Even though the current implementation buffers, the trait design is correct.
3. **`Arc<AssistantMessage>` for event partials** (ADR-2) — O(1) clone for fan-out.
4. **`Arc<HarnessState>` for concurrent control** (ADR-3) — `steer()`/`follow_up()`/`cancel()` are all `&self`.
5. **Hand-written `Deserialize` for strict wire compat** (ADR-1) — correct trade-off vs serde's limited `deny_unknown_fields` on tagged enums.
6. **v1 migration in JSONL** — backward-compatible read of Python's session format.
7. **`SseAccumulator`** design — already supports incremental feeding, just unused.
8. **`SessionState::from_entries`** — pure replay from append-only tree, correctly handles compaction/branching/labels.

### Python strengths not yet ported (by design, but worth noting)

1. **Extension system** (`extensions/api.py`, `runtime.py`) — isolation boundaries, generation liveness, fail-safe hooks. Deliberately dropped in v1.
2. **`CodingSession` as composition root** — despite being a god-object, it correctly orchestrates persistence, tools, system-prompt, compaction, branching, and model switching.
3. **Context-window management** — token estimation, auto-compaction threshold, overflow-retry.
4. **Skills + AGENTS.md discovery** — project-aware system prompt assembly.
5. **OAuth device flow** — browser-based credential exchange for OpenAI Codex / Anthropic.
