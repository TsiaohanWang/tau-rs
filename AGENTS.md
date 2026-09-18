# tau-rs 重写指南（Python → Rust）

> 本文档是 tau-rs 的**权威重写约定与决策记录**，面向所有参与重写的工程师与 AGENT。
> 开工任何模块前请完整读一遍；模块完成后回来更新第 7 节状态表，新取舍补进第 6 节 ADR。
>
> - Python 权威源：`../tau/src/`（与 `tau-rs/` 同级目录），解释器 `../tau/.venv/bin/python`。
> - 本文语言约定：正文中文，标识符、命令、代码保持英文原文。
> - 冲突处理：Python 源码 > 本文 ADR > 本文其他描述。任何"顺手修正"都要先升级为 ADR。
> - 事实性断言于 2026-09-18 对照固定版本（pydantic 2.13.4 / serde 1.0.229 / serde_json 1.0.151）逐条验证，证据见 §10.4；升级依赖后必须重跑探针。

---

## 1. 重写初衷与总原则

1. **行为等价优先（wire contract）**：Rust 版的 JSON 序列化输出与可接受的反序列化输入，必须与 Python 版一致。Python 是唯一权威，不是"参考"。
2. **静态类型收益最大化**：能用类型表达的不留运行时检查。
   - `isinstance` 链 → 穷尽 `match`
   - `Literal[...]` → enum
   - 可空 → `Option`
   - 无界 `int` → 明确宽度（见 ADR-007）
   - `Protocol` / duck typing → trait 或具体类型
3. **不为类型洁癖买单**：收益不明确的完全静态安全不做。例如不用过程宏/宏生成 token 类型来消灭"构造期配错判别子"的可能（见 ADR-002）。
4. **简明优先**：普通 serde derive + 具名小函数 + 构造器。**禁止 proc macro**；`macro_rules!` 目前为 0，新增需要 ADR（理由：宏让 wire 行为难以 grep，重写者无法逐行对照 Python）。
5. **依赖最小化**：当前只有 `serde` + `serde_json`。新增 crate 必须写 ADR：用途、替代方案、体积/维护成本。
6. **不做顺手改进**：与 Python 不同的行为，要么写进模块头注释与本文 ADR，要么不做。
7. **命名对齐**：Rust 模块名、类型名与 Python 一致（`tau_agent/messages.py` → `tau_agent/messages.rs`；`UserMessage` → `UserMessage`），便于逐行对照。
8. **每个差异都要解释**：模块头写 deviations 索引；具体差异点就近写"Python 怎么做 → Rust 怎么做 → 为什么 → 代价/边界"。参见 `messages.rs` / `provider_events.rs` 的现状，这是硬性要求（ADR-009）。

---

## 2. 标准工作流（每写一个模块的固定动作）

1. 通读 Python 原文件：先 imports 与 `__all__`，再模型，再函数。
2. **用 Python 实测语义，不要凭印象。** pydantic 行为常常反直觉：缺 discriminator、`validate_by_name`、`_to_camel` + `str.title()` 的数字坑、smart union 顺序、`exclude_none`。先写 10 行脚本跑 `validate_python` / `model_dump_json` 看结果。
3. 生成 goldens：把 Python 的真实输出贴进 Rust 测试。至少覆盖典型值、空值、边界、legacy 形状。
4. 写 Rust：模块头 deviations + 第 4 节通用约定 + 结构体/函数。
5. 写测试：exact-string + Value 对比 + acceptance corpus + drift test（见第 5 节）。
6. `cargo fmt` → `cargo test` → `cargo clippy`（除 dead_code 外必须清零）。
7. 更新本文第 7 节状态表；有新取舍补 ADR。
8. 提交：一个模块一个原子提交，信息写明"模块 + 对的 wire 行为范围"。

---

## 3. 目录与分层映射

两个仓库是同级目录：`tau-rs/`（本仓库）与 `../tau/`（Python）。

