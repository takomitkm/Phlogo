# 17 M3 · CheckJob 调度（周期 + 手动触发）

> 状态：**M3 主体完成（S68）+ M3.1 收尾（S69）+ M3.1-2 再收紧（S71）+ M3.1-3 执行侧互斥（S72）**。
> 基线 `9767908`（M3 主体）→ S69 `4abd0fd` → S71 `e1d5105` → S72 见 ACTION-LOG。
> M3.1 依据（2026-11-20 复审）：多实例周期身份、真实可解密筛选、handler 失败重试、
> 崩溃窗口恢复、四类故障注入测试。
> M3.1-2 依据（2026-11-20 二轮复审）：S69 的 `pushed` 布尔与 `New/LostBeforePush` 三态被否
> （并发竞争 + 永久漏推窗口），要求重写为带租约的投递状态机、timer 真正锚到全局边界、
> 手动触发可恢复、0013 迁移复原。见 §9。
> M3.1-3 依据（2026-11-22 三轮复审）：六项通过/基本通过，但**执行侧没有互斥**（`SELECT status`
> 是读不是认领），M3.1-2 暂缓最终验收。本轮补**主库执行租约 + 原子认领**与真实并发测试。见 §10。

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
封印的载荷 → 行存在但 `jobs` 表中不含该节点。

> ⚠️ S69 的原断言 `jobs.payload LIKE '%m31-wrongkey%'` **无效**（CheckJob 只序列化节点 UUID，
> 没有 server 字段，查询恒为 0）——二轮复审指出后已在 S71 改为**解析 payload 取 `node_ids`**
> 断言，见 §9.4。

### 8.3 handler 失败重试（裁定问题 3）

**原缺陷**：`handle_check` 把错误吞进日志返回 `()` —— Apalis 视为任务完成，RetryPolicy
永远不会触发（裁定确认）。

**修复**：`handle_check` 改返回 `Result<(), anyhow::Error>`；check worker 加
`.retry(RetryPolicy::retries(3))`（workspace `apalis` feature 加 `retry`）。业务 run 行仍在
`run_check_once` 内部翻成 `failed`；重试用于瞬态故障（连接失败/缺终态帧/DB 错误），幂等落库
保证重投不重复计数。测试 `handler_returns_err_on_failure` 直接调用 handler 断言 `Err` +
run 行 `failed`。

### 8.4 崩溃窗口恢复（裁定问题 4 / 已知窗口）

> ⛔ **本小节的方案已被二轮复审否掉，S71 重写为 §9.2 的租约状态机。** 保留原文只为记录
> 「为什么 `pushed` 布尔不够用」：`pushed` 同时表达「正在 push」与「已成功入队」，因此
> 认领后崩溃会永久漏推，push 成功后崩溃会重复提交。

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

## 9. M3.1-2 再收紧（S71，二轮复审裁定落地）

二轮复审对 S69（`4abd0fd`）的裁定：五项里 2/3/5 部分接受，**`claim_for_push()` 否**。理由不是
「实现有 bug」而是「模型选错了」——`pushed boolean` 把「正在投递」和「已投递成功」压成同一个
状态位，因此无论怎么补 UPDATE 都同时存在漏推与重推窗口。本轮按裁定重做状态机，并顺手修掉三处
测试证据问题（其中一处是**假通过**）。

### 9.1 周期 timer 真正锚到全局 UTC 边界（裁定 1）

**原缺陷**：S69 只对齐了 *标签*（`aligned_period_label`），第一个 tick 仍是
`interval_at(now + interval, interval)` —— 相位来自各进程自己的启动时刻。两个 worker 启动差
几分钟，tick 时刻就永久错开几分钟，落到相邻槽位后标签不同，去重约束根本看不到这场竞争。

**修复**：
- `next_boundary_after(not_before, interval)` 返回 `(边界时刻, 该槽标签)`；worker 以
  `startup + interval` 作 `not_before`，得到首个 tick 与其标签（满足「启动后至少等一个完整周期」）。
- tick 间隔就是 `interval_minutes * 60`（`check_secs`），与边界严格同周期，因此后续 tick 天然落在
  全局边界序列上。
- **标签只从「计划边界」派生，且按算术推进**：每 tick 结束执行
  `planned_at += interval; planned_label = aligned_period_label(planned_at, interval)`。
  绝不在 tick 之后用 `Utc::now()` 重算——那正是裁定禁止的「延迟执行时按当前时间取标签」；
  而且延迟后补齐的连续 tick 会全部落到同一个「当前边界」，把同一逻辑周期重复执行。

