## 3-04 测活历史持久化 · Check History Persistence

### 目标 / Goal

把一次测活批次的结果**持久化**到方案 §5.1 的新表 `check_runs` / `node_check_results` / `node_status`，
并把 Phlogo 的 `NodeInitialCheck` + `NodeDeepCheck` 合并写入统一的 `node_check_results`；保证**幂等重试**
（同 `run_id` 重跑是 no-op 或一致结果）。本步骤是 p3-03 输出的落库端。

要点：

1. 用 `sqlx::Transaction` 把「写 `check_runs` + 批量写 `node_check_results` + 写 per-tag `node_status`」包成原子事务（对应 Phlogo `PostResults` 的 Phase 1 事务，`internal/checker/handler.go:136`）。
2. 初筛 + 复筛合并为 `node_check_results` 一行（`metrics` 用 JSONB 承载 `latency_ms` / `avg_latency_ms` / `jitter_ms` / `speed_kbps` / `region` / `streaming` / `metadata`），`result_status` 标记 alive/dead/unknown。
3. per-tag `node_status` 写入遵循 p3-03 的「迟到不覆盖」规则（仅当 `checked_at` 更新才覆盖）。
4. **幂等**：以 `run_id` 为键，重试重跑不产生重复行。

### 范围 / Scope

- 新建/修改：`migrations/`（建 `check_runs` / `node_check_results` / `node_status`）、`crates/database`（SQLx 查询）、`crates/checker-miaospeed` 或 `crates/lifecycle`（持久化编排，调用 p3-03 的纯函数）。
- **映射对象**：`internal/db/model/node.go` 的 `CheckerRun` / `NodeInitialCheck` / `NodeDeepCheck` → 新表。
- 不在此步：评分（`ComputeAndUpdateTagScore`，P4）、缓存重建（`TriggerRebuild`，P5）、死源评估（`handleDeadNodes`，P4/P5）。Phlogo 的 `port.go` `ScoringPort` / `CacheRebuildPort` 在此仅留接口占位，不调用真实实现。

### 前置 / Prerequisites

- p3-03 已完成：得到 `Vec<NormalizedCheckResult>`、`unreported`（→ Unknown）、per-tag `PerTagHealth`、`cross_tag_alive` 聚合结果。
- P1 已完成：`database` crate 的 `PgPool` 与 migration 运行机制（`02-p1-core-skeleton-data/`）。
- 配置项 `node_alive_check_window_minutes` 默认 240（CONVENTIONS §2.6）。
- 已读取：`internal/db/model/node.go`、`internal/checker/handler.go` `buildCheckRecords`（`:194`）+ `applyRegionUpdates`（`:263`）+ Phase 1 事务（`:136`）。

### 步骤 / Steps

#### Step 1 — 目标表（方案 §5.1，映射 Phlogo 模型）

> Phlogo `node_initial_checks` + `node_deep_checks` 合并为 `node_check_results`（CONVENTIONS §2.3 #7）；
> `checker_runs` → `check_runs`（增加 `status` / `external_job_id` / `started_at` / `finished_at`）。

```sql
-- migrations/xxxx_check_runs.up.sql
CREATE TABLE check_runs (
    id              UUID PRIMARY KEY,            -- == Phlogo CheckerRun.ID / p3-01 run_id
    checker_tag     TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'running', -- running|success|failed
    external_job_id TEXT,                        -- 对接 Apalis job id（方案 §5.1）
    node_count      INT  NOT NULL DEFAULT 0,
    started_at      TIMESTAMPTZ NOT NULL,
    finished_at     TIMESTAMPTZ
);

CREATE TABLE node_check_results (
    id           BIGSERIAL PRIMARY KEY,
    node_id      UUID NOT NULL REFERENCES nodes(id),
    check_run_id UUID NOT NULL REFERENCES check_runs(id),
    checker_tag  TEXT NOT NULL,
    metrics      JSONB NOT NULL,                -- 合并初筛+复筛指标（见 Step 2）
    result_status TEXT NOT NULL,                -- alive|dead|unknown（方案 §6.2 视角）
    checked_at   TIMESTAMPTZ NOT NULL,
    UNIQUE (node_id, check_run_id, checker_tag) -- 幂等键：同 run 同节点同 tag 仅一行
);

CREATE TABLE node_status (
    node_id          UUID NOT NULL REFERENCES nodes(id),
    checker_tag      TEXT NOT NULL,
    health           TEXT NOT NULL,             -- unknown|alive|degraded|dead（方案 §6.2）
    last_checked_at  TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (node_id, checker_tag)           -- per-tag 健康快照（CONVENTIONS §2.3 #7）
);
```

