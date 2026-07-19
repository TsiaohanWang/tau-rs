# Phase 4 实施计划 — 配置与持久化

> 状态：✅ 已完成（2026-07-19）
> 目标：实现 session 文件 I/O、session 目录管理、catalog 深度合并，为 Phase 5 CodingSession 奠定基础。

## 1. 范围

### 1.1 包含
- `JsonlSessionStorage`：session 文件读写（JSONL 格式）
- `SessionManager`：session 目录管理、index.jsonl 维护
- `catalog` 深度合并：内置 catalog + 用户 catalog
- CLI 集成：print 模式自动创建 session 并持久化

### 1.2 不包含（防 scope creep）
- 不做 OAuth 条目支持（Phase 8）
- 不做 Anthropic token 自动刷新（Phase 8）
- 不做 `settings.json`（Phase 7）
- 不做 `tui.json`（Phase 7）
- ~~不做 CodingSession 组合根（Phase 5）~~ — 骨架已实现（`session/coding_session.rs`）
- ~~不做 compaction（Phase 5）~~ — 基础已实现（`session/compaction.rs` + `session/context_window.rs`）
- ~~不做 命令系统（Phase 5）~~ — system prompt 组装器已实现（`session/prompt.rs`）
- ~~不做 REPL 工具事件显示~~ — 已实现

---

## 2. Crate 结构

在 `tau-coding` crate 中新增模块：

```
crates/tau-coding/src/
├── lib.rs
├── tools/           # Phase 3
│   └── ...
├── session/
│   ├── mod.rs
│   ├── storage.rs           # JsonlSessionStorage
│   ├── manager.rs           # SessionManager
│   ├── coding_session.rs    # CodingSession 组合根骨架（Phase 5 预置）
│   ├── compaction.rs         # Compaction 基础逻辑（Phase 5 预置）
│   ├── context_window.rs    # 上下文窗口计算（Phase 5 预置）
│   └── prompt.rs            # System prompt 组装器（Phase 5 预置）
└── config/
    └── catalog.rs   # catalog 深度合并
```

---

## 3. Session 存储设计

### 3.1 `session/storage.rs` — JsonlSessionStorage

**职责**: 单个 session 文件的读写。

```rust
use std::path::PathBuf;
use tau_types::SessionEntry;
use tau_agent::session::jsonl::{entry_to_json_line, entry_from_json_line};

pub struct JsonlSessionStorage {
    path: PathBuf,
}

impl JsonlSessionStorage {
    pub fn new(path: PathBuf) -> Self;
    pub async fn read_all(&self) -> Result<Vec<SessionEntry>, SessionError>;
    pub async fn append(&self, entry: &SessionEntry) -> Result<(), SessionError>;
    pub async fn append_batch(&self, entries: &[SessionEntry]) -> Result<(), SessionError>;
    pub fn path(&self) -> &Path;
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
    #[error("Migration error: {0}")]
    Migration(String),
}
```

**实现要点**:
- 追加写入：每次 append 在文件末尾追加一行 JSON
- 原子写入：使用 tempfile + rename 防止数据损坏
- 读取时迁移：read_all 对每行调用 entry_from_json_line（已含 v1 迁移）
- 并发安全：使用文件锁防止并发写入

### 3.2 `session/manager.rs` — SessionManager

**职责**: 管理 session 目录结构和 index.jsonl。

```rust
use std::path::{Path, PathBuf};
use crate::session::storage::JsonlSessionStorage;

pub struct SessionManager {
    sessions_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionIndexEntry {
    pub session_id: String,
    pub session_path: String,
    pub created_at: f64,
    pub cwd: Option<String>,
    pub title: Option<String>,
}

impl SessionManager {
    pub fn new(sessions_dir: PathBuf) -> Self;
    pub fn prepare(&self, project_dir: &Path) -> Result<PathBuf, SessionError>;
    pub fn create(&self, project_dir: &Path) -> Result<(PathBuf, JsonlSessionStorage), SessionError>;
    pub fn load(&self, project_id: &str, session_id: &str) -> Result<JsonlSessionStorage, SessionError>;
    pub fn list(&self, project_dir: &Path) -> Result<Vec<SessionInfo>, SessionError>;
    pub fn load_index(&self, project_dir: &Path) -> Result<Vec<SessionIndexEntry>, SessionError>;
    pub fn append_to_index(&self, project_dir: &Path, entry: &SessionIndexEntry) -> Result<(), SessionError>;
}

#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub session_id: String,
    pub created_at: f64,
    pub title: Option<String>,
    pub entry_count: usize,
}
```

