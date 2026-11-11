## 3-01 Apalis Check Job 入队与执行 · Apalis Check Job — Enqueue & Execute

### 目标 / Goal

在 `core-worker` 内用 **Apalis（PostgreSQL backend）** 承载测活任务的调度，替代 Phlogo 当前被动等待外部
`paily-check` 通过 `POST /api/v1/check/results` 回调的模式（见 `internal/checker/handler.go` `PostResults` 被动监听）。
本步骤只解决「**任务如何被创建、被 Worker 领取、如何重试/超时/恢复**」，不涉及 MiaoSpeed 协议（见 p3-02）与结果持久化（见 p3-04）。

要点：

1. 定义可入队的 `CheckJob`（携带 `run_id`、`checker_tag`、`node_ids`、`presets`）。
2. 由 API / 周期调度器通过 Apalis `Monitor` 入队；Worker 注册 handler 领取并执行。
3. 失败触发**幂等重试**；长任务有**超时**；Worker 重启可**恢复**未完成任务。
4. **Apalis Job 状态与 Node Lifecycle 状态严格分离**（方案 §3.1）——Job 只管「这批测活跑没跑完」，不表示节点 alive/dead。

### 范围 / Scope

- 新建/修改：`apps/worker`（Apalis Worker 装配）、`crates/checker-miaospeed`（Job 定义与其 handler 入口）、
  `migrations/`（引入 Apalis 官方 PostgreSQL migration，见前置）。
- 不在此步：MiaoSpeed WebSocket 协议细节（p3-02）、结果解析与 Node ID 映射（p3-03）、历史持久化（p3-04）、评分与缓存重建（P4/P5）。
- **不做**：把节点健康状态写进 Apalis 任务表；不手写猜测 Apalis 内部表结构（方案 §3.1）。

### 前置 / Prerequisites

- 已完成 P0-03 spike（`01-p0-upstream-verification/p0-03-apalis.md`）：固定 Apalis 版本、PostgreSQL backend、retry/timeout、任务恢复、Board 兼容性均已验证。
  > ⚠️ 下方具体 API 名（`Job` derive、`PostgresStorage`、`Monitor::push`、`WorkerBuilder::register`、retry/timeout 方法名）为**预期形态**，
  > 确切签名以 P0-03 固定版本为准；写代码前先回填到本页。
- P1 已完成：`core-worker` 骨架、SQLx + PostgreSQL、`database` crate 可取得 `PgPool`（`02-p1-core-skeleton-data/`）。
- `node_ids` 对应的 `nodes` 行已存在（`02-p1-04-source-node-crud.md` 保证每个内部节点有稳定 `Uuid`）。

### 步骤 / Steps

#### Step 1 — 引入依赖并应用 Apalis 官方 migration

Apalis 的 PostgreSQL 任务表由官方 migration 创建，**不要手写 `CREATE TABLE apalis_*`**（方案 §3.1）。

```toml
# crates/checker-miaospeed/Cargo.toml （或 apps/worker）
apalis = { version = "<P0-03 固定的版本>", features = ["postgres", "tracing"] }
uuid = { version = "1", features = ["serde", "v4"] }
serde = { version = "1", features = ["derive"] }
```

应用 migration（与业务 migration 一起跑）：

```bash
# 在已 `sqlx database create` 的 Postgres 上执行 Apalis 官方 migration
# 具体路径以 P0-03 固定版本的 apalis 包内 migrations 为准
cargo sqlx migrate run
# 仅校验离线查询与 migration 是否匹配（CI 使用）
cargo sqlx prepare --check
sqlx migrate run --check
```

#### Step 2 — 定义 `CheckJob`

`run_id` 即方案 §5.1 `check_runs.id` 与 Phlogo `CheckerRun.ID`，全链路唯一，**重试幂等的依据**（见 p3-04）。

```rust
// crates/checker-miaospeed/src/job.rs
use apalis::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Job, Serialize, Deserialize, Clone, Debug)]
#[job(name = "check")] // 确切 attribute 名以 P0-03 固定版本为准
pub struct CheckJob {
    /// 与 check_runs.id 一致，重试幂等键
    pub run_id: Uuid,
    /// 测活视角；Phlogo 的 checker_tag（如 "default" / "miaospeed"）
    pub checker_tag: String,
    /// 本次批次要测的节点（内部 Uuid）
    pub node_ids: Vec<Uuid>,
    /// 测活预设名；映射 MiaoSpeed 测试集合，具体名以 P0-02 验证（方案 §8.1，勿硬编码未验证 Matrix 名）
    pub presets: Vec<String>,
}
```

#### Step 3 — 入队（enqueue）

由 API handler 或周期调度器触发。下面的 `monitor` 来自 `PostgresStorage`。