#### Step 2 — Go 模型 → SQLx 行映射

| Phlogo（`internal/db/model/node.go`） | 新表字段 | 类型映射 |
|---|---|---|
| `CheckerRun.ID` (uuid PK) | `check_runs.id` | `Uuid` → `UUID PRIMARY KEY` |
| `CheckerRun.CheckerTag` | `check_runs.checker_tag` | `String` → `TEXT` |
| `CheckerRun.NodeCount` | `check_runs.node_count` | `i32` → `INT` |
| `CheckerRun.CompletedAt` | `check_runs.started_at/finished_at` | `DateTime` → `TIMESTAMPTZ`（方案新增 started/finished） |
| `NodeInitialCheck.LatencyMs`（`-1`=dead） | `node_check_results.metrics->'latency_ms'` | `i32` → `JSONB int` |
| `NodeInitialCheck.CheckedAt` | `node_check_results.checked_at` | `DateTime` → `TIMESTAMPTZ` |
| `NodeDeepCheck.AvgLatencyMs/JitterMs/SpeedKbps` | `node_check_results.metrics->'avg_latency_ms'/'jitter_ms'/'speed_kbps'` | `i32` → `JSONB int`（0=未测） |
| `NodeDeepCheck.StreamingResult`（JSON）/ `Metadata`（JSON） | `node_check_results.metrics->'streaming'/'metadata'` | `String(JSON)` → `JSONB` |
| `NodeDeepCheck.CheckerTag` | `node_check_results.checker_tag` | — |
| `NodeInitialCheck.CheckRunID` | `node_check_results.check_run_id` | — |
| （`applyRegionUpdates` 来源） | `metrics->'region'` 或 `nodes.region` | 见 Step 4 |
| （per-tag 快照，Phlogo 无独立表） | `node_status(node_id, checker_tag, health, last_checked_at)` | 方案 §5.1 新增 |

```rust
// crates/database/src/check.rs
use serde_json::json;

/// 把 p3-03 的 NormalizedCheckResult 收敛为 node_check_results 的一行
pub fn build_metrics(r: &NormalizedCheckResult) -> serde_json::Value {
    let mut m = json!({
        "latency_ms": r.initial.latency_ms, // -1 = dead
    });
    if let Some(d) = &r.deep {
        m["avg_latency_ms"] = json!(d.avg_latency_ms);
        m["jitter_ms"]       = json!(d.jitter_ms);
        m["speed_kbps"]      = json!(d.speed_kbps); // 0 = 未测
        m["streaming"]       = d.streaming.clone();
        m["metadata"]        = d.metadata.clone();
    }
    if let Some(region) = &r.region {
        m["region"] = json!(region);
    }
    m
}

pub fn result_status_of(r: &NormalizedCheckResult) -> &'static str {
    if r.initial.latency_ms == -1 { "dead" } else { "alive" }
}
```

#### Step 3 — 幂等持久化（事务 + ON CONFLICT）

对应 Phlogo `PostResults` Phase 1：`db.Transaction(func(tx){ CreateInBatches(initial); CreateInBatches(deep); Create(run) })`（`internal/checker/handler.go:136`）。
Rust 侧用 `sqlx::Transaction` + `ON CONFLICT DO NOTHING`（依托 Step 1 的 `UNIQUE (node_id, check_run_id, checker_tag)`）实现幂等：