| Python | Rust | 状态 |
|---|---|---|
| `src/tau_agent/types.py` | `src/tau_agent/types.rs` | ✅（`JSONPrimitive` 由 `serde_json::Value` 覆盖，无需单独别名） |
| `src/tau_agent/messages.py` | `src/tau_agent/messages.rs` | ✅ 33 tests |
| `src/tau_agent/provider_events.py` | `src/tau_agent/provider_events.rs` | ✅ 8 tests |
| `src/tau_agent/tools.py` | `src/tau_agent/tools.rs` | ⬜ 下一个候选 |
| `src/tau_agent/provider.py` | `src/tau_agent/provider.rs` | ⬜ 受 async 决策阻塞 |
| `src/tau_agent/events.py` | `src/tau_agent/events.rs` | ⬜ |
| `src/tau_agent/tool_history.py` | `src/tau_agent/tool_history.rs` | ⬜ |
| `src/tau_agent/harness.py` | `src/tau_agent/harness.rs` | ⬜ 受 async 决策阻塞 |
| `src/tau_agent/loop.py` | `src/tau_agent/loop.rs` | ⬜ 受 async 决策阻塞 |
| `src/tau_agent/session/entries.py` | `src/tau_agent/session/entries.rs` | ⬜ |
| `src/tau_agent/session/jsonl.py` | `src/tau_agent/session/jsonl.rs` | ⬜ 迁移边界 |
| `src/tau_agent/session/storage.py` | `src/tau_agent/session/storage.rs` | ⬜ |
| `src/tau_agent/session/memory.py` | `src/tau_agent/session/memory.rs` | ⬜ |
| `src/tau_agent/session/tree.py` | `src/tau_agent/session/tree.rs` | ⬜ |
| `src/tau_ai/*.py`（约 5400 行） | `src/tau_ai/*.rs` | ⬜ 空占位 |
| `src/tau_coding/**` | — | ⏸ 明确暂缓 |

规则：

- 一个 Python 模块对一个 Rust 文件；不要合并或拆分（除非 ADR 说明）。
- 模块声明集中在 `src/tau_agent/mod.rs`、未来的 `src/tau_ai/mod.rs`。
- 目前是单 binary crate（`src/main.rs`）。是否增加 `lib.rs` 见第 8 节。
- `tests/fixtures/` 目前为空；legacy 数据从 Python 仓 `tests/fixtures/` 复制（如 `legacy_compaction.jsonl`）。

---

## 4. 通用约定（Golden Rules）

### 4.1 Wire 命名

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(crate) struct Example {
    // wire: "someField"; Python 字段名 "some_field" 作为 alias 接受
    #[serde(alias = "some_field")]
    pub(crate) some_field: String,
}
```

- 序列化永远 camelCase，对齐 `serialize_by_alias=True`。
- **Python 多词字段名要作为 alias**：`WireModel` 开了 `validate_by_name=True`，Python 同时接受 `someField` 和 `some_field`。每个多词字段加 `#[serde(alias = "python_name")]`（legacy session 依赖它，例如 `usage.cache_read`）。
- **数字/缩写特例必须显式写 wire 名**：`_to_camel("cache_write_1h")` 经 `str.title()` 得到 `cacheWrite1H`，而 serde camelCase 是 `cacheWrite1h`。凡 Python 别名与 serde 推导可能不同的字段，都显式 `rename` 加测试。
- 关键字字段用 `r#type`，必要时补 `#[serde(rename = "type")]`。
- 默认 dump 输出所有字段（含 `null`），对齐 `model_dump_json()`；不要给模型加 `skip_serializing_if`（见 ADR-006）。

### 4.2 严格性与默认值

- 每个 struct 加 `#[serde(deny_unknown_fields)]`，对应 `extra="forbid"`。
- 逐字段显式默认：
  - `bool` / `Vec` / 集合 → `#[serde(default)]`
  - 动态默认值（timestamp、`"unknown"`、`true`）→ `#[serde(default = "fn")]`
  - `Option<T>` 缺失自动为 `None`，不要再加 default
- **容器级 `#[serde(default)]` 只在"所有缺省值都能由 `Default` 表达，且没有必填判别子"时使用。** 反例：`AssistantMessage` 最初用了容器级 default，导致缺 `role` 的对象被静默当成 assistant。现在改为逐字段默认，`role` 必填。
- 显式 `null` 需要回退默认值时用 `deserialize_null_default`（对应 Python `model_validator` 把 `usage=None` 改写为 `Usage()`）。
- Python 的 `before` validator 提供的便利输入（如 `content="..."`）默认不实现，统一交给迁移层或构造器（见 4.9）。

### 4.3 判别子（`Literal[...]` / discriminated union）

Python 每个类一个 `Literal[...]` 字段；Rust 用：

1. 一个共享 enum（`MessageRole`、`ContentType`、`StopReason`、`DoneReason`、`ErrorReason`…），`#[serde(rename_all = "camelCase")]`。
2. 每个 struct 字段 `#[serde(deserialize_with = "xxx_role")]`；
3. 一个 3 行的 `expect_*` helper + 每个判别子一行 wrapper（`user_role`、`assistant_role`…）；
4. 有 Python 默认值的判别字段再加 `default = "default_xxx"`；
5. `new()` / 构造函数固定判别子；**不 derive `Default`**（共享默认会给错判别子）；
6. `as_str()` + `Display` + drift test（见 4.6）。

代价（已接受，见 ADR-002）：struct literal 理论上能配错判别子；反序列化永远会校验。

### 4.4 联合类型（union）选型决策树

先问两个问题：