**测试证据**：`boundary_timer_shares_identity_across_start_times`（不同启动时刻同槽→同身份；
跨边界→不同；重启后仍与既有实例对齐；延迟 tick 用原计划标签）。

### 9.2 投递状态机（裁定 2，本轮主项）

迁移 `0014_check_push_state` 把 `check_runs` 的投递状态拆成三态 + 租约：

| 列 | 含义 |
| --- | --- |
| `push_state` | `pending`（待投递）/ `dispatching`（某实例持有租约）/ `enqueued`（已确认入队，投递终态） |
| `lease_until` | 租约到期时刻；过期即可被**任何**实例重认领 |
| `lease_owner` | 认领者 uuid；`mark_enqueued` 只接受当前 owner，旧实例无法覆盖新租约 |

**单一原子 claim**（`claim_for_push`）：一条
`INSERT ... ON CONFLICT (id) DO UPDATE ... WHERE (push_state='pending' OR (push_state='dispatching'
AND lease_until < now())) AND status='started' RETURNING (xmax = 0)`。
新建与恢复走**同一条**竞争路径，PostgreSQL 行锁保证并发只有一个调用者拿到 `rows_affected=1`
—— 复审缺陷 A（`New` 与 `LostBeforePush` 同时成立）在结构上消失，不存在「新建分支绕过竞争控制」。

**缺陷 B（认领即永久漏推）**：不再有「认领时置已推」的语义。认领只写 `dispatching` + 租约，崩在
push 之前 → 租约（60s）过期后任意实例可重认领并重推。

**缺陷 C（push 成功后崩溃）**：`mark_enqueued` 在 `storage.push` 成功后才执行，且带 owner 守卫；
若崩在「push 成功、mark 之前」，行留在 `dispatching`，租约过期后会被**再推一次**——即
**at-least-once**，这是明确接受的设计，不是 bug。于是「队列里有两条同一 run_id 的 job」必须由
**执行侧**收敛，而不是靠「数据库标了 pushed 就等于队列里只有一条」。

> ⚠️ 本条原方案只是「`run_check_once` 里读一次 `status`，`success` 就返回」——三轮复审指出那是
> **读不是认领**：两条投递都读到 `started` 就会都进 MiaoSpeed。S72 已改为**主库执行租约 + 原子
> 认领**（迁移 0015），见 §10。

**跨主库与 jobs 库没有分布式事务，因此绝不声称 exactly-once**；对外语义一律写 at-least-once +
执行侧互斥（§10）。

**错误不吞**：`claim`/`mark_enqueued` 的 DB 错误一律 `?` 上抛（原 `let _ = mark_pushed` 已删除）。
owner 守卫导致 0 行的情形不是错误，但会**明确记 warn**（「租约已被接管，本批会被当前持有者再推一次」），
不留静默分支。

**Apalis 0.7.4 没有确定性 job id**（查证：`apalis.jobs.id` 由存储生成，`push` 不接受调用方 id），
所以本轮不假设它存在，重复投递只靠执行侧互斥（§10）收敛。

### 9.3 手动触发可恢复（裁定 3）

- 请求体新增 `idempotency_key`（可空）；服务端 period label = `manual-<key>`，**verbatim** 用于派生
  `schedule_id`，因此同一个 key 必然算出同一批 `run_id`。
- 同 key 重发 → run 行已 `enqueued` 则跳过（不建第二个逻辑运行）；若上次崩在 push 前，则按 §9.2
  的过期租约恢复。
- 无 key → 沿用原随机 label（一次性触发语义不变）。
- 响应回显 `period`，便于客户端核对与重放；**不要求用户猜测服务端标签**。
- 权限面不变：仍 AdminJWT-only，ServiceBearer 403，匿名 401；>500 个 node_ids 仍 422 拒绝。

已知窗口（如实记录）：批次组成按「当次可调度节点」重新切分，若两次调用之间节点集合变了，同一
label 的 batch k 覆盖的节点可能不同；执行侧仍按 `run_id` 去重（先到者执行），故不会重复测活，
但「恢复」可能只覆盖部分节点。需要严格一致时应由服务端 outbox 扫描器接管（未做，见 §11）。
另：**幂等键不绑定请求内容**——服务端不保存 `node_ids`/`batch_size` 的摘要，同一个 key 配不同请求
会算到同一批 run_id 上（见 §10.4、§11）。

### 9.4 可解密筛选测试：从「假通过」修成真证据（裁定 4）

三重问题，逐条修：