```rust
// crates/checker-miaospeed/src/persist.rs
use sqlx::postgres::PgTransaction;

pub async fn persist_check_run(
    tx: &mut PgTransaction<'_>,
    run_id: Uuid, tag: &str, started_at: DateTime<Utc>,
    results: &[NormalizedCheckResult], unreported: &[Uuid],
) -> Result<(), sqlx::Error> {
    // 1) check_runs：失败重跑时 upsert（幂等）
    sqlx::query!(
        r#"INSERT INTO check_runs (id, checker_tag, status, node_count, started_at, finished_at)
           VALUES ($1,$2,'success',$3,$4,now())
           ON CONFLICT (id) DO UPDATE SET status='success', finished_at=now(), node_count=$3"#,
        run_id, tag, (results.len() + unreported.len()) as i32, started_at,
    ).execute(&mut **tx).await?;

    // 2) node_check_results：每行 ON CONFLICT DO NOTHING（同 run 重跑=no-op）
    for r in results {
        sqlx::query!(
            r#"INSERT INTO node_check_results
                 (node_id, check_run_id, checker_tag, metrics, result_status, checked_at)
               VALUES ($1,$2,$3,$4,$5,$6)
               ON CONFLICT (node_id, check_run_id, checker_tag) DO NOTHING"#,
            r.node_id, run_id, r.checker_tag, build_metrics(r),
            result_status_of(r), r.checked_at,
        ).execute(&mut **tx).await?;
    }
    // unreported -> Unknown 结果行（与 p3-03 一致）
    for nid in unreported {
        sqlx::query!(
            r#"INSERT INTO node_check_results
                 (node_id, check_run_id, checker_tag, metrics, result_status, checked_at)
               VALUES ($1,$2,$3,'{}'::jsonb,'unknown',now())
               ON CONFLICT (node_id, check_run_id, checker_tag) DO NOTHING"#,
            nid, run_id, tag,
        ).execute(&mut **tx).await?;
    }
    Ok(())
}

/// per-tag 健康快照：迟到不覆盖（p3-03 should_overwrite 语义）
pub async fn upsert_node_status(
    tx: &mut PgTransaction<'_>, s: &PerTagHealth,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"INSERT INTO node_status (node_id, checker_tag, health, last_checked_at)
           VALUES ($1,$2,$3,$4)
           ON CONFLICT (node_id, checker_tag) DO UPDATE
             SET health = EXCLUDED.health,
                 last_checked_at = EXCLUDED.last_checked_at
             WHERE EXCLUDED.last_checked_at > node_status.last_checked_at"#, // 仅当更新才覆盖
        s.node_id, s.checker_tag, s.health_as_str(), s.last_checked_at,
    ).execute(&mut **tx).await?;
    Ok(())
}
```

#### Step 4 — region 更新（对应 Phlogo `applyRegionUpdates`）

Phlogo 在事务外用 `CASE WHEN` 批量更新 `nodes.region`（`internal/checker/handler.go:263`）。Rust 侧：
结果里的 `region`（来自 `deep.region`）走 `nodes.region` 的批量 CASE 更新，或写入 `metrics->'region'` 由订阅层读取；
**不在本步骤里做跨 tag 的节点级 `alive` 直写**（那是 p3-03 `cross_tag_alive` 的聚合结果，落到 P4 生命周期层）。

```rust
// 批量更新 nodes.region（best-effort，事务外；与 Phlogo 行为一致）
pub async fn apply_region_updates(pool: &PgPool, updates: &[(Uuid, String)]) -> Result<(), sqlx::Error> {
    // 构造 CASE WHEN ... END 批量更新；分批 500（对应 Phlogo batchSize=500）
    // 具体 SQL 同 handler.go:284 的 caseExpr，用参数化防止注入
    todo!("分批 CASE WHEN 更新，参数化；见 internal/checker/handler.go:284")
}
```

#### Step 5 — 编排（串联 p3-02/03/04，被 p3-01 的 handler 调用）

```rust
// crates/checker-miaospeed/src/run.rs
pub async fn run_check(&self, job: &CheckJob) -> Result<(), CheckerError> {
    let raw = self.adapter.submit_and_collect(job).await?; // p3-02
    let mut results = self.adapter.normalize(&raw, &self.id_map, job.run_id, &job.checker_tag, now());
    let (results, unreported) = reconcile(&job.node_ids, &results); // p3-03 Step 3
    let snapshots = build_per_tag_health(&results);                 // p3-03 Step 4

    let mut tx = self.pool.begin().await?;
    persist_check_run(&mut tx, job.run_id, &job.checker_tag, started, &results, &unreported).await?;
    for s in &snapshots { upsert_node_status(&mut tx, s).await?; }
    tx.commit().await?;

    // 下列为 P4/P5 职责，本步骤仅占位接口（对应 port.go ScoringPort/CacheRebuildPort）
    // self.scoring.compute_and_update_tag_scores(&results).await?; // P4
    // self.cache.trigger_rebuild().await?;                        // P5
    Ok(())
}
```

### 验收 / Acceptance

1. **集成测试：持久化且幂等**：
   ```bash
   cargo test --test integration check_persistence_idempotent
   ```
   预期：exit 0；提交 N 个节点跑一次后：
   - `SELECT COUNT(*) FROM check_runs WHERE id=$run` == 1
   - `SELECT COUNT(*) FROM node_check_results WHERE check_run_id=$run` == N
   - `SELECT COUNT(*) FROM node_status WHERE checker_tag=$tag` == N
   用**同一 `run_id` 重跑一次**：上述计数**不变**（no-op / 一致），断言通过。

