## 3-03 结果解析与 Node ID 映射 · Result Parsing & Node ID Mapping

### 目标 / Goal

把 MiaoSpeed 经 WebSocket 返回的结果（`p3-02` `collect` 拿到的原始帧）做**解析、单位规范化**，并**精准映射回内部 `node_id`**；
正确处理 **partial success、timeout、disconnect、reconnect、duplicate results**；并实现「**迟到结果不得覆盖更新的状态快照**」
与「**跨 tag 存活合并**」两条核心规则（源自身 `internal/checker/handler.go` 的 `batchAliveDecision` 与 `CONVENTIONS.md` §2.7 / §2.9）。

要点：

1. 每行结果都关联到内部 `node_id`（来自 `p3-02` 的 vendor_id → node_id 映射）。
2. **单位规范化**：`latency_ms` / `jitter_ms` / `speed_kbps` 单位明确；`-1` = dead（沿用 Phlogo `NodeInitialCheck.LatencyMs` 约定），`speed_kbps = 0` = 未测（沿用 `NodeDeepCheck.SpeedKbps`）。
3. partial / timeout / disconnect / reconnect / duplicate 都被显式建模，不产生错位或重复快照。
4. **迟到结果不覆盖新快照**：写入 `node_status` 时比较 `checked_at`，仅当新结果更新时才覆盖（方案 §6.4、CONVENTIONS §2.7/§2.9）。
5. **跨 tag 存活合并**：窗口内任一 `checker_tag` 报告 alive，则该节点 alive（Phlogo `batchAliveDecision` 的 `DISTINCT ON (node_id, checker_tag)` 语义）。

### 范围 / Scope

- 新建/修改：`crates/checker-miaospeed`（解析/规范化/映射/存活判定逻辑）、`crates/lifecycle` 或 `crates/domain`（写入 `node_status`、跨 tag 聚合的纯函数，便于单测）。
- 不在此步：`check_runs` / `node_check_results` 的**落库**（见 p3-04）；评分与缓存重建（P4/P5）。
- 本步骤输出**待持久化的领域对象**（如 `NormalizedCheckResult` + `PerTagHealth`），由 p3-04 写库。

### 前置 / Prerequisites

- p3-02 已完成：Adapter 返回 `Vec<CheckResult>`（含 `internal_node_id`、`initial`、`deep`）。
- Phlogo 现状已读取：`internal/checker/handler.go` `buildCheckRecords`（`:194`）、`batchAliveDecision`（`:303`）、`applyRegionUpdates`（`:263`）。
- `CONVENTIONS.md` §2.7（测活契约）、§2.9（生命周期/迟到结果）。
- 配置项 `node_alive_check_window_minutes` 默认 **240**（CONVENTIONS §2.6）。

### 步骤 / Steps

#### Step 1 — 规范化内部结果类型

```rust
// crates/checker-miaospeed/src/result.rs
use uuid::Uuid;
use chrono::DateTime;

#[derive(Debug, Clone)]
pub struct NormalizedCheckResult {
    pub node_id: Uuid,
    pub checker_tag: String,
    pub run_id: Uuid,                // check_runs.id，幂等/去重键
    pub checked_at: DateTime<Utc>,  // 结果时间戳；迟到比较依据
    pub initial: InitialResult,     // latency_ms；-1 = dead
    pub deep: Option<DeepResult>,
    pub region: Option<String>,     // 来自 deep.region（Phlogo applyRegionUpdates 来源）
}

// 单位约定（与 Phlogo 一致）：
//   latency_ms / avg_latency_ms / jitter_ms : 毫秒 (i32)
//   speed_kbps                              : kbps (i32)，0 = 未测
//   latency_ms == -1                        : 不可达/dead
pub struct InitialResult { pub latency_ms: i32 }   // -1 = dead
pub struct DeepResult {
    pub avg_latency_ms: i32,
    pub jitter_ms: i32,
    pub speed_kbps: i32,             // 0 = 未测
    pub streaming: serde_json::Value,
    pub metadata: serde_json::Value,
}
```

#### Step 2 — 解析 + 单位规范化（从原始帧到 `NormalizedCheckResult`）