- (1) 具体类型会不会作为别的 struct 的普通字段被**单独序列化**？
- (2) Python 那边是 discriminated union 还是 smart union？

| 情况 | 判据 | Rust 形态 | 现有例子 |
|---|---|---|---|
| A | 会单飞，且 Python 类自带 tag | tag 放 struct，父枚举 `#[serde(untagged)]`，靠字段校验分派 | `AssistantContent` / `ToolCall`（`toolcall_end.tool_call` 单飞）、`AgentMessage` |
| B | 只经 union 使用，Python discriminated union | `#[serde(tag = "type")]`，payload 不带 tag | `AssistantMessageEvent`（未来的 `AgentEvent` 同型） |
| C | 混合 | 先确认带 tag 的变体能否覆盖所有单飞位置；不能则按 A | — |

- `#[serde(tag = "...")]` 与 payload 自带同名字段会冲突（序列化重复 key、反序列化缺字段），这不是风格问题。
- untagged 的报错会变粗（`data did not match any variant ...`）；需要更好报错时由上层先读 raw value 的 tag 再分派，模型层保持简单。
- 不要同时维护"带 tag 结构体"和"内部 tag 枚举"两份表示。

### 4.5 数值类型

| Python | Rust | 理由 |
|---|---|---|
| timestamp（messages，ms `int`） | `u64` | 真实时钟值域 |
| token 计数 | `u64` | 非负 |
| content_index | `usize` | 用于索引，拒绝负数 |
| `exit_code: int \| None` | `Option<i32>` | POSIX 状态含信号负值 |
| `code: str \| int` | untagged `String \| i64` | Python 可负、无界 |
| bool 字段 | `#[serde(default)]` | serde 不自动默认 |

> ⚠️ session entry 的 timestamp 是 **float 秒**，messages 是 **int 毫秒**，不要混用（见 7.2 第 7 条）。

### 4.6 字符串映射

enum → wire 字符串必须手写 `as_str()` + `Display`（serde rename 表运行时不可查询），并配 drift test：

```rust
for role in [MessageRole::User, MessageRole::Assistant, /* ... */] {
    assert_eq!(serde_json::to_string(&role).unwrap(), format!("\"{role}\""));
}
```

同类规则适用于任何手写 wire 字符串（如 `ToolHistoryRepair::diagnostic_data()` 的 camelCase key）。

### 4.7 JSON 对象与键序

- 用 `types.rs::JsonObject`（`serde_json::Map`），不要 `HashMap`（顺序随机）或 `BTreeMap`（排序）。
- `serde_json` 已开 `preserve_order`，对齐 Python dict 的插入序；session round-trip 依赖它。
- `exclude_none=True` 的"递归剪除 null"写在 session 写层，不要污染模型层（ADR-006）。

### 4.8 Python 属性 / 访问器映射

- `@property` → Rust method（`text()`、`tool_calls()`、`input_schema()`）。
- 属性只在部分变体存在（如 `event.partial` 对 `done`/`error` 不存在）→ `Option` 访问器。
- 联合类型上的共同属性（`message.role` / `timestamp`）→ enum 上穷尽 `match` 的访问器。
- 隐式 union 转换 → 显式 `From` impl；每个目标类型只保留必要数量，避免 `.into()` 推断歧义。

### 4.9 被放弃的 Python 便利及去处

| Python 便利 | Rust 处理 | 去处 |
|---|---|---|
| before-validator `content="..."`（AssistantMessage / ToolResultMessage / AgentToolResult） | 不在反序列化实现 | session 迁移层 |
| 单类校验时 `role` 默认值 | role 必填 | ADR-003 |
| `exclude_none=True` | 模型保留 null | session 写层递归剪除 |
| 函数默认参数（如 `assistant_content(..., tool_calls=())`） | 必填 + `IntoIterator` | 调用点传 `[]` |
| 无界 `int` | 明确宽度 | ADR-007 |
| `dataclass(frozen=True, slots=True)` | 普通 struct + `Clone`/`Copy` | — |
| `Protocol` | trait / trait object / 泛型 | 按模块决策（第 7、8 节） |
| `@model_validator(mode="before")` 的其它改写 | 具名 helper / 构造器 / 迁移 | 视语义 |
| `isinstance` 分派 | 穷尽 `match` | — |

### 4.10 可见性、模块与依赖

- 一切 `pub(crate)`：struct / enum / 字段 / 函数。字段直接 `pub(crate)`，不写 getter 层。
- 不复制 Python `__init__.py` 的 facade 导出；模块间直接按路径引用（facade 问题见第 8 节）。
- 模块头必须写：Python 来源、wire 契约、deviations 索引。
- 依赖白名单：`serde`（derive）、`serde_json`（preserve_order）。新增依赖需 ADR。

---

## 5. 测试方法论与完成标准（DoD）

### 5.1 测试分层

