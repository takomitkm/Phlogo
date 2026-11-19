# 17 M3 · CheckJob 调度（周期 + 手动触发）

> 状态：**M3 主体完成（S68）+ M3.1 可靠性收尾（S69，复审裁定落地）**。
> 基线 `9767908`（M3 主体）→ M3.1 提交见 ACTION-LOG。
> M3.1 依据（2026-11-20 复审）：多实例周期身份、真实可解密筛选、handler 失败重试、
> 崩溃窗口恢复、四类故障注入测试。

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

## 8. M3.1 可靠性收尾（S69，复审裁定五点全落地）

复审确认 M3 主体有三处实质缺口 + 一个已知窗口。逐项修复：

### 8.1 多实例周期身份（裁定问题 1）

**原缺陷**：period 标签取 `Utc::now().format("%Y%m%dT%H%M")`，而各 worker 的 tick 起点是自己的
`interval_at(now + interval)` —— 两个启动时刻不同的 worker 对同一逻辑周期会得到不同标签，
`check_runs` 唯一约束根本无法把它们关联起来（裁定原文的 12:00:30 / 12:01:30 例子成立）。

**修复**：`aligned_period_label(now, interval_minutes)` —— 把当前时间**对齐到周期边界**
（epoch 分钟数 `div_euclid(interval)`），标签形如 `period-20261120T1200+30m`。同一槽位内
任何时刻启动的 worker 都派生出**同一** schedule_id/run_id；标签内编码间隔长度，改间隔也
不会与旧标签碰撞。测试 `aligned_period_label_is_stable_across_start_times` 钉死：同槽不同
启动时刻同标签、跨槽不同、间隔不同不同。

### 8.2 筛选改为真实可解密（裁定问题 2）

**原缺陷**：`schedulable_node_ids` 只 JOIN `node_payloads` 判"有记录"，密钥不匹配/密文损坏/
JSON 解析失败要到 CheckJob 执行时才发现 —— 与"只调度可解密节点"的约定不一致（临时密钥重启
场景尤甚）。

**修复**：调度期逐节点 `open()` + JSON 解析验证；不可解密的排除并计数（日志只报**数量**，
绝不报载荷内容）。测试 `undecryptable_payloads_are_excluded_from_scheduling`：用**另一把密钥**
封印的载荷 → 行存在但 `jobs` 表中不含该节点（grep `payload LIKE '%m31-wrongkey%'` 为 0）。

### 8.3 handler 失败重试（裁定问题 3）

**原缺陷**：`handle_check` 把错误吞进日志返回 `()` —— Apalis 视为任务完成，RetryPolicy
永远不会触发（裁定确认）。

**修复**：`handle_check` 改返回 `Result<(), anyhow::Error>`；check worker 加
`.retry(RetryPolicy::retries(3))`（workspace `apalis` feature 加 `retry`）。业务 run 行仍在
`run_check_once` 内部翻成 `failed`；重试用于瞬态故障（连接失败/缺终态帧/DB 错误），幂等落库
保证重投不重复计数。测试 `handler_returns_err_on_failure` 直接调用 handler 断言 `Err` +
run 行 `failed`。

### 8.4 崩溃窗口恢复（裁定问题 4 / 已知窗口）

**原缺陷**：`record_check_run` → `storage.push` 跨两个数据库无原子性；崩溃在中间则 run 行
已存在而队列无任务，同 period 再跑会被当作重复跳过，只能等下个周期；手动触发**没有**下个
周期兜底。

**修复**：`check_runs` 新增 `pushed boolean DEFAULT false`（迁移 0013 扩充）+ 三态
`claim_for_push()`：

```
INSERT (pushed=false) 成功        → New          → push → 成功后 mark_pushed
行存在 & pushed=false & started   → LostBeforePush → 重推 → mark_pushed（唯一赢家，UPDATE 竞争安全）
行存在 & pushed=true 或已终态      → AlreadyClaimed → skip
push 失败                          → mark_unpushed（下轮可再恢复）
```

`enqueue_scheduled` 返回 `(pushed, skipped, re_pushed, nodes, undecryptable)`；手动触发走同一
claim 协议，同 label 重调或下个周期都能恢复丢失的 push。测试
`lost_push_is_recovered_by_the_next_pass`：第一轮全 push → 人为把 batch 0 重置为 unpushed →
第二轮 `re_pushed==1 且 pushed==0 且 skipped==n1-1`（恰好恢复一批）。

### 8.5 测试基建（裁定第五点）

- jobs 测试库改为**强制** `PHLOGO_TEST_JOBS_DSN`：缺省即跳过（worker 与 curse 测试均如此），
  不再回退 `DATABASE_URL`——杜绝把业务库当队列。
- API 上限 501 个显式 id：由 `truncate(500)` 静默截断改为 **422 明确拒绝**（新增断言）。

### 8.6 运维纪律（本次学到的）

**迁移一旦应用就不得修改**：S68 提交后我给 0013 追加了 `pushed` 列，dev 库的 sqlx 校验直接
报 `migration 13 was previously applied but has been modified`。本次在 dev 库手动回滚 0013
（删 configs 行、drop 列、删 `_sqlx_migrations` 行）后重放修复；生产环境同样操作需停机窗口。
往后 schema 变更一律新版本号。

### 8.7 M3.1 门禁

worker **11**（+4 故障注入）/ curse **37** / database **4** / distribution **43** / adapter **18** /
checker **11** 全绿；workspace fmt + clippy **0**。

## 9. 遗留

1. **增量测活**（只测新/变过的节点）、连续失败降级、评分接入（M5）未做——裁定明确不混入 M3。
2. `enqueue_for_active_sources`（fetch）仍是每 5 分钟全量 active 源入队（无 period 去重）；
   fetch 本身幂等性可接受，但如需同策略可复用 `enqueue_scheduled` 模式（另行裁定）。
3. 深度矩阵/协议载荷保真属 **M4**（**冻结中**，裁定不进入）；`InvokeDuration` 归一化接 p3-03 也在 M4。
4. push 失败自动恢复依赖**下个周期**的同标签 pass（周期 ≥ 1 分钟）；需要秒级恢复可加独立
   扫描器（未做，按裁定保持最小范围）。
