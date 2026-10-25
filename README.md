# MEtools

ME 的个人数据服务。数据存放在一个 SQLite 文档库中，唯一写入进程是 `me-server`；agent 通过 `me-client` 访问，服务端同时提供可编辑看板和自描述命令接口。

```text
┌──────────────────────┐        ┌──────────────────────────────┐
│ ME agents            │        │ Browser                      │
│ me-client (CLI)      │        │ dashboard (embedded HTML)    │
└──────────┬───────────┘        └──────────────┬───────────────┘
           │ HTTP + Bearer 密码                 │ HTTP + session cookie
           └────────────────┬───────────────────┘
                            ▼
                 ┌─────────────────────┐
                 │ me-server (axum)    │
                 │  /api/v1/programs   │  self-describing commands
                 │  /api/v1/data       │  read API
                 │  /api/v1/dashboard  │  aggregated view model
                 │  /                     dashboard HTML
                 └──────────┬──────────┘
                            │ in-process command handlers
          ┌─────────────────┼──────────────────┐
          ▼                 ▼                  ▼
     personal            health/life          goal
          └─────────────────┴──────────────────┘
                            │
              me-store (SQLite documents)
        BEGIN IMMEDIATE + version + idempotent receipts
```

## 核心变化

- `me-server` 是唯一的写入进程，数据只存在于 `~/.local/share/me/me.db`（或 `--db` 指向的路径）。
- `me-client` 是给 ME agent 使用的命令行客户端；读数据、查命令契约、写数据都通过 HTTP。
- 看板在 `/`，支持行动（含重复行动）、Goal/里程碑/检查点和资料的受控编辑；日程页按层级与行动日期展示时间安排。
- 写请求使用 `request_id`、`expected_version` 和一个 SQLite 事务；最近 128 条幂等回执保存在文档内。
- domain 只依赖 `DocumentStore` 抽象，SQLite 实现集中在 `me-store`；WASI 文件写入通道已删除。

## 快速开始

### 1. 构建

```bash
cargo build --release -p me-server -p me-client
```

产物：

```text
target/release/me-server
target/release/me-client
```

### 2. 初始化数据库

默认库路径是 `$XDG_DATA_HOME/me/me.db`，回落 `~/.local/share/me/me.db`。

新库：

```bash
./target/release/me-server init
./target/release/me-server check
```

`check`、`backup`、`serve` 只打开已存在的库；路径写错会报 `NOT_FOUND` 而不是创建空库。

已有旧的 `Data/` 目录时，不要用 `me-server import` 完成迁移。先启动服务并配置好 `me-client`，再按 [docs/ME_MIGRATION.md](docs/ME_MIGRATION.md) 的“旧读新写”流程：

1. 用旧方法读取 `Data/`，把原始记录保存到 `Core/runs/<run-id>/old-data/`。
2. 用 `me-client describe` 确认命令字段，再用 `me-client write` 逐条写入服务端。
3. 用 `me-client get` / `summary` 回读并和旧读取结果核对。
4. 核对通过后才删除旧 `Data/`。

`me-server import` 仅用于离线恢复和旧版本兼容，不能替代“旧方法读取 + `me-client` 写入”的迁移流程。如果旧 `Data/` 已经被删除，先从 git 历史或备份恢复只读副本，再按上述步骤迁移。

已有 schema v3/v4/v5/v6 的数据库（旧 `tasks`/`current_goals`/`task_links`，或尚未补行动循环字段）先备份，再在服务运行时执行 schema 升级；v5 为旧行动补 `duration_minutes=30` 和相等的 `buffer_minutes`，v7 补 `recurrence`/`recurrence_completed_count` 等循环字段。旧的 `calendar` 事件集合不再读写，可在备份后删除。旧 ME 数据迁移仍按上面的“旧读新写”流程：

```bash
./target/release/me-server backup --to ~/me-before-actions.db
./target/release/me-client write personal.migrate --expected-version <tasks version> \
  --set collection=actions --set target_schema_version=7
./target/release/me-client write goal.migrate --expected-version <goals version> \
  --set target_schema_version=2
```

`personal.migrate` 删除旧 `tasks` 集合；`goal.migrate` 删除 `directions`/`current_goals`/`task_links`，旧记录 ID 保留。

### 3. 启动并登录

本机默认密码是 `password`：

```bash
./target/release/me-server serve --bind 127.0.0.1:8787
```

