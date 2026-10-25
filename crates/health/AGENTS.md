# crates/health

## 功能

提供个人领域健康相关的命令实现（集合：`health`），由 `me-server` 通过 `DocumentStore` 调用；写入在 SQLite 事务内完成，回执保留最近 128 条：

- **初始化**：
  - `init`：在数据库中初始化 `health` 集合（若不存在则创建默认文档；若已存在则校验结构并返回当前版本）。
- **个人身体状态维护（Body Status）**：
  - `body.set_profile`：更新基础身体指标与参考目标（身高 `height_cm`、目标体重 `target_weight_kg`、血型 `blood_type`、过敏源 `allergies`、慢性病或健康状况 `conditions`、备注 `notes`）。
  - `body.log`：记录一次身体状态测量或健康日志（日期 `date`、体重 `weight_kg`、体脂率 `body_fat_pct`、静息心率 `heart_rate_bpm`、收缩压/舒张压 `systolic_bp`/`diastolic_bp`、状态症状 `symptoms`、精力评分 `energy_level` 1-5、备注 `notes`）。
  - `body.update`：按 `record_id` 与记录版本修改一条身体状态记录。
- **每日喝水管理（Daily Hydration）**：
  - `water.set_goal`：设置每日目标饮水量（`daily_goal_ml`，默认 `2000` ml）。
  - `water.log`：记录一次饮水摄入（日期 `date`、饮水量 `amount_ml`、饮品类型 `beverage`、摄入时刻 `recorded_at`、备注 `notes`），并返回当日累计饮水量与达标状态。
  - `water.update`：按 `record_id` 与记录版本修改或更正一条饮水记录。
- **每周饮食计划（Weekly Diet Plan）**：
  - `diet.set_week`：创建或更新某一周（以周一日期 `week_start` 标识）的饮食计划与营养目标（每日热量目标 `daily_calorie_target`、饮食偏好/禁忌 `preferences`、每日三餐及加餐安排 `days`、备注 `notes`）。
  - `diet.set_meal`：按周起始日 `week_start`、星期 `day_of_week`（`mon`..`sun`）和餐次 `meal_type`（`breakfast` / `lunch` / `dinner` / `snack`）设置或更新单餐计划（食物列表 `items`、预估热量 `calories`、备注 `notes`）。
- **睡眠管理（Sleep Management）**：
  - `sleep.set_goal`：设置目标睡眠时长与作息参考（目标时长 `target_duration_minutes`、目标入睡时间 `target_bedtime`、目标起床时间 `target_wake_time`）。
  - `sleep.log`：记录一次睡眠（归属日期 `date`、入睡时刻 `slept_at`、醒来时刻 `woke_at`、总时长 `duration_minutes`、质量评分 `quality_score` 1-5、深睡时长 `deep_sleep_minutes`、夜醒次数 `awakenings`、备注 `notes`）。
  - `sleep.update`：按 `record_id` 与记录版本修改一条睡眠记录。

## 调用协议

所有调用都经 `me-server`：

```text
GET  /api/v1/programs/{program}                         describe 概览
GET  /api/v1/programs/{program}/commands/{command}      单命令契约
POST /api/v1/programs/{program}/commands/{command}      写入
me-client write {program}.<command> --expected-version <n> --set <key>=<value>
```

写入请求使用标准信封（`request_id`、`expected_version`、`input`）；`request_id` 用于幂等重试，`expected_version` 指向集合或记录版本，以 `describe` 为准。成功与失败响应只包含一个 JSON 值，错误在 `error.code` / `error.message`。
