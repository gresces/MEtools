# crates/store

## 功能

`me-store` 提供 `shared::DocumentStore` 的两个实现：

- `SqliteStore`：生产实现，唯一允许依赖 `rusqlite` 的 crate。库结构为单表 `documents(collection, version, updated_at, body)`，写入用 `BEGIN IMMEDIATE` 事务，`backup_to` 用 `VACUUM INTO`。
- `MemoryStore`：零依赖的内存实现，供 domain 测试与本地实验使用。

## 规则

- `rusqlite` 不得出现在本 crate 之外的 `[dependencies]`。
- 本 crate 只依赖 `shared`，不得反向依赖 domain/server/client。
- 事务顺序由 `shared::apply_mutation` 决定；这里只负责加载、落盘和回滚。
- `SqliteStore::open` 负责创建父目录（`0700`）和库文件（`0600`），设置 `busy_timeout=5s`、WAL、`synchronous=NORMAL`、32 MB cache、内存临时表和 `user_version`。
- `DocumentStore::version` 只读取集合版本，供 HTTP `ETag` 条件请求使用；SQLite 实现直接查 `documents.version`，不得加载 `body`。
- SQLite busy/locked 映射为 `WRITE_BUSY`；其余数据库错误映射为 `INTERNAL_ERROR`。
- `backup_to` 目标已存在时返回 `INVALID_INPUT`，不要覆盖。
