# crates/server

## 功能

`me-server` 是 axum HTTP 服务，负责：

- 认证：Bearer 密码与看板 HttpOnly cookie。
- 命令代理：把 `POST /api/v1/programs/{program}/commands/{command}` 转成进程内 domain handler 调用。
- 自描述接口：`/api/v1/programs`、`/api/v1/programs/{program}`、`/commands/{command}`。
- 只读数据 API：`/api/v1/data/{resource}`（含 `actions`）。
- 看板聚合：`/api/v1/dashboard/summary`。
- 看板静态资源：`/assets/dashboard.css`、`/assets/dashboard.js`、`/assets/dashboard-core.js`（内容哈希查询参数、一年 immutable 缓存，CSP `script-src 'self'`）。
- 运维命令：`init`、`check`、`import`（仅离线恢复/兼容旧 `Data/`，不作为迁移路径）、`backup`（`VACUUM INTO`）。

## 模块

```text
src/
├── lib.rs        serve 入口、优雅关闭
├── main.rs       clap 子命令
├── config.rs     Serve/Init/Check/Import/Backup/Password 参数与默认库路径
├── state.rs      AppState{Arc<SqliteStore>}、ProgramRegistry
├── auth.rs       密码/cookie 认证、登录、会话
├── data.rs       资源映射、从 Store 读取、过滤、日历展开、init/check
├── import.rs     旧 Data/ 离线恢复（不作为迁移路径）
├── dashboard.rs  看板 view model
├── routes.rs     axum 路由、写入/读取 handler、错误映射
└── json.rs       shared::JsonValue <-> serde_json
assets/
├── dashboard.html      结构
├── dashboard.css       样式
├── dashboard.js        视图与编辑 UI（module）
├── dashboard-core.js   版本表、信封、错误解析
└── dashboard-core.test.mjs  node:test 纯函数测试
tests/
└── api.rs        HTTP 集成测试
```

## 规则

- 写入必须调用 `CommandSpec.handler`，不要在 server 里重新实现领域写入。
- 路由只把 `&dyn DocumentStore` 交给 handler；`AppState` 持有具体 `Arc<SqliteStore>`，因为 `backup_to` 是 SQLite 专有操作。
- domain handler 和重读取/聚合路径（数据读取、日历展开、看板 summary）都是同步阻塞的；通过 `tokio::task::spawn_blocking` 调用。版本号条件检查可以留在 async 侧，但不要在这里加载整份文档。
- 数据读接口返回 `ETag` 并在 `If-None-Match` 命中时返回 `304`；写接口成功后集合版本变化，缓存自然失效。不要为 304 重新序列化文档。
- 响应经过 gzip/brotli 压缩；静态资源使用内容哈希查询参数与长缓存。
- 不要在事务/闭包内再次调用 `store.load`（同一连接会重入锁）。
- 读取只允许 `Resource` 白名单。不要新增任意路径读取接口。
- 默认移除 `recent_requests`；只有显式 `include_receipts=true` 才返回。
- 错误响应保持 `status/error.code/error.message` 结构；HTTP 状态按 `routes.rs` 的映射。
- 密码不写日志。认证失败只返回通用消息。
- 新增路由时补 `tests/api.rs` 覆盖认证、成功路径和错误路径。
- 看板编辑只调用命令 API，不新增按资源写路由。带 `me_session` cookie 的写请求必须带 `X-ME-CSRF: 1`；存在 `Origin` 时只接受与 `Host` 一致的 authority。Bearer 客户端不要求 CSRF。
- 看板每个写调用点按 spec 版本表显式选择集合/记录版本；`dashboard-core.js` 的 `VERSION_SCOPE`/`versionFor` 由 `node:test` 覆盖，新增写命令必须同步。冲突保留草稿，禁止静默覆盖。
- 静态资源通过 `include_str!` 嵌入；修改后重新编译，并运行 `node --check crates/server/assets/dashboard.js` 与 `node --test crates/server/assets/dashboard-core.test.mjs`。
- 看板 GET 请求会带 `If-None-Match`、合并同路径 in-flight 请求并缓存 304 响应体；刷新失败必须回滚到旧快照。GET 和幂等写命令的重试必须复用同一个 request id。

## 测试

```bash
cargo test -p me-server
```

测试会在 `target/test-tmp/` 创建临时数据目录，并在结束时删除。