**目录结构**：
```
~/.tau/sessions/
├── <project_hash>/
│   ├── index.jsonl
│   ├── <session_id>.jsonl
│   └── <session_id>.jsonl
└── <project_hash>/
    └── ...
```

**项目标识**: 使用项目根目录的绝对路径的 SHA256 哈希前 12 位作为 project_hash。

---

## 4. Catalog 深度合并

### 4.1 `config/catalog.rs`

```rust
use tau_cli::config::CatalogConfig;

/// 深度合并两个 catalog 配置
///
/// 合并规则：
/// - overlay 的 provider 覆盖 base 的同名 provider
/// - 保留两者独有的 provider
/// - schema_version 使用 overlay 的值
pub fn merge_catalogs(base: &CatalogConfig, overlay: &CatalogConfig) -> CatalogConfig;
```

**合并逻辑**：
1. 以 base 的 providers 为基础
2. 遍历 overlay 的 providers
3. 如果 overlay 的 provider 名称已存在于 base 中，替换它
4. 如果不存在，追加到结果中
5. schema_version 使用 overlay 的值

---

## 5. CLI 集成

### 5.1 Print 模式集成 Session 持久化

```rust
// 在 print_once_openai / print_once_anthropic 中：
let session_manager = SessionManager::new(tau_home.sessions_dir());
let (session_path, storage) = session_manager.create(&cwd)?;

// 追加 SessionInfoEntry
storage.append(&SessionEntry::SessionInfo(SessionInfoEntry::new())).await?;

// 运行 harness，收集事件
let stream = harness.prompt(&prompt)?;
while let Some(event) = stream.next().await {
    match event {
        AgentEvent::MessageEnd { message, .. } => {
            // 持久化消息
            storage.append(&SessionEntry::Message(MessageEntry::new(message))).await?;
        }
        AgentEvent::ToolExecutionEnd { result, .. } => {
            // 持久化工具结果
        }
        _ => {}
    }
}

// 追加 LeafEntry
storage.append(&SessionEntry::Leaf(LeafEntry::default())).await?;
```

---

## 6. 测试计划

### 6.1 单元测试

| 模块 | 测试用例 |
|------|----------|
| storage | 正常读写、空文件、损坏行、v1 迁移 |
| manager | 创建 session、列出 sessions、加载 index |
| catalog | 空 overlay、覆盖 provider、追加 provider |

### 6.2 集成测试

- CLI print 模式自动创建 session 文件
- Session 文件格式正确（JSONL，每行一个对象）
- Session 文件可被 Python tau 读取（兼容性验证）

---

## 7. 验收

- [x] `cargo build --workspace` 零警告
- [x] `cargo test --workspace --features tau-agent/testing` 全绿（130 测试）
- [x] `tau-coding` session 模块单元测试通过（6 storage + 5 manager = 11 测试）
- [x] `tau-coding` catalog 模块单元测试通过（8 测试）
- [x] CLI print 模式自动创建 session 文件并持久化消息
- [x] Session 文件格式与 Python 兼容（JSONL + v1 迁移）
- [x] `cargo clippy --workspace --all-targets --features tau-agent/testing -- -D warnings` 通过
- [x] `cargo fmt --check` 通过

---

## 8. 实施顺序

1. ~~实现 `session/storage.rs` (JsonlSessionStorage)~~ ✅
2. ~~实现 `session/manager.rs` (SessionManager)~~ ✅
3. ~~实现 `config/catalog.rs` (catalog 合并)~~ ✅
4. ~~重构 CLI 集成 session 持久化~~ ✅
5. ~~全量测试 + clippy + fmt~~ ✅
6. ~~文档更新~~ ✅

---

## 9. 实施完成记录（2026-07-19）

### 已完成文件