1. **断言无效**（复审已指出）：`jobs.payload LIKE '%m31-wrongkey%'` 恒为 0——CheckJob 只序列化
   node UUID。现在 `pushed_jobs()` **反序列化** `apalis.jobs.payload` 取 `CheckJob.node_ids`，
   断言「healthy UUID 在集合内」（防空过）且「wrong-key UUID 不在任何 job 内」。
2. **夹具污染（本轮真正的卡点）**：夹具用固定 server 名 `m31-idem.example` + 
   `SELECT id FROM nodes WHERE server=...` 取节点。历史 panic 留下的 8 行先被选中，而那些行的载荷
   是**上一个进程的临时密钥**封的（`PHLOGO_PAYLOAD_KEY` 缺省即 ephemeral，见 `payload-crypto`），
   本进程解不开 → 被调度期筛成 undecryptable → 触发返回 `(0,0,0)`。**这不是产品缺陷**。
   修法：夹具 server 每次运行唯一（`m31-idem-<uuid>.example`），并**直接用 `seed_node` 返回的 id**。
3. **断言口径错误**：`total` 是**筛选后**候选数，永远不含错钥节点，`assert!(total >= 2)` 自相矛盾 →
   改为断言「healthy 至少 1 个候选」+ 上述 payload 解析断言。

**附带地，测试不再手抄 v5 recipe**：`schedule_id_for_label` / `scheduled_run_id` 从 worker 导出，
测试直接调用。上一轮测试手写了一份，多包了一层命名空间又多了 `manual-` 前缀，导致恢复用的
`UPDATE` 打到不存在的行、断言「静默验证了别的东西」——这正是「测试自己复刻生产逻辑」的典型翻车。

### 9.5 故障注入测试（裁定 5，全部落在地面）

强制一次性 `PHLOGO_TEST_JOBS_DSN`（缺省**跳过**，绝不回退 `DATABASE_URL`）。九项：

| # | 窗口 / 要求 | 测试 | 断言要点 |
| --- | --- | --- | --- |
| 1 | 周期身份与相位 | `boundary_timer_shares_identity_across_start_times` | 同槽同身份、跨槽不同、重启对齐、延迟 tick 用原计划标签 |
| 2 | 两个 scheduler 同时 claim 同一批 | `concurrent_claims_yield_exactly_one_winner` | 真并发（独立连接 + 同步屏障），**恰好一个**拿到投递权 |
| 2b | 两个实例同时恢复同一过期租约 | `concurrent_recovery_of_expired_lease_has_one_winner` | 过期租约只被一个抢到 |
| 3 | push 成功、标记前崩溃 | `succeeded_run_is_not_reexecuted_on_redelivery` | 允许重复投递，但已 success 的 run **不再跑真实测活** |
| 4 | mark/claim DB 错误被吞 | `state_transition_errors_propagate` | 错误上抛；stale owner 的 mark 是 **`false`**（可观测），不是静默成功 |
| 5 | 同幂等键重复手动触发 | `manual_trigger_idempotency_key_recovers` | 首次 push 1、同键再发 0 push 1 skip、过期租约后**由本次触发恢复** |
| 6 | 错钥节点不得入队 | `undecryptable_payloads_are_excluded_from_scheduling` | 解析 payload 断言 UUID（见 §9.4） |
| 7 | handler 错误必须上抛 | `handler_returns_err_on_failure` | `Err` + 业务行 `failed` |
| 7b | **RetryLayer 真实重投**（裁定 5.6） | `retry_layer_redelivers_until_exhausted` | 真 `Monitor` + 真队列：handler 执行 **4** 次（1+3 retries），队列 `attempts=4`，终态 `Killed`（不再被 fetch） |

7b 的隔离手法值得记：用**测试自有 job 类型** `RetryProbe`——`PostgresStorage<T>` 按
`type_name::<T>()` 划 namespace（`job_type`），所以它既不会被别的测试的 CheckJob 干扰，也不会把
它们跑挂。apalis 的终态语义也在此确认：重试策略放弃 → `Killed`（`kill` 路径），不是 `Failed`。

### 9.6 迁移纪律（裁定 6）

- `0013_check_schedule.{up,down}.sql` **逐字节**恢复为 `30ea99c` 内容：连注释都不加，因为 sqlx 的
  校验和是**整文件**哈希——加一行注释同样会触发
  `migration 13 was previously applied but has been modified`。
- 新增 `0014_check_push_state`：up 建状态列 + 恢复索引，并 `DROP COLUMN IF EXISTS pushed`（清掉被
  否决的那列；全新库上是 no-op）；down 只回滚它加的东西，不重建 `pushed`。
