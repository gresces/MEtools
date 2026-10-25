# 从 ME + WASM 迁移到 me-server + me-client

这份文档说明 ME 仓库接入新架构时需要改变什么。MEtools 侧已经把数据搬迁到 SQLite 文档库（`--db`），删除了 WASI 文件写入通道；ME 侧需要把原来的 wasmtime 调用和直接读文件替换为 HTTP client 调用。

## 0. 迁移铁律：旧读新写

迁移旧 ME 数据时必须按“旧读新写”执行，不能省略旧数据读取步骤：

1. **保留旧数据**：迁移完成并核对通过前，不要删除旧 ME 仓库的 `Data/` 或旧数据库快照。
2. **旧方法读取**：使用旧 workflow、旧脚本，或直接读取 `Data/**/*.json` 得到原始记录。把读取结果保存到 `Core/runs/<run-id>/old-data/`，并记录文件、记录数、版本和读取时间。
3. **`me-client` 写入**：先 `me-client describe` 确认新命令字段，再用 `me-client write` 逐资源、逐记录写入服务端。
4. **回读核对**：用 `me-client get` / `summary` 重新读取，与旧读取结果核对数量、标题、状态、时间和引用关系。
5. **最后删除旧数据**：只有核对通过后才删除旧 `Data/` 或旧快照。

`me-server import` 只用于离线恢复和旧版本兼容，不作为本迁移流程的写入路径；它会绕过命令 API 的逐条字段校验、版本检查和幂等回执。也不要用 SQL 或直接复制 JSON 到数据库代替迁移。如果已经误用 `import`，保留旧数据，在空库或确认无冲突的目标库上按旧读新写流程重做。

## 1. 角色变化

| 旧架构 | 新架构 |
| --- | --- |
| ME 仓库的 `Data/` 是事实源 | 服务端的 SQLite 文档库（默认 `~/.local/share/me/me.db`）是唯一事实源 |
| agent 直接读 `Data/*.json` | 迁移时先用旧方法读取；迁移完成后通过 `me-client get` / `summary` 读取 |
| 写操作调用 `wasmtime run ... Function/bintools/*.wasm` | 写操作调用 `me-client write <program>.<command>` |
| `Function/bintools/` 安装 WASM 文件 | `me-client` 安装到 PATH；密码放到配置文件 |
| `Function/workflows/bintools_install.md` | 替换为 client 安装与配置流程 |
| `Data/` 作为可读副本 | 迁移时作为旧读来源；核对完成后删除，ME 仓库不再保存数据 |

迁移写入不能直接从旧文件生成数据库文档，必须经过“旧方法读取记录 → `me-client write` 提交 → 回读验证”的链路。

`Core/runs/`、workflow 文本、skill 边界可以保持不变；变化的是数据访问层。

## 2. 安装 client

在 ME agent 所在机器：

```bash
cargo build --release -p me-client
install -m 0755 target/release/me-client ~/.local/bin/me-client
```

配置：

```bash
mkdir -p ~/.config/me
printf '%s\n' '<server password>' > ~/.config/me/server.password
chmod 600 ~/.config/me/server.password
cat > ~/.config/me/client.json <<'JSON'
{
  "server": "https://me.example.com",
  "token_file": "/home/me/.config/me/server.password",
  "timeout_secs": 30
}
JSON

me-client ping
me-client status
```

## 3. 读取：迁移期用旧方法，迁移后用 client

### 3.1 迁移期（必须使用旧方法）

迁移旧数据时，先读取旧 ME 仓库的 `Data/`。不要把这一步替换成 `me-server import`，也不要直接查询新数据库来“反推”旧数据。

```bash
cat Data/personal/profile.json
cat Data/personal/tasks.json
cat Data/personal/health.json
cat Data/personal/life.json
cat Data/goal/goals.json

# 每个项目有独立的 project.json
find Data/projects -name project.json -print -exec cat {} \;
```

旧文件不存在时跳过；存在几个就读取几个。如果 `Data/` 已经被删除，先从 git 历史或备份恢复只读副本，再执行本节读取；不要用新数据库反推旧数据。Windows PowerShell 可以用 `Get-Content` 和 `Get-ChildItem -Recurse -Filter project.json` 执行等价读取。把输出保存到 `Core/runs/<run-id>/old-data/`，并记录每个文件的记录数、`version`、`updated_at` 和字段差异。读取完成前不要删除 `Data/`。

### 3.2 迁移完成后

迁移完成并通过回读核对后，workflow 和 agent 改用 client 读取：

```bash
me-client get actions
me-client get goals
me-client actions --status open
me-client summary
```

