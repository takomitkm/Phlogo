## 5-01 状态机与合法转换 · State Machine & Valid Transitions

### 目标 / Goal

1. 在 `crates/lifecycle`（域枚举）+ `crates/registry`（实体/关系）中定义四个状态枚举：
   `SourceStatus`、`FetchPolicy`、`NodeHealth`、`NodeMembership`。
2. 明确每个枚举的合法转换（状态机），并用**状态机测试套件**覆盖合法/非法转换。
3. 把 Phlogo 中散落的关联清理（来源删除、孤儿删除、死源清理）收敛为**事务内的原子操作**，
   保证并发与重试幂等（plan §6.3 / §6.4）。

> Net-new 声明：Phlogo 当前**没有** `Replace/Accumulate` 区分，也**没有** `Orphaned/Retired`
> 状态——`internal/node/upsert.go` 的 `removeSourceMappings` 直接物理删除孤儿节点。
> 下列 `Suspended` 也是相对于 Phlogo 仅 `active/dead` 的新增。这些都是 Rust Core 的新增语义。

### 范围 / Scope

- **新建**：`crates/lifecycle/src/state.rs`（四个枚举 + `Transition` 规则表）、
  `crates/lifecycle/src/transitions.rs`（转换校验 + 错误类型）。
- **修改**：`crates/registry` 的 `Source`、`Node`、`NodeSource` 实体增加状态列；
  `migrations/` 增加 `sources.status` 扩展、`nodes`/`node_sources` 的 `membership`/`health` 相关列
  （具体由 p4-02 落 `node_status`）。
- **迁移逻辑**：`SourceService::delete`、`RemoveSourceMappings`、死源清理改为事务封装。
- **不做**：实际抓取/测活执行（P2/P3 负责）；评分计算（p4-03）；订阅输出（P5）。
- **不做**：`Degraded` 的派生计算（放到 p4-02 / `LIFECYCLE.md`）。

### 前置 / Prerequisites

- P1 已完成：`nodes` / `sources` / `node_sources` 基础表与 CRUD（见 `CONVENTIONS.md §3` 映射）。
- 已定义 `NormalizedNode` 指纹（plan §7 / P2），本步骤复用其 `identity_hash`。
- `sqlx` + Postgres 可用；`cargo sqlx prepare` 离线查询已就绪。

### 步骤 / Steps

#### 步骤 1 — 定义四个状态枚举

在 `crates/lifecycle/src/state.rs`：

```rust
// 来源状态（plan §6.1）。Phlogo 仅 active/dead，Suspended 为新增。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceStatus {
    Active,
    Suspended,
    Dead,
}

// 抓取策略（plan §6.1）。Phlogo 无此区分，Replace/Accumulate 均为 net-new。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FetchPolicy {
    Replace,    // 整次成功抓取后，用新集合替换该来源旧关联
    Accumulate, // 只增量加关联，不因一次未见到旧节点而删关联
}

// 节点健康（plan §6.2）。Phlogo 仅有 nodes.alive bool；按 node_id+checker_tag 维度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeHealth {
    Unknown,
    Alive,
    Degraded, // net-new，派生规则待 LIFECYCLE.md 冻结
    Dead,
}

// 节点归属（plan §6.3）。Phlogo 无此状态，Orphaned/Retired 为 net-new。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeMembership {
    Linked,    // 至少被一个来源引用
    Orphaned,  // 无任何来源引用，进入保留窗口计时
    Retired,   // 超过保留期限，待清理策略物理删除
}
```

#### 步骤 2 — 状态转换规则表（合法转换）

`crates/lifecycle/src/transitions.rs`。规则直接来自 plan §6.1 / §6.3：

```rust
pub enum TransitionError {
    InvalidSourceTransition(SourceStatus, SourceStatus),
    InvalidMembershipTransition(NodeMembership, NodeMembership),
}

// SourceStatus: Active <-> Suspended 可逆；二者均可 -> Dead；Dead 为终态（不可复活）。
// 与 Phlogo 一致：MarkDeadAndClean 用条件 UPDATE status != 'dead' 防并发重复清理
// （见 internal/source/service.go:182）。
pub fn can_transition_source(from: SourceStatus, to: SourceStatus) -> bool {
    use SourceStatus::*;
    matches!(
        (from, to),
        (Active, Suspended)
            | (Suspended, Active)
            | (Active, Dead)
            | (Suspended, Dead)
            // Dead 终态：不可转回（防止僵尸来源复活，见 upsert.go 死源保护）
            | (Dead, Dead) // 幂等
    )
}

// NodeMembership: Linked -> Orphaned（无来源引用时）；
// Orphaned -> Retired（超过保留窗口）；Retired -> 物理删除（清理策略，非状态转换）；
// 任何来源重新引用 Orphaned/Retired 节点 -> Linked（复活）。
pub fn can_transition_membership(from: NodeMembership, to: NodeMembership) -> bool {
    use NodeMembership::*;
    matches!(
        (from, to),
        (Linked, Orphaned)
            | (Orphaned, Linked)   // 被新来源重新引用
            | (Orphaned, Retired)  // 保留窗口到期
            | (Linked, Linked)     // 幂等
            | (Orphaned, Orphaned) // 幂等
            | (Retired, Linked)    // 清理前被重新引用（边缘情况，需事务保护）
    )
}
```