- **本轮实测**（一次性库 `phlogo_mig_scratch`）：顺序执行 0001…0014 → 断言 `push_state`/
  `lease_until`/`lease_owner` 三列存在、`pushed` 不存在、恢复索引存在、两个 configs 键存在
  （3/0/1/2）；再跑 `0014.down` → 3 列与索引归零、configs 仍为 2（只回滚自己）；再跑 `0013.down`
  → configs 归零；最后删库。
- dev 库历史修复（**仅 dev**）：先前它记录的是「被修改过的 0013」校验和，且已应用过 0014。做法是
  删掉 13/14 两行历史 + 清掉 0013 插入的 configs 行，让 sqlx 按序重放 0013→0014（14 的 up 全是
  `IF NOT EXISTS`，重放安全）。生产环境同样操作需停机窗口。

### 9.7 门禁（本轮实测，含新增测试）

worker **16**（`check_wiring` 14 含 9 项故障注入 + 2 个其它测试二进制）/ curse **37** /
database **4** / distribution **43** / adapter **18** / checker **11** 全绿；
workspace `rustfmt --check`（含递归）+ `clippy --all-targets -- -D warnings` **0**。
一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，`apalis.jobs` 里 **12** 条任务（证明确实入队，
不是空跑）；生产 `phlogo_jobs` 与正式队列零接触。

> 上面是 **S71 当时的数字**。当前门禁见 §10.5（S72 加入执行侧互斥后 worker 变为 **18**）。

## 10. M3.1-3 执行侧原子认领（S72，三轮复审裁定）

三轮复审：全局 timer / 手动幂等 / 错钥测试 / 迁移纪律 **通过**，投递状态机、故障注入与 RetryLayer
**基本通过**，但判定 **M3.1-2 暂缓最终验收**——理由只有一条：**执行侧没有互斥**。

### 10.1 缺陷与根因

复现条件就是 §9.2 缺陷 C 的尾部：push 成功、`mark_enqueued` 之前崩溃 → 投递租约过期 → 另一实例
把它重推 → 队列里出现**同一 `run_id` 的两条 job** → 两个 worker 同时取走。

旧代码（S71）：

```
record_check_run(...)            // INSERT ... ON CONFLICT DO NOTHING，返回值没人用
SELECT status FROM check_runs    // 读！
if status == 'success' { return }// 只挡「已经成功」
MiaoSpeed.check(...)             // 两个执行者都读到了 'started' → 都进来
```

`record_check_run` 的返回值本可以表示「这次是不是我建的行」，但只有「第一次投递」为真，重投永远为
假，所以它换不来执行权；`SELECT` 也不是认领。**投递侧解决的是「谁可以 push」，执行侧需要另一个
租约。**

### 10.2 执行租约（迁移 0015）

`check_runs` 新增 `exec_owner uuid` + `exec_lease_until timestamptz`，并加部分索引
`idx_check_runs_exec_lease (exec_lease_until) WHERE exec_lease_until IS NOT NULL`（供将来自动恢复
扫描 / 运维查询用；**本轮没有**扫描器，见 §10.4）。**0014 未改动**（已应用迁移不可再改，§9.6 纪律）。

**认领是一条语句**（`claim_for_exec`）：

```sql
UPDATE check_runs
   SET exec_owner = $2,
       exec_lease_until = now() + make_interval(secs => $3),
       status = 'started',
       finished_at = NULL
 WHERE id = $1
   AND status IN ('started','failed')
   AND (exec_lease_until IS NULL OR exec_lease_until < now())
RETURNING exec_owner
```

- 并发调用者在行锁上串行化，**恰好一个**拿到 `Some(owner)`；其余拿到 `None`。
- `None` 不是错误：run 只有一个主人——要么已终态（`success` 不在可认领状态里），要么租约在另一个
  活着的 worker 手里。这种投递对 Apalis 是 **no-op（返回 Ok）**，不制造假失败。
- 认领同时**开启一次新尝试**（`status='started'`、`finished_at=NULL`）：`failed` 是可认领的
  （Apalis 重试路径），不重置的话终态写入永远被 `finished_at IS NULL` 挡住，重试成功也记不进
  `success`。

#### 三个数字是**有依赖关系**的（S73 修正）

租约长度不能随手定。实现里是一条不等式链（`check_job.rs` 的 `CHECK_RUN_TIMEOUT_SECS` /
`EXEC_LEASE_SECS` 常量注释里也钉着）：

```text
CHECK_RUN_TIMEOUT_SECS (180s) < EXEC_LEASE_SECS (240s) < apalis reenqueue_orphaned_after (300s)
   活着执行者一定在租约被     死掉的执行者，等到孤儿 job                 …所以那次重投**能赢**
   抢走之前结束                被重投时租约已过期                        认领，而不是空转
```

