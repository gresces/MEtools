# crates/goal

## 功能

提供个人人生规划的命令实现（集合：`goals`），由 `me-server` 通过 `DocumentStore` 调用；写入在 SQLite 事务内完成，回执保留最近 128 条：

- **初始化**：`init`：创建或校验 `goals` 集合（schema v2），默认时区 `Asia/Shanghai`、回顾周期 `weekly`。
- **迁移**：`migrate`：把 v1 文档迁移到 v2；`directions` 转为顶层 Goal（目标分组，`parent_goal_ids` 为空），`direction_ids` 转为 `parent_goal_ids`，`current_goals` 转为 `checkpoints`，`task_links` 转为 `action_links`。

目标分四层，UI 与命令使用同一套名字：

| 层级 | 记录 | 时间尺度 |
| --- | --- | --- |
| Goal（目标分组/目标） | `goals`（`parent_goal_ids` 可嵌套） | 任意 horizon（年/月/长期） |
| 里程碑 | `milestones` | 周/月 |
| 检查点 | `checkpoints` | 小时（1..600 分钟） |
| 行动 | `actions` + `actions.queue` | ≤30 分钟，原子步骤 |

- **目标分组**：`parent_goal_ids` 为空的顶层 Goal 是目标分组，非空是具体目标；分组可包含子目标、里程碑和检查点，并支持多层嵌套。分组用于归类，不计入活跃目标统计；删除分组会级联删除整棵子树，行动本身保留。
- **Goal**：`goal.create`、`goal.update`；必须有非空 `success_criteria`。具体目标的 `parent_goal_ids` 必须非空且引用已有 Goal；顶层目标分组必须传 `group=true` 且 `parent_goal_ids` 为空，二者不能同时满足。层级必须无环；`goal.update` 的 `recalculate_progress=true` 按关联里程碑平均进度重算，并置 `progress_basis=milestone_weighted`。
- **里程碑（Milestones）**：`milestone.create`、`milestone.update`；必须有非空 `exit_criteria`；创建时自动加入所属 Goal 的 `milestone_ids`；修改 `goal_id` 会自动迁移归属。
- **检查点（Checkpoints）**：`checkpoint.create`、`checkpoint.update`；必须挂在某个 milestone 下，`estimate_minutes` 为 1..600；`status` 为 `planned`/`active`/`done`/`cancelled`。
- **行动关联（Action Links）**：`personal.action.create` 创建行动时按 `checkpoint_id` 自动写入 `goal_id`/`milestone_id`/`checkpoint_id` 关联；`action_link.set` 按 `action_id` 替换关联，`action_link.remove` 删除关联；`role` 为 `core`/`supporting`/`maintenance`；`goal_ids`/`goal_id`/`milestone_id`/`checkpoint_id` 可选，但至少一个；`checkpoint_id` 必须属于给出的 `milestone_id`/`goal_id`。
- **定期回顾**：`review.set` 更新 `review.cadence` 与 `review.next_review_at`；回顾时间是提醒，不是行动截止时间。
- **进度证据**：`progress.record` 向 Goal 或里程碑追加 `progress_events`（`at`/`kind`/`summary`/`evidence`/`progress_after`），并同步当前 `progress` 与 `last_progress_at`。
- **删除**：`goal.delete` 级联删除子目标、里程碑、检查点和行动关联；`milestone.delete` 级联检查点和关联；`checkpoint.delete` 删除检查点和关联。被关联的行动本身不删除。

## 数据边界

- `goals` 集合是人生规划的事实源，记录“为什么做、要达到什么结果、当前推进到哪里”。
- `actions` 集合是行动的事实源，记录“具体做什么、需要多久、什么时候完成、行动本身处于什么状态”。
- `goals` 集合不复制行动标题、时长或状态；只保存 `milestone.action_ids` 与 `action_links` 中的行动 ID。
- `actions` 集合存在时，引用的行动 ID 必须真实存在，否则返回 `NOT_FOUND`；集合不存在时跳过行动交叉校验。
- `status` 表示生命周期，`health` 表示推进健康度，`progress` 表示 0–100 的完成程度，`next_action` 表示当前最小可执行动作。
- `paused`、`achieved`、`abandoned`、`completed` 不作为默认驱动力，只用于解释历史。
- 目标或里程碑的最新进展以当前 `progress` 与 `last_progress_at` 为准；`progress_events` 只是审计证据。
- 所有引用必须指向已存在的 Goal、里程碑或检查点；`action_links` 同时给出 `goal_id` 与 `milestone_id` 时，里程碑必须属于该 Goal。
- 层级引用必须无环；旧 ID 保留（`direction-*`、`current-*`、`task-*`），新记录使用 `goal-*`、`checkpoint-*`、`action-*`。

## 调用协议

所有调用都经 `me-server`：

```text
GET  /api/v1/programs/{program}                         describe 概览
GET  /api/v1/programs/{program}/commands/{command}      单命令契约
POST /api/v1/programs/{program}/commands/{command}      写入
me-client write {program}.<command> --expected-version <n> --set <key>=<value>
```

写入请求使用标准信封（`request_id`、`expected_version`、`input`）；`request_id` 用于幂等重试，`expected_version` 指向集合或记录版本，以 `describe` 为准。成功与失败响应只包含一个 JSON 值，错误在 `error.code` / `error.message`。