- 需要版本号时读取响应里的 `version`。
- 读取行动时默认不包含已归档；需要时加 `--all`。
- `summary` 适合决策上下文；`get` 适合需要完整字段的场景。
- 如果 workflow 依赖本地 `Data/` 路径，迁移后改成 `me-client get <resource> > Core/runs/<run-id>/<resource>.json`，用完即弃；不要继续把本地 `Data/` 当权威源。

## 4. 写入：把旧读取结果逐条改用 me-client

迁移写入必须从第 3.1 步的旧读结果出发，不要用 `me-server import`、SQL 或手工复制 JSON 代替。旧工具到新命令的对应关系如下：

旧写入方式（仅用于字段对照，迁移时不要执行）：

```bash
wasmtime run --dir Data/personal Function/bintools/personal.wasm \
  --dir Data/personal task.create \
  --request-id req-1 --expected-version 41 \
  --set title="..."
```

迁移时使用：

```bash
me-client write personal.action.create \
  --request-id migrate-task-001 --expected-version 41 \
  --set title="..." --set checkpoint_id=checkpoint-...
```

常用映射：

| 旧工具 / 旧数据 | 新命令 |
| --- | --- |
| `Data/personal/profile.json`、`personal.wasm profile.update` | `me-client write personal.profile.update` |
| `Data/personal/tasks.json`、`personal.wasm task.create` | `me-client write personal.action.create` 或 `me-client action add`（必须带已存在的 `checkpoint_id`） |
| `personal.wasm task.update` | `me-client write personal.action.update` 或 `me-client action done/update` |
| `Data/personal/calendar.json`（旧事件） | 不再迁移事件记录；重复安排改用 `personal.action.create` 的 `recurrence`，一次性安排改为行动 |
| `Data/personal/health.json`、`health.wasm body.log` | `me-client write health.body.log` |
| `Data/personal/life.json`、`life.wasm clothes.wear` | `me-client write life.clothes.wear` |
| `Data/goal/goals.json`、`goal.wasm goal.create/update` | `me-client write goal.goal.create/update`（目标分组用 `group=true` 且空 `parent_goal_ids`，具体目标用非空 `parent_goal_ids`） |
| `goal.wasm current_goal.create/update` | `me-client write goal.checkpoint.create/update` |
| `goal.wasm task_link.set/remove` | `me-client write goal.action_link.set/remove` |
| `goal.wasm progress.record` | `me-client write goal.progress.record` 或 `me-client goal progress` |

写之前先：

```bash
me-client describe <program> <command>
```

`--input` 的语义：

```bash
# 旧：完整信封
{"request_id":"...","expected_version":41,"input":{...}}

# 新：只给 input 对象，信封由 client 生成
me-client write personal.action.create --expected-version 41 --input '{"title":"...","checkpoint_id":"checkpoint-..."}'
```

### 4.1 迁移顺序与 ID 映射

按依赖顺序写入，并记录旧 ID 到新 ID 的映射：

1. **profile**：读取 `profile.json`，用 `personal.profile.update` 写入资料。
2. **goals 骨架**：旧 `directions` 转成顶层 Goal（目标分组，`group=true` 且 `parent_goal_ids` 为空）；旧 `goals` 通过非空 `parent_goal_ids` 挂到分组或其它目标下；旧 `current_goals` 转成检查点并挂到对应里程碑（旧数据没有对应里程碑时先建一个承载里程碑）。先不写 `milestone.action_ids` 和 `task_links`，等行动 ID 映射完成。能保留的 Goal/里程碑/检查点 ID 显式传入。
3. **actions**：读取 `tasks.json`，按旧 `task_links`/`current_goals` 找到对应检查点，逐条调用 `personal.action.create` 并传 `checkpoint_id`。`action.create` 不接受旧 `id`，服务端会生成新的 `action-*`；把返回的 `changed_records` 或回读结果记录为 `task-* → action-*` 映射，供后续关联和日历使用。
4. **goal 关联**：用映射后的 `action-*` ID 调用 `goal.action_link.set` 补齐旧 `task_links` 的 goal/milestone/checkpoint、`role` 和 `contribution`；用 `goal.milestone.update` 写回旧里程碑的 `action_ids`。
5. **旧日程**：旧的 `calendar.json` 事件不再迁移；其中重复安排用 `action.create` 的 `recurrence` 重建为循环行动，一次性安排重建为普通行动。
6. **health / life**：读取 `health.json` 和 `life.json`，按对应命令逐条写入记录。
7. **projects**：当前只有读取接口；旧 `project.json` 先保留在 `Core/runs/<run-id>/old-data/` 作为只读快照，不要为了写入它而改用 `me-server import`。

