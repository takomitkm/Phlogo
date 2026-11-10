## 3-04 抓取对账与孤儿清理 · Fetch Reconciliation & Orphan Cleanup

### 目标 / Goal

完成「抓取 → 解析 → 指纹」之后的**入库闭环**：

- 记录 **Fetch Run 历史**（`source_fetch_runs`，替代 Phlogo 的 `source_fetch_histories`）。
- 实现 **Replace / Accumulate 对账**：按 `sources.fetch_policy` 决定「消失的节点是否删映射」。
- **所有关联清理在同一个 DB 事务内完成**（[`CONVENTIONS.md`](CONVENTIONS.md) §2.9：「所有关联清理必须在数据库事务内完成」）。
- **事务内孤儿清理**：映射移除后节点若再无 `node_sources` 引用，标记 `membership = Orphaned`（物理删除推迟到 P4）。
- **死源保护**：来源已 `Dead`（或 `Suspended`）时拒绝提交，避免僵尸映射复活。

### 范围 / Scope

- **扩展** `crates/registry`（对账逻辑 `reconcile_fetch_result`）、`crates/lifecycle`（`MarkDeadAndClean` 迁移）。
- **新增 migration**：`source_fetch_runs` 表；`sources.fetch_policy` 列（若 P1 未建）；`nodes.membership` 列（Linked/Orphaned/Retired，[`CONVENTIONS.md`](CONVENTIONS.md) §2.9）。
- **依赖**：`p2-01`/`p2-02`/`p2-03` 的产出（`NormalizedNode` 带 `identity_hash`）；P1 的 `sources`/`nodes`/`node_sources` CRUD。
- **不做**：实际抓取（`p2-01`）、解析（`p2-02`）、指纹（`p2-03`）。物理删除孤儿节点与其测活历史推迟到 P4（见风险）。

### 前置 / Prerequisites

- P1 已落地：`sources`（含 `status`、`fetch_policy`）、`nodes`、`node_sources`（含 `first_seen_at`/`last_seen_at`，[`CONVENTIONS.md`](CONVENTIONS.md) §2.3）、`SourceStatus`/`FetchPolicy`/`NodeMembership` 枚举。
- `p2-03` 已为 `NormalizedNode` 计算 `identity_hash`。
- 工具链：SQLx + PostgreSQL、Tokio、`uuid`、测试用 ephemeral Postgres（`testcontainers` 或 `sqlx` + docker）。
- 依据：[`CONVENTIONS.md`](CONVENTIONS.md) §2.9（生命周期）、方案 §5.1/`source_fetch_runs`、§6（`SourceStatus`/`FetchPolicy`/`NodeMembership`）、`internal/node/upsert.go`、`internal/source/service.go`。

### 步骤 / Steps

**Step 1 — 新增 `source_fetch_runs` 表（migration）**

方案 §5.1：替换 Phlogo `source_fetch_histories`（`source_id, success, node_count, fetched_at`），新增状态与时间字段。

```sql
CREATE TABLE source_fetch_runs (
    id            UUID PRIMARY KEY,
    source_id     UUID NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    status        TEXT NOT NULL,            -- started | finished | failed
    started_at    TIMESTAMPTZ NOT NULL,
    finished_at   TIMESTAMPTZ,
    counts       JSONB NOT NULL DEFAULT '{}'::jsonb, -- {fetched, parsed, upserted, removed}
    error         TEXT
);
CREATE INDEX idx_source_fetch_runs_source ON source_fetch_runs(source_id);
```

**Step 2 — 死源/挂起守卫 + Replace/Accumulate 对账（均在一个事务内）**

对齐 `internal/node/upsert.go` 的 `UpsertFromFetchResult` 语义，并区分 `fetch_policy`：