```rust
// apps/api 或 apps/worker 内的调度入口
use apalis::prelude::*;
use apalis::postgres::PostgresStorage;

pub async fn enqueue_check(
    storage: &PostgresStorage<CheckJob>,
    run_id: Uuid,
    checker_tag: String,
    node_ids: Vec<Uuid>,
    presets: Vec<String>,
) -> Result<(), apalis::Error> {
    let monitor = storage.monitor().await?; // 确切方法名以 P0-03 为准
    monitor
        .push(CheckJob { run_id, checker_tag, node_ids, presets })
        .await?;
    Ok(())
}
```

手动触发（调试 / 测试）可直接通过 API `POST /api/v1/checks`（方案 §11）入队；周期触发由 Worker 内定时器发起。

#### Step 4 — Worker 注册与执行 handler

```rust
// apps/worker/src/main.rs （节选）
use apalis::prelude::*;
use apalis::postgres::PostgresStorage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let pool = PgPoolOptions::new().connect(&db_url).await?;
    let storage = PostgresStorage::new(pool);
    storage.setup().await?; // 确保 Apalis 内部表就绪

    let worker = WorkerBuilder::new(storage.clone())
        // 注册 handler；retry/timeout 策略以 P0-03 固定版本 API 为准
        .register::<CheckJob>(check_job_handler)
        .build();
    worker.run().await?;
    Ok(())
}

// handler：仅编排，真实测活交给 checker-miaospeed Adapter（见 p3-02 / p3-03 / p3-04）
async fn check_job_handler(job: CheckJob, data: Data<AppState>) -> Result<(), Error> {
    let run = data.checker.run_check(&job).await?; // 跨 p3-02/03/04 的编排入口
    Ok(())
}
```

#### Step 5 — 重试 / 超时 / 恢复语义

- **重试（retry）**：对可重试错误（网络抖动、MiaoSpeed 暂时不可达）配置有限次重试 + 退避。
  重试必须**幂等**——handler 用 `run_id` 去重（见 p3-04 `ON CONFLICT (check_run_id)`）。
  > 确切 retry/backoff API 以 P0-03 固定版本为准（预期形如 `.retry(Retry::retries(3))` 或 worker 级 `with_retry`）。
- **超时（timeout）**：单次 CheckJob 设硬超时（Phlogo 原 goroutine 用 `context.WithTimeout(30*time.Minute)`，见 `internal/checker/handler.go:123`）。
  超时视为失败 → 触发重试或标记 `check_runs.status = 'failed'`。
- **恢复（recovery）**：Apalis PostgreSQL backend 在 Worker 崩溃后把 in-flight job 重新排队（方案 §3.1）。
  因此 handler 必须能安全地被重复执行（幂等），不能假设「只跑一次」。

#### Step 6 — Job 状态 ≠ 节点状态（关键边界）

- Apalis 任务表记录 `pending/running/done/failed` —— 这是**任务调度**状态。
- 节点 alive/dead/unknown 来自结果处理（p3-03），落在 `node_status`，由 `node_alive_check_window_minutes`（默认 240）窗口与跨 tag 合并计算（p3-03）。
- 不要因为「任务 done」就推断节点 alive，也不要把节点状态写进 Apalis 内部表（方案 §3.1、§6.4）。
- Phlogo 对照：`PostResults` 用 `checkerTag := middleware.CheckerTagFromCtx(c)` 注入 tag，用 per-tag mutex 串行化；
  Rust 侧改为 Job 携带 `checker_tag`，由 Apalis 天然提供去重与并发控制，无需手搓 mutex。

### 验证结果 / Verified Results（已实测，2026-11-11）

> 依据 P0-03 spike 的 0.7.4 API 实测形态：`PostgresStorage::new(pool)` + `PostgresStorage::setup(&pool)`（官方 migration）+
> `Monitor::new().register(WorkerBuilder::new(..).enable_tracing().data(...).backend(storage).build_fn(handler))` + `storage.push(job)`。
> `postgres` feature 在 **`apalis-sql`** 上（`apalis` 0.7.4 本体无此 feature）。

**落地**：
- `apps/worker` 改 lib+bin：`CheckJob{run_id, checker_tag, node_ids, presets}`（serde，即 Apalis 任务载荷）；
  `record_check_run`（`INSERT .. ON CONFLICT (id) DO NOTHING`，返回是否首投）与 `finish_check_run`（`WHERE finished_at IS NULL` 单向转移）——**at-least-once 重投全部幂等**；
  `handle_check` 为 Apalis handler（编排入口，真实 MiaoSpeed 调用留 p3-02+）。
- migration `0004_check_runs`（主库；`paily_jobs` 承载 Apalis 官方表）。
- **关键决策：Apalis 用独立数据库 `paily_jobs`**（`PAILY_DATABASE_JOBS_DSN` 覆盖，默认由主 DSN 改库名派生）——
  apalis 0.7.4 的 `setup()` 内嵌 sqlx migration 与业务 `migrate!` 共用 `_sqlx_migrations` 版本序列会冲突
  （实测 `Migrate(VersionMissing(1))` panic）；独立库既解决冲突又满足方案 §3.1「业务表与 Apalis 内部表严格分离」。