打开 <http://127.0.0.1:8787/>，输入 `password` 即可查看看板。

要换密码（推荐）：

```bash
PASSWORD_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/me/server.password"
mkdir -p "$(dirname "$PASSWORD_FILE")"
./target/release/me-server password > "$PASSWORD_FILE"
chmod 600 "$PASSWORD_FILE"

./target/release/me-server serve \
  --bind 127.0.0.1:8787 \
  --password-file "$PASSWORD_FILE"
```

### 4. 使用客户端

```bash
export ME_SERVER_URL=http://127.0.0.1:8787
# 客户端参数名仍是 token，值是服务端密码。
export ME_SERVER_TOKEN_FILE="$PASSWORD_FILE"

./target/release/me-client ping
./target/release/me-client summary
./target/release/me-client actions --status open
# 行动必须挂在已存在的检查点下；先按 docs/CLIENT.md 5.4 创建目标分组/目标/里程碑/检查点
./target/release/me-client action add --title "升级 ME 客户端" --checkpoint-id checkpoint-cli --priority high
```

完整的客户端说明见 [docs/CLIENT.md](docs/CLIENT.md)。

## 配置

### 服务端

| 参数 / 环境变量 | 默认值 | 说明 |
| --- | --- | --- |
| `--bind` / `ME_BIND` | `127.0.0.1:8787` | 监听地址 |
| `--db` / `ME_DB` | `$XDG_DATA_HOME/me/me.db` | SQLite 文档库路径 |
| `--password` / `ME_SERVER_PASSWORD` | `password` | 看板与 API 密码；默认值只用于本机监听 |
| `--password-file` / `ME_SERVER_PASSWORD_FILE` | 无 | 密码文件，读取后去除空白 |
| `--no-auth` | 关闭 | 关闭认证；只允许本机监听，除非同时 `--allow-insecure` |
| `--utc-offset-hours` / `ME_UTC_OFFSET_HOURS` | `8` | 看板“今天”的时区偏移 |
| `--session-ttl-hours` / `ME_SESSION_TTL_HOURS` | `720` | 看板 cookie 有效期 |
| `--secure-cookie` / `ME_SECURE_COOKIE` | 关闭 | 通过 HTTPS 访问时给 cookie 加 `Secure` |

默认密码 `password` 只在 loopback 绑定下可用；绑定非 loopback 地址时必须显式设置 `--password`/`--password-file`（或承担风险使用 `--allow-insecure`），否则拒绝启动。`--utc-offset-hours` 只影响看板和读取模型的日期计算；domain 写入的时间戳仍按 `Asia/Shanghai` 生成。生产环境请参考 [docs/DEPLOY.md](docs/DEPLOY.md)。

### 客户端

客户端按以下顺序解析配置：

1. 命令行参数（`--server`、`--token`、`--token-file`、`--timeout`）。
2. 环境变量（`ME_SERVER_URL`、`ME_SERVER_TOKEN`、`ME_SERVER_TOKEN_FILE`、`ME_CLIENT_CONFIG`）。
3. 当前目录 `.me-client.json`。
4. `$XDG_CONFIG_HOME/me/client.json` 或 `~/.config/me/client.json`。
5. 默认值：`http://127.0.0.1:8787`、30 秒超时。

示例 `~/.config/me/client.json`：

```json
{
  "server": "https://me.example.com",
  "token_file": "/home/me/.config/me/server.password",
  "timeout_secs": 30
}
```

不要把密码明文提交进仓库；`.me-client.json`、`*.token`、`*.password` 已加入 `.gitignore`。

## API 概览

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| `GET` | `/healthz` | 公共健康检查，不需要认证 |
| `POST` | `/api/v1/auth/login` | 用密码换取 HttpOnly 看板 cookie |
| `POST` | `/api/v1/auth/logout` | 清除看板 cookie |
| `GET` | `/api/v1/auth/session` | 当前会话 |
| `GET` | `/api/v1/status` | 服务、数据库路径、程序和时区 |
| `GET` | `/api/v1/programs` | 命令程序列表 |
| `GET` | `/api/v1/programs/{program}` | `describe` 等价契约 |
| `GET` | `/api/v1/programs/{program}/commands/{command}` | 单命令契约 |
| `POST` | `/api/v1/programs/{program}/commands/{command}` | 写入 |
| `GET` | `/api/v1/data/{resource}` | 读取 `profile`/`actions`/`health`/`life`/`goals`/`projects` |
| `GET` | `/api/v1/versions/{resource}` | 轻量读取集合/记录版本，不返回集合正文 |
| `GET` | `/api/v1/dashboard/summary` | 看板聚合数据 |
| `GET` | `/` | 内嵌看板 HTML |

