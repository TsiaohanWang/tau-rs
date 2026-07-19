# Phase 2 实施计划 — tau-ai：HTTP + 两个 provider

> 状态：✅ 已完成（2026-07-18）
> 目标：在 `tau-agent` 已固化的 `ModelProvider` trait 之上，实现 HTTP 传输层与两个真实 provider 适配器，使 Rust 端能真正对接 API（Anthropic messages API、OpenAI-compatible chat completions）。

## 1. 范围

### 1.1 包含
- `tau-ai` crate：`reqwest` 异步客户端（代理规整、超时、TLS）、**手写 SSE 行解析**（`sse.rs`，不依赖第三方 SSE 库，便于精确控制与单测）、retry/退避（`retry.rs`，含 429 限流专用退避 + `Retry-After` 支持）、`canonicalize_provider_stream`（`stream.rs`，将 vendor SSE 流规整为 `BoxStream<'_, AssistantMessageEvent>`）。
- `AnthropicProvider`（`anthropic.rs`）：messages API，thinking → `thinking` block（adaptive effort），tool use 格式。
- `OpenAICompatibleProvider`（`openai.rs`）：chat completions 先行，`reasoning_effort` 映射 thinking level；作为 opencode 等兼容端点的统一载体。

### 1.2 不包含
- 不做 OAuth（Phase 8）。
- 不做 provider 数量广度扩展（google/mistral/codex，Phase 8）。
- 不碰 `tau-coding` 的工具/会话逻辑（`tau-ai` 仅实现 trait，零业务依赖）。

## 2. 设计决策（ADR）

### ADR-P2-1 手写 SSE 解析，不引入 `eventsource`/`sse` 第三方 crate
**背景**：现成 SSE 库多绑定特定运行时或提供 push 模型；本项目需要 pull-based（`BoxStream`）以保留 generator 背压语义（见 architecture ADR-5）。
**决策**：`sse.rs` 暴露纯函数 `parse_sse_line(&str) -> Option<SseEvent>` 与带缓冲的 `SseAccumulator`，只产出 `serde_json::Value` 载荷，JSON 解释留给 provider。
**收益**：零依赖、可单测、对不可信网络输入安全（见 `docs/review-2026-07-19.md` P0-b 已加 proptest）。

### ADR-P2-2 provider 只实现 `ModelProvider` trait，HTTP 细节内聚
**背景**：`tau-agent` 拥有 trait 且零 HTTP 依赖（依赖倒置）。
**决策**：`tau-ai` 是 trait 的唯一 HTTP 实现者；provider 构造接收 `Arc<dyn ModelProvider>` 所需的 config（provider/model/system/tools），流内部用 reqwest POST + SSE 解析 + `canonicalize_provider_stream` 转换。
**代价**：每个 provider 需手写 payload 构造与响应解析。
**收益**：核心 brain 完全脱离网络；测试用 `FakeProvider`（Phase 1，feature `testing`）零网络即可驱动 loop。

### ADR-P2-3 retry / 退避集中处理
**背景**：SSE 中途断流、429 限流、5xx 均需重试；不同错误需不同策略（429 尊重 `Retry-After`）。
**决策**：`retry.rs` 提供带 `tokio::time::sleep` 的指数退避 + `Retry-After` 解析；默认 `max_retries = 5`（5.8 落地，opencode 默认模型改 `nemotron-3-ultra-free`）。
**收益**：provider 代码只关心"发起一次请求"，重试对上层透明。

## 3. 测试（已完成）
- `tau-ai` 单元测试：SSE 解析（基础 + proptest 性质测试，见 P0-b）、payload 构造（含 `reasoning_effort` 映射）、retry/backoff 逻辑。
- 集成测试：`wiremock` mock server 模拟 Anthropic / OpenAI SSE fixture → 事件流与 Python 抓取样本一致（各 ~6 测试）。
- 验证：`cargo test -p tau-ai --features tau-agent/testing` 全绿；真实免费模型端到端验证（5.7/5.8）。

## 4. 验收
- [x] `reqwest` 客户端 + 手写 SSE 解析器落地，无第三方 SSE 依赖。
- [x] `AnthropicProvider` / `OpenAICompatibleProvider` 实现 `ModelProvider::stream_response`。
- [x] `canonicalize_provider_stream` 将 vendor 流规整为 `BoxStream<'_, AssistantMessageEvent>`，pull-based（drop = cancel）。
- [x] retry/退避含 429 `Retry-After` 支持，默认 5 次。
- [x] `tau-ai` 单测 + wiremock 集成测试全绿；clippy / fmt 干净。

---

> 注：本仓库各 Phase 计划文档（phase-1 ~ phase-7）为撰写时快照。测试权威总数以 `cargo test --workspace` 实时结果为准（默认 200 / `--features tui` 205）；架构状态见 `docs/architecture.md` 文件头。