- **为什么必须有 timeout**：adapter 自身**没有任何超时**（查证 `crates/checker-miaospeed`，无
  `timeout`）；一个「接了 WebSocket 但永不发终态帧」的 MiaoSpeed 会把 worker 槽位永久占住，而且
  执行会跨过租约边界 → 另一条投递可以抢到租约并**同时**进 MiaoSpeed。所以 `run_check_once` 用
  `tokio::time::timeout` 包住 `adapter.check`，超时按失败处理（`fail_check_run` + 释放租约），
  Apalis 可重试。可用 `PHLOGO_CHECK_TIMEOUT_SECS` 覆盖（测试用 1s；会被夹到租约以下）。
- **为什么租约必须短于 apalis 的孤儿窗口**：apalis-sql 0.7.4 默认 `keep_alive=30s`、
  `reenqueue_orphaned_after=300s`，且 `reenqueue_orphaned()` 会把「worker 心跳已死」的
  `Running` job 重置回 `Pending`（`last_error='Job was abandoned'`）。若租约比这个窗口长，重投会
  撞上**仍然有效的租约** → `claim_for_exec` 返回 `None` → 该投递按 no-op 返回 `Ok` → Apalis 把它
  标成 done，**唯一一次恢复机会被烧掉**，run 永远停在 `started`。所以租约取 240s < 300s。
- 180s 是按**当前只发 ping**（`TEST_PING_RTT`，秒级）定的。P3 设计稿记的 30 分钟硬超时是上游
  `context.WithTimeout(30*time.Minute)` 的等价目标；**M4 深度矩阵上线时必须同时抬高 timeout 与
  租约**（或改为执行期心跳续租），否则上面这条链会断。见 §11.8。

#### 这条链不靠上游默认值「碰巧成立」

上游默认值不是契约，所以两件事都做在代码里（S73 复核的 minor 项）：

- `jobs_storage()` 用 `apalis_sql::Config::new(type_name::<CheckJob>())` **显式钉住**
  `keep_alive = 30s` / `reenqueue_orphaned_after = 300s`（`APALIS_KEEP_ALIVE_SECS` /
  `APALIS_ORPHAN_REENQUEUE_SECS`）。namespace 必须保持 `type_name::<CheckJob>()` —— 它是
  `apalis.jobs.job_type` 的判别列，改掉会孤儿化已入队任务。所有构造 `CheckJob` 队列的地方
  （`main.rs` 的 check worker、`trigger_check_batch`、测试）都走这个函数。
- 三个常数之间有**编译期断言**：`const _: () = assert!(CHECK_RUN_TIMEOUT_SECS < EXEC_LEASE_SECS)` 与
  `assert!(EXEC_LEASE_SECS < APALIS_ORPHAN_REENQUEUE_SECS)` —— 把超时抬到租约之上、或把租约抬到孤儿
  窗口之上，都会直接**编译失败**，而不是等到线上出现「空转烧掉恢复机会」。
- 升级 apalis 时：我们显式设置这两项，不受上游默认值变化影响；但若上游改了
  `reenqueue_orphaned_after` 的**语义**（不再把心跳已死的 `Running` job 重置），要重新评估 §10.4.1。

#### 规则：claim 之后到终态之间，**每一条**退出路径都必须释放租约（S75）

`claim_for_exec` 拿到租约之后，`run_check_once` 里任何一次提前返回都**必须**走
`fail_check_run(pool, run_id, exec_owner, reason)`（owner 守卫 + 同语句释放租约 + 标记 `failed`）。
裸 `?` / 裸 `return Err(..)` 都是 bug，原因不是「少了一次失败标记」，而是**租约泄漏**：

```
主库一次抖动 → 早退且仍持有 exec_owner + 未过期租约
             → run 停在 started
             → worker 心跳还活着 → apalis 不当孤儿重投
             → Apalis 立即重试的那几次全撞有效租约 → 按 no-op 返回 Ok 并完成
             → 再没有投递 → run 永久卡在 started
```

S75 就是这么抓到的：`existing_ids()`（测活**返回之后**读主库的一步）当时是裸 `?`。已改成走
`fail_check_run`，并加测试 `db_error_after_checker_releases_the_execution_lease`（用真实的
`ACCESS EXCLUSIVE` 锁 + `lock_timeout` 制造该错误，断言租约字段清空；基线对照：回退成裸 `?` 时该
测试失败 `left:"started"`）。

