## 5-02 按 Tag 健康与孤儿/退役清理 · Per-Tag Health & Orphan/Retired Cleanup

### 目标 / Goal

1. 以 `node_id + checker_tag` 维度维护健康快照 `node_status`（plan §5.1、§6.2），
   替代 Phlogo 单一的 `nodes.alive bool`；支持 `Unknown/Alive/Degraded/Dead` 四态
   （`Degraded` 为 net-new）。
2. 实现**跨 tag 存活合并**：任一 checker 在窗口内报告存活，节点即视为 alive
   （对照 `internal/checker/handler.go` `batchAliveDecision`）。
3. 实现 `Orphaned → Retired → 物理删除` 保留窗口（net-new，Phlogo 当前是孤儿立即物理删除）。
4. 保证三条核心不变量：**不同 tag 独立**、**不可达节点不删来源关联**、**迟到结果不覆盖新快照**。

### 范围 / Scope

- **新建**：`node_status` 表（`node_id`, `checker_tag`, `health`, `last_checked_at`）；
  `nodes`/`node_sources` 增加 `membership`、`first_seen_at`/`last_seen_at`（见 p4-01）。
- **修改**：把 `internal/checker/handler.go` 的 `batchAliveDecision` 重写为 per-tag 快照写入 +
  跨 tag 合并；把 `internal/node/upsert.go` 的孤儿物理删除改为「标记 `Orphaned` + 定时器 `Retired`」。
- **新建**：孤儿/退役清理 Job（plan §2.1 `LIFE` + `cleanup`，原 `internal/cleanup`）。
- **不做**：评分计算（p4-03）；订阅输出是否过滤 `Retired`（P5 消费状态）。

### 前置 / Prerequisites

- p4-01 已完成：`NodeHealth`/`NodeMembership` 枚举与转换规则已定义。
- `check_runs` / `node_check_results`（plan §5.1）已落库，结果带 `check_run_id` 与 `checked_at`
  （P3 负责写入，本步骤消费）。
- 配置项 `node_alive_check_window_minutes`（默认 240，见 `CONVENTIONS.md §2.6`）与保留窗口参数可用。

### 步骤 / Steps

#### 步骤 1 — `node_status` 表与 per-tag 快照写入（对照 checker/handler.go）

Phlogo 现状（`internal/checker/handler.go:303` `batchAliveDecision`）：
- 用 `DISTINCT ON (node_id, checker_tag) ... ORDER BY node_id, checker_tag, checked_at DESC`
  取窗口内（默认 240min）每 (node,tag) 最新 initial；`latency_ms != -1` 即 alive。
- 跨 tag 合并：汇总所有 tag 的最新 initial，**任一** `latency_ms != -1` → 该节点 `alive=true`，
  否则 `alive=false`（handler.go:321-373，写回 `nodes.alive` 单字段）。
- 串行化：`tagLock(tag)` per-tag mutex（handler.go:53），迟到结果靠 `ORDER BY checked_at DESC` 取最新。

Rust 重写：保留上述「取窗口内最新 + 跨 tag 合并」语义，但目标从 `nodes.alive` 单字段改为
`node_status` 的 per-tag 快照（plan §5.1 / §5.3 第 7 条「按 node_id+checker_tag 维护，避免互相覆盖」）。

```sql
-- node_status（plan §5.1）
CREATE TABLE node_status (
    node_id       uuid        NOT NULL REFERENCES nodes(id),
    checker_tag   text        NOT NULL,
    health        text        NOT NULL,           -- unknown|alive|degraded|dead
    last_checked_at timestamptz NOT NULL,
    PRIMARY KEY (node_id, checker_tag)
);
```

```rust
// crates/checker-miaospeed/src/alive.rs （重写 batchAliveDecision 的快照部分）
// 1) 取窗口内每 (node,tag) 最新 initial（DISTINCT ON 等价写法，SQLx 手写）
// 2) 写 node_status(health, last_checked_at) 用 ON CONFLICT(node_id,checker_tag) DO UPDATE
//    仅当新 checked_at >= 旧 last_checked_at 才覆盖（迟到结果保护，见步骤 3）
// 3) 跨 tag 合并出 per-node alive：任一 tag 的 latest initial latency != -1 -> alive
```

跨 tag 合并逻辑（handler.go:339-345 的 `aliveSet`）原样保留：遍历所有 tag 的窗口内最新 initial，
`latency_ms != -1` 即加入 alive 集合；其余进 dead 集合。Rust 侧把结果同时写到
`node_status`（per-tag）并可派生一个 per-node 聚合视图（**不**回写旧的 `nodes.alive` 单字段语义）。

#### 步骤 2 — 孤儿 / 退役保留窗口（对照 node/upsert.go）