```rust
impl MiaoSpeedAdapter {
    /// 把 p3-02 拿到的原始结果帧解析为规范化结构，并按 vendor_id 映射回 node_id
    pub fn normalize(
        &self,
        raw: &[RawResult],                 // 来自 collect()
        id_map: &HashMap<String, Uuid>,    // vendor_id -> node_id
        run_id: Uuid,
        tag: &str,
        now: DateTime<Utc>,
    ) -> Result<Vec<NormalizedCheckResult>, CheckerError> {
        let mut out = Vec::new();
        for r in raw {
            let node_id = *id_map.get(&r.vendor_id)
                .ok_or_else(|| CheckerError::UnknownVendorId(r.vendor_id.clone()))?;
            // 单位规范化：缺失字段给明确缺省（-1=dead，speed 0=未测），空串 region 视为 None
            let initial = InitialResult { latency_ms: r.latency_ms.unwrap_or(-1) };
            let deep = r.deep.as_ref().map(|d| DeepResult {
                avg_latency_ms: d.avg_latency_ms.unwrap_or(-1),
                jitter_ms: d.jitter_ms.unwrap_or(0),
                speed_kbps: d.speed_kbps.unwrap_or(0), // 0 = 未测
                streaming: d.streaming.clone().unwrap_or(serde_json::json!({})),
                metadata: d.metadata.clone().unwrap_or(serde_json::json!({})),
            });
            out.push(NormalizedCheckResult {
                node_id, checker_tag: tag.to_string(), run_id,
                checked_at: r.checked_at.unwrap_or(now),
                initial, deep,
                region: r.deep.as_ref().and_then(|d| d.region.clone()).filter(|s| !s.is_empty()),
            });
        }
        Ok(out)
    }
}
```

#### Step 3 — 去重与 partial / duplicate 处理

```rust
/// 同一次 run 内按 (node_id) 去重；后到的相同 node_id 视为重复，保留已归一化的一条
/// partial：提交的 node_ids 中未出现在结果里的，标记为 Unknown（见方案 §6.2）
pub fn reconcile(
    submitted: &[Uuid],
    results: &[NormalizedCheckResult],
) -> (Vec<NormalizedCheckResult>, Vec<Uuid> /* unreported -> Unknown */) {
    let mut seen = HashSet::new();
    let mut dedup = Vec::new();
    for r in results {
        if seen.insert(r.node_id) { dedup.push(r.clone()); }
    }
    let reported: HashSet<_> = dedup.iter().map(|r| r.node_id).collect();
    let unreported = submitted.iter().filter(|n| !reported.contains(n)).copied().collect();
    (dedup, unreported)
}
```

#### Step 4 — 按 tag 健康快照与「迟到不覆盖」规则

> 把 Phlogo `batchAliveDecision` 的「`latency_ms != -1` 即 alive、跨 tag 合并」重写为纯函数。
> **迟到不覆盖**：写入 `node_status` 时，仅当 `incoming.checked_at > existing.last_checked_at` 才更新（CONVENTIONS §2.7 / §2.9；方案 §6.4）。

```rust
// crates/lifecycle/src/health.rs
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeHealth { Unknown, Alive, Degraded, Dead } // 方案 §6.2

/// 单次结果 -> 该 tag 下该节点的健康
pub fn health_from_result(r: &NormalizedCheckResult) -> NodeHealth {
    if r.initial.latency_ms == -1 { NodeHealth::Dead }
    else if r.deep.is_some() { NodeHealth::Alive } // 有 deep 视为可用 alive
    else { NodeHealth::Alive } // 初筛可达即 alive（与 Phlogo latency!=-1 一致）
}

/// 是否用 incoming 覆盖 existing：仅当 incoming 更新。返回 false = 迟到，丢弃
pub fn should_overwrite(existing: Option<&PerTagHealth>, incoming: &NormalizedCheckResult) -> bool {
    match existing {
        None => true,
        Some(e) => incoming.checked_at > e.last_checked_at, // 迟到结果不覆盖
    }
}

pub struct PerTagHealth {
    pub node_id: Uuid,
    pub checker_tag: String,
    pub health: NodeHealth,
    pub last_checked_at: DateTime<Utc>,
}
```

#### Step 5 — 跨 tag 存活合并（cross-tag alive merge）

> 源自身 `batchAliveDecision`：`SELECT DISTINCT ON (node_id, checker_tag) ... ORDER BY node_id, checker_tag, checked_at DESC`，
> 在 `node_alive_check_window_minutes`（默认 240）窗口内，**任一 tag 的 `latency_ms != -1` 即 alive**。
> Rust 侧对每个 `(node_id, tag)` 取窗口内最新结果，再按 node_id 做 OR 聚合（Phlogo 用两次 UPDATE：alive / dead 集合）。

```rust
/// 给定某 node 的若干按 tag 最新快照（窗口内），合并为跨 tag 是否 alive
/// 与 Phlogo 「A node is alive if ANY checker reported it alive recently」一致
pub fn cross_tag_alive(snapshots: &[PerTagHealth], window: i64 /* minutes */, now: DateTime<Utc>) -> bool {
    let cutoff = now - chrono::Duration::minutes(window);
    snapshots.iter().any(|s| {
        s.last_checked_at >= cutoff && s.health == NodeHealth::Alive
    })
}
```

#### Step 6 — timeout / disconnect / reconnect 的处理约定

- **timeout**：`collect` 超过 Job 超时（`p3-01` 的 30min 上限）未返回 → 视为本次 run 失败，未返回节点走 partial（Step 3 `unreported` → Unknown）。
- **disconnect**：WebSocket 断连（`p3-02`）后 `reconnect`；Adapter 必须能（a）`re-collect` 同一 `job_id`，或（b）`re-submit` 并刷新 vendor_id 映射。
- **duplicate results**：MiaoSpeed 在重连后可能重复下发同一节点结果；Step 3 的按 `(node_id)` 去重 + Step 4 的「迟到不覆盖」共同保证不产生重复/错误快照。

