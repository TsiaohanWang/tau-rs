# Phase 3/4 实施计划 — 综合执行方案

> 状态：📋 计划中
> 目标：实现 tau-coding crate（内置工具 + session 持久化），集成到 CLI。

## 总体策略

**依赖链**: tau-coding 依赖 tau-types + tau-agent，CLI 依赖 tau-coding。

**并行机会**: Phase 3 的工具实现与 Phase 4 的 session 存储可以并行开发（无依赖关系）。

---

## Phase 3: 内置工具

### 步骤 1: 创建 tau-coding crate 骨架

1. 创建 `crates/tau-coding/Cargo.toml`
2. 创建 `crates/tau-coding/src/lib.rs`
3. 更新 workspace `Cargo.toml` 添加成员
4. 验证: `cargo build --workspace`

### 步骤 2: 实现 tools/read.rs

**文件**: `crates/tau-coding/src/tools/read.rs`

```rust
// 功能：读取文件内容，支持行范围
// Schema: path (required), offset (default 0), limit (default 2000)
// 输出：带行号的文件内容
```

测试用例:
- 正常读取
- 文件不存在
- 行范围（offset + limit）
- 二进制文件检测

### 步骤 3: 实现 tools/write.rs

**文件**: `crates/tau-coding/src/tools/write.rs`

```rust
// 功能：创建或覆盖文件内容
// Schema: path (required), content (required)
// 特性：自动创建目录，原子写入
```

测试用例:
- 正常写入
- 创建目录
- 覆盖文件

### 步骤 4: 实现 tools/edit.rs

**文件**: `crates/tau-coding/src/tools/edit.rs`

```rust
// 功能：查找并替换文件中的文本
// Schema: path (required), old_text (required), new_text (required)
// 校验：old_text 必须存在且唯一
```

测试用例:
- 正常替换
- old_text 不存在
- 多处匹配（报错）
- 原子写入

### 步骤 5: 实现 tools/bash.rs

**文件**: `crates/tau-coding/src/tools/bash.rs`

```rust
// 功能：执行 shell 命令
// Schema: command (required), timeout (default 120)
// 特性：process_group(0) + killpg + 输出截断
```

测试用例:
- 正常执行
- 超时
- 非零退出码
- 输出截断

### 步骤 6: 实现 tools/mod.rs

**文件**: `crates/tau-coding/src/tools/mod.rs`

```rust
pub fn create_coding_tools(cwd: &Path) -> Vec<AgentTool> {
    vec![
        read::create_tool(cwd),
        write::create_tool(cwd),
        edit::create_tool(cwd),
        bash::create_tool(cwd),
    ]
}
```

### 步骤 7: 重构 tau-cli 集成 harness

**修改**: `crates/tau-cli/src/main.rs`

1. 添加 tau-coding 依赖
2. 替换直接 provider 调用为 harness 调用
3. 处理工具执行事件
4. 验证: CLI --print 模式可执行工具调用

---

## Phase 4: 配置与持久化

### 步骤 8: 实现 session/storage.rs

**文件**: `crates/tau-coding/src/session/storage.rs`

```rust
// JsonlSessionStorage: session 文件读写
// - new(path): 创建实例
// - read_all(): 读取所有 entries（带 v1 迁移）
// - append(entry): 追加单个 entry
// - append_batch(entries): 批量追加
```

测试用例:
- 正常读写
- 空文件
- 损坏行处理
- v1 迁移

### 步骤 9: 实现 session/manager.rs

**文件**: `crates/tau-coding/src/session/manager.rs`

```rust
// SessionManager: session 目录管理
// - new(sessions_dir): 创建实例
// - prepare(project_dir): 准备项目目录
// - create(project_dir): 创建新 session
// - load(project_id, session_id): 加载已有 session
// - list(project_dir): 列出项目 sessions
```

测试用例:
- 创建 session
- 列出 sessions
- 加载 index

### 步骤 10: 实现 config/catalog.rs

**文件**: `crates/tau-coding/src/config/catalog.rs`

```rust
// merge_catalogs(base, overlay): 深度合并两个 catalog
// 合并规则：
// - overlay 的 provider 覆盖 base 的同名 provider
// - 保留两者独有的 provider
// - schema_version 使用 overlay 的值
```

测试用例:
- 空 overlay
- 覆盖 provider
- 追加 provider

### 步骤 11: CLI 集成 session 持久化

**修改**: `crates/tau-cli/src/main.rs`

1. 添加 SessionManager 初始化
2. print 模式自动创建 session 文件
3. 运行时持久化消息和工具结果
4. 验证: session 文件格式与 Python 兼容

---

## 执行顺序（优化版）

```
Week 1: Phase 3 核心工具
├── Day 1-2: crate 骨架 + read/write 工具
├── Day 3-4: edit/bash 工具
└── Day 5: 工厂函数 + 单元测试

Week 2: Phase 3 CLI 集成 + Phase 4 核心
├── Day 1-2: CLI harness 集成
├── Day 3-4: session/storage.rs
└── Day 5: session/manager.rs

Week 3: Phase 4 完成 + 集成测试
├── Day 1-2: config/catalog.rs
├── Day 3-4: CLI session 持久化集成
└── Day 5: 全量测试 + 文档更新
```

---

## 验收检查清单

### Phase 3 验收
- [ ] `cargo build --workspace` 零警告
- [ ] `tau-coding` 4 个工具单元测试通过
- [ ] `create_coding_tools()` 返回 4 个工具
- [ ] CLI `--print` 模式可执行工具调用
- [ ] `cargo fmt --check` 通过

### Phase 4 验收
- [ ] `session/storage.rs` 单元测试通过
- [ ] `session/manager.rs` 单元测试通过
- [ ] `config/catalog.rs` 单元测试通过
- [ ] CLI print 模式自动创建 session 文件
- [ ] Session 文件格式与 Python 兼容
- [ ] Catalog 合并结果正确

### 最终验收
- [ ] `cargo test --workspace --features tau-agent/testing` 全绿（68 + 28 = 96 测试）
- [ ] `cargo clippy --workspace` 无警告
- [ ] `cargo fmt --check` 通过
- [ ] 所有文档更新完成
