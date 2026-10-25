# AGENTS.md

本文件描述 MEtools 的项目边界与开发约束。架构或目录发生结构性变化时同步更新。

## 项目说明

MEtools 是 ME 的个人数据服务：

- `me-server`：常驻 HTTP 服务，负责所有数据写入、只读 API、自描述命令契约和看板。
- `me-client`：ME agent 使用的 CLI / Rust client，通过 HTTP 访问服务端。
- `personal`、`health`、`life`、`goal`：领域逻辑与数据契约，只依赖 `shared::DocumentStore` 抽象，不依赖 SQLite。
- `store`（`me-store`）：`DocumentStore` 的实现 crate；`rusqlite` 只允许出现在这里。

服务端是唯一写入者。任何绕过 `me-server` 直接修改数据库的行为都会破坏版本和幂等语义。agent 只能通过 `me-client` 读写。

## 工作区

```text
crates/
├── shared/      JSON、协议、锁、时间、幂等
├── personal/    profile / actions / migrate
├── health/      body / water / diet / sleep
├── life/        clothes
├── goal/        goals / milestones / checkpoints / action_links / review / progress
├── store/       SqliteStore（rusqlite 唯一使用点）与 MemoryStore
├── server/      axum、认证、读取 API、看板、init/check/import/backup
├── client/      me-client CLI、配置解析、HTTP client
└── test-support/ 测试用 argv 分发，不参与发布
```

## 命令协议

每个 program 仍然提供统一的 `describe` 契约：

```text
GET  /api/v1/programs
GET  /api/v1/programs/{program}
GET  /api/v1/programs/{program}/commands/{command}
POST /api/v1/programs/{program}/commands/{command}
```

写入请求：

```json
{"request_id":"req-...","expected_version":42,"input":{...}}
```

规则：

- `--help` / `describe` 不访问数据库；HTTP 侧对应 GET 接口。
- 标准输出/HTTP 响应只包含一个 JSON 值；错误放在 `error.code` 和 `error.message`。
- 写入必须使用 `request_id` + `expected_version`；重试保留 `request_id`。
- 单个 SQLite 事务、版本递增和最近 128 条幂等回执由 `shared::apply_mutation` 与 `me-store` 维护。
- 新增命令时同时更新 `CommandSpec`（字段、规则、示例）和 handler，并为 HTTP 路径补测试。
- 读取 API 只允许暴露白名单资源；不要把任意文件路径暴露给客户端。

## 数据边界

数据存在一个 SQLite 文档库里（默认 `$XDG_DATA_HOME/me/me.db`，回落 `~/.local/share/me/me.db`），表结构：

```sql
documents(collection TEXT PRIMARY KEY, version INTEGER, updated_at TEXT, body TEXT)
```

集合名固定为 `profile`、`actions`、`health`、`life`、`goals`、`projects`。旧的 `tasks` 集合由 `personal.migrate` 一次性迁移为 `actions`；schema v1 的 `goals` 由 `goal.migrate` 迁移到 v2。旧的 `calendar` 事件集合已废弃，服务端不再读写。

- 旧 ME 数据迁移必须“旧读新写”：先用旧方法读取 `Data/` 或旧数据库快照，再用 `me-client write` 逐条写入并回读核对；不要用 `me-server import` 作为迁移路径。`import` 只用于离线恢复和旧版本兼容。

- `--db` / `ME_DB` 指定库路径；没有 `--data-dir`。
- 依赖方向：`shared` 不依赖存储实现；domain 的 `[dependencies]` 不得出现 `me-store`/`rusqlite`（`[dev-dependencies]` 允许 `me-store`）；`rusqlite` 只在 `me-store`。
- `actions` 集合是行动与重复事实源；文档级 `queue` 是 open 行动的执行顺序（索引 0 为当前行动）。每个行动有 `duration_minutes`、弹性 `buffer_minutes` 和可选 `recurrence`；弹性默认等于时长。循环行动保持单条记录，`action.complete` 完成一次后按 `recurrence_start_at`/`recurrence_completed_count` 推进到下一次，次数耗尽后自动结束。行动文档 schema v7。`action.create` 必须提供已存在的 `checkpoint_id`，并在同一事务写入 `goals.action_links`。
- `goals` 集合只保存行动 ID 关联，不复制行动标题/状态；`parent_goal_ids` 为空表示目标分组，非空表示具体目标；层级为目标分组/Goal（任意 horizon，可嵌套）→ 里程碑（`milestones`）→ 检查点（`checkpoints`）→ 队列行动（≤30 分钟）。新建目标必须有非空 `parent_goal_ids`；顶层目标分组必须显式传 `group=true` 且父级为空。目标分组用于归类，不计入活跃目标统计；删除分组级联整棵子树，行动保留。
- 目标合并后不再有 `directions`、`current_goals`、`task_links`。旧 ID（如 `direction-*`、`task-*`、`current-*`）保留以维持引用稳定；新记录使用 `goal-*`、`action-*`、`checkpoint-*`。
- 读取默认不返回内部 `recent_requests`；需要排查时显式请求。
- 看板允许受控编辑，但写入必须经过命令 API；带 `me_session` cookie 的写请求必须带 `X-ME-CSRF: 1`，存在 `Origin` 时校验同源。Bearer 客户端不受 CSRF 约束。
- 看板每个写调用点按 spec 版本表显式传集合或记录版本；`dashboard-core.js` 的 `VERSION_SCOPE`/`versionFor` 由 `node:test` 覆盖，新增写命令必须同步。冲突时保留草稿，禁止静默覆盖。完成/归档的撤销使用新 `request_id`。

## 构建与测试

```bash
cargo build --release -p me-server -p me-client
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
node --check crates/server/assets/dashboard.js
node --test crates/server/assets/dashboard-core.test.mjs
```

数据库运维：

```bash
me-server init --db <path>
me-server import --from <ME repo> --db <path>   # 仅离线恢复/兼容，不作为迁移路径
me-server check --db <path>
me-server backup --db <path> --to <file.db>
```

## 安全

- 默认只监听 `127.0.0.1`；无认证时禁止绑定非 loopback（除非显式 `--allow-insecure`）。
- 密码不写入日志；看板 cookie 使用 `HttpOnly; SameSite=Strict`。
- 公网部署必须使用 HTTPS 反向代理，并设置 `--secure-cookie`。
- 不把数据库文件、密码文件、`.me-client.json` 提交到公开仓库。
- 默认密码 `password` 只允许 loopback；非 loopback 必须显式设置 `--password`/`--password-file`。
- 不要把 `--password` 放在进程参数中；优先 `--password-file` 或 systemd credential。

## 文档

- [README.md](README.md)：架构、快速开始、API 概览。
- [docs/CLIENT.md](docs/CLIENT.md)：客户端完整用法和 ME 接入方式。
- [docs/API.md](docs/API.md)：HTTP API 与版本语义。
- [docs/DEPLOY.md](docs/DEPLOY.md)：systemd、HTTPS、备份、升级。
- [docs/ME_MIGRATION.md](docs/ME_MIGRATION.md)：ME 侧迁移到 client 的步骤。
- 各 crate 的 `AGENTS.md`：局部实现约束。