### 验收 / Acceptance

1. **每条结果精准映射到 node_id**：
   ```bash
   cargo test -p checker-miaospeed --lib result::tests::maps_each_result_to_correct_node_id
   ```
   预期：pass；`normalize` 输出的每个 `node_id` 与 `id_map` 一致，无 `UnknownVendorId`。

2. **迟到结果不覆盖新快照**：
   ```bash
   cargo test -p checker-miaospeed --lib result::tests::late_result_does_not_overwrite_newer_snapshot
   ```
   预期：先写 `checked_at = T+10` 的快照，再来的 `checked_at = T` 结果被 `should_overwrite` 判为 `false`，`node_status` 不变。

3. **partial / disconnect / duplicate 覆盖**：
   ```bash
   cargo test -p checker-miaospeed --lib result::tests::partial_and_duplicate_and_reconnect
   ```
   预期：提交的 5 个节点只返回 3 个 → 2 个 `unreported` 标记 Unknown；重复下发同一节点 → 去重后仅 1 条；reconnect 后结果一致。

4. **跨 tag 存活合并**：
   ```bash
   cargo test -p checker-miaospeed --lib result::tests::cross_tag_alive_merge
   ```
   预期：tag A dead（latency=-1）、tag B alive 且 `checked_at` 在窗口内 → `cross_tag_alive == true`；
   两 tag 都 dead 或 alive 超出窗口 → `false`。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo | Rust（本步骤） |
|---|---|
| `internal/checker/handler.go` `buildCheckRecords`（`:194`）：`NodeInitialCheck{LatencyMs}` + `NodeDeepCheck{AvgLatencyMs,JitterMs,SpeedKbps,StreamingResult,Metadata}` + region 提取 | `NormalizedCheckResult`（`result.rs`）：合并 initial/deep，单位与 `-1`/`0` 约定完全一致；`region` 来自 `deep.region`（对应 `applyRegionUpdates`） |
| `internal/checker/handler.go` `batchAliveDecision`（`:303`）：`DISTINCT ON (node_id, checker_tag)` 取窗口内最新，`latency_ms != -1` → alive，跨 tag OR | `health_from_result` + `cross_tag_alive`（Step 4/5），窗口 `node_alive_check_window_minutes` 默认 240（CONVENTIONS §2.6） |
| `internal/checker/handler.go` 两次 `UPDATE alive/dead`（`:356-373`） | 改为先写 per-tag `node_status`（`should_overwrite` 把关），跨 tag 聚合在 lifecycle（本步纯函数，落库见 p3-04） |
| `CONVENTIONS.md` §2.7 / §2.9「迟到结果不能覆盖更新的状态快照」 | `should_overwrite` 仅当 `incoming.checked_at > existing.last_checked_at`；否则丢弃 |
| `internal/checker/handler.go` per-tag mutex 串行化（`:53`） | 由 Apalis Job 的 `run_id` 幂等 + 写入层 `ON CONFLICT` 替代（见 p3-01 / p3-04） |

### CI 提示 / CI notes

> 仅描述未来 CI 命令，**不落地 YAML**。

- **纯函数单测**：`cargo test -p checker-miaospeed --lib result` + `cargo test -p lifecycle --lib health`（不依赖数据库/网络）。
- **复用 golden fixtures**：`tests/fixtures/checker/` 放「原始帧 → 期望 `NormalizedCheckResult`」用例；`cargo test` 全量跑。
- **lint/build**：`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`（CONVENTIONS §5）。
- **回归守护**：任何「迟到不覆盖」「跨 tag 合并」规则变更都需同步本步测试，CI 必须 pass 才能合入。

### 风险 / Risks

- **单位/缺省歧义**：MiaoSpeed 结果字段的缺省/空值语义以 **P0-02 协议**为准；`-1`=dead、`0`=未测是 Phlogo 约定，需确认 MiaoSpeed 是否一致，否则规范化会错（已在 Step 2 用 `unwrap_or` 显式兜底，但语义需 P0-02 校验）。
- **迟到比较依赖时钟**：`checked_at` 必须来自可信来源（优先 MiaoSpeed 结果自带时间戳，回退到本机 `now`）；时钟回拨会破坏「迟到不覆盖」，需监控。
- **跨 tag 合并与 per-tag 快照的边界**：写入 `node_status` 是 per-tag（方案 §5.1、CONVENTIONS §2.3 #7），跨 tag 聚合是派生视图/计算；务必保证聚合不反向污染单个 tag 快照。
- **partial 的 Unknown 与 Degraded 区分**：方案 §6.2 有 `Unknown/Alive/Degraded/Dead` 四态；本步 `unreported → Unknown`，有 deep 才 `Alive`。`Degraded` 的判定（高延迟/抖动）建议放到 P4 评分/健康层，本步先不引入以免与 Phlogo 基线偏离。
