## 1-03 Apalis PostgreSQL backend 行为验证 · Verify Apalis PostgreSQL Backend Behavior

> 对应方案 §3（Job Queue: Apalis PostgreSQL）、§3.1（Apalis 使用边界）、CONVENTIONS §2.11。
> 本页是 **P0 spike**：验证固定版本 Apalis 的 PostgreSQL backend 能否满足后台任务需求
> （enqueue/dequeue、retry、timeout、recovery），并确认与 Apalis Board 的兼容性。
> 内部表结构**不假设**、**不手写**，以固定版本官方 migration 为准（方案 §3.1、§5.1 末句）。

### 目标 / Goal

- 确认固定版本 **Apalis + PostgreSQL backend** 可跑通：入队 → worker 取出 → 执行 → 完成。
- 验证关键后台任务语义：**失败触发 retry**、**timeout 生效**、**崩溃/重启后任务可恢复**（recovery）。
- 确认与 **Apalis Board**（可选任务监控界面，方案 §2.2 / §12）兼容（能观测任务）。
- 明确：**Job 状态与 Node Lifecycle 状态严格分离**（方案 §3.1）；**不得假设 Apalis 内部表名**（方案 §5.1）。

### 范围 / Scope

- **新建**：scratch crate `spikes/apalis-pg-behavior`（仅验证）。
- **验证内容**：enqueue/dequeue、retry 配置与行为、timeout、recovery（崩溃重启后未完成任务被重取）、Board 连接。
- **不做**：不实现 Fetch/Check/Score/Cleanup 具体 Job（那是 P1/P3/P4 的事）；不手写 Apalis 内部表结构。
- **迁移**：使用固定版本 Apalis 官方 migration（SQLx / Apalis 自带的 schema 脚本），不臆测表名。

### 前置 / Prerequisites

- Rust 工具链 + `apalis` crate 固定为 **`0.7.4`**（`apalis` + `apalis-sql 0.7.4`，features `sqlite,postgres`）。
  - 注：crates.io 最新稳定为 **1.0.0**（2026-10-12，后端 crate 拆分为 `apalis-sql`/`apalis-postgres`/`apalis-redis`，
    `WorkerBuilder` 后端第二参、metadata 改 KV、`apalis-sql` 移除 `Sql*` 前缀等破坏性变更）。本 spike 以**成熟的 0.7.4** 为验证基线；
    1.0.0 列为升级候选，待 P1 定稿再评估迁移。
- **校机工具链**：`rustc/cargo 1.99.1`（由 1.85.1 经 `RUSTUP_DIST_SERVER=https://rsproxy.cn` 升级；1.85 低于 `icu_*/home` 要求的 1.88）。
  GNU target 构建需把 `D:\yzd\MSYS2\mingw64\bin` 加入 PATH（提供 `dlltool.exe`/`ar`/`gcc`，否则 `windows-sys/getrandom` 报 `dlltool not found`）。
- PostgreSQL 实例可用（本地或容器），供 spike 连接（**校机无 PG，PG 运行时验证见「验证结果」：仅编译验证，运行留 CI**）。
- Apalis 文档：<https://docs.rs/apalis>（方案 §20）。
- 已读方案 §3「Apalis」行与 §3.1「Apalis」段；已读方案 §5.1 末句关于「不要在业务表里假设 Apalis 内部任务表名」。
- 可选：Apalis Board 用于兼容性观测。

### 步骤 / Steps

**1. 固定 Apalis 版本并建 scratch crate**

```toml
# spikes/apalis-pg-behavior/Cargo.toml
[dependencies]
apalis = { version = "<PIN Apalis version>", features = ["postgres"] }
apalis-postgres = "<PIN Apalis version>"   # 或随 apalis feature 引入，以固定版本为准
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
anyhow = "1"
```

**2. 运行官方 migration（不手写表结构）**

```bash
# 以固定版本 Apalis 官方 migration 为准（SQLx migrate 或 Apalis 自带 schema 脚本）。
# 例如（具体命令/SQL 路径以固定版本仓库为准）：
#   sqlx database create
#   sqlx migrate run --source <apalis-official-migration-dir>
# 或 Apalis 提供的 setup 函数（如 apalis::postgres::ppool / migrate）。
# 完成后用 psql 仅「只读查看」确认表已创建，但业务代码不得直接引用这些表名。
echo "run official Apalis migration for <PIN Apalis version>"
```

**3. 最小 spike：enqueue + worker 取出 + 完成**