### 写入请求

标准信封：

```json
{
  "request_id": "req-20261020-001",
  "expected_version": 42,
  "input": {
    "title": "Write the client guide"
  }
}
```

- `request_id` 可省略；服务端会生成并回显。重试同一请求时应保留原值。
- `expected_version` 必填。集合级或记录级的语义由命令契约决定。
- `input` 是命令字段对象。
- 也支持把字段直接放在顶层（省略 `input`），但推荐使用标准信封。

响应成功时直接返回 domain handler 的结果，例如：

```json
{
  "status": "succeeded",
  "request_id": "req-20261020-001",
  "changed_records": ["action-..."],
  "new_versions": {"action-...": 2},
  "collection_version": 43,
  "summary": "create_action: action-..."
}
```

失败时返回统一错误：

```json
{
  "status": "failed",
  "request_id": "req-20261020-001",
  "error": {"code": "VERSION_CONFLICT", "message": "version has changed"}
}
```

HTTP 状态映射：

| 业务码 | HTTP |
| --- | --- |
| `INVALID_INPUT` | `400` |
| `UNAUTHORIZED` | `401` |
| `NOT_FOUND` | `404` |
| `VERSION_CONFLICT` / `TIME_CONFLICT` | `409` |
| `WRITE_BUSY` | `423` |
| `DATA_INVALID` / `INTERNAL_ERROR` | `500` |

## 看板

- 总览：行动队列（当前行动在首位，其后是下一步）、目标、饮水、睡眠、洗衣队列。
- 行动：按队列顺序展示（当前 / #2 / #3…），支持状态筛选、搜索、截止时间；每个行动有行动时长和弹性时间，弹性默认等于时长，列表用实心时长条 + 斜纹弹性条展示。行动可选 `recurrence` 成为循环行动；列表显示循环次数/当前第几次，完成控件改为“完成本次”，每次完成推进到下一次，次数耗尽后自动结束。
- 目标：四层结构——目标分组/Goal（任意 horizon，`parent_goal_ids` 可嵌套）、里程碑、检查点、关联行动。
  - 目标分组：创建时必须显式 `group=true` 且 `parent_goal_ids` 为空；用于归类目标和里程碑；不计入活跃目标数、平均进度和概览 Top 列表；删除分组会级联删除整棵子树，行动本身保留。
  - 目标/里程碑：新目标的 `parent_goal_ids` 必须非空且引用已存在的目标；里程碑在目标内按 `start_date` 排序（缺失时用 `due_at`，未排期最后）。
  - 行动：新行动必须提供已存在的 `checkpoint_id`，服务端会在同一事务写入检查点/里程碑/目标关联；没有检查点时不能创建行动。
  - 时间线（甘特）：每周固定宽度绘制刻度和网格，左侧标题列固定，跨度较长时横向滚动，日期标签不会重叠；分组按层级包含子目标，Goal 下显示里程碑，折叠分组会收起整棵子树。
- 日程：层级列表、周/月/年视图，数据全部来自行动与目标层级；不再有独立日历事件。
  - 层级列表按日期分组展示所选层级的当前安排；周视图顶部全天条显示未定时层级项，24 小时网格显示定时行动，按 `due_at` 连续渲染实心行动时长块和斜纹弹性块。
  - 循环行动在列表与网格中显示循环次数/当前第几次；完成一次后行动自动推进到下一次。
  - 月/年视图用圆点和层级条展示目标、里程碑、检查点与行动覆盖的日期。
  - 日程页提供统一的层级按钮：行动、检查点、里程碑、目标、目标分组；周/月/年默认分别为行动/检查点/里程碑，可随时开关，列表与日历同步过滤。
- 健康：饮水、睡眠、14 天趋势和身体记录。
- 生活：洗衣队列、正在穿、全部衣物。

看板写入仍只调用命令 API，不新增按资源的写路由。编辑范围：

