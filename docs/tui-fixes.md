# TUI Bug 修复记录（Phase 7 完善）

> 提交范围：`b1e36ad` → `7f8ddf2`（5 轮修复，6 个 commits）
> 验证：clippy/fmt 零警告，200（默认）/ 205（--features tui）测试全绿

---

## 修复 1 — 输入框不清空 + 退格无效 + 重复发送 + 无法滚动

**提交**: `b1e36ad`

| 问题 | 根因 | 修复 |
|------|------|------|
| **输入框不清空** | `run_prompt` 阻塞外层事件循环，UI 无法重绘 | Enter 时 `mem::take` 清空 input 后立即返回外层循环重绘 |
| **退格无效** | 流式期间内层循环未处理编辑键 | `handle_streaming_key` 包含完整编辑键处理 |
| **重复发送** | `start_prompt` 手动调了 `add_user_message`，adapter 处理 `MessageEnd(User)` 事件时又调了一次 → 加倍 | 删除手动调用，只靠事件驱动 |
| **无法滚动** | `scroll_offset_rows` 未接线；语义错误（`scroll_to_bottom` 设 0 但 ratatui `scroll(0)` = 顶部而非底部） | 新增 `auto_scroll` 标志 + `page_up/page_down`；UI 中 auto-scroll 时自动计算底部偏移；PageUp/PageDown/Up/Down/Ctrl-L 接线 |

**架构修正**: 内层 `run_stream_loop` 复用外层单一 key channel，不再创建第二个 crossterm reader，避免事件竞争。

---

## 修复 2 — 光标不可见 + 光标位置偏差

**提交**: `347a723`

| 问题 | 根因 | 修复 |
|------|------|------|
| **光标不可见** | `setup_terminal()` 调 `hide_cursor()` 后全程隐藏，用户看不到输入位置 | 改为 `show_cursor()`，每帧 `set_cursor_position` 更新位置 |
| **光标位置偏差** | prompt `"› "` 是 Unicode（3 字节 / 1 列宽），cursor 计算用 byte len 而非 display width，光标实际偏移 2 列 | 改为 ASCII prompt `"> "`（2 字节 = 2 列宽） |

---

## 修复 3 — 光标狂晃 + 退格 Unicode panic

**提交**: `b169503`

| 问题 | 根因 | 修复 |
|------|------|------|
| **光标狂晃** | `show_cursor()` 后终端光标在 ratatui 每帧渲染间隙闪烁——这是 TUI 的固有问题 | 恢复 `hide_cursor()`；在 `draw_input` 中用 `Span::styled`（白底黑字块）渲染**自定义光标**，不依赖终端光标 |
| **退格 panic** (`assertion failed: is_char_boundary`) | cursor 用字符计数（+1 / -1），但 `String::insert/remove` 需要 byte position。中文字符（3 字节）：cursor=1 后下次 insert 落在字节中间 → panic | 新增 `cursor_byte_left/right/backspace/delete` 四个 Unicode 安全辅助函数<br>• `Char` 插入后 `cursor += ch.len_utf8()`<br>• `Backspace/L/R/Delete` 均基于 `char_indices()` 定位正确 byte 边界<br>• 光标始终保持在有效 UTF-8 boundary |

---

## 修复 4 — Assistant 消息重复 + Delete 键缺失

**提交**: `53e154b`

| 问题 | 根因 | 修复 |
|------|------|------|
| **Assistant 消息重复** | `finalize_assistant` 中 `flush_assistant_buffer()` 添加增量文本后，`add_assistant_message()` 再添加 canonical 相同文本 → 加倍 | 删除 `finalize_assistant` 中的 `flush_assistant_buffer()` 调用；canonical 消息已包含最终文本 |
| **Delete 键缺失** | 未实现 | 新增 `cursor_byte_delete()`（删除光标后一字符，Unicode 安全）并发线 |

**⚠ 不佳回**: 此修复不完全——`finalize_assistant` 删了 `flush_assistant_buffer` 调用，但 buffer 内容未清空 → 随后 `AgentEnd.flush()` 仍取出并添加为第二条。

---

## 修复 4.5 — 发送后输入框不清空（命令/shell 流错误启动）