1. **exact-string**：`serde_json::to_string` 对比 Python `model_dump_json()` 逐字节，用于短 payload，锁定字段顺序。
2. **Value 对比**：大 payload（嵌套 `AssistantMessage` 等）用 `to_value` vs `json!({...})`，可读性更好。
3. **acceptance corpus**：涉及 union / 校验时，把 Python `TypeAdapter(...).validate_python` 的 accept/reject 结论排成 `[(&str, bool)]` 表，逐条断言 `is_ok() == expected`。corpus 必须由 Python 实跑生成，不能手写。
4. **drift test**：手写映射（`as_str`、手写 camelCase key）必须断言与 serde/预期一致。
5. **别名**：每个多词字段至少覆盖 camelCase 与 snake_case 两条输入；同时出现两种拼写时 serde 报重复字段（比 Python 严格，已记录为可接受差异）。

### 5.2 测试放置

- 单元测试放**源文件内** `#[cfg(test)] mod tests`：类型都是 `pub(crate)`，integration tests 看不到。
- `tests/fixtures/` 只放跨模块或外部数据（legacy session JSONL 等），从 Python 仓 `tests/fixtures/` 复制而不是重写。

### 5.3 DoD

- [ ] `cargo fmt --check` 干净
- [ ] `cargo test` 全绿，且新增 `#[test]` 的数量与 `cargo test` 实际运行数一致
- [ ] `cargo clippy` 除 dead_code 外无告警
- [ ] 模块头 deviations 完整；每个差异点有理由注释
- [ ] 本文第 7 节状态表已更新
- [ ] 新取舍已补 ADR

> 真实教训：漏写 `r#"..."#` 的结尾 `#` 会把后续测试吞进字符串，rustfmt / clippy / 编译都不报错，只是该测试静默不再运行。新增测试后必须核对数量（可用 `grep -c '#\[test\]'` 与 `cargo test -- --list` 对照）。

---

## 6. 决策记录（ADR）

### ADR-001：wire 与输入契约以 Python 为准

- **决定**：序列化输出、反序列化接受的拼写与默认值，逐项对齐 Python；goldens 由 Python 实跑生成。
- **代价**：Rust 侧要写 alias、默认值函数、drift test；不能享受 serde 默认行为。

### ADR-002：不引入宏；判别子用"共享 enum + 字段校验"

- **背景**：Python 用每类 `Literal[...]`；完全静态化需要每类型独立 token 类型。
- **决定**：共享 enum + `expect_*` helper + 每字段 wrapper；`new()` 固定判别子；反序列化强制校验。
- **否决方案**：`macro_rules!`/proc macro 生成 token（不可 grep、重写者难对照）；const generic token（对读者太隐晦）。
- **代价**：构造期可以配错判别子（测试兜底）；这是明知的取舍。

### ADR-003：`role` 必填

- **背景**：Python 单类校验时 `role` 有默认值，`AgentMessage` union 缺 discriminator 则报错；两者不能同时满足于一个 serde 实现。
- **决定**：采用 union 语义，`role` 必填。
- **理由**：wire 数据永远带 role；必填能防止 untagged union 把缺 role 的对象误判成 assistant。
- **代价**：单独反序列化一条消息且缺 role 会失败（真实场景不存在）。

### ADR-004：content 的 tag 在结构体；事件 union 的 tag 在枚举

- **背景**：最初 content 枚举用 `#[serde(tag="type")]`，`toolcall_end.tool_call` 单飞时丢失 `"type":"toolCall"`。
- **决定**：content block 自带 `r#type`，父枚举 untagged；事件 payload 不带 tag，枚举 internally tagged。
- **理由**：两者的使用形态不同——block 会单飞，事件只经 union。不对称是有依据的。

### ADR-005：`serde_json` 开启 `preserve_order`

- **决定**：`JsonObject` 保持插入序。
- **理由**：对齐 Python dict；session round-trip 不产生无意义 diff。
- **代价**：引入 `indexmap` 传递依赖。

### ADR-006：null 与 `exclude_none` 的归属

- **决定**：模型层默认输出 `null`（对齐 `model_dump_json()`）；`exclude_none=True` 由 session 写层递归剪除；`content="..."` 由迁移层转换。
- **理由**：Python 的 `exclude_none` 是 per-call dump 选项，模型层无法同时满足两种调用；迁移是持久化边界，符合 Python 注释里的设计意图。

### ADR-007：整数宽度映射

- 见 4.5 表格。无界 `int` 按语义选 `u64` / `usize` / `i32` / `i64`；越界与负数按格式错误拒绝。

### ADR-008：测试内联在源文件

- **理由**：全 crate 使用 `pub(crate)`，integration test 无法访问；golden 与实现放在一起便于对照。

### ADR-009：差异注释是硬性要求