- 行动：新建时必须选择检查点；编辑标题、时长、弹性时间和重复规则；一次性行动用勾选完成，循环行动用“完成本次”推进并显示循环次数。
- 目标：目标分组/目标、里程碑、检查点创建、编辑与删除；行动关联检查点，检查点内可直接新建行动。
- 资料：任意键值对，值可为字符串、数字、布尔、数组、对象或 null；`display_name` 与 `timezone` 为固定保留字段。

删除语义：删除目标分组/目标会级联删除其子目标、里程碑、检查点和行动关联；删除里程碑会级联检查点和关联；删除检查点只删除检查点和关联。行动本身保留。删除行动会移除其目标关联。

编辑使用 `{request_id, expected_version, input}` 信封；冲突时保留草稿，由用户选择“载入最新”或“用最新版本重试”。完成/归档提供 8 秒撤销。带 `me_session` cookie 的写入必须发送 `X-ME-CSRF: 1`，存在 `Origin` 时还会校验同源；Bearer 客户端不受影响。静态资源带内容哈希查询参数，使用一年 immutable 缓存；脚本 CSP 为 `script-src 'self'`。

### 高延迟与弱网

- JSON 和静态资源启用 gzip/brotli 压缩。
- 数据读接口和看板聚合返回 `ETag`，客户端用 `If-None-Match` 条件请求；无变化时服务端只返回 `304`，不重新序列化资源。
- 看板并行加载独立资源，缓存已加载视图；刷新失败时保留旧数据，不把页面清空。
- 读取/写命令都在 `spawn_blocking` 中执行；GET 和幂等写命令带超时与重试，写请求复用同一个 `request_id`，响应丢失后重试不会重复写入。
- SQLite 使用 WAL、`synchronous=NORMAL`、32 MB cache 和自动 checkpoint；批量写入不逐条 fsync。

## 目录

```text
crates/
├── shared/         JSON、协议、时间、幂等回执、DocumentStore trait
├── store/          DocumentStore 实现：SqliteStore（唯一依赖 rusqlite）与 MemoryStore
├── personal/       profile / actions / migrate
├── health/         body / water / diet / sleep
├── life/           clothes
├── goal/           goals / milestones / checkpoints / action_links / review / progress
├── server/         axum HTTP server、读模型、导入/备份 CLI、内嵌看板
├── client/         me-client CLI + Rust client library
└── test-support/   测试用 argv 分发（不参与发布二进制）
```

数据以集合名存放在一个数据库里：`profile`、`actions`、`health`、`life`、`goals`、`projects`。每个集合是一份 JSON 文档（`documents.body`），带 `version`、`updated_at` 和最近 128 条幂等回执。旧的 `calendar` 事件集合已废弃。

## 开发与测试

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --check crates/server/assets/dashboard.js
node --test crates/server/assets/dashboard-core.test.mjs
cargo run -p me-server -- init --db /tmp/me-test.db
cargo run -p me-server -- serve --db /tmp/me-test.db --password test-password-1234
```

依赖方向：domain crate 只依赖 `shared` 的 `DocumentStore` 抽象；`rusqlite` 只出现在 `me-store`；`shared` 不依赖任何存储实现。可用 `cargo tree -p personal -e normal` 检查。

## 文档

- [docs/CLIENT.md](docs/CLIENT.md)：客户端完整用法、退出码和 agent 接入方式。
- [docs/API.md](docs/API.md)：HTTP API、版本语义和响应结构。
- [docs/DEPLOY.md](docs/DEPLOY.md)：systemd、HTTPS、备份、升级和故障排查。
- [docs/ME_MIGRATION.md](docs/ME_MIGRATION.md)：ME 从旧方法读取数据并通过 `me-client` 写入的迁移步骤。

## 数据与安全

- 服务端是唯一写入者；所有写入都经过 domain handler。
- 默认只监听 `127.0.0.1`，并且默认需要密码（本机默认为 `password`）。
- 密码不会写入日志；看板 cookie 是 `HttpOnly; SameSite=Strict`。
- 生产部署建议放在 Caddy/Nginx 后面启用 HTTPS，并设置 `--secure-cookie`。
- 备份用 `me-server backup --to <file.db>`（内部 `VACUUM INTO`）；不要在服务运行时直接 `cp` 带 WAL 的 `me.db`。
- 数据库是**明文 SQLite**，权限 `0600`、父目录 `0700`，但不提供加密保护；同用户进程仍可打开它。
- 个人资料、健康和行动/日程属于敏感数据。不要把数据库文件、密码文件或 `.me-client.json` 提交到公开仓库。