**提交**: `262bc93`

| 问题 | 根因 | 修复 |
|------|------|------|
| **发送后不清空** | `dispatch_line` 处理完命令后返回 `Ok(false)`，但 Enter 处理**无条件**继续 `session.prompt(&input)` → `run_stream_loop`，导致命令文本被当作 chat 消息重复发送，输入框无法回到 idle | `dispatch_line` 返回三态 `LineResult` 枚举：`Quit` → 退出 / `Stream` → 启动流 / `Handled` → 命令已处理。`Stream` 分支才启动流，`Handled` 直接回 idle 重绘 |

---

## 修复 5 — 彻底修复重复 + 光标残留 + 输入不清空

**提交**: `7f8ddf2`

| 问题 | 真正根因 | 修复 |
|------|---------|------|
| **Assistant 消息重复（真因）** | 修复 4 只删了 `flush_assistant_buffer` 调用，但 buffer 内容未清空 → 随后的 `AgentEnd.flush()` 仍取出相同文本添加为第二条 | `finalize_assistant` 中加 `self.assistant_buffer.clear()`（增量 buffer 仅作为 AgentEnd 兜底保留） |
| **光标高亮残留** | `Span::raw("")` 空 span 传入 ratatui `Paragraph`，diff 渲染引擎无法正确清理上一帧的光标位置 | 所有 span 添加前检查 `!is_empty()`，不推空 span |
| **输入不清空** | 同上——空输入时 `before=""` 产生空 span，Paragraph 渲染异常导致界面残留旧文本 | 连同修复 |

---

## 完整事件流（修复后）

```
用户输入 "hello" → Enter
  ├─ handle_idle_key: mem::take(input) 清空输入
  ├─ dispatch_line: 返回 LineResult::Stream
  ├─ session.prompt("hello"): 创建流
  └─ run_stream_loop: 内层 select! 循环
      ├─ draw_frame: 显示空输入框
      ├─ stream event → adapter.apply → 滚动到底
      ├─ MessageStart(Assistant) → begin_assistant
      ├─ MessageUpdate(TextDelta) × N → assistant_buffer 累积
      ├─ MessageEnd(Assistant) → finalize_assistant
      │   ├─ truncate(provisional items)
      │   ├─ assistant_buffer.clear()         ← 关键：丢弃增量
      │   └─ add_assistant_message(canonical) ← 唯一一次添加
      └─ AgentEnd → flush() → buffer 已空，无操作
→ 返回 idle: draw_frame 显示空输入框 + 1 条 Assistant 消息
```

---

## 当前 TUI 键位映射

### 空闲状态

| 按键 | 功能 |
|------|------|
| Enter | 发送消息 |
| Esc | 无操作 |
| Ctrl-D | 退出 |
| Ctrl-O | 展开/收起工具结果 |
| Ctrl-T | 显示/隐藏 thinking 块 |
| Ctrl-L | 跳到最新（恢复自动滚动） |
| PageUp / ↑ | 向上滚动（暂停自动滚动） |
| PageDown / ↓ | 向下滚动 |
| Backspace | 删除光标前一字（Unicode 安全） |
| Delete | 删除光标后一字（Unicode 安全） |
| ← → Home End | 光标移动（字节边界安全） |

### 流式运行中

| 按键 | 功能 |
|------|------|
| Enter | Steer（发送 follow-up） |
| Esc / Ctrl-C | 取消当前流 |
| Ctrl-O / Ctrl-T / Ctrl-L | 同空闲 |
| PageUp / ↑ / PageDown / ↓ | 滚动（同空闲） |
| Backspace / Delete / ← → 等 | 编辑 steer 文本 |

---

## 架构约束（已落实）

- TUI 仅依赖 `tau-types` 事件 + `CodingSession` 只读接口
- `AgentHarness` handle 克隆后独立于 `&mut session`，steer/cancel 不占用 session 借用
- 流驱动期间复用单一 crossterm key channel（不创建第二个 reader）
- `feature = "tui"` 默认关闭，无 TUI 构建不拉 ratatui
- 光标追踪全部 Unicode 安全（`char_indices` + byte position）