- **要求**：模块头 deviations 索引 + 差异点就近 rationale（Python 做法 / Rust 做法 / 原因 / 代价）。
- **样板**：`messages.rs`、`provider_events.rs` 的文件头与关键字段注释。

---

## 7. 重写路线图与各模块已知坑

### 7.1 已完成

- `types.rs`：`JsonValue` / `JsonObject` 别名。
- `messages.rs`（33 tests）：全部消息与内容模型；wire 契约由 Python 输出逐字节锁定；含 acceptance corpus、全量 alias/数值边界/角色覆盖。
- `provider_events.rs`（8 tests）：12 种流事件 + tagged union；全事件 snake_case 与 `partial()` 覆盖。

### 7.2 建议顺序与已知坑

先 portable core（`tau_agent`），再 `tau_ai`；`tau_coding` 最后。

1. **`tools.rs`（Python 118 行）**
   - `AgentToolResult.content` 与 messages 的 `ToolResultContent` 同构，直接复用 `ToolResultContent` / `TextContent` / `ImageContent`。
   - `content="..."` normalizer 同 ADR-006 不实现（事件里的 `partial_result` 由构造侧保证）。
   - 三类 Protocol/Callable：`ToolExecutor`（async，带 signal/on_update）、`ToolCallRenderer`、`ToolResultRenderer`。**需要决策**：trait object 还是泛型参数；建议 trait object（Python 是运行期装配）。
   - `Mapping[str, JSONValue]` → `JsonObject`；`execution_mode` → enum；`input_schema` → method。
2. **`provider.rs`（37 行）— 本轮最大未决：async。**
   - Python：`def stream_response(...) -> AsyncIterator[AssistantMessageEvent]`。
   - 选项：(a) tokio + async-trait；(b) runtime-agnostic `futures_core::Stream` + `Pin<Box<dyn Stream + Send>>`；(c) 同步 trait（会偏离）。
   - `CancellationToken` Protocol → `Arc<AtomicBool>`。
   - **先拍板再写**，否则 loop / harness / tau_ai 全部返工。
3. **`events.rs`（87 行）**
   - `AgentEvent` 是 `discriminator="type"` 的 union，且只在 union 路径使用 → 按 4.4-B。
   - payload 嵌套 `AgentMessage` / `ToolResultMessage` / `AgentToolResult` / `AssistantMessageEvent`，注意多层 tag 并存。
   - `args: dict` → `JsonObject`。
4. **`tool_history.rs`（169 行）**
   - 纯逻辑、无 serde 模型；`isinstance` → 穷尽 `match`。
   - `diagnostic_data()` 是**手写 camelCase key**（`synthesizedResults` 等），不是 serde 模型；必须手写 + 测试。
   - 算法语义逐条对齐：先占用"已相邻"结果 → 合成缺失结果（文案 `"Tool call interrupted by user"`）→ 丢弃孤儿/重复（真实结果优先于合成结果）→ 重排；四个计数器语义要一致。
5. **`harness.rs`（259 行）**
   - `EventListener = Callable[[AgentEvent], Awaitable[None] | None]`：Python 用 `isawaitable` 兼容同步/异步；Rust 需统一签名（建议 async-only，同步闭包用 `async {}` 包）。
   - `QueuedMessages` / `AgentHarnessConfig` → struct；queue_mode 状态机、取消、turn 计数。
6. **`loop.rs`（376 行）**
   - async generator → Stream（依赖 2 的决策）；事件顺序与 `MessageUpdateEvent` 嵌套是 wire 行为，逐条测试。
   - `monotonic_ns` → `Instant`；first-output 判定集合 = TextDelta / ThinkingDelta / ToolCallStart / Delta / End，写成明确 predicate。
   - `max_turns < 1` 与超限错误文案要和 `_error_message` 一致。
7. **`session/entries.rs`（144 行）**
   - ⚠️ **entry 的 `timestamp` 是 float 秒**（`time()`），messages 是 int 毫秒；迁移时还会出现毫秒 ÷ 1000。
   - `replaces_entry_ids` 有 `exclude_if=lambda ids: not ids`：**空列表序列化时要省略**，反序列化要默认空列表。
   - `type: Literal[...]` 是 union 判别子且 entry 只经 union 使用 → internally tagged。
   - `extra="forbid"`；`parent_id: str | None`。