```rust
// crates/registry/src/reconcile.rs (骨架/目标实现)
pub async fn reconcile_fetch_result(
    tx: &mut Transaction<'_, Postgres>,
    source_id: Uuid,
    success: bool,
    nodes: &[NormalizedNode],     // 已带 identity_hash
) -> Result<FetchRunSummary, ReconcileError> {
    // 1) 记录 run（started → finished/failed）
    let run_id = Uuid::new_v4();
    //    INSERT source_fetch_runs(... status=started ...)

    if !success {
        // 失败 → 保留一切旧关联；只更新 run=failed
        //    UPDATE source_fetch_runs SET status='failed', finished_at=now()
        return Ok(summary_without_changes());
    }

    // 2) 死源/挂起守卫：对齐 upsert.go 的 `if src.Status == "dead" { skip }`
    let status: String = /* SELECT status FROM sources WHERE id = ? */;
    if status == "dead" || status == "suspended" {
        // 不改动任何映射；仅记录 run=finished
        return Ok(summary_noop());
    }

    // 3) 空结果
    if nodes.is_empty() {
        match fetch_policy(source_id, tx).await? {
            FetchPolicy::Replace => {
                // 删除该来源全部映射（事务内），并对变孤儿节点标记 Orphaned
                remove_source_mappings(tx, source_id, MarkOrphaned::Yes).await?;
            }
            FetchPolicy::Accumulate => {
                // 空结果不删任何旧关联（保留全部）
            }
        }
        // UPDATE run=finished
        return Ok(summary);
    }

    // 4) 成功有节点：逐节点 upsert + 保映射
    let mut new_ids = HashSet::new();
    for nn in nodes {
        //   INSERT INTO nodes (identity_hash, protocol, server, port, raw_config_reference,
        //                        fingerprint_version, membership, first_seen_at, last_seen_at)
        //   ON CONFLICT (identity_hash) DO UPDATE SET last_seen_at=now(), raw_config_reference=excluded...
        //   INSERT INTO node_sources (node_id, source_id, first_seen_at, last_seen_at)
        //   ON CONFLICT (node_id, source_id) DO UPDATE SET last_seen_at=now()
        new_ids.insert(node_id);
    }

    match fetch_policy(source_id, tx).await? {
        FetchPolicy::Replace => {
            // 删除「该来源」下、不在 new_ids 的映射（事务内）
            //   DELETE FROM node_sources WHERE source_id = ? AND node_id != ALL(new_ids)
            //   对每个被删后 node_sources 计数为 0 的节点：标记 membership='Orphaned', orphaned_at=now()
            remove_stale_mappings_and_mark_orphans(tx, source_id, &new_ids).await?;
        }
        FetchPolicy::Accumulate => {
            // 不删除旧映射；仅新增
        }
    }

    // UPDATE run=finished, counts={...}
    Ok(summary)
}
```

**Step 3 — 孤儿清理与原子性**

对齐 `internal/node/upsert.go` 的 `removeSourceMappings`（删除映射后，节点若再无来源引用则清理）。但按方案 §6.3，Rust **不立即物理删除**，而是标记 `Orphaned`：物理删除 + 测活历史清理推迟到 P4 的 Retired 流程。

```rust
// 标记孤儿：用 NOT EXISTS 保证原子性（对齐 Go 的
// DELETE FROM nodes WHERE id = ? AND NOT EXISTS (SELECT 1 FROM node_sources ...)）
async fn mark_orphaned_if_lonely(
    tx: &mut Transaction<'_, Postgres>,
    node_id: Uuid,
) -> Result<(), ReconcileError> {
    // UPDATE nodes SET membership='Orphaned', orphaned_at=now()
    //   WHERE id = ? AND membership='Linked'
    //     AND NOT EXISTS (SELECT 1 FROM node_sources WHERE node_id = ?)
    Ok(())
}
```

**Step 4 — `MarkDeadAndClean` 迁移（`crates/lifecycle/src/source.rs`）**

对齐 `internal/source/service.go` 的条件 UPDATE 防并发重复清理：

```rust
// 对齐 MarkDeadAndClean：WHERE status != 'dead' 的条件 UPDATE，
// RowsAffected==0 说明已被别的 caller 抢先，本 caller 跳过清理。
pub async fn mark_dead_and_clean(tx, source_id) -> Result<(), Error> {
    let n = /* UPDATE sources SET status='dead' WHERE id = ? AND status != 'dead' */;
    if n == 0 { return Ok(()); }      // 已死，跳过
    remove_source_mappings(tx, source_id, MarkOrphaned::Yes).await?; // 同 Step 2/3
    Ok(())
}
```

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地**：
- Migration **`0003_node_membership`**：`nodes.membership varchar(16) DEFAULT 'Linked'`、`nodes.orphaned_at`、`nodes.raw_config_reference`（`source_fetch_runs` 与 `sources.fetch_policy` 在 P1 的 0001 已建，无需重复）。
- `crates/database/src/reconcile.rs`：事务内 SQL 原语——`insert_fetch_run`/`finish_fetch_run`、`source_status`/`fetch_policy_of`、`upsert_node_by_hash`（`ON CONFLICT (identity_hash) DO UPDATE` 保 last_seen/raw）、`link_node_source`（幂等+刷新 last_seen）、`revive_if_orphaned`、`remove_stale_mappings`（`NOT (node_id = ANY($2))` + `RETURNING`）、`remove_source_mappings`、`mark_orphaned_if_lonely`（**原子 `NOT EXISTS`**，对齐 Go 写法）。
- `crates/registry/src/reconcile.rs`：`reconcile_fetch_result(pool, source_id, success, nodes, error)`——单事务内：记 run(started)→**失败即 failed 保留一切**→**死源/挂起守卫**（`dead|suspended` → noop finished）→逐节点 upsert+link+复活→按 `fetch_policy`（非 `accumulate` 即 Replace，**默认 Replace 保持 Phlogo 行为**）删消失映射并标记孤儿→run finished + counts。另有 `delete_source`（先删映射标孤儿再删源，同一事务）。

