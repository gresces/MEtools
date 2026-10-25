# me-server HTTP API

Base URL 示例：`http://127.0.0.1:8787`。

除 `/healthz`、`/`、`/api/v1/auth/login`、`/api/v1/auth/logout` 外，所有 `/api/v1/*` 接口都需要认证：

```http
Authorization: Bearer <password>
```

看板使用登录后设置的 `me_session` HttpOnly cookie。客户端始终使用 Bearer 密码。

所有响应都是单值 JSON。数据读接口与看板聚合支持条件请求：响应带 `ETag`，客户端可发送 `If-None-Match`，内容未变化时返回 `304 Not Modified`。支持 gzip/brotli 压缩。错误响应：

```json
{
  "status": "failed",
  "request_id": "req-...",
  "changed_records": [],
  "new_versions": {},
  "summary": "function request failed",
  "error": {
    "code": "INVALID_INPUT",
    "message": "..."
  }
}
```

认证失败使用简化结构：

```json
{
  "status": "failed",
  "error": {"code": "UNAUTHORIZED", "message": "authentication required"}
}
```

## 1. 健康检查与会话

### `GET /healthz`

无需认证。用于反向代理、systemd 或客户端连通性检查。

```json
{"status":"ok","service":"me-server","version":"0.1.0"}
```

### `POST /api/v1/auth/login`

```json
{"password":"<server password>"}
```

成功时设置：

```http
Set-Cookie: me_session=<password>; HttpOnly; SameSite=Strict; Path=/; Max-Age=2592000
```

响应：

```json
{"status":"succeeded","authenticated":true,"expires_in":2592000}
```

### `POST /api/v1/auth/logout`

清除 cookie。

### `GET /api/v1/auth/session`

需要认证。返回当前会话、服务版本和启动时间。

### `GET /api/v1/status`

需要认证。返回数据库路径（`db`）、认证模式、时区、程序列表和可读资源。

### 看板静态资源

`GET /assets/dashboard.css`、`GET /assets/dashboard.js`、`GET /assets/dashboard-core.js` 无需认证，使用内容哈希查询参数，返回 `Cache-Control: public, max-age=31536000, immutable`；脚本和样式只允许同源加载。

## 2. 命令契约

### `GET /api/v1/programs`

```json
{
  "status": "succeeded",
  "programs": [
    {
      "name": "personal",
      "summary": "...",
      "collections": ["profile", "actions"],
      "commands": [{"name":"action.create","summary":"..."}]
    }
  ]
}
```

### `GET /api/v1/programs/{program}`

包含命令列表、读取/写入的集合名、版本规则和错误码。

### `GET /api/v1/programs/{program}/commands/{command}`

等价于 `describe <command>`，包含字段、规则、示例 JSON 请求和示例字段调用。

## 3. 写入

### `POST /api/v1/programs/{program}/commands/{command}`

请求头：

```http
Authorization: Bearer <password>
Content-Type: application/json
X-ME-CSRF: 1                         # 仅 cookie 认证的看板写入需要
Idempotency-Key: req-20261020-001   # 可选；body 没有 request_id 时使用
```

带 `me_session` cookie 的写入必须发送 `X-ME-CSRF: 1`。如果请求带 `Origin`，其 authority 必须与 `Host` 一致，否则返回 `403 FORBIDDEN`。Bearer 客户端不受此限制。

标准请求体：

```json
{
  "request_id": "req-20261020-001",
  "expected_version": 42,
  "input": {
    "title": "Write docs"
  }
}
```

也接受省略 `input` 的平铺形式：

```json
{
  "request_id": "req-20261020-001",
  "expected_version": 42,
  "title": "Write docs"
}
```

`request_id` 可以省略，服务端生成 `req-<uuid>`。`expected_version` 必须是非负整数。

成功响应直接使用 domain handler 的结果，并返回 `x-me-request-id` 响应头。常见字段：

```json
{
  "status": "succeeded",
  "request_id": "req-...",
  "changed_records": ["action-..."],
  "new_versions": {"action-...": 2},
  "collection_version": 43,
  "summary": "create_action: action-..."
}
```

### 版本语义

不同命令的 `expected_version` 指向不同对象，必须以 `describe` 为准：