| 文件 | 行数 | 内容 |
|------|------|------|
| `crates/tau-coding/src/session/mod.rs` | 6 | 模块导出 + re-export |
| `crates/tau-coding/src/session/storage.rs` | ~180 | `JsonlSessionStorage`（read_all/append/append_batch + 6 测试） |
| `crates/tau-coding/src/session/manager.rs` | ~240 | `SessionManager`（项目哈希、create/load/list、index.jsonl）+ 5 测试 |
| `crates/tau-coding/src/config/mod.rs` | 10 | 模块导出 |
| `crates/tau-coding/src/config/catalog.rs` | ~230 | `CatalogConfig`/`CatalogProvider`/`ProviderKind` + `merge_catalogs` + 内置 catalog 加载 + 8 测试 |
| `crates/tau-coding/data/catalog.toml` | 6391 | 内置 provider catalog（从 Python 项目复制） |
| `crates/tau-cli/src/main.rs` | 重构 | 统一 `print_once` / `run_repl` + session 持久化 |

### Cargo.toml 变更

`tau-coding` 新增依赖：
- `sha2 = "0.10"` — 项目目录 SHA-256 哈希
- `serde = { workspace = true }` — `SessionIndexEntry` 序列化
- `toml = "0.8"` — 内置与用户 catalog 解析

### 关键实现决策

1. **JsonlSessionStorage**:
   - `read_all` 使用 `tau_agent::session::jsonl::entry_from_json_line`（包含 v1 迁移）
   - `append`/`append_batch` 通过 `tokio::fs::OpenOptions::append` 以追加模式写入
   - 损坏行将 `SessionJsonlError{line_number}` 传播到 `SessionError::Jsonl`
   - 文件不存在时 `read_all` 返回空 `Vec` 而非错误
   - `append` 自动创建父目录（`tokio::fs::create_dir_all`）

2. **SessionManager**:
   - 项目哈希：`SHA-256(canonical_path)[..12]`（12 个 hex 字符）
   - 目录布局：`~/.tau/sessions/<hash>/<session_id>.jsonl` + `index.jsonl`
   - `create` 使用 `uuid::Uuid::new_v4().simple()` 生成 session id，向 `index.jsonl` 追加一行
   - session 文件**懒创建**（首次 `append` 时才落盘）
   - `list` 逐行扫描各 session 文件返回 entry 计数

3. **Catalog 配置**:
   - `tau-coding` 拥有自己的 `CatalogConfig`/`CatalogProvider` 类型（不依赖 `tau-cli`，避免环形依赖）
   - 内置 catalog 通过 `include_str!("../../data/catalog.toml")` 嵌入（6391 行）
   - `merge_catalogs`：overlay 的重名 provider **整体替换** base 中同 name 条目（对齐 Python catalog loader 的用户可见行为），保留各自独有条目；`schema_version` overlay 优先（0 时回退 base）
   - `ProviderKind` 枚举：按 `kind` + `api` 派遣到 `Anthropic` / `OpenaiCompatible` / `OpenaiResponses`
   - `load_user_or_default`：用户文件缺失时返回纯内置

4. **CLI 集成**:
   - 合并 4 个函数（`print_once_anthropic`、`print_once_openai`、`run_repl_anthropic`、`run_repl_openai`）为统一 `print_once` + `run_repl`
   - 接收 `Arc<dyn ModelProvider + Send + Sync>`；在 main 中根据 provider kind 构造后转为 trait object
   - `open_or_create_session`：使用 `SessionManager::create`，seed 一条 `SessionInfo` 行
   - `persist_message`：追加 `MessageEntry` + `LeafEntry`（对齐 Python 的 journal 形状）
   - print 模式下 session 持久化失败**不中断**主流程（log 后继续）
   - REPL 下先持久化 user 消息，再驱动 `harness.prompt`；assistant 通过 `MessageEnd` 事件捕获并持久化
   - `build_harness` 工厂函数消除重复配置代码

### 测试统计

| 测试套件 | Phase 3 | Phase 4 | 总计 |
|----------|---------|---------|------|
| tau-types 单元测试 | 4 | — | 4 |
| tau-agent 单元测试 | 10 | — | 10 |
| tau-agent 集成测试 | 11 | — | 11 |
| tau-ai 单元测试 | 19 | — | 19 |
| tau-ai 集成测试 | 10 | — | 10 |
| tau-cli 单元测试 | 4 | — | 4 |
| tau-cli 集成测试 | 10 | — | 10 |
| **tau-coding 单元测试** | 43 | 19 | 62 |
| **总计** | **130** | **19** | **149** |