**实测（校机，PG 18.6）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0**（41.67s） |
| `cargo test --workspace`（含 `DATABASE_URL`） | **0**——`registry::reconcile` **6 场景全过**：`replace_removes_vanished_and_marks_orphaned`、`failed_fetch_keeps_mappings`、`accumulate_keeps_old`、`delete_source_keeps_shared_nodes`、`empty_replace_orphans_all`、`dead_source_is_guarded` |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |

**验收对照（§验收 1–8）**：1✓（消失映射删除+node2 Orphaned、node1 Linked）2✓（failed 保留两映射、run=failed）3✓（Accumulate 旧映射保留）4✓（删 A 后 B 的映射与 Linked 不动）5✓（空 Replace 全删+`orphaned_at` 非空）6✓（dead 源 upserted=0、旧映射原样）7✓（映射删除与孤儿标记在同一 `pool.begin()/commit()`；失败分支/守卫分支同样单事务）8✓（clippy 0）。

**说明/偏差**：
- 孤儿**只标记不物理删**（P4 Retired 流程接管物理删除与测活历史清理）——与 Go `removeSourceMappings` 立即 DELETE 的差异已在文档记录。
- `lifecycle` crate 未单独建（`mark_dead_and_clean` 的条件 UPDATE 语义将由 P4 清理 Job 落地；本页死源守卫已覆盖「迟到结果不改 dead 源」的核心不变量）。
- 测试用 `std::slice::from_ref(&n)` 替代 `&[n.clone()]`（clippy `cloned_ref_to_slice_refs`）。

### 验收 / Acceptance

机器可校验项（集成测试，需 Postgres）：**`cargo test --test integration reconciliation`**

下列每个场景用一个独立的数据库事务/fixture 证明（退出码 `0`，全部 `ok`）：

1. **Replace 成功删除该来源消失节点的关联**
   - 准备：来源 A 关联 node1 + node2（`node_sources` 两行；node1 仅被 A 引用，node2 仅被 A 引用）。
   - 动作：`reconcile_fetch_result(A, success=true, [node1])`，`fetch_policy=Replace`。
   - 断言：`node_sources` 中 `(A,node2)` **已删除**；node2 的 `membership == Orphaned`（因再无来源引用）；node1 仍 Linked。

2. **失败抓取保留旧关联**
   - 准备：来源 A 关联 node1 + node2。
   - 动作：`reconcile_fetch_result(A, success=false, [])`。
   - 断言：`node_sources` 两行**都还在**；`source_fetch_runs` 该 run `status='failed'`；无任何节点进入 Orphaned。

3. **Accumulate 不删除旧关联**
   - 准备：来源 A 关联 node1（`fetch_policy=Accumulate`）。
   - 动作：`reconcile_fetch_result(A, success=true, [node2])`（node1 未出现）。
   - 断言：`node_sources` 中 `(A,node1)` 与 `(A,node2)` **都存在**（都保留）。

4. **删除来源不影响其它来源仍引用的节点**
   - 准备：node1 同时被来源 A、B 关联（`node_sources` 两行）。
   - 动作：`DELETE source A`（调用 `lifecycle` 的 source 删除，内部 `remove_source_mappings`）。
   - 断言：`node_sources` 中 `(A,node1)` 删除；`(B,node1)` 保留；node1 仍 exists 且 `membership=Linked`。

5. **无来源引用的节点进入 Orphaned**
   - 准备：node1 仅被来源 A 关联。
   - 动作：移除最后一条 `(A,node1)` 映射（Replace 空结果 或 删除来源 A）。
   - 断言：node1 的 `membership == Orphaned` 且 `orphaned_at` 非空（`NOT EXISTS node_sources` 校验通过）。

6. **死源保护**
   - 准备：来源 A `status='dead'`，已有 node1 关联。
   - 动作：`reconcile_fetch_result(A, success=true, [])` 或带新节点的成功结果。
   - 断言：`node_sources` **未被改动**（未复活），run 仅记录 finished/noop。