> 写新代码时的自查法：把 `run_check_once` 里 `claim_for_exec` 之后的每个 `?` / `return` 逐个数一遍，
> 确认它们后面跟着的是 `fail_check_run`。

**终态写入同样带 owner 守卫，并在同一条语句里释放租约**：

| 路径 | 语句 | 0 行时 |
| --- | --- | --- |
| 成功 | `finish_check_run_owned_in`（与结果同一事务）：`SET status='success', finished_at=now(), exec_owner=NULL, exec_lease_until=NULL WHERE id=$1 AND exec_owner=$2` | **整事务回滚**——绝不把结果与 `success` 提交到别人拥有的 run 上 |
| 失败 | `fail_check_run(owner, reason)`：同样守卫 + 释放租约 | 只 warn「租约已被接管，不覆盖新主人」 |

**实现陷阱（本轮实翻车，值得记）**：`persist_check_run_in` 在**同一个事务里**已经把
`status='success', finished_at=now()` 写掉了（M2.1 的「结果+终态同事务」）。所以终态守卫**不能**
再要求 `finished_at IS NULL`——第一次实现就是这么写的，测试立刻报
`execution lease lost before the terminal write`，因为该条件永远不成立。正确做法是：终态归属由
**租约 owner** 判定，由事务回滚兜底。

### 10.3 证据（含基线对照）

| 测试 | 内容 | 结果 |
| --- | --- | --- |
| `exec_claim_is_exclusive` | 6 路并发认领同一 run（独立连接池），断言**恰好 1 个赢家**，且行里记录的 `exec_owner` 就是它 | 通过；不依赖时序 |
| `concurrent_executions_of_one_run_id_hit_miaospeed_once` | 两个 handler 同时执行**同一 `run_id`**（屏障 + 两个独立池），mock MiaoSpeed 把首个请求挂 400ms；断言 checker **只收到 1 次** submit、输家 `Ok`、run 终态 `success`、结果 1 行、租约已清 | 通过 |
| **基线对照** | 把 worker 换回 `e1d5105` 的 `check_job.rs`/`lib.rs` 跑同一测试 | **`left: 2, right: 1`** —— 旧代码确实打两次，测试有区分力，不是「永远绿」 |
| `hung_checker_times_out_and_releases_the_lease` | mock 接了请求但 30s 不回终态帧；把 `PHLOGO_CHECK_TIMEOUT_SECS` 设成 1s → 断言 `run_check_once` 返回 `Err`、耗时 < 10s、只发生 1 次 submit、run 变 `failed`、`exec_owner`/`exec_lease_until` 均已释放 | 通过（这才是「互斥不会被一次挂住的测活破坏」的实测证据） |
| `db_error_after_checker_releases_the_execution_lease` | 用**真实 DB 错误**打掉「测活返回之后」那一步：测试在 mock 回帧后对 `nodes` 取 `ACCESS EXCLUSIVE` 锁，worker 侧池带 `lock_timeout=300ms` → `existing_ids` 超时失败；断言 `Err`、只 1 次 submit、`status='failed'`、租约字段清空 | 通过（**基线对照**：回退成裸 `?` 时该测试失败 `left:"started" / right:"failed"`） |
| `every_early_exit_after_the_exec_claim_releases_the_lease` | **源码形状守卫**：扫描 `run_check_once` 里「拿到租约之后」的每一处早退（`return Err(..)` **和** `?` 终止的语句——带或不带结尾 `;`），要求每处都在**所属语句块内**先 `fail_check_run` | 通过（**两个控制实验**：注入 `?;` 形状的泄漏 → 守卫**点名那一行**；注入**合法**的（已守卫）早退 → 守卫**不误报**） |
| `failed_push_releases_the_delivery_claim` | 把 storage 指向一个**不存在的数据库**让 push 必然失败 → 断言调度器派生的那一行回到 `pending`（而不是停在 `dispatching`），且可立即重新认领 | 通过（证明 push 失败不会把批次永久卡住） |

### 10.4 恢复的真实边界（S73 修正，别过度声称也别过度悲观）

1. **执行侧崩溃是可以被自动恢复的**——但不是靠我们的扫描器，而是靠 apalis 自带的孤儿重投：
   apalis-sql 0.7.4 默认 `reenqueue_orphaned_after=300s`，worker 心跳（`keep_alive=30s`）停 300s 后，
   它持有的 `Running` job 会被重置回 `Pending` 重新投递；因为执行租约（240s）**短于**这个窗口，
   那次重投能赢认领并把检查重跑。前提是「先崩的 worker 心跳真的停了」。
