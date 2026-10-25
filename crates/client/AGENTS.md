# crates/client

## 功能

`me-client` 提供：

- `me-client` CLI：读取数据、查询命令契约、通用写入、行动/目标快捷命令。
- Rust library：`ApiClient`、`ClientConfig`、错误类型和 JSON 辅助函数。

## 模块

```text
src/
├── lib.rs    库导出
├── main.rs   clap CLI 与快捷命令
├── api.rs    阻塞 HTTP client、错误与退出码
├── config.rs 配置优先级与 token 文件
└── util.rs   --set 解析、嵌套字段、版本读取
```

## 规则

- stdout 只输出一个 JSON 值；诊断写 stderr。
- 不要打印 token；错误信息不要包含 Authorization。
- 通用 `write` 使用 `<program>.<command>`，在第一个点处拆分。
- 快捷命令可以自动读取版本；但只在明确映射的情况下使用 `--auto-version`，并保持“单次尝试、不自动重试”。
- `--request-id` 用于幂等重试；网络错误后不要生成新的请求 ID。
- 写入前优先调用 `describe`；不要在 client 中复制 domain 字段规则。
- 新增快捷命令时：
  - 使用服务端 API，不直接读写文件。
  - 明确输入映射和版本来源。
  - 补 `util.rs` 单元测试，并在 `docs/CLIENT.md` 增加示例。
- 退出码保持：0 成功、2 参数/4xx、4 版本冲突、5 认证、6 服务端、7 网络。

## 测试

```bash
cargo test -p me-client
```
