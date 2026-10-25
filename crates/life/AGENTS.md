# crates/life

## 功能

提供个人领域生活相关的命令实现（集合：`life`），由 `me-server` 通过 `DocumentStore` 调用；写入在 SQLite 事务内完成，回执保留最近 128 条：

- **初始化**：
  - `init`：在数据库中初始化 `life` 集合（若不存在则创建默认文档；若已存在则校验结构并返回当前版本）。
- **衣物管理（Clothing & Laundry Management）**：
  - `clothes.add`：新增一件衣物记录（名称 `name`、分类 `category`：`top` / `bottom` / `outerwear` / `underwear` / `shoes` / `accessory` / `other`、季节 `seasons`：`spring` / `summer` / `autumn` / `winter` / `all`、颜色 `color`、品牌 `brand`、存放位置 `location`、洗衣护理方式 `care_mode`：`machine` / `hand` / `dry_clean` / `air_only`、建议洗涤前最大穿着次数 `max_wears_before_wash`、备注 `notes`）。新衣物默认状态为 `clean`（干净可穿）、`wear_count_since_wash = 0`。
  - `clothes.update`：按 `item_id` 与记录版本修改衣物的属性、存放位置、护理方式或状态（归档衣物需先恢复再修改）。
  - `clothes.wear`：记录穿着某件衣物（支持 `clean` 或仍可继续穿的 `worn` 衣物；将状态设为 `wearing`，或当 `put_back=true` 时穿完脱下为 `worn`/`dirty`，递增 `wear_count_since_wash` 与累计穿着次数 `total_wears`，更新 `last_worn_date`；当穿着次数达到 `max_wears_before_wash` 时自动标记为待清洗 `dirty` 并返回提示 `needs_wash`）。
  - `clothes.mark_dirty`：将衣物标记为待清洗状态（`dirty`），支持附带原因或污渍说明 `notes`。
  - `clothes.wash`：处理衣物清洗流转，支持单个或批量 `item_ids` 及动作 `action`：
    - `start`：开始清洗（状态转为 `washing`）
    - `dry`：洗完晾晒/烘干中（状态转为 `drying`）
    - `complete`：完成清洗与收纳（状态转为 `clean`，重置 `wear_count_since_wash = 0`，递增累计清洗次数 `total_washes`，更新 `last_washed_date`）
  - `clothes.archive`：按 `item_id` 与记录版本归档（退役/收纳封存）或恢复一件衣物（正在 `wearing`、`washing`、`drying` 中的衣物需先完成当前状态后再归档）。

## 调用协议

所有调用都经 `me-server`：

```text
GET  /api/v1/programs/{program}                         describe 概览
GET  /api/v1/programs/{program}/commands/{command}      单命令契约
POST /api/v1/programs/{program}/commands/{command}      写入
me-client write {program}.<command> --expected-version <n> --set <key>=<value>
```

写入请求使用标准信封（`request_id`、`expected_version`、`input`）；`request_id` 用于幂等重试，`expected_version` 指向集合或记录版本，以 `describe` 为准。成功与失败响应只包含一个 JSON 值，错误在 `error.code` / `error.message`。