每一步写入后都重新读取受影响资源，确认写入数量和返回的 `new_versions`。

## 5. 版本与重试

- `--auto-version` 适合大多数 agent 写入：client 先读取当前版本，再发送一次。
- 迁移时为每条旧记录固定一个 `request_id`（例如 `migrate-task-001`），并在超时后用原 `request_id` 和原 `expected_version` 重试。
- 需要严格控制并发时，先在 `get` 响应中读取版本，再显式传 `--expected-version`。
- `VERSION_CONFLICT` 表示数据已变化：重新读取，不要盲目覆盖。
- `WRITE_BUSY` 表示数据库写事务被占用：等待后重试同一个请求。
- 写操作完成后重新读取受影响资源，确认 `new_versions` 和实际内容。

## 6. workflow 调整建议

- `goal_aligned_decision.md`：迁移完成后，把读取 `tasks.json`/`goals.json` 的步骤改成 `me-client actions` / `me-client goals` / `me-client summary`；迁移期间仍按旧方法读取原始数据。
- `skill_execution.md`：把“写入 Function”改成“写入 `me-client write`”，仍保留验证和 run 记录。
- `multi_agent_orchestration.md`：Executor 的写入步骤改为 client 调用；Controller 不需要知道 HTTP 细节。
- `bintools_install.md` / `wasmtime_install.md`：删除或标记为历史；新版 MEtools 不再构建 WASI 程序，也不再需要 wasmtime。
- `Data/` 文档：说明迁移时必须先用旧方法读取，写入通过 `me-client`，核对通过后才删除 `Data/`；导入后由服务端数据库独占，agent 不再直接读取。

## 7. 数据迁移与备份

### 7.1 迁移旧 ME 数据

1. **冻结旧写入并备份**：停止旧 agent 的写入，复制或打包 `Data/` 到只读位置；如果 `Data/` 已被删除，先从 git 历史或备份恢复。如果旧服务仍在运行，先记录当前状态。
2. **准备目标库**：新库执行 `me-server init`；已有库先执行 `me-server backup --db <path> --to <file.db>`。启动或确认 `me-server serve` 正在运行，并让 `me-client status` 通过。
3. **旧方法读取**：按第 3.1 节读取 `Data/`，保存在 `Core/runs/<run-id>/old-data/`，记录记录数和版本。
4. **`me-client` 写入**：按第 4.1 节顺序逐条写入；记录 `request_id`、返回的 `new_versions` 和 ID 映射。
5. **回读核对**：运行 `me-client summary` 和各资源的 `me-client get`，与旧读取结果核对数量、字段、状态和引用。
6. **删除旧数据**：核对通过后才删除旧 `Data/`；在此之前保留旧快照用于回滚。

不要用 `me-server import` 完成上述迁移。它只用于离线恢复或旧版本兼容，不能替代旧方法读取和 `me-client` 写入。需要检查数据库时运行：

```bash
me-server check --db ~/.local/share/me/me.db
```

### 7.2 已有数据库的 schema 升级

这是数据库内部 schema 升级，不是旧 ME 数据迁移。升级前先备份：

```bash
me-server backup --db ~/.local/share/me/me.db --to ~/me-before-actions.db
me-client write personal.migrate --expected-version <tasks version> \
  --set collection=actions --set target_schema_version=7
me-client write goal.migrate --expected-version <goals version> \
  --set target_schema_version=2
```

v5 会为旧行动补齐 `duration_minutes=30` 和相等的 `buffer_minutes`；如果旧记录已有自定义时长，弹性默认取该时长。v7 补齐循环字段；旧的 `calendar` 事件集合不再读写，可在备份后删除。

### 7.3 备份与临时快照

- 权威副本只在服务端数据库；备份用 `me-server backup --to <file.db>`，或对数据库文件做停机复制。
- 不要把明文数据重新导出成 `Data/*.json` 提交进仓库。
- 如果 ME 仓库确实需要历史快照，把 `me-client get` 的输出放到 `Core/runs/` 等临时位置，不要重新建立第二事实源。

## 8. 安全

- 不把密码写进 ME 仓库、run 记录、workflow 示例或日志。
- 不在 ME 的 `AGENTS.md` 中放真实密码。
- 通过公网访问时使用 HTTPS；`me-client` 默认支持 TLS。
- 看板 cookie 只用于浏览器；agent 使用 Bearer 密码（客户端参数名仍是 token）。