8. **`session/jsonl.rs`（171 行）— legacy 兼容的唯一入口**
   - 写：`exclude_none=True`、单行 JSON；Rust 需递归剪 null。
   - 读：JSON 解析 → `_migrate_session_entry` → 严格 validate；错误带行号。
   - `_migrate_message` legacy 表（用 Python `tests/fixtures/legacy_compaction.jsonl` 做 fixture 逐条测试）：
     - user + `custom_type`/`customType` → role `custom`
     - assistant：string content → text block；`tool_calls`/`toolCalls` → blocks；`usage.cost is None` → `{}`
     - role `tool` → `toolResult`；`name`→`toolName`；`tool_call_id`→`toolCallId`；`ok`→`isError=!ok`；string content→blocks；`data`+`details` 合并；`error`→内容兜底
     - entry `type=="custom_message"`：`customType`→`custom_type`
     - 无 target 的 `label` → 最早 branchable entry
9. **`session/storage.rs`（203）/ `memory.rs`（208）/ `tree.rs`（40）**
   - 文件与内存存储、树结构；`new_entry_id()`（定义在 `entries.py`）用 UUID（依赖未定）；文件写入的原子性/恢复语义按 Python 实现逐条对齐。
10. **`tau_ai/`（约 5400 行，最大块）**
    - 抽象：`provider.py`、`stream.py`、`content.py`、`_provider_events.py`；基础设施：`http.py`、`retry.py`、`http_errors.py`、`env.py`、`model_catalog.py`、`model_limits.py`；各家 provider：`openai_compatible`、`anthropic`、`google`、`mistral`、`openai_codex`、`fake` 等。
    - **未决**：HTTP/SSE 依赖选型、重试/超时策略、代理与 env 处理、model catalog JSON 的打包方式。
    - `_provider_events.py` 的 `ProviderToolCallEvent.tool_call: ToolCall` 现在可直接复用 messages.rs 的单飞 `ToolCall`。
11. **`tau_coding/**`：CLI / TUI / RPC / session_export**，依赖 Textual/Rich 等 Python 生态，**明确暂缓**；先把 portable core 做稳。

### 7.3 不要做的事

- 不要为了"更 Rust"改 wire；不要改类型名；不要合并模块。
- 没有 Python golden 之前不要写序列化测试（会锁死错误行为）。
- 不要给模型层加 `skip_serializing_if` 图省事（破坏 `model_dump_json()` 对齐）。
- 目前 crate 是 binary，不要把 `pub(crate)` 提升为 `pub`（API 边界见第 8 节）。

---

## 8. 未决问题（需要拍板并升级为 ADR）

1. **async 运行时与 Stream 抽象**（阻塞 `provider` / `harness` / `loop` / `tau_ai`）。
2. **crate 形态**：继续 binary-only，还是加 `lib.rs`（集成测试、SDK、未来 workspace 都需要）。加 lib 后要重新审视 `pub(crate)` 策略。
3. **错误处理策略**：核心 portable 层建议自定义 error enum（可后续引入 `thiserror`），应用层再用 `anyhow` 之类。
4. **HTTP / SSE 依赖**选型。
5. **数据文件打包**：model catalog 等 JSON 用 `include_str!` 还是运行时资源。
6. **cargo features 划分**：是否按 provider 拆 feature（首版可全量）。
7. **Python facade（`__init__.py`）的处理**：是否在 Rust 提供等价 re-export 模块，还是按模块直接引用。

---

## 9. 新模块开工 Checklist

- [ ] 通读 Python 原文件，记录所有 `Literal` / 默认值 / validator / drop-in 便利
- [ ] 用 Python 实跑语义探针（accept/reject、dump 形状、边界）
- [ ] 生成 goldens（exact-string / Value / corpus）
- [ ] 按第 4 节写 Rust；模块头写 deviations
- [ ] 复用已有模型（如 `ToolResultContent`、`JsonObject`），不重复定义
- [ ] 单测齐全（含 alias、默认值、拒绝用例、drift）
- [ ] `cargo fmt` + `cargo test` + `cargo clippy`
- [ ] 更新第 7 节状态表；新取舍补 ADR

---

## 10. 附录

### 10.1 常用命令

```bash
# 在 tau-rs/
cargo fmt
cargo test
cargo clippy --all-targets

# 在 ../tau/（Python 权威源）
../tau/.venv/bin/python - <<'PY'
from tau_agent.messages import *
print(AssistantMessage(content=[TextContent(text="x")], timestamp=1).model_dump_json())
PY
```

### 10.2 golden 生成模板

```python
# 生成 Rust 测试用的字面量：先想清楚要锁的行为，再粘贴输出
from tau_agent.messages import AssistantMessage, TextContent
print(AssistantMessage(content=[TextContent(text="x")], timestamp=1).model_dump_json())
```

```python
# acceptance corpus：只收集 accept/reject 结论，不收集异常文本
import json
from pydantic import TypeAdapter, ValidationError
from tau_agent.messages import AgentMessage

cases = [("user string", {"role": "user", "content": "x"}), ...]
adapter = TypeAdapter(AgentMessage)
for label, payload in cases:
    try:
        adapter.validate_python(payload)
        ok = True
    except ValidationError:
        ok = False
    compact = json.dumps(payload, separators=(",", ":"))
    print(f'    (r#"{compact}"#, {str(ok).lower()}), // {label}')
```