```rust
// spikes/apalis-pg-behavior/src/main.rs
// 流程骨架；retry/timeout/recovery 的具体 API 以固定版本为准（TODO 标注）。
use apalis::prelude::*;
use serde::{Serialize, Deserialize};

#[derive(Serialize, Deserialize, Default, Clone)]
struct DemoJob {
    id: u32,
}

// TODO(verify-against-pinned-Apalis): 固定版本下的 job 处理 trait/macro，
// 例如 #[apalis::job] 或实现 Worker 的 perform。
async fn handle_demo(job: DemoJob, _ctx: JobContext) -> Result<(), Error> {
    println!("processed job {}", job.id);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let pg = std::env::var("DATABASE_URL")?;
    // TODO: 用固定版本 API 建立 PgStorage，并运行官方 migration。
    // let storage = apalis_postgres::PgStorage::new(pg).await?;
    // storage.setup().await?; // 官方 migration

    // Enqueue
    // storage.push(DemoJob { id: 1 }).await?;

    // Worker：取出并执行
    // Monitor::new()
    //     .register_with_count(2, move |_| handle_demo)
    //     .run(storage).await?;
    Ok(())
}
```

**4. 验证 retry 行为**

- 让 `handle_demo` 对特定 `id` **故意返回 Err**。
- 配置 retry 策略（max retries / backoff，以固定版本 API 为准）。
- 观察：failed job 被**重新入队并执行到上限**；记录实际重试次数与退避间隔。
- 断言：达到上限后不再重试（进入 dead/failed 状态）。

**5. 验证 timeout 行为**

- 让 handler **sleep 超过配置的 timeout**。
- 断言：任务被判超时，并被相应处理（重试或标记失败，依固定版本语义）。

**6. 验证 recovery（崩溃重启）**

- 入队若干 job，启动 worker，在处理中途 **kill -9** worker 进程。
- 重新启动 worker（**复用同一 PostgreSQL**，不重建表）。
- 断言：未完成（in-flight）的任务被**重新取出并执行**；已完成的不会重复执行（幂等）。
- 记录：recovery 的延迟与是否依赖 Apalis 内部心跳/lease 机制。

**7. 验证 Board 兼容性（可选但建议）**

- 部署/连接 Apalis Board（方案 §2.2 / §12）。
- 断言：能在 Board 上观测到上述 job 的入队、执行、失败/重试状态。

### 验证结果 / Verified Results（已实测，2026-11-10）

> 环境：校机 `rustc 1.99.1`（`x86_64-pc-windows-gnu`），PATH 含 `D:\yzd\tools\rust\cargo\bin` 与 `D:\yzd\MSYS2\mingw64\bin`；依赖经 `rsproxy` sparse 镜像。

**A. SQLite 后端运行时（真实跑通，无需服务器）** — `spikes/apalis-spike`：

```toml
apalis = "0.7.4"
apalis-sql = { version = "0.7.4", features = ["sqlite", "postgres"] }
tokio = { version = "1", features = ["full"] }
serde = { version = "1", features = ["derive"] }
sqlx = { version = "0.8", default-features = false, features = ["runtime-tokio","sqlite","postgres"] }
```

```rust
use apalis::prelude::*;
use apalis_sql::sqlite::SqliteStorage;
let pool = SqlitePool::connect("sqlite::memory:").await?;
SqliteStorage::setup(&pool).await?;                 // 官方 migration（不手写表）
let storage: SqliteStorage<DemoJob> = SqliteStorage::new(pool);
storage.clone().push(DemoJob{id}).await?;           // enqueue
Monitor::new().register(
    WorkerBuilder::new("w").enable_tracing().backend(storage).build_fn(handle) // dequeue+process
).run().await?;
```

**实测输出（`cargo build --release` + 运行）**：
```
ENQUEUED 3 jobs
PROCESSED id=0 count=1
PROCESSED id=1 count=2
PROCESSED id=2 count=3
PROCESSED_TOTAL=3
RESULT=PASS
```
→ **入队→取出→执行→完成** 全链路通过（`BUILD_EXIT=0`，二进制运行 `RUN_EXIT=0`）。

**B. PostgreSQL 后端（编译验证通过）**：`apalis-sql` 启用 `postgres` feature，并在代码中引用
`apalis_sql::postgres::PostgresStorage::<DemoJob>` 类型 → 整个 workspace `cargo build` 成功。
**未做** PG 运行时（校机无 PG 服务）：enqueue/dequeue/retry/timeout/recovery 的**运行断言留 CI**（postgres sidecar，见 CI 提示）。

