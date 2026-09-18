# tau-rs

`tau`（Python）的 Rust 重写版。目标：**wire 行为与 Python 完全等价**，同时用 Rust 的静态类型去掉 Python 运行期必需的操作（`isinstance` 分派、鸭子类型、运行期 literal 校验等）。

## 目录结构

```text
src/tau_agent/            生产代码：Python tau_agent/ 的逐模块重写
  messages.rs             消息与内容模型
  provider_events.rs      助手流事件
  tools.rs                工具定义与结果
  provider.rs             provider 契约（futures-core Stream）
  types.rs                JsonValue / JsonObject
  tests/                  仅测试（#[cfg(test)]，与生产代码物理分离）
    messages.rs / provider_events.rs / tools.rs / provider.rs
    differential.rs       Python ↔ Rust 双端差分对比
    support.rs            测试专用工具（block_on）
src/tau_ai/               tau_ai facade（重导出）；provider 适配器待写
src/main.rs               stub 入口
tests/fixtures/           差分语料（model_corpus.jsonl / model_expected.jsonl）
tools/                    语料源（model_corpus.py）与生成脚本（gen_fixtures.py / fixtures.sh）
AGENTS.md                 重写指南：约定、ADR、路线图、测试方法论（开工前先读）
```

## 常用命令

```bash
cargo test                     # 生产代码 + 双端差分测试
cargo clippy --all-targets
./tools/fixtures.sh            # 用 Python/pydantic 重新生成差分语料
./tools/fixtures.sh --check    # 校验提交的语料与 Python 行为一致
```

Python 权威源在 `../tau/`。差分测试会现场调用 `../tau/.venv/bin/python`（可用 `TAU_PYTHON` 覆盖；找不到解释器时跳过 live 对比，仅比对提交的 fixtures）。

## 状态

- 已完成：`types` / `messages` / `provider_events` / `tools` / `provider`（50 tests；212 条双端语料，27 条已登记分歧）。
- 下一步：`events` → `tool_history` → `harness` / `loop` → `session` → `tau_ai` 适配器。
- 约定、决策与坑位记录见 [`AGENTS.md`](AGENTS.md)。