### 10.3 pydantic ↔ serde 语义对照速查

| 语义 | pydantic (Python) | serde (Rust) | 约定 |
|---|---|---|---|
| camelCase 别名 | `alias_generator=_to_camel` | `rename_all="camelCase"` | 数字/缩写特例显式 rename |
| 同时接受 snake_case | `validate_by_name=True` | `#[serde(alias=...)]` | 4.1 |
| 未知字段 | `extra="forbid"` | `deny_unknown_fields` | 4.2 |
| 缺字段默认 | 每字段默认值 | 逐字段 `default` | 容器级慎用 |
| 显式 null 回退默认 | `before` validator | `deserialize_null_default` | 4.2 |
| `Literal` | 运行期校验 | enum | 4.3 |
| discriminated union | 缺 tag 报错 | internally tagged 报错 | 4.4 |
| smart union | 按序尝试，但会继续寻找更优匹配（默认） | untagged 严格按序、首个成功即返回 | 对 `str \| int` 这类同值多表示，按 Python 实测结论排列变体并加 corpus |
| property | `@property` | method | 4.8 |
| 无界 int | `int` | 明确宽度 | 4.5 / ADR-007 |
| dict 键序 | 保序 | `preserve_order` | 4.7 |
| `exclude_none` | dump 选项 | 手写递归剪除 | session 层 |
| 默认参数 | 函数默认值 | `IntoIterator` / 构造器 | 4.9 |
| Protocol | 结构化子类型 | trait | 按模块决策 |
| async iterator | `AsyncIterator` | Stream（未决） | 第 8 节 |
| `dataclass(frozen)` | 不可变值对象 | struct + `Clone` | — |

---

### 10.4 验证记录（2026-09-18）

本文档的事实性断言不是凭记忆写的：逐条对照 **实际运行结果** 与 **对应版本官方文档/源码** 双重验证。依赖或 Python 版本升级后必须重跑本节探针并更新结论。

**验证环境**

| 组件 | 版本 |
|---|---|
| Python | 3.12.14 |
| pydantic | 2.13.4 |
| rustc / cargo | 1.98.0 |
| serde / serde_derive | 1.0.229 |
| serde_json | 1.0.151 |
| indexmap（preserve_order 传递依赖） | 2.14.2 |

**验证手段**

1. **Python 实跑探针**：对 pydantic 语义直接 `validate_python` / `model_dump_json`，记录 OK/FAIL（模板见 §10.2）。
2. **Rust 固定版本探针**：`serde = "=1.0.229"`、`serde_json = "=1.0.151"` 的最小工程，实测下表 serde 行为（复跑见下）。
3. **官方文档/源码（对应版本）**：docs.rs 固定版本页面、serde.rs、vendored crate 源码、pydantic 2.13 文档、Cargo Book。

**引用文档（均为对应版本）**

- pydantic config（`validate_by_name` / `validate_by_alias` / `serialize_by_alias`）：https://docs.pydantic.dev/2.13/api/config/
- pydantic unions（smart union 定义、`union_tag_not_found`）：https://docs.pydantic.dev/2.13/concepts/unions/
- serde_json 1.0.151 Map：https://docs.rs/serde_json/1.0.151/serde_json/map/index.html
- serde field attributes：https://serde.rs/field-attrs.html
- Cargo Book（integration tests 为独立 crate）：https://doc.rust-lang.org/cargo/reference/cargo-targets.html
- vendored 源码：`~/.cargo/registry/src/<registry>/serde-1.0.229/`、`serde_derive-1.0.229/`、`serde_json-1.0.151/`

**关键断言 → 证据**