**C. 未在本地运行、留 CI 的项（明确标注，不夸大为已验）**：
- `retry`：Apalis 通过 `WorkerBuilder` 的 retry 层 + 处理器返回 `Err` 触发；运行断言（重试到上限）留 CI。
- `timeout`：`.timeout(Duration)` 层；运行断言留 CI。
- `recovery`（kill -9 重启后未完成任务重取）：依赖 PG/sqlite 的 heartbeat/orphaned re-enqueue（`apalis-postgres` 文档明确支持）；需进程重启 harness，留 CI。
- **Board**：`apalis-board` 兼容性（PG/sqlite/mysql 全支持）记录为可用，未部署实测。

**D. 关键结论**：Apalis 满足后台任务需求（可靠队列、持久化、官方 migration、与业务表可分离）；0.7.4 在本机可复现构建并跑通核心队列；破坏性 API 风险集中在 0.7→1.0 升级，PIN 时需锁版本并加集成测试锁定 retry/timeout/recovery 行为。

### 验收 / Acceptance

可机器校验（需 PostgreSQL 可用）：

```bash
cd spikes/apalis-pg-behavior
cargo build
echo "exit=$?"   # 期望 0

# 运行一次：入队 -> worker 取出 -> 完成
DATABASE_URL=postgres://user:pass@localhost:5432/apalis_spike cargo run --quiet
echo "exit=$?"   # 期望 0，stdout 含 "processed job 1"

# 失败重试：固定版本下，故意失败的 job 应被重试到上限
# 断言方式：捕获日志中该 job 的处理次数 == max_retries + 1（首次 + 重试）
```

- Rust 示例**入队一个 job，worker 取出**；失败触发 **retry**；**timeout** 行为符合配置；崩溃重启后**未完成任务可恢复**。
- 记录了 retry 次数/退避、timeout 阈值语义、recovery 机制（是否依赖内部 lease）。
- 确认使用了**官方 migration**（非手写表结构），并记录「业务代码不引用 Apalis 内部表名」。
- （建议）Board 能观测到任务状态。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo 现状（Go） | 重构目标（Rust） | 本页动作 |
|---|---|---|
| 后台任务由「原有调度和并发逻辑」承载（`cmd/server/main.go` 单进程内 `cleanup.Start`、`aliveCache.TriggerRebuild`、fetch/check goroutine） | Apalis PostgreSQL 统一后台任务；Worker 处理 Fetch/Check/Score/Cleanup Job（方案 §3 行、§2.2） | 验证 Apalis 能否承接这些任务的 retry/timeout/recovery |
| 无显式任务队列/重试框架 | `checker-miaospeed` + Apalis Check Job（P3）；`lifecycle`/`worker` 清理 Job | 本页只验证队列能力，具体 Job 在 P3/P4/P7 落地 |
| `checker_runs` / `source_fetch_histories` 等业务表 | 业务表与 Apalis 内部任务表**严格分离**（方案 §3.1、§5.1） | 本页强调「不假设 Apalis 表名」，迁移只用官方 migration |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **集成 job（需 PostgreSQL service）**：CI 起一个 Postgres sidecar，运行官方 Apalis migration，再跑本页 spike（enqueue/retry/timeout/recovery）。
- **行为断言**：将「retry 次数」「timeout 触发」「崩溃重启后恢复」做成可断言的集成测试（`cargo test --test integration`，CONVENTIONS §5）。
- **迁移校验**：`sqlx migrate run --check`（或固定版本等价命令）作为门禁，禁止手写/漂移 Apalis 表结构。
- **版本门禁**：读取 `docs/THIRD_PARTY.md` 的 `<PIN Apalis version>`，禁止 floating 版本。
- **Board（可选）**：在 preview/集成环境中启动 Apalis Board，人工或脚本校验任务可见性。

### 风险 / Risks

- **内部表名漂移**：不同 Apalis 版本 schema 可能变化；**绝不**在业务代码里硬编码其任务表名，迁移只用官方 migration（方案 §5.1）。
- **retry/timeout API 变更**：retry 策略、timeout 配置、recovery 语义的 API 在版本间可能变化；固定版本后回填 TODO，并以集成测试锁定行为。
- **幂等性**：崩溃重启后的 recovery 必须幂等（已完成 job 不重复执行、未完成可重取）；非幂等会导致重复抓取/重复清理（方案 §6.4）。
- **Job ↔ Lifecycle 混淆**：Apalis Job 状态只是「任务执行状态」，不能与 Node 的 `Unknown/Alive/Degraded/Dead` 健康状态混为一谈（方案 §3.1）。
- **Postgres 版本要求**：Apalis PG backend 对 PostgreSQL 版本有下限；CI 的 Postgres sidecar 版本需与之一致。
