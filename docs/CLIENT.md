# me-client 使用说明

`me-client` 是 ME agent 访问 `me-server` 的命令行客户端。它负责：

- 读取数据（profile、actions、health、life、goals、projects）。
- 查询服务端命令契约（`programs`、`describe`）。
- 通过任意 program command 写入。
- 为常用行动操作自动获取版本并生成 `request_id`。

标准输出始终是一个 JSON 值；诊断信息写到标准错误。默认紧凑输出，加 `--pretty` 便于人工阅读。

## 1. 安装与配置

```bash
cargo build --release -p me-client
install -m 0755 target/release/me-client ~/.local/bin/me-client
```

最小配置：

```bash
export ME_SERVER_URL="http://127.0.0.1:8787"
export ME_SERVER_TOKEN="<server password>"
```

更推荐使用密码文件（客户端参数名仍是 token）：

```bash
install -d -m 0700 ~/.config/me
printf '%s\n' '<server password>' > ~/.config/me/server.password
chmod 600 ~/.config/me/server.password

cat > ~/.config/me/client.json <<'JSON'
{
  "server": "http://127.0.0.1:8787",
  "token_file": "/home/me/.config/me/server.password",
  "timeout_secs": 30
}
JSON
```

服务端凭据是密码；本机默认值为 `password`，未配置密码的本地服务可以直接使用它。客户端沿用 `--token` / `ME_SERVER_TOKEN` / `token_file` 这些名称，填入的值就是服务端密码。

配置文件按以下顺序查找，使用第一个存在的文件：

1. `$ME_CLIENT_CONFIG`
2. 当前目录 `.me-client.json`
3. `$XDG_CONFIG_HOME/me/client.json`；未设置 `XDG_CONFIG_HOME` 时使用 `~/.config/me/client.json`

字段优先级：

| 字段 | 优先级（高 → 低） |
| --- | --- |
| `server` | `--server` > `ME_SERVER_URL` > 配置文件 `server` > `http://127.0.0.1:8787` |
| `token` | `--token` > `--token-file` / 配置文件 `token_file` > 配置文件 `token` > `ME_SERVER_TOKEN` |
| `timeout` | `--timeout` > 配置文件 `timeout_secs` > 30 秒 |

`--timeout` 和 `timeout_secs` 会被限制在 1–600 秒。

## 2. 连通性

```bash
me-client ping
```

`ping` 调用 `/healthz`，不需要密码。确认认证是否正常：

```bash
me-client status
me-client programs
```

## 3. 读取数据

```bash
me-client actions
me-client actions --status open
me-client actions --status open --tag work --priority high --limit 20
me-client actions --all
me-client get profile
me-client get health
me-client get life
me-client get goals
me-client get projects
me-client summary
```

- `actions` 默认返回未归档行动，单次最多 200 条；响应中的 `total` 是过滤后的总数，`count` 是实际返回数。用 `--limit` 调整，`--all` 包含已归档。
- `--status` 支持逗号分隔：`--status open,in_progress`。
- `get <resource>` 返回原始 JSON 包装：

  ```json
  {
    "status": "succeeded",
    "resource": "actions",
    "version": 42,
    "updated_at": "...",
    "total": 10,
    "count": 10,
    "data": { "...": "..." }
  }
  ```

- 读取默认不返回内部 `recent_requests` 幂等回执。需要排查时加：

  ```bash
  me-client get actions --query include_receipts=true
  ```

- `summary` 是看板使用的聚合视图，适合快速了解状态；原始数据仍以 `get` 为准。

## 4. 查询命令契约

写之前先看契约，不要猜字段：

```bash
me-client describe personal
me-client describe personal action.create
me-client describe goal progress.record
me-client describe health water.log
me-client describe life clothes.wash
```

也可以列出全部 program：

```bash
me-client programs
```

`describe <program> <command>` 会返回字段、必填项、规则、示例 JSON 请求和示例字段调用。字段约束以服务端当前版本为准。

## 5. 写入

### 5.1 通用 write

```bash
me-client write <program>.<command> \
  --request-id "req-20261020-001" \
  --expected-version 42 \
  --set key=value \
  --set patch.status=done
```

- `<program>.<command>` 在第一个点处拆分：`personal.action.create` 表示 program=`personal`、command=`action.create`。
- `--input '<json-object>'` 传入完整 input 对象；`--input-file path.json` 从文件读取。
- `--set key=value` 可重复；支持点分嵌套路径，例如 `--set patch.status=done`。
- 值会先按 JSON 字面量解析：`true`、`false`、`null`、数字、数组、对象、带引号字符串；否则按普通字符串处理。例如：

  ```bash
  --set tags='["work","urgent"]'
  --set due_date=2026-11-01
  --set notes='contains spaces'
  ```

