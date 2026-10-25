# crates/personal

## 功能

提供个人领域的命令实现（集合：`profile`、`actions`），由 `me-server` 通过 `DocumentStore` 调用，写入在 SQLite 事务内完成：

- 个人资料：`profile.update`
- 行动队列：`action.create`、`action.update`、`action.complete`、`action.archive`、`action.queue`
- `action.create` 必须提供已存在的 `checkpoint_id`；处理器在同一事务内从检查点解析所属里程碑/目标并写入 `goals.action_links`，再递增 goals 版本。`checkpoint_id` 只用于建立关联，不写入行动记录。
- 每个行动有 `duration_minutes`（1..600）和 `buffer_minutes`（0..600，弹性时间）。创建时 `buffer_minutes` 默认等于 `duration_minutes`；更新时长且没有显式改弹性时，若弹性仍等于旧时长则跟随新时长。
- 循环行动有 `recurrence`（与旧日历事件相同的频率/间隔/次数/截止/星期规则）、`recurrence_start_at`（首次截止锚点）和 `recurrence_completed_count`。`action.complete` 完成一次：未结束时把 `due_at`/`due_date` 推进到下一次并保留在队列；次数或 `until` 耗尽时标记 done 并移出队列。
- 修改 `recurrence` 会重置完成次数并重新以当前截止为锚点；已开始的循环行动不允许只改截止日期/时间。
- 行动删除：`action.delete` 硬删除行动、队列记录和目标关联。
- 结构迁移与初始化：`migrate`、`init`；旧的 `calendar` 事件集合已废弃。

行动文档（schema v7）包含 `items` 与文档级 `queue`：

- `queue` 是 open、未归档行动的执行顺序，索引 0 为当前行动，索引 1 为下一步。
- 新建行动自动追加到队列；完成、取消或归档会自动移出；重新打开或恢复会重新入队。
- 调整顺序用 `action.queue`（`action_id` + `position`），`expected_version` 用集合版本。
- `personal.migrate` 将旧 `tasks` 集合迁移为 `actions`（保留原版本/队列）；缺失队列按“有截止优先、截止升序、优先级、创建时间”派生。v5 补齐 `duration_minutes=30` 与相等弹性，v7 补齐 `recurrence=null`、`recurrence_completed_count=0` 等循环字段。
- 旧行动 ID（`task-*`）保留，新行动使用 `action-*`。

## 调用协议

所有调用都经 `me-server`：

```text
GET  /api/v1/programs/{program}                         describe 概览
GET  /api/v1/programs/{program}/commands/{command}      单命令契约
POST /api/v1/programs/{program}/commands/{command}      写入
me-client write {program}.<command> --expected-version <n> --set <key>=<value>
```

写入请求使用标准信封（`request_id`、`expected_version`、`input`）；`request_id` 用于幂等重试，`expected_version` 指向集合或记录版本，以 `describe` 为准。成功与失败响应只包含一个 JSON 值，错误在 `error.code` / `error.message`。
