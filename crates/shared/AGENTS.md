# crates/shared

## 功能

为领域 crate 和 `me-server` 提供统一命令协议与公共基础设施：

- 命令契约（`CommandSpec`/`ProgramSpec`）与 `describe`/`help` 响应构建。
- 标准请求信封（`request_id`、`expected_version`、`input`）校验。
- 标准 JSON 响应与错误结构（`error.code`、`error.message`、`error.details`）。
- 零外部依赖的轻量 JSON 解析与序列化。
- `Document`、`DocumentStore` trait 与 `apply_mutation`（幂等重放、版本递增、回执）。
- 日期/时间校验、请求摘要。

## 边界与规则

- 本 crate 不依赖任何存储实现（禁止 `rusqlite`/`me-store`），只定义抽象与纯算法。
- `apply_mutation` 只负责幂等重放、变更、版本递增和回执；`expected_version` 的语义由 domain 在 mutate 闭包内检查。
- 所有协议响应必须只包含一个 JSON 值；错误放在 `error.code`/`error.message`。
- 修改协议行为时同时检查 `me-server` 的 HTTP 映射和 `me-client` 的请求构造。
- 不要在 server/client 中重复实现版本、幂等或 JSON 校验逻辑。