- `--expected-version` 必填；除非使用 `--auto-version`。
- `--auto-version` 会先通过 `/api/v1/versions/{resource}` 读取轻量版本元数据（不下载集合正文），再发送一次写入；不会自动重试。并发写入仍可能返回 `VERSION_CONFLICT`。
- `personal.migrate` 是唯一的例外：必须显式传 `--expected-version`，避免误迁移。
- 版本探针需要当前版本的 server；客户端与 server 应一起升级，否则 `--auto-version` 会收到 `404 NOT_FOUND`。
- `--request-id` 省略时自动生成。网络超时后如果要安全重试，应复用同一个 `request_id`。
- 服务端所有 program command（当前 42 个）都可用 `write` 调用；快捷命令只是常用场景的封装，不是完整的命令白名单。

### 5.2 示例

创建行动（集合版本）：

```bash
me-client write personal.action.create \
  --auto-version \
  --set title="写客户端文档" \
  --set checkpoint_id=checkpoint-xxxx \
  --set duration_minutes=30 \
  --set priority=high \
  --set due_date=2026-11-01 \
  --set tags='["me","docs"]'
```

行动时长由 `duration_minutes`（1..600）表示，弹性时间由 `buffer_minutes`（0..600）表示；不传弹性时默认等于时长。`action.update` 只改时长且未显式给弹性时，若弹性仍等于旧时长，会跟随新时长。

更新行动（记录版本）：

```bash
me-client write personal.action.update \
  --auto-version \
  --set action_id=action-xxxx \
  --set patch.status=done
```

记录目标进度（goals 顶层版本）：

```bash
me-client write goal.progress.record \
  --auto-version \
  --set target=goal \
  --set target_id=goal-portfolio \
  --set kind=artifact \
  --set summary="完成客户端接入文档" \
  --set evidence=docs/CLIENT.md \
  --set progress_after=35
```

更新个人资料（键值自由，值支持 JSON）：

```bash
me-client write personal.profile.update \
  --auto-version \
  --set patch.display_name="Agent 开发者" \
  --set patch.occupation="Engineer" \
  --set 'patch.skills=["Rust","SQLite"]'
```

删除资料键：

```bash
me-client write personal.profile.update \
  --auto-version \
  --input '{"remove":["occupation"]}'
```

`--set` 按点号拆分路径；键本身包含点号时，用 `--input` 传完整 JSON。`display_name` 和 `timezone` 不能删除。

记录饮水（先看 `describe health water.log` 确认字段）：

```bash
me-client write health.water.log \
  --auto-version \
  --set date=2026-10-20 \
  --set amount_ml=300 \
  --set beverage=water
```

记录衣物穿着：

```bash
me-client write life.clothes.wear \
  --auto-version \
  --set item_id=cloth-xxxx
```

创建循环行动：

```bash
me-client action add \
  --title "每日复盘" \
  --checkpoint-id checkpoint-xxxx \
  --due-date 2026-10-25 \
  --duration-minutes 30 --buffer-minutes 30 \
  --recurrence '{"frequency":"daily","count":8}'
```

循环行动保持一条记录；`me-client action complete action-xxxx` 完成本次并推进到下一次，次数耗尽后自动结束。修改 `--recurrence` 会重置完成次数。

### 5.3 行动快捷命令

快捷命令会自动读取集合版本或记录版本，并生成 `request_id`。如果需要在网络超时后严格复用同一个请求，请改用通用 `write` 并显式传 `--request-id`。

```bash
me-client action list --status open
me-client action add --title "整理实验记录" --checkpoint-id checkpoint-xxxx --duration-minutes 30 --buffer-minutes 30 --priority high --due-date 2026-11-01 --tag work --tag experiment
me-client action update action-xxxx --notes "补充失败样本" --priority normal
me-client action complete action-xxxx
me-client action done action-xxxx
me-client action reopen action-xxxx
me-client action queue action-xxxx 0
me-client action archive action-xxxx
me-client action restore action-xxxx
me-client action delete action-xxxx
```

`action update` 支持的字段：

```text
--title --notes --priority --status --due-date --due-at --tag --clear-due --duration-minutes --buffer-minutes --recurrence
```

- `--due-date` 与 `--due-at` 互斥。
- `--recurrence` 接受 JSON 对象；`null` 清除循环。修改重复规则会重置完成次数。
- `--clear-due` 会同时清除 `due_date` 和 `due_at`。
- `--tag` 可重复；提供时会替换原标签数组。
- `action queue <id> <position>` 使用 actions 集合版本；`position=0` 是当前行动，只有 open、未归档行动可以入队。
- `action delete` 使用行动记录版本；会删除行动并移除目标关联，不可恢复。需要留档时先运行 `me-client get actions --all --limit 1000 > actions-backup.json`。

### 5.4 目标与层级命令

四层结构：目标分组/Goal → 里程碑 → 检查点 → 行动。创建具体目标时必须提供非空 `parent_goal_ids`；顶层目标分组必须显式传 `group=true` 且父级为空。行动必须通过 `checkpoint_id` 挂在已存在的检查点下，服务端会在同一事务写入关联。旧 `direction`、`current_goal`、`task_link` 命令已删除。