| 命令 | 版本来源 |
| --- | --- |
| `personal.action.create` | `actions` 集合顶层 `version`；同一事务读取/写入 `goals`，响应 `new_versions.goals` 返回关联后的 goals 版本 |
| `personal.action.update` / `action.complete` / `action.archive` / `action.delete` | 行动记录 `version` |
| `personal.profile.update` | profile 顶层 `version` |
| `personal.migrate` | 当前 `actions` 或旧 `tasks` 集合顶层 `version` |
| `goal.migrate` | `goals` 顶层 `version` |
| `health.body.update` / `water.update` / `sleep.update` | 记录 `version` |
| 其他 `health.*` | `health` 集合顶层 `version` |
| `life.clothes.*`（除 `wash`、`add`） | 衣物记录 `version` |
| `life.clothes.add` / `clothes.wash` | `life` 集合顶层 `version` |
| `goal.*` | `goals` 集合顶层 `version` |

### 个人资料键值

`personal.profile.update` 的 input 为：

```json
{
  "patch": {"occupation": "Engineer", "skills": ["Rust"], "age": 30, "remote": true},
  "remove": ["old_key"]
}
```

- `patch` 与 `remove` 至少一个非空；同一键不能同时出现。
- 值可以是任意 JSON；`null` 会作为值保存，删除只能通过 `remove`。
- 键长度 1–64 字符，不能有首尾空白或控制字符；单值序列化后不超过 16 KiB，资料最多 256 个用户键。
- `version`、`updated_at`、`updated_by`、`recent_requests` 为保留键，不可写、不可删。
- `display_name`、`timezone` 必须是非空字符串，且不能删除。
- 删除不存在的键是无操作。

### 幂等

- 同一 `request_id` + 同一操作 + 同一 `expected_version` + 同一 input 会重放上次成功回执。
- 同一 `request_id` 但内容不同会返回 `INVALID_INPUT`。
- 回执保留在每个集合文档最近 128 条。
- 网络超时后重试应保留原 `request_id`。

## 4. 读取

### `GET /api/v1/versions/{resource}`

只返回集合版本元数据，不返回集合正文。客户端在写入前获取 `expected_version` 时使用该接口，避免为了一个版本号下载整个集合。

| 参数 | 说明 |
| --- | --- |
| `path` | 记录数组的点分对象路径，例如 `items`、`body.records`；默认 `items` |
| `id` | 记录 ID；提供时返回该记录的 `record_version` |

不传 `id`：

```json
{
  "status": "succeeded",
  "resource": "actions",
  "version": 42,
  "updated_at": "2026-10-20T22:00:00+08:00"
}
```

传 `path` + `id`：

```text
GET /api/v1/versions/actions?path=items&id=action-...
```

```json
{
  "status": "succeeded",
  "resource": "actions",
  "version": 42,
  "updated_at": "2026-10-20T22:00:00+08:00",
  "record_id": "action-...",
  "record_version": 3
}
```

记录不存在返回 `404 NOT_FOUND`；`path` 存在但缺少 `id` 返回 `400 INVALID_INPUT`。写入仍需在 POST 中携带 `expected_version`，并发语义不变。

### `GET /api/v1/data/{resource}`

`resource`：

| 资源 | 集合 |
| --- | --- |
| `profile` | `profile` |
| `actions` | `actions` |
| `health` | `health` |
| `life` | `life` |
| `goals` | `goals` |
| `projects` | `projects`（`items` 数组） |

响应：

```json
{
  "status": "succeeded",
  "resource": "actions",
  "version": 42,
  "updated_at": "2026-10-20T22:00:00+08:00",
  "total": 12,
  "count": 5,
  "data": {"items": ["..."]}
}
```

`projects` 返回 `data` 数组。

默认不返回 `recent_requests`。需要调试幂等回执时：

```text
GET /api/v1/data/actions?include_receipts=true
```

### actions 查询参数

| 参数 | 说明 |
| --- | --- |
| `status` | 逗号分隔，例如 `open,in_progress` |
| `tag` | 精确匹配标签 |
| `priority` | `urgent` / `high` / `normal` / `medium` / `low` |
| `include_archived` | `true` 时包含已归档 |
| `limit` | 1..1000，默认 200 |

`total` 是过滤后、截断前的数量；`count` 是实际返回数量。

`data.queue` 是 open、未归档行动的执行顺序（索引 0 为当前行动）；过滤参数只作用于 `data.items`，不改变 `data.queue`。