| # | 断言 | 证据 |
|---|---|---|
| 1 | `WireModel` 配置 = `extra="forbid"` / `validate_by_name=True` / `validate_by_alias=True` / `serialize_by_alias=True` / `alias_generator=_to_camel` | `../tau/src/tau_agent/messages.py:14-33` + Python 探针打印 `model_config` |
| 2 | `_to_camel("cache_write_1h") == "cacheWrite1H"`（`str.title()` 把 `1h` 变 `1H`） | Python 探针 + `messages.py:14` |
| 3 | `AgentMessage` 缺 `role` 报错；单类 `model_validate` 缺 `role` 取默认值（user/assistant/toolResult） | Python 探针（`TypeAdapter(AgentMessage)` vs `UserMessage.model_validate`） |
| 4 | content block 缺 `type` 取默认；`type` 与载荷不匹配被拒 | Python 探针 |
| 5 | `content="..."` 被 `AssistantMessage` / `ToolResultMessage` / `AgentToolResult` 的 before-validator 接受 | Python 探针 + `messages.py` / `tools.py:31` |
| 6 | `model_dump_json()` 默认输出 `null`；`exclude_none=True` 递归剔除（`textSignature`、`responseModel` 等消失） | Python 探针 + `session/jsonl.py:21` |
| 7 | 旧 session 的 `usage.cache_read` 依赖 `validate_by_name`（迁移不重命名 usage 键） | `session/jsonl.py:132-137` + `tests/test_session.py` 的 legacy usage 用例 |
| 8 | serde：缺失的 `Option<T>` 字段为 `None`（无需 `default`） | serde-1.0.229 `src/private/de.rs:24-50`（`missing_field` 的 `deserialize_option` → `visit_none`）+ Rust 探针 |
| 9 | serde `rename_all="camelCase"` 对 `cache_write_1h` 产出 `cacheWrite1h`（小写 h） | serde_derive-1.0.229 `src/internals/case.rs:101-104` + Rust 探针 |
| 10 | 内部 tag enum + payload 同名字段：序列化产生重复 key，反序列化报 `missing field` | Rust 探针（输出 `{"type":"text","type":"text","text":"x"}` / 错误 `missing field type`） |
| 11 | alias 与 primary 同时出现：serde 报 `duplicate field`；Python alias 胜出 | Rust 探针 + serde_derive-1.0.229 `src/de/struct_.rs:266-269` + Python 探针 |
| 12 | untagged 失败文本 = `data did not match any variant of untagged enum ...` | serde_derive-1.0.229 `src/de/enum_untagged.rs:39` + Rust 探针 |
| 13 | `deny_unknown_fields` 在 untagged variant 内生效 | Rust 探针 + crate 测试 `agent_message_validates_roles_and_unknown_fields` |
| 14 | `deserialize_with` 无 `default` 时字段必填；加 `default` 后缺失取默认 | Rust 探针（`missing field n` / `Ok(0)`） |
| 15 | `bool` 缺失字段需显式 `#[serde(default)]`，否则报 `missing field` | Rust 探针 |
| 16 | `serde_json::Map` 默认 BTreeMap（键排序）；`preserve_order` 为 IndexMap（保持插入序） | serde_json-1.0.151 `src/map.rs:3-7,33-36` + docs.rs 同版本页面 + Rust 探针（`{"a":2,"m":3,"z":1}` vs `{"z":1,"a":2,"m":3}`） |
| 17 | Cargo integration tests 各自是独立 crate，只能访问 library 的 public API | Cargo Book `cargo-targets` 原文："Cargo will compile each of these files as a separate crate ... Integration tests can use the public API of the package's library." |
| 18 | pydantic smart union ≠ 严格 left-to-right：按序尝试并继续寻找更优匹配 | pydantic 2.13 unions 文档原文 + Python 探针（`str \| int` 行为） |
| 19 | pydantic discriminated union 提取不到 tag 时报 `union_tag_not_found` | pydantic 2.13 unions 文档 + Python 探针 |
| 20 | 模块行数、测试数、目录状态（§3、§7.1） | `wc -l` 实测；`cargo test` = messages 33 + provider_events 8 = 41；`tests/fixtures/` 为空；`src/tau_ai/*.rs` 为 0 行 |

**复跑探针**

```bash
# Python（在 ../tau/ 下）
.venv/bin/python - <<'PY'
from tau_agent.messages import _to_camel, WireModel, Usage, AgentMessage
from pydantic import TypeAdapter, ValidationError
print(_to_camel("cache_write_1h"))                        # cacheWrite1H
print(WireModel.model_config["extra"])                    # forbid
try:
    TypeAdapter(AgentMessage).validate_python({"content": "x"})
except ValidationError:
    print("missing role rejected")
print(Usage.model_validate({"cache_read": 1}).cache_read)  # 1
PY
```

```bash
# Rust 语义探针（一次性，不属于仓库依赖）
cargo new --bin /tmp/serde_verify && cd /tmp/serde_verify
# Cargo.toml：
#   serde = { version = "=1.0.229", features = ["derive"] }
#   serde_json = { version = "=1.0.151", features = ["preserve_order"] }
# src/main.rs 覆盖以下用例后 cargo run -q：
#   1) struct Opt { a: Option<String>, b: String } 解析 {"b":"x"}
#   2) #[serde(rename_all = "camelCase")] 的 cache_write_1h 序列化
#   3) #[serde(tag = "type")] enum + payload 自带 type 字段
#   4) #[serde(alias = "cache_read")] 同时给 cacheRead/cache_read
#   5) #[serde(untagged)] 失败时的错误文本
#   6) serde_json::Map 解析 {"z":1,"a":2,"m":3} 后的输出键序
```

> 探针只用于语义验证；不要把探针工程或额外依赖加入 tau-rs。