7. **全部清理在事务内**
   - 断言：上述每个会改动映射的场景，映射删除与 Orphaned 标记在**同一个**事务提交；中途回滚时映射与 membership 都不变（可用注入失败的测试事务验证原子性）。

8. **Lint** —— `cargo clippy -p registry -p lifecycle --all-targets -- -D warnings` 退出码 `0`。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- **`internal/node/upsert.go` `UpsertFromFetchResult`**：
  - 失败 → `return nil`（no-op，保留旧节点）→ Rust Step 2 的 `!success` 分支。
  - 死源 → 查 `status=="dead"` 后 `return nil` → Rust Step 2 的死源/挂起守卫。
  - 空 → `removeSourceMappings` → Rust Replace 空结果的 `remove_source_mappings`。
  - 成功有节点 → 逐节点事务内 `SELECT hash` upsert + `OnConflict DoNothing` 保映射 + 删旧映射 + 删孤儿 → Rust Step 4（Replace）。
- **`internal/node/upsert.go` `removeSourceMappings`**：删该来源映射，并对变孤儿节点 `DELETE FROM nodes WHERE ... NOT EXISTS (...)` 同时清理测活历史（`node_initial_checks`/`node_deep_checks`/`node_tag_score`）。**Rust 差异**：本步骤只在事务内**标记** `Orphaned`（不立即物理删、不立即清测活历史），真正的删除/历史清理按方案 §6.3 推迟到 P4 Retired 流程。
- **`internal/source/service.go` `MarkDeadAndClean`**：`WHERE status != 'dead'` 条件 UPDATE（`RowsAffected==0` 跳过）→ Rust Step 4 完全对齐。
- **`internal/source/service.go` `Delete`**：删源前先 `RemoveSourceMappings` 再删 `source_fetch_histories` 与 `sources` → Rust Step 4 的 source 删除沿用同一顺序。
- **关键新增**（[`CONVENTIONS.md`](CONVENTIONS.md) §2.9）：Phlogo **没有** Replace/Accumulate 区分（本质是永远 Replace）、**没有** Orphaned/Retired 状态（直接物理删孤儿）。这两点是 P2/P4 的重点新增，本步骤把 `fetch_policy` 分支与 `Orphaned` 标记首次引入。

### CI 提示 / CI notes（仅描述，不落地 YAML）

- 单独 job：`cargo test --test integration`（需要 ephemeral Postgres，如 `testcontainers` 起 `postgres:16`，或 CI 服务提供的 service container）。失败即阻断合并。
- 该 job 与 `p2-01` 的纯单元 SSRF 测试分开：一个要网络、一个要 DB，互不可混。
- 在 CI 里加一个「原子性回归」子测试：故意让删除孤儿后的提交失败，断言映射与 membership 都未变。
- `cargo sqlx prepare` / `sqlx migrate run --check` 必须随 migration 一起过（见 [`CONVENTIONS.md`](CONVENTIONS.md) §5）。
- 参见方案 §15 的「Replace 成功后只删除该来源中已消失节点的关联」「Accumulate 不因一次未发现旧节点而删除关联」「删除一个来源不影响其他来源引用的节点」「所有来源关系消失后节点进入 Orphaned」等核心验收项。

### 风险 / Risks

- **孤儿清理时机变化（高）**：Phlogo 现在**立即物理删除**孤儿节点并清测活历史；Rust 改为先标记 `Orphaned`。必须保证 **P4 的 Retired/清理 Job 接管物理删除与历史清理**，否则会出现「节点已无来源但数据 + 测活历史残留」、且 `node_scores`/`node_status` 等快照越积越多。
- **Replace 默认值**：Phlogo 行为等价于永远 Replace；新代码若 `fetch_policy` 默认值不是 Replace，会导致回退（旧关联不被清理）。默认必须设为 Replace 以保持行为一致。
- **Accumulate 的正确性**：Accumulate 只增不删，但节点 `identity_hash` 可能因 `p2-03` 算法升级而变化（同一物理节点旧哈希 ≠ 新哈希），需要在 `p2-03` 的迁移里同步迁移 `node_sources`，否则 Accumulate 会把「同一节点」当成两个。
- **事务隔离级别**：`NOT EXISTS node_sources` 的孤儿判定必须在同一事务内，避免并发 caller 在「计数」与「删除」之间插入新映射；沿用 Go 的原子 `DELETE ... WHERE NOT EXISTS` 写法。
- **死源/挂起守卫范围**：本步骤以 `Dead` 为主守卫（对齐 Go）；`Suspended` 是否也应拒绝 in-flight 结果需与 P4 状态机确认（Suspended 语义是「暂停抓取但保留关联」——正常不会派发抓取，但迟到的旧结果也应以 no-op 处理）。