2. **仍然没有我们自己的 outbox / 过期租约扫描器**（投递租约与执行租约都没有）。由此留下的真实缺口：
   - **周期任务不会回头救旧周期**：下一周期用的是**新** `run_id`，上周期卡住的记录不会被下一周期
     重推；只有 apalis 的孤儿重投（上面 1）能救，且只针对「job 还在队列里、worker 心跳已死」的情形。
   - **队列行已被消费的情况**：若那条 job 已被标成 `Done`/`Killed`（例如某次投递在租约仍有效时
     按 no-op 返回了 `Ok`），后面就没人再投递它，run 会停在 `started`。§10.2 的三个不等式正是为了
     让这条路径不出现，但一旦有人改坏常数（例如把租约调回 900s），缺口就会重新打开。
   - 手动触发的恢复仍靠**同一 idempotency key 重发**（§9.3），这是唯一「人按一下就能救」的入口。
3. **幂等键不绑定请求内容**：服务端不保存 `node_ids`/`batch_size` 摘要，同 key 配不同请求会让
   batch k 对应不同节点集合。执行侧仍按 `run_id` 去重（先到者执行），所以不会重复测活，但
   「恢复」的语义不完整。后续应为 key 保存请求摘要（fingerprint）。
4. **投递侧（`push_state='dispatching'`）的过期租约也没有人回收**（S76 实测确认）：迁移 0014 建了
   `idx_check_runs_push_recovery` 索引，但**没有任何代码查询它** —— 没有扫描器、没有后台任务。
   后果具体是：
   - `enqueue_scheduled` 里 `storage.push(job).await?` 失败 → 该批次停在 `dispatching`，租约 60s 后过期；
   - 但**周期调度不会回头重推它**：下一周期用的是**新** `run_id`（`scheduled_run_id(schedule_id, batch)`
     按周期 label 派生），旧 run_id 再也不会被 `claim_for_push` 命中；
   - 所以一个 push 失败的批次，**在周期任务里就是永久丢失**（节点要等下一周期才被重新测活，
     因为每周期都会按新 run_id 重新调度同一批节点——所以业务上「不会漏测」，但那一轮的
     `check_runs` 行会永远停在 `dispatching`）；
   - **手动触发**则可以救：同一 `idempotency_key` 重发 → 同一个 label → 同一个 `run_id` →
     `claim_for_push` 命中「过期 dispatching」→ 重推。
   ⇒ 结论：**投递侧没有自动恢复；可自动恢复的只有「同一幂等键重发」。** 这与执行侧不同
   （执行侧靠 apalis 孤儿重投自愈，见 §10.4.1）。要统一，得写那个扫描器（用现成的
   `idx_check_runs_push_recovery` 索引），或让周期调度在派发前先回收本周期 label 下的旧行。
   > **S76 已把「push 失败」这一最常见的入口堵上**：`storage.push()` 出错时立刻
   > `release_push_claim()` 把该行从 `dispatching` 放回 **`pending`**（owner 守卫），于是下一次尝试
   > 可以**立即**重试，不必等 60s 租约过期；而「租约自然过期、但没人再推」的那种（进程在 push
   > 之后、mark 之前硬死）仍然无自动回收 —— 那才是需要扫描器的那一类。测试
   > `failed_push_releases_the_delivery_claim`。
   >
   > **S80 实测量化了这条缺口的实际影响**（校机 dev 库）：44 行处于「过期 `dispatching` +
   > `status='started'`」，而生产队列里对应的 job **根本不存在**（只有 1 条 job）—— 即这些是
   > **纯粹的死行**。但**节点不会因此漏测**：`schedulable_node_ids` 按 `nodes.membership='Linked'`
   > 全量取候选、**不看历史 run 行**，下一周期照常用新 `run_id` 重新调度同一批节点。
   > ⇒ 这类死行是**可观测性/运维噪声**（`check_runs` 里永远停在 `started` 的行），不是漏测。
   > 要清理它们需要一个按 `push_state`/`lease_until` 的运维查询或定期清理任务（登记为遗留）。
5. **fetch（抓取）侧有同一类问题，但不在 M3 范围**（S76 实测记录，未修）：`handle_fetch` 把错误吞进
   日志（`let _ = registry::reconcile_fetch_result(..)`），返回 `()`；而 fetch worker **没有**配
   `.retry(RetryPolicy::..)`（check 侧在 M3.1 裁定 3 已经改成「返回 Err + RetryPolicy(3)」）。
   ⇒ 抓取失败时 Apalis 认为任务**成功**、不会重试，只留一条 `source_fetch_runs` 失败记录；
   下一个 5 分钟周期会重新入队，所以业务上不会永久漏抓，但**单次失败没有重试**。
   修法与 check 侧同构（`handle_fetch` 返回 `Result` + worker 加 retry 层），属 P2/fetch 阶段，
   需要另行裁定（不在 M3 的「小范围」内，本轮不擅自扩范围）。