Phlogo 现状（`internal/node/upsert.go:169` `removeSourceMappings`）：真孤儿节点**立即**
`DELETE FROM nodes WHERE ... NOT EXISTS (...)` 并先删 `node_initial_checks`/`node_deep_checks`/
`node_tag_scores`。即「孤儿 == 物理删除」，无 `Orphaned`/`Retired` 中间态。

Rust 引入保留窗口（plan §6.3「无任何来源引用 → 标记 Orphaned 并记录时间；超过保留期限 → 标记 Retired，
再按清理策略物理删除」）：

```rust
// 清映射后，对失去全部引用的节点：不立即删，而是置 membership=Orphaned + orphaned_at=now
fn on_lost_all_sources(tx, node_id) {
    if tx.orphan_count(node_id)? == 0 {
        tx.set_membership(node_id, Orphaned, now)?; // 记录 orphaned_at
    }
}

// 定时清理 Job（cleanup）：
//   Orphaned 且 now - orphaned_at > retention -> membership=Retired（或直接进入物理删除策略）
//   Retired 且超过更长期限 -> 物理删除节点 + 其 node_status / node_scores / node_check_results
```

保留窗口参数（net-new，需在迁移 seed 固定默认值）：建议新增 `orphan_retention_days`
（如复用 `CONVENTIONS.md §2.6` 的 `node_dead_window_days=4` 作初值，或单独指定），并在 P4 验收里明确。

#### 步骤 3 — 迟到结果不覆盖新快照（plan §6.4、CONVENTIONS §2.7/§2.9）

Phlogo 当前靠两点保证：
- per-tag mutex `tagLock`（handler.go:53）串行化同一 tag 的提交；
- `ORDER BY checked_at DESC` + `DISTINCT ON` 保证「窗口内最新」胜出（handler.go:327-331）。

Rust 强化为显式守卫：写 `node_status` 时 `ON CONFLICT DO UPDATE ... WHERE excluded.last_checked_at >= node_status.last_checked_at`
（或先读后比），确保迟到的旧 `checked_at` 结果**不能**回退已写入的较新快照。此守卫是 plan §15
「迟到的旧测活结果不能覆盖新状态」的直接实现。

```rust
// 伪代码：迟到保护
let existing = tx.get_node_status(node_id, tag)?;
if existing.is_none_or(|e| new_checked_at >= e.last_checked_at) {
    tx.upsert_node_status(node_id, tag, health, new_checked_at)?;
}
```

### 验证结果 / Verified Results（已实测，2026-11-11）

> `node_status` 表与迟到守卫在 P3-04/P4-01 已落地；本步骤补齐**跨 tag 聚合**与**清理 Job 编排**。

**落地**（`crates/lifecycle/src/ops.rs` 扩展）：
- `run_cleanup_cycle(pool) -> CleanupReport{retired, deleted}`：一个 tick 内 `retire_expired_orphans`（读 `orphan_retention_days=14`）→ `delete_retired_nodes`（`NOT EXISTS` 守卫）。**同周期先退役后删除**——超期孤儿在一个 tick 内走完 Retired→物理删。
- `cross_tag_alive_map(pool, window)`：重写 `batchAliveDecision` 的聚合部分——`GROUP BY node_id` + `bool_or(health IN ('alive','degraded'))`，仅统计 `last_checked_at >= now()-window` 的快照；返回 `node_id -> alive`（**不回写**旧 `nodes.alive` 单字段，聚合视图由 P5 分发消费）。
- `alive_window_minutes`：读 `node_alive_check_window_minutes`（0001 seed 默认 240）。

**实测（校机，PG 18.6）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0**（34.55s） |
| `cargo test --workspace` | **0**——`lifecycle::p402` **4/4** |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |

**4 场景（验收 A–D）**：
- A `per_tag_health_independent` ✓：(N,a)=alive 与 (N,b)=dead 并存互不影响。
- B `unreachable_node_keeps_source_mapping` ✓：dead 上报后 2 条映射仍在、节点未被物理删。
- C `orphan_detection_and_retention` ✓：失去最后映射→Orphaned+`orphaned_at` 记录、节点仍在（对照 Phlogo 立即删）；新鲜孤儿不退役（14 天窗口生效）；回溯 30 天后一个 cleanup tick 完成 Retired→物理删，`node_status` 历史随之清零。
- D 迟到不覆盖：由 P3-04 `node_status_late_write_no_overwrite`（同守卫 SQL）覆盖，本页不重复实现。
- 附加 `cross_tag_map_window` ✓：窗口内仅 dead→false；任一 tag alive→true。

**说明/偏差**：`Degraded` 派生规则按规格风险条目**留 TODO**（先 Unknown/Alive/Dead 三态跑通，`bool_or` 已把 degraded 计入 alive，规则冻结于 `LIFECYCLE.md` 后只需改派生层）；清理 Job 的 Apalis 定时触发在 P7 compose 装配（本步提供 `run_cleanup_cycle` 可调用入口）。

