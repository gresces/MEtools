# crates/test-support

## 功能

测试专用：把测试用的 argv/JSON 参数解析成 `Request`，调用 `CommandSpec.handler`，把响应写回 `Vec<u8>`。domain crate 只把它作为 `[dev-dependencies]` 使用。

## 规则

- 不得被 server/client 的生产依赖引用，也不得进入发布二进制。
- 这里可以重复少量 argv 解析逻辑；不要在 `shared` 里恢复 CLI 入口。
- 新增测试辅助函数时保持无状态，测试之间不共享 store。