### 10.5 门禁（S79 实测）

worker **22**（`check_wiring` 20，含 15 项故障注入：周期身份 / 并发认领 / 并发恢复过期租约 / 重投不再重复执行 /
状态错误传播 / 同键恢复 / 错钥排除 / handler Err / RetryLayer 真实重投 / 执行租约互斥 / 同一 run_id 只打
一次 / 挂死测活超时 / 测活后 DB 错误 / 早退形状守卫 / push 失败释放认领）/ curse **37** / database **4** /
distribution **43** / adapter **18** / checker **11** 全绿；
workspace `rustfmt --check` + `clippy --all-targets -- -D warnings` **0**。迁移 0001…0015 在一次性库
`phlogo_mig_scratch` 全序执行 → (push_state 三列 3、`pushed` 0、恢复索引 1、configs 2、执行租约两列 2、
执行租约索引 1) → `0015.down` 只清执行租约（两列 0、索引 0、push_state 三列**仍为 3**）→ `0014.down`
→ `0013.down` → 删库。一次性 `phlogo_jobs_test_m3` 建库→测试→销毁（**删除由链内断言校验：
`TESTDB_LEFT` 计数必须为 0**，S73 修掉了「内联长链尾部不执行却 EXIT=0」的假通过），生产
`phlogo_jobs` 零接触。

## 11. 遗留

1. **增量测活**（只测新/变过的节点）、连续失败降级、评分接入（M5）未做——裁定明确不混入 M3。
2. `enqueue_for_active_sources`（fetch）仍是每 5 分钟全量 active 源入队（无 period 去重）；
   fetch 本身幂等性可接受，但如需同策略可复用 `enqueue_scheduled` 模式（另行裁定）。
3. 深度矩阵/协议载荷保真属 **M4**（**冻结中**，裁定不进入）；`InvokeDuration` 归一化接 p3-03 也在 M4。
4. 投递语义是 **at-least-once**：崩溃窗口内可能有重复 push；执行侧已由**原子执行租约**收口（§10），
   所以同一 `run_id` 只会有一个执行者进 MiaoSpeed。
5. **没有我们自己的统一 outbox / 过期租约恢复扫描器**（投递租约与执行租约都没有）。执行侧崩溃靠
   apalis 的孤儿重投（300s，见 §10.4.1），周期任务不会回头救旧周期。**投递侧的过期租约连 apalis 也
   救不了**（队列里根本没有那个 job），只能靠同一幂等键重发——见 §10.4.4。要「无人值守地恢复所有
   过期未完成 run」就需要该扫描器（`idx_check_runs_push_recovery` 索引已就位），本轮按裁定未加。
6. 常数是一组**互相约束**的值，改动前先读 §10.2 的不等式链：投递租约 60s（`DELIVERY_LEASE_SECS`）、
   执行租约 240s（`EXEC_LEASE_SECS`）、单次执行超时 180s（`CHECK_RUN_TIMEOUT_SECS`）、
   apalis 孤儿窗口 300s（`APALIS_ORPHAN_REENQUEUE_SECS`，我们**显式设置**而非依赖默认）与
   `APALIS_KEEP_ALIVE_SECS=30`。其中「timeout < 租约 < 孤儿窗口」由**编译期断言**强制；改任何一个
   都会先编译失败，再谈运行行为。
7. **幂等键未绑定请求摘要**：同 key 配不同 `node_ids`/`batch_size` 不报错、也不区分；后续应为
   `manual-<key>` 保存请求 fingerprint 并在不匹配时拒绝（412/409）。
8. **投递侧死行需要运维清理**（S80 实测）：校机 dev 库里有 44 行「过期 `dispatching` +
   `status='started'`」且队列里无对应 job —— 永远不会被认领、也永远不会变终态。节点不受影响
   （见 §10.4.4），但这些行是噪声。清理方式（运维 SQL / 定期任务 / 迁移）待定。
8. **M4 深度矩阵上线时**：P3 设计稿记的是 30 分钟硬超时（上游 `context.WithTimeout(30*time.Minute)`
   等价），当前实现只有 180s（够 ping 批次）。必须**同时**抬高 timeout 与执行租约（或改成执行期心跳
   续租），并重新核算它与 apalis 孤儿窗口的关系——否则 §10.2 的链会断。