#### 步骤 3 — 把关联清理收敛为事务（对照 Phlogo）

Phlogo 现状（`internal/source/service.go` + `internal/node/upsert.go`）：

- `SourceService::Delete`（service.go:143）先 `cleanup.RemoveSourceMappings` 再删 `source_fetch_histories` 与 `sources`。
- `removeSourceMappings`（upsert.go:169）删 `node_sources` 后，对真孤儿节点删测活历史再 `DELETE FROM nodes WHERE id=? AND NOT EXISTS (SELECT 1 FROM node_sources ...)`。
- `UpsertFromFetchResult`（upsert.go:38）单节点 upsert 用事务 `SELECT-then-INSERT` 防并发重复映射；
  空结果/死源走 `removeSourceMappings`。

Rust 等价实现（伪代码，放在 `crates/registry/src/source_repo.rs` 的事务里）：

```rust
// 删除来源：事务内 删映射 -> 清理孤儿节点历史 -> 物理删孤儿 -> 删来源
tx.transaction(|t| {
    let mappings = t.load_node_sources(source_id)?;
    t.delete_node_sources(source_id)?;
    for m in mappings {
        // 仅当该节点确实再无其它来源引用才清理（原子 NOT EXISTS 守卫）
        if t.orphan_count(m.node_id)? == 0 {
            t.delete_check_history(m.node_id)?; // initial/deep/scores
            t.delete_node_if_orphaned(m.node_id)?; // AND NOT EXISTS node_sources
        }
    }
    t.delete_source(source_id)?;
    Ok(())
})
```

死源清理沿用 Phlogo 的「条件 UPDATE + 仅赢家清理」幂等模式（service.go:182 的
`WHERE status != 'dead'` + `RowsAffected==0` 跳过）：

```rust
let affected = t.update_source_status_if(source_id, Dead, "status != 'dead'")?;
if affected > 0 {
    remove_source_mappings_in_tx(t, source_id)?; // 仅赢家执行清理
}
```

`UpsertFromFetchResult` 的**死源保护**（upsert.go:52，`Status == "dead"` 直接跳过）必须保留：
来源已 `Dead` 时拒收在途 fetch 结果，避免复活僵尸映射。

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地**：
- 新 crate **`crates/lifecycle`**：`state.rs`（`SourceStatus{Active,Suspended,Dead}` / `FetchPolicy{Replace,Accumulate}` / `NodeHealth{Unknown,Alive,Degraded,Dead}` / `NodeMembership{Linked,Orphaned,Retired}`，全部带 `as_str()`）+ `transitions.rs`（规则表 + `can_transition_*` + `transition_*` 校验 + `TransitionError`）+ `ops.rs`（事务化 DB 清理）。
- migration **`0006_lifecycle`**：`nodes.retired_at` + config seed `orphan_retention_days=14`（规格建议复用窗口或新增 seed——选择新增专用键，默认 14 天，P4-02 的清理 Job 用同一读取路径）。
- `ops.rs`：`mark_dead_and_clean`（**条件 UPDATE `status <> 'dead'`，仅赢家清理**，同事务内删映射+标孤儿）、`mark_orphaned_if_lonely`（原子 `NOT EXISTS` 守卫）、`retire_expired_orphans`（`make_interval(days => $1)`，写 `retired_at`）、`delete_retired_nodes`（`NOT EXISTS node_sources` 守卫，复活节点幸存）。

**实测（校机，PG 18.6）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0**（35.41s） |
| `cargo test --workspace` | **0**——`transitions` 2/2（全部合法/非法转换断言）+ `lifecycle::p401` 2/2 |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |

**PG 集成 2 场景**：`mark_dead_and_clean_is_idempotent`（共享节点仍 Linked、独占节点 Orphaned、二次调用 skip、映射清零、status=dead）✓；
`orphan_retire_delete_lifecycle`（孤儿 → 回溯 30 天 → Retired → 复活重链 → 物理删守卫下幸存且 Linked）✓。

