# 17 M3 · CheckJob 调度（周期 + 手动触发）

> 状态：**完成**。基线 `9767908`；本页随 ACTION-LOG **S68** 提交。
> 裁定依据（2026-11-19 用户复审）：只调度 Linked + 有可读载荷的节点；分批；初期只用
> `TEST_PING_RTT`；明确并发与去重；手动触发 AdminJWT only；首次 tick 不得立即重复入队。

## 1. M3 与 M2 的分界

M2 证明的是「**给定**一个 CheckJob，能完成真实测活并落库」。M3 证明的是「**系统**能正确决定
什么时候测、测哪些节点、如何避免重复、任务失败后如何恢复」。两组测试互不替代。

## 2. 调度架构

```
worker main (tokio spawn)
  └─ check_tick = interval_at(now + interval, interval)   ← 首次 tick 显式推迟一个整周期
       └─ enqueue_scheduled(storage, pool, period_label, cfg)
            ├─ schedulable_node_ids(): Linked JOIN node_payloads（无载荷不参与）
            ├─ schedule_id = Uuid::v5("phlogo-check-sch", period_label)   ← 确定性
            ├─ 按 check_batch_size 分批
            └─ 每批: run_id = Uuid::v5(schedule_id, batch_index)
                 ├─ record_check_run()（INSERT ON CONFLICT DO NOTHING）
                 ├─ 插入成功 → storage.push(job)          ← 只有“赢得 run 行”的实例入队
                 └─ 冲突      → skip（同周期重复调度被丢弃）

API: POST /api/v1/check/trigger (AdminJWT only)
  └─ trigger_check_batch(...): 同一服务；显式节点列表（先过滤为可调度节点）或默认全量
       └─ period = "manual-<ts>-<uuid4>"（每次触发独立逻辑运行）
```

**关键点**：去重不依赖 Apalis 原生 dedup（0.7.4 无确定性 task id 的 push），而是**先抢
`check_runs` 行、赢了才 push job**——`run_id` 是 (period, batch) 的确定性 v5 UUID，多
worker 实例竞争同周期时，只有一个实例能插入 run 行并入队，其余全部 skip。这也保证 worker
重启后同分钟内不会重复调度（period 标签分钟粒度）。

## 3. 周期与批次配置

| 配置键（configs 表） | 默认 | 含义 |
| --- | --- | --- |
| `check_schedule_interval_minutes` | `30` | 调度周期（分钟）。**不是** `node_alive_check_window_minutes`（那是健康窗口，裁定明确不混用） |
| `check_batch_size` | `50` | 每批节点数 |

矩阵：初期**只有 `TEST_PING_RTT`**（裁定：不擅自开启下载测速，M4 再议）。
API 手动触发的 batch_size 允许覆盖，clamp 到 [1, 100]。

## 4. 去重与并发

| 场景 | 行为 |
| --- | --- |
| 同周期重复调度（多实例/重启） | `run_id` 相同 → `check_runs` 冲突 → skip，不产生第二个 job |
| worker 崩溃在 push 之前 | run 行已建但没有 job；下个周期新 label 会重新调度（旧行按 run 历史保留） |
| worker 崩溃在 push 之后 | job 正常执行；`run_check_once` 幂等（重复投递不重复计数，M2 行为） |
| 手动触发 | label 带 uuid4 后缀 → 每次都是新逻辑运行（同秒两次也不合并） |

## 5. API 权限矩阵

`POST /api/v1/check/trigger`：

| 身份 | 结果 |
| --- | --- |
| 无凭据 | **401** |
| ServiceBearer | **403**（fail-closed：`auth.rs` 白名单**未**添加此路由，裁定明确不扩大） |
| AdminJWT | **202** + `{enqueued_batches, duplicate_batches, nodes, batch_size}` |
| 显式列表全不可调度 | **422**（`no schedulable nodes in the request`） |

API 只做节点选择 + 分批入队；**绝不直接调用 MiaoSpeed**（worker 是唯一执行者）。显式列表
先过滤为「存在 + 有载荷」的节点，未知/无载荷 id 不会产生 ghost 调度；单次请求上限 500 节点。

## 6. 测试（独立可销毁 `phlogo_jobs_test_m3`，不污染正式队列）

| 用例 | 断言 |
| --- | --- |
| `check_run_persists_adapter_results`（M2 回归） | 落库/健康/幂等照旧 |
| `missing_endpoint_fails_the_run` | 无端点 → failed |
| `early_close_fails_the_run`（**M2.1 新增**） | 无终态帧 → run `failed`，**0 行结果**（不允许表面成功） |
| `scheduled_enqueue_is_deduplicated_by_period`（M3） | 新周期 push 全量；同周期全 skip；新周期重推；无载荷节点被排除 |
| `manual_trigger_enqueues_explicit_nodes`（M3） | 显式节点 1 批；两次触发各自独立；未知 id 被过滤（0 push 0 count） |
| `check_trigger_permission_matrix`（curse，真 TCP） | 401 / 403 / 202+stats / 422 |
| `no_duplicate_work`、`fetch_loop_end_to_end` | 回归通过 |

`CheckJob` 新增 `schedule_id` 字段（serde default，向后兼容旧队列消息）。

## 7. 测试基建注意

- 测试库 `phlogo_jobs_test_m3` 由 gates 脚本创建/销毁；apalis 表由测试内
  `PostgresStorage::setup` 生成；**不触碰** 生产 `phlogo_jobs`。
- dev 主库含历史测试夹具（17 个带载荷节点），调度测试断言全部使用**相对量**（同周期
  push == n、重跑全 skip），不写绝对数。
- 显式不使用「随机 run_id 每 N 分钟全量入队」模式（裁定第二点）——那会重复执行真实测活。

## 8. 遗留

1. **增量测活**（只测新/变过的节点）、连续失败降级、评分接入（M5）未做——裁定明确不混入 M3。
2. `enqueue_for_active_sources`（fetch）仍是每 5 分钟全量 active 源入队（无 period 去重）；
   fetch 本身幂等性可接受，但如需同策略可复用 `enqueue_scheduled` 模式（另行裁定）。
3. 深度矩阵/协议载荷保真属 **M4**；`InvokeDuration` 归一化接 p3-03 也在 M4。
