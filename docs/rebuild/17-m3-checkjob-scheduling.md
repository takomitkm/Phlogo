# 17 M3 · CheckJob 调度（周期 + 手动触发）

> 状态：**M3 主体完成（S68）+ M3.1 收尾（S69）+ M3.1-2 再收紧（S71，复审二轮裁定落地）**。
> 基线 `9767908`（M3 主体）→ S69 `4abd0fd` → S71 见 ACTION-LOG。
> M3.1 依据（2026-11-20 复审）：多实例周期身份、真实可解密筛选、handler 失败重试、
> 崩溃窗口恢复、四类故障注入测试。
> M3.1-2 依据（2026-11-20 二轮复审）：S69 的 `pushed` 布尔与 `New/LostBeforePush` 三态被否
> （并发竞争 + 永久漏推窗口），要求重写为带租约的投递状态机、timer 真正锚到全局边界、
> 手动触发可恢复、0013 迁移复原。见 §9。

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
**at-least-once**，这是明确接受的设计，不是 bug。重复执行由**执行侧**闸门挡住：

```
run_check_once: record_check_run(...) 之后，若 check_runs.status='success' → 直接 Ok(()) 返回
```

所以「队列里有两条同一 run_id 的 job」不会变成「两次真实 MiaoSpeed 测活」。**跨主库与 jobs 库
没有分布式事务，因此绝不声称 exactly-once**；对外语义一律写 at-least-once + 执行侧一致。

**错误不吞**：`claim`/`mark_enqueued` 的 DB 错误一律 `?` 上抛（原 `let _ = mark_pushed` 已删除）。
owner 守卫导致 0 行的情形不是错误，但会**明确记 warn**（「租约已被接管，本批会被当前持有者再推一次」），
不留静默分支。

**Apalis 0.7.4 没有确定性 job id**（查证：`apalis.jobs.id` 由存储生成，`push` 不接受调用方 id），
所以本轮不假设它存在，重复投递只靠上面的执行侧闸门收敛。

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
但「恢复」可能只覆盖部分节点。需要严格一致时应由服务端 outbox 扫描器接管（未做，见 §10）。

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

## 10. 遗留

1. **增量测活**（只测新/变过的节点）、连续失败降级、评分接入（M5）未做——裁定明确不混入 M3。
2. `enqueue_for_active_sources`（fetch）仍是每 5 分钟全量 active 源入队（无 period 去重）；
   fetch 本身幂等性可接受，但如需同策略可复用 `enqueue_scheduled` 模式（另行裁定）。
3. 深度矩阵/协议载荷保真属 **M4**（**冻结中**，裁定不进入）；`InvokeDuration` 归一化接 p3-03 也在 M4。
4. 投递语义是 **at-least-once**：崩溃窗口内可能有重复 push，靠执行侧 `success` 闸门收敛；需要
   「无人值守地恢复所有过期未投递 run」时，应加统一 outbox/恢复扫描器（本轮按裁定只做
   client-key 恢复，未加扫描器）。
5. 租约 60s（`DELIVERY_LEASE_SECS`）：投递链路耗时若长期超过它，会出现重复 push（仍被执行侧挡住），
   需要按部署环境调大。