**实测（校机，PG 18.6）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0**（42.54s） |
| `cargo test --workspace` | **0**——`worker::check_job::no_duplicate_work`（重复投递 4 次仅 1 行 `check_runs`、finish 单向转移）+ 既有全部测试 |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |
| **worker 真机 smoke（12s）** | `paily_jobs` 中 apalis 官方表建立；主库 `check_runs` 计数 2→**3**（handler 消费了新入队 demo job，`finished_at` 非空） |

**验收对照**：#1 入队并执行 ✓（真机 smoke）；#2/#3 幂等重试/无重复副作用 ✓（集成测试，`run_id` 键控）；#4 worker 重启恢复——由 apalis PG backend 的 at-least-once + 本页幂等副作用保证，进程级 kill/restart harness 留 CI（P0-03 已标注）。

**留待后续**：retry/timeout 层（`.retry()`/`.timeout()`）在 p3-02 接真实 MiaoSpeed 调用时一并配；`node_check_results`/`node_status` 持久化在 p3-04。

### 验收 / Acceptance

1. **入队并被执行**：
   ```bash
   cargo run -p worker &
   # 用 API 或测试入队一个 CheckJob，断言 handler 被调用
   cargo test -p checker-miaospeed --test check_job_enqueue
   ```
   预期：测试 pass（exit 0）；日志出现 `run_id` 且 handler 执行一次。

2. **失败触发幂等重试**：
   ```bash
   cargo test -p checker-miaospeed check_job::tests::retry_is_idempotent
   ```
   预期：故意让 handler 前两次失败，第三次成功；断言 `check_runs` 中该 `run_id` 仅 1 行、`node_check_results` 行数不随重试翻倍（幂等由 `run_id` + `ON CONFLICT` 保证，见 p3-04）。

3. **无重复工作（no duplicate work）**：
   ```bash
   cargo test -p checker-miaospeed check_job::tests::no_duplicate_work
   ```
   预期：同一 `run_id` 被投递两次（模拟 at-least-once），handler 的「业务副作用」（如写入 `check_runs`）只发生一次。

4. **Worker 重启可恢复**：kill worker 进程后重启，未完成的 CheckJob 在超时/恢复窗口内被重新领取并执行完成。
   预期：退出码 0；`check_runs` 中出现对应 `finished_at` 非空行。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo | Rust（本步骤） |
|---|---|
| `internal/checker/handler.go` `PostResults`：`go func(...){...}()` 异步 goroutine + `context.WithTimeout(30*time.Minute)` + `tagLock(tag)` per-tag mutex（`:118-124`、`:53-56`） | Apalis `CheckJob` + `PostgresStorage` + `WorkerBuilder::register`；timeout 改为 Job 级超时；并发/去重由 Apalis 提供 |
| `internal/checker/handler.go:116` `checkerTag := middleware.CheckerTagFromCtx(c)`：从 HTTP 中间件注入 `checker_tag` | `CheckJob.checker_tag` 字段随任务携带，不再依赖 HTTP 中间件 |
| `internal/db/model/node.go` `CheckerRun{ID, CheckerTag, NodeCount, CompletedAt}` | `CheckJob.run_id` == `check_runs.id`（方案 §5.1 增加 `status`/`external_job_id`/`started_at`/`finished_at`，见 p3-04） |
| 触发方式：外部 `paily-check` 主动 `POST /api/v1/check/results`（被动） | Worker 主动 `Monitor::push(CheckJob)`（主动编排，见 p3-02 提交） |

### CI 提示 / CI notes

> 仅描述未来 CI 对应的命令/Job，**不落地 YAML**。

- **单元测试**（Apalis 行为，含 retry/idempotent/无重复）：
  `cargo test -p checker-miaospeed check_job`（建议用 `MemoryStorage` 跑纯逻辑，Postgres backend 跑在 integration job）。
- **集成测试**（真实 Postgres + Apalis PG backend）：放在 `cargo test --test integration`，由 CI 起一个 Postgres service container。
- **migration 一致性**：`sqlx migrate run --check` 与 `cargo sqlx prepare --check` 必须过（防止手写 Apalis 内部表）。
- **lint/build**：`cargo build --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`（见 `CONVENTIONS.md` §5）。

### 风险 / Risks

- **Apalis API 细节未冻结**：derive macro 名、retry/timeout/backoff、monitor 方法名、Board 兼容性均依赖 **P0-03 固定版本**；本页所有具体调用以「P0-03 回填」标注，先验证再落地代码。
- **幂等边界**：Apalis 为 at-least-once，handler 必然被重复执行；若 p3-04 的 `ON CONFLICT (check_run_id)` 漏写，重试会把 `node_check_results` / `check_runs` 翻倍——第 2/3 条验收即为此兜底。
- **超时 vs 重试放大**：30min 超时 + 重试会显著拉长任务停留时间，需确认 Apalis 的 `retry` 计数与 `timeout` 不叠加成超长占用（P0-03 验证恢复时间）。
- **Job 状态外溢到节点状态**：务必守住 §3.1 边界，避免把 `check_runs.status` 当作节点健康（见 Step 6）。