每个行动包含 `duration_minutes`（1..600）、弹性 `buffer_minutes`（0..600）和可选 `recurrence`。创建时 `buffer_minutes` 默认等于 `duration_minutes`；`action.update` 只改时长且未显式给弹性时，若弹性仍等于旧时长则跟随新时长。循环行动还带 `recurrence_start_at`、`recurrence_completed_count` 和 `recurrence_last_completed_at`；`action.complete` 完成一次并推进 `due_at`/`due_date`，次数或 `until` 耗尽时标记 done。行动文档 schema v7；旧库需要先执行 `personal.migrate`。

`goals` 文档（schema v2）包含：

- `goals[]`：`id`、`parent_goal_ids`（可嵌套、必须无环）、`title`、`purpose`、`constraints`、`horizon`、`success_criteria`、`created_at`、`milestone_ids` 等；
  - `parent_goal_ids` 为空表示目标分组，非空表示具体目标。分组同样是 `goals[]` 记录，不单独建集合，可包含子目标、里程碑和检查点，并支持多层嵌套。
  - 新建具体目标时 `parent_goal_ids` 必须非空且引用已存在的目标；新建顶层目标分组时必须显式传 `group=true`，且 `parent_goal_ids` 为空。`group` 只用于创建，不写入记录。
  - 删除目标分组会级联删除整棵子树（子目标、里程碑、检查点和行动关联）；被关联的行动本身保留。
- `milestones[]`：`goal_id`、`action_ids`、`exit_criteria`、`dependencies`；
- `checkpoints[]`：`milestone_id`、小时级步骤、`estimate_minutes`；
- `action_links[]`：`action_id`、`goal_ids`、`goal_id`、`milestone_id`、`checkpoint_id`、`role`、`contribution`。
  - `personal.action.create` 必须提供已存在的 `checkpoint_id`；服务端从检查点解析所属里程碑和目标，并在同一事务写入 `action_links`（`goal_ids` 为空、`role=core`、`contribution` 为空）。`checkpoint_id` 不写入行动记录。

旧 ID（`direction-*`、`current-*`、`task-*`）保留；新记录使用 `goal-*`、`checkpoint-*`、`action-*`。

### `GET /api/v1/dashboard/summary`

看板聚合视图。主要字段：

```json
{
  "status": "succeeded",
  "generated_at": "2026-10-20T22:00:00+08:00",
  "today": "2026-10-20",
  "timezone_offset_hours": 8,
  "profile": {
    "display_name": "...",
    "timezone": "Asia/Shanghai",
    "occupation": "Engineer",
    "skills": ["Rust", "SQLite"]
  },
  "versions": {
    "profile": 5, "actions": 177,
    "health": 3, "life": 19, "goals": 126
  },
  "actions": {
    "total": 87, "open": 85, "overdue": 3, "due_today": 1,
    "done_7d": 2,
    "focus": ["行动对象"]
  },
  "goals": {
    "active_goals": 4,
    "average_progress": 0,
    "milestones_due_soon": 1,
    "top": ["目标摘要"],
    "upcoming_milestones": ["里程碑摘要"]
  },
  "health": {
    "water": {"today_ml": 0, "goal_ml": 2000, "ratio": 0.0, "series": []},
    "sleep": {"last_night_minutes": null, "goal_minutes": 420, "quality": null, "series": []},
    "body": null
  },
  "life": {
    "total": 9, "clean": 9, "needs_wash": 0, "wearing": 0,
    "items_to_wash": [], "items_wearing": []
  },
  "projects": {"active": 0, "total": 0, "items": []}
}
```

`profile` 包含所有非保留资料键值，形状由用户数据决定；`version`、`updated_at`、`updated_by`、`recent_requests` 不会出现在 summary 中。

`goals.active_goals`、`goals.average_progress` 和 `goals.top` 只统计非分组目标：`parent_goal_ids` 为空的目标视为目标分组，不计入目标数。

## 5. 错误码

| 业务码 | HTTP | 说明 |
| --- | --- | --- |
| `INVALID_INPUT` | 400 | 字段、JSON、版本号格式错误 |
| `UNAUTHORIZED` | 401 | 密码缺失或错误 |
| `NOT_FOUND` | 404 | 资源、记录、程序或命令不存在 |
| `VERSION_CONFLICT` | 409 | `expected_version` 与当前版本不一致 |
| `WRITE_BUSY` | 423 | 写锁超时 |
| `DATA_INVALID` | 500 | 数据文件损坏或不可读写 |
| `INTERNAL_ERROR` | 500 | 服务端内部错误 |

退出码与错误码的客户端映射见 [CLIENT.md](CLIENT.md#6-退出码与错误)。