2. **初筛/复筛合并校验**：查 `node_check_results.metrics` 同时含 `latency_ms` 与（有 deep 时）`avg_latency_ms`/`jitter_ms`/`speed_kbps`/`streaming`/`metadata`。
   ```bash
   cargo test --test integration check_results_metrics_roundtrip
   ```
   预期：JSONB 字段与提交值一致；`speed_kbps=0` 表示未测被正确保留。

3. **迟到不覆盖落库校验**：先写 `last_checked_at=T+10` 的 `node_status`，再持久化 `checked_at=T` 的结果，断言 `health`/`last_checked_at` 未被改回。
   ```bash
   cargo test --test integration node_status_late_write_no_overwrite
   ```
   预期：行未被更新（`WHERE EXCLUDED.last_checked_at > node_status.last_checked_at` 生效）。

4. **migration 一致性**：
   ```bash
   sqlx migrate run --check && cargo sqlx prepare --check
   ```
   预期：exit 0，说明 `migrations/` 与离线查询一致。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo | Rust（本步骤） |
|---|---|
| `internal/db/model/node.go` `CheckerRun{ID,CheckerTag,NodeCount,CompletedAt}` | `check_runs{id,checker_tag,status,external_job_id,node_count,started_at,finished_at}`（方案 §5.1 扩展） |
| `internal/db/model/node.go` `NodeInitialCheck` + `NodeDeepCheck`（两张表） | 合并 `node_check_results{id,node_id,check_run_id,metrics,result_status,checked_at}`（CONVENTIONS §2.3 #7） |
| `internal/checker/handler.go:136` Phase 1 事务 `CreateInBatches(initial,500)` + `CreateInBatches(deep,500)` + `Create(run)` | `sqlx::Transaction` + 批量 `INSERT ... ON CONFLICT DO NOTHING`（本页 Step 3） |
| `internal/checker/handler.go:194` `buildCheckRecords`：拆 initial/deep + region 提取 | `build_metrics` + `result_status_of`（Step 2）；region 走 Step 4 |
| `internal/checker/handler.go:263` `applyRegionUpdates`（事务外 CASE WHEN 分批 500） | `apply_region_updates`（事务外，分批 500，参数化） |
| `internal/checker/handler.go:303` `batchAliveDecision` 写 `nodes.alive` | 本步骤**不**直写 `nodes.alive`；per-tag `node_status` + 跨 tag 聚合留 P4（保留 p3-03 合并规则） |
| `internal/checker/port.go` `ScoringPort` / `CacheRebuildPort` | 本步骤仅占位调用（P4/P5 实现） |
| 幂等性来源：per-tag mutex 串行化（`:53`）+ 单次 goroutine | 幂等来源：`run_id` 唯一键 + `ON CONFLICT` + Apalis at-least-once 重试用同一 `run_id`（见 p3-01） |

### CI 提示 / CI notes

> 仅描述未来 CI 命令，**不落地 YAML**。

- **集成测试**（需 Postgres service container）：`cargo test --test integration`（覆盖 p3-04 第 1-3 条验收），CI 起 Postgres 并 `sqlx migrate run` 后运行。
- **离线查询校验**：`cargo sqlx prepare --check` 与 `sqlx migrate run --check`（CONVENTIONS §5；防止 `migrations/` 与 `query!` 漂移）。
- **build/lint/fmt**：`cargo build --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`。
- **幂等回归**：`check_persistence_idempotent` 必须每次 CI 运行（重跑同 `run_id` 是 P3 阶段级验收之一，见 `index.md`）。

### 风险 / Risks

- **JSONB 单位漂移**：`metrics` 里的 `latency_ms` 等若被序列化层改了类型（如 `-1` 变 `null`），`result_status` 判定会错；CI 用 `check_results_metrics_roundtrip` 锁死。
- **幂等键选择**：必须唯一约束 `(node_id, check_run_id, checker_tag)`，否则同 run 重试会翻倍（Phlogo 靠 per-tag mutex 避免，Rust 靠约束）；删约束会破坏 p3-01 的幂等验收。
- **迟到覆盖的并发**：`node_status` 的 `WHERE EXCLUDED.last_checked_at > node_status.last_checked_at` 在并发 upsert 下正确（Postgres 行级锁），但需 integration 测试在并发提交下验证。
- **跨 tag 聚合落点**：本步骤只写 per-tag `node_status`，跨 tag `alive` 聚合（Phlogo `nodes.alive` 字段）在哪层、何时物化，需在 P4 明确，避免重复计算或不一致。
- **许可证**：仅持久化本平台数据，不引入 MiaoSpeed 代码；AGPL 义务由进程边界满足（CONVENTIONS §2.11）。`cargo-deny check licenses` / SBOM 须在 P7 覆盖 MiaoSpeed 二进制本身。