**验收对照**：状态机断言（Active↔Suspended 可逆、Dead 终态、Linked→Orphaned→Retired、Orphaned/Retired→Linked 复活、Linked↛Retired）全过；死源保护沿用 P2-04 的 `reconcile_fetch_result` 守卫（dead/suspended 拒收，已在 P2 实测）；事务幂等即 `mark_dead_and_clean` 集成场景。
`make_interval` 参数用 `i32` 绑定（PG 的 `days => int` 无 bigint 重载，实测踩坑）。

### 验收 / Acceptance

状态机测试套件（建议放在 `crates/lifecycle/tests/transitions.rs`）：

```bash
cargo test -p lifecycle transitions
# 期望退出码 0；覆盖以下断言：

# 合法：Active -> Suspended -> Active 可逆
assert!(can_transition_source(Active, Suspended));
assert!(can_transition_source(Suspended, Active));

# 合法：Active/Suspended -> Dead；Dead 不可复活
assert!(can_transition_source(Active, Dead));
assert!(can_transition_source(Suspended, Dead));
assert!(!can_transition_source(Dead, Active));
assert!(!can_transition_source(Dead, Suspended));

# 合法：Linked -> Orphaned -> Retired；Orphaned -> Linked 复活
assert!(can_transition_membership(Linked, Orphaned));
assert!(can_transition_membership(Orphaned, Retired));
assert!(can_transition_membership(Orphaned, Linked));

# 非法：Linked 不能直接 Retired；Retired 不能直接 Linked 外的状态（依规则表）
assert!(!can_transition_membership(Linked, Retired));

# 死源保护：status='dead' 的来源拒收 upsert（模拟 upsert.go:52）
# 期望返回 Err(SourceDead) 且 node_sources 行数不变
```

事务幂等测试（`crates/registry` 集成测试，需 Postgres）：

```bash
cargo test -p registry --test source_delete_idempotent
# 期望退出码 0：并发两次 delete 同一来源，node_sources 与孤儿节点只被清理一次；
# 第二次 delete 对孤儿节点的 NOT EXISTS 守卫不应再删任何行。
```

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `internal/source/service.go`：`Status active/dead`、`ignore_dead`、`MarkDeadAndClean`
  （条件 UPDATE 防并发重复清理，service.go:182）→ 映射为 `SourceStatus` + `FetchPolicy` + 事务化清理。
- `internal/node/upsert.go`：`UpsertFromFetchResult` 的 SELECT-then-INSERT 事务（upsert.go:107）、
  `removeSourceMappings` 的 `NOT EXISTS` 原子守卫（upsert.go:157, 192）→ 映射为 `NodeMembership`
  转换与 `delete_node_if_orphaned` 守卫。
- **缺失对照（net-new）**：`Suspended`、`FetchPolicy`(Replace/Accumulate)、`Orphaned`/`Retired`、
  按 tag 的 `NodeHealth`——Phlogo 代码里均不存在，需新建表/枚举/迁移。

### CI 提示 / CI notes

- 单测覆盖：`cargo test -p lifecycle -p registry`（状态机 + 事务幂等）。
- 集成（Postgres）：`cargo test --test integration lifecycle_transitions`。
- 迁移必须：`sqlx migrate run --check` 通过（新增 `sources.status` 取值扩展、`node_sources` 时间戳、
  后续 `node_status`/`membership` 列由 p4-02 落）。
- 并发幂等建议用 `cargo test` 单测里的 `tokio::join!` 双写模拟 race，或集成测试里两条并行事务。

### 风险 / Risks

- **`Suspended` 语义需与 API/前端对齐**：plan §6.1 只说「暂停抓取但保留关联」，未定义它是否参与测活/
  分发；需在 `LIFECYCLE.md` 冻结前明确，否则前端开关无统一口径。
- **`Orphaned -> Retired` 的保留窗口长度**：plan §6.3 只说「超过保留期限」，未给数值；建议复用
  `CONVENTIONS.md §2.6` 的 `node_dead_window_days` 或在迁移 seed 新增 `orphan_retention_days`，并在
  P4 验收里固定一个默认值（见 p4-02）。
- **事务原子性 vs Apalis Job 重试**：plan §6.4 要求 Job 重试幂等。若清理拆成多个事务，重试可能重复
  触发；务必把「删映射 + 清孤儿 + 删来源」放进同一事务，或用 `status != 'dead'` 同款条件 UPDATE 守卫。
- **Phlogo 死源保护只拦 fetch**：`upsert.go` 只拦 `UpsertFromFetchResult`，未拦测活写历史；
  Rust 侧需确认测活在来源 `Dead` 时是否也应跳过（避免给僵尸来源写新 `node_status`）。