```bash
# 目标分组（group=true，parent_goal_ids 为空）
me-client write goal.goal.create --auto-version \
  --set id=goal-career \
  --set group=true \
  --set title="职业发展" \
  --set horizon=years \
  --set success_criteria='["完成一段实习"]'

# 具体目标：必须挂到已存在的目标分组或其它目标下
me-client write goal.goal.create --auto-version \
  --set id=goal-portfolio \
  --set parent_goal_ids='["goal-career"]' \
  --set title="作品集" \
  --set success_criteria='["发布两个项目"]'

me-client write goal.milestone.create --auto-version \
  --set goal_id=goal-portfolio \
  --set title="完成第一个项目" \
  --set exit_criteria='["可演示"]' \
  --set due_at=2026-12-31

me-client write goal.checkpoint.create --auto-version \
  --set milestone_id=milestone-first \
  --set title="写接口边界说明" \
  --set estimate_minutes=120

# 行动创建时自动关联检查点（同时带出里程碑和目标）
me-client write personal.action.create --auto-version \
  --set title="写接口边界说明" \
  --set checkpoint_id=checkpoint-xxxx

# 需要改关联时才用 action_link.set
me-client write goal.action_link.set --auto-version \
  --set action_id=action-xxxx \
  --set goal_id=goal-portfolio \
  --set milestone_id=milestone-first \
  --set checkpoint_id=checkpoint-xxxx \
  --set contribution="补充接口文档"

me-client goal progress goal-portfolio \
  --percent 35 \
  --summary "完成客户端接入文档" \
  --evidence docs/CLIENT.md \
  --kind artifact
```

`goal progress` 调用 `goal.progress.record`，会同时更新目标当前进度和 `last_progress_at`，并追加 `progress_events` 证据。旧 ID 可以继续使用；新记录由服务端生成 `goal-*`、`checkpoint-*`、`action-*`。

## 6. 退出码与错误

| 退出码 | 含义 |
| --- | --- |
| `0` | 成功 |
| `2` | 参数错误、服务端 4xx、记录不存在 |
| `4` | `VERSION_CONFLICT` / `TIME_CONFLICT` |
| `5` | `UNAUTHORIZED` / `FORBIDDEN` |
| `6` | 服务端 5xx 或响应解析失败 |
| `7` | 网络错误、连接失败、超时 |

失败时 stdout 仍是 JSON：

```json
{
  "status": "failed",
  "request_id": "req-...",
  "error": {
    "code": "VERSION_CONFLICT",
    "message": "version has changed"
  }
}
```

处理规则：

- `VERSION_CONFLICT`：重新读取目标资源/记录，确认新状态后重试。
- `WRITE_BUSY`：短暂等待后重试，保持同一个 `request_id`。
- `UNAUTHORIZED`：检查密码文件、环境变量和服务端密码是否一致。
- `NETWORK_ERROR`：确认服务器可达；如果请求可能已经到达服务端，先用 `request_id` 重试，避免生成新请求。

## 7. ME agent 接入方式

ME 中的读取和写入应统一替换为 client 调用：

```text
读取事实：me-client get <resource>
读取聚合：me-client summary
查询契约：me-client describe <program> <command>
写入数据：me-client write <program>.<command> ...
```

推荐流程：

1. 先 `me-client summary` 或 `me-client get <resource>` 获取当前状态和版本。
2. 写之前 `me-client describe <program> <command>` 确认字段。
3. 优先使用 `--auto-version`；如果 agent 需要显式控制并发，则把读到的版本传给 `--expected-version`。
4. 使用通用 `write` 时，为可重试操作保留 `--request-id`。
5. 每次写入后重新读取受影响的资源，验证 `new_versions` 和实际数据。
6. 不在 ME 本地直接编辑数据库；服务端是唯一权威副本。

### 从旧 ME 数据迁移

迁移旧数据时，读取仍必须使用旧 ME 仓库的方法（旧 workflow、旧脚本或直接读取 `Data/**/*.json`）；写入必须使用 `me-client write`。顺序是“旧方法读取原始记录 → `me-client describe` 确认字段 → `me-client write` 写入 → 回读核对 → 最后删除旧 `Data/`”。

不要用 `me-server import`、SQL 或手工复制 JSON 代替迁移流程。完整步骤见 [ME_MIGRATION.md](ME_MIGRATION.md)。

### 数据来源

`me-client` 只通过 HTTP 访问 `me-server`；服务端数据存放在 SQLite 文档库中（默认 `~/.local/share/me/me.db`）。不要直接读取或编辑数据库文件，也不要在 ME 仓库里保留可写的 `Data/` 副本。

**写入后验证**：重新读取受影响的资源，确认 `new_versions` 和实际内容。

## 8. 安全建议

- 使用 `--token-file` 或 `ME_SERVER_TOKEN_FILE`（值为服务端密码），避免密码出现在 shell history 和进程列表。
- 密码文件权限设为 `600`。
- 通过公网访问时必须在反向代理上启用 HTTPS，并给服务端加 `--secure-cookie`。
- 不要把数据库文件、密码文件、`.me-client.json` 提交到公开仓库。
- 看板的写入同样必须经过命令 API（session cookie + `X-ME-CSRF`）；不要尝试在浏览器里直接构造写请求或修改数据库绕过版本控制。