### 验收 / Acceptance

#### A. 不同 tag 独立（per-tag independence）

```bash
cargo test -p checker-miaospeed per_tag_health_independent
# 期望退出码 0：对 node N，tag A 写入 health=Alive，tag B 写入 health=Dead；
# 断言 node_status 中 (N,A)=Alive 且 (N,B)=Dead 同时存在，互不影响。
```

#### B. 不可达节点不删来源关联

```bash
cargo test -p lifecycle unreachable_node_keeps_source_mapping
# 期望退出码 0：构造 node N 关联 source S1、S2；对 N 以 tag A 上报 latency=-1（dead）。
# 断言：node_sources 中 (N,S1)、(N,S2) 行仍存在；nodes 中 N 未被物理删除；
#       仅 node_status(N,A)=Dead 被写入。
```

#### C. 孤儿检测 + 保留窗口（不立即物理删除）

```bash
cargo test -p lifecycle orphan_detection_and_retention
# 期望退出码 0：
#  (1) 删去 node N 的最后一个来源关联后，N.membership = Orphaned 且 orphaned_at 已记录；
#      nodes 表中 N 仍存在（未被立即物理删除，对照 Phlogo 立即删除的差异）。
#  (2) 快进 orphaned_at 超过 retention 后跑清理 Job，N.membership = Retired，
#      再超期后物理删除且关联测活历史被清理。
```

#### D. 迟到结果不覆盖新快照

```bash
cargo test -p checker-miaospeed late_result_no_overwrite
# 期望退出码 0：先提交 checked_at=t1（alive），再提交迟到结果 checked_at=t0<t1（dead）；
# 断言 node_status(N,tag).health=Alive 且 last_checked_at=t1（未被 t0 覆盖）。
```

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `internal/checker/handler.go` `batchAliveDecision`（handler.go:303）：跨 tag 存活合并 → 重写为
  per-tag `node_status` 快照 + 跨 tag 聚合；per-tag mutex 与 `checked_at DESC` 语义保留。
- `internal/node/upsert.go` `removeSourceMappings`（upsert.go:169）：孤儿立即物理删除 → 改为
  `Orphaned` 标记 + 定时 `Retired` 物理删除（net-new 保留窗口）。
- `internal/source/service.go` `MarkDeadAndClean`（service.go:182）：死源清理仍走条件 UPDATE 幂等；
  本步骤保证死源清理**不**误删仍被其它来源引用的节点（与 upsert.go 的 `NOT EXISTS` 守卫一致）。
- **缺失对照（net-new）**：per-tag `node_status` 表、`NodeHealth::Degraded`、`Orphaned/Retired`、
  迟到结果显式 `checked_at` 守卫——Phlogo 均不存在。

### CI 提示 / CI notes

- 单测：`cargo test -p lifecycle -p checker-miaospeed`（per-tag 独立性 + 迟到保护 + 孤儿保留）。
- 集成（Postgres + MiaoSpeed stub）：`cargo test --test integration health_orphan`
- 迁移校验：`sqlx migrate run --check`（`node_status` 表、`membership`/`orphaned_at` 列必须存在）。
- 建议加一个「快进时间」的集成用例（用可注入 clock）验证 `Orphaned→Retired→删除` 窗口，避免只测即时行为。

### 风险 / Risks

- **`Degraded` 派生规则未冻结**：plan §6.2 列了 `Degraded` 但没给计算口径（基于 jitter？延迟分位数？
  连续 N 次部分失败？）。在 `LIFECYCLE.md`（plan §18）冻结前，p4-02 无法完整验收四态；
  建议 P4 先用 `Unknown/Alive/Dead` 三态跑通，`Degraded` 留 TODO 并明确标记。
- **跨 tag 合并后是否还需要 per-node alive 字段**：plan §5.1 只定义了 per-tag `node_status`，
  未要求回写单节点 alive；但分发/存活率看板可能想要聚合值。决定聚合位置（视图 vs 冗余列）影响迁移。
- **保留窗口默认值未定**：plan §6.3 未给天数；若不复用 `node_dead_window_days`，需新增 seed 并在
  P4 验收固定，否则 `Orphaned→Retired` 时间不可复现。
- **迟到结果守卫与并发**：Phlogo 仅 per-tag mutex 串行化同 tag；不同 tag 并发提交同节点时，
  `checked_at` 守卫是真正的跨 tag 保护，必须在 Rust 侧显式实现（不能只靠 mutex）。
- **测活在来源 `Dead` 时是否跳过**：p4-01 已指出 fetch 有死源保护，但测活写 `node_status` 是否也要
  跳过 `Dead` 来源下的节点，需明确，否则僵尸来源仍会被写新健康快照。
