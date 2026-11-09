## 6-02 地区配额与节点数上限 · Region Quota & Node Limit

### 目标 / Goal

把 Phlogo `internal/distribution/selector.go` 的**地区比例配额 + 节点数上限选区算法**逐函数移植到 Rust `crates/distribution/src/selector.rs`，**行为等价**（EQUIVALENCE），并写对照 `docs/distribution.md` 的等价性测试：

- 三种 preset：`common` / `all` / `other`，优先级 `URL preset > tag.default_preset > 全局 default_preset > common`（见 `internal/distribution/handler.go` 与 CONVENTIONS §2.8）。
- 按地区比例分配配额（proportional quota）。
- 池内选择三分支：`pool ≤ quota` 全取；`pool ≤ 1.5×quota` 取 top-N 评分；`pool > 1.5×quota` 用 **Efraimidis-Spirakis A-Res 加权抽样**（权重 `max(score, 0.01)`）。
- `common` preset：HK/TW/SG/US/JP 优先，常用地区间平均配额，超额地区均匀随机抽样，不足时从其他常用地区随机补 → 按评分补其他地区 → 补 UN。
- 输出按命名地区顺序排序，同地区内按评分降序编号 `seq`（如 `hk01`）。

### 范围 / Scope

- **新建** `crates/distribution/src/selector.rs`（及 `tests/selector_equivalence.rs`）。
- **复用** p5-01 后的 `NodeView` / `NodeStatus` / `NodeScore` 视图类型（候选集由 p5-03 的筛选 + 缓存提供）。
- **不修改** Phlogo Go 源码（本任务只写文档）。Phlogo 当前 `cache.NodeCacheEntry` 的 `Region` / `Score` / `SeqNo` 等字段在 Rust 侧对应 `NodeView { region, score, .. }`。

### 前置 / Prerequisites

- 已读并理解 `internal/distribution/selector.go` 全文（本页锚定其函数：`SelectWithPreset`/`Select`/`collectAll`/`collectCommon`/`collectProportional`/`assembleOutput`/`selectFromPool`/`selectFromPoolRandom`/`weightedSample`/`computeQuotas`/`randomSample`/`topNByScore`/`normaliseRegion`/`seqID`/`sortRegionsByNamingOrder`）。
- 已读 `docs/distribution.md` 的地区策略小节（common/all/other 语义）。
- 已读 CONVENTIONS §2.8（选区算法要点）与 §2.6（`max_distribute=200`）。
- Rust 工具链；`rand`（可种子化 `StdRng` / `fastrand`）用于抽样分支。

### 步骤 / Steps

#### 1. 类型与常量

```rust
// crates/distribution/src/selector.rs
#[derive(Clone)]
pub struct Candidate {            // 对应 Go cache.NodeCacheEntry 的选区相关字段
    pub id: String,
    pub region: String,           // 原始地区码（可能为空）
    pub score: f64,               // node_scores.score（0..1）
}
pub struct SelectedNode {
    pub candidate: Candidate,
    pub seq_no: usize,            // 1-based，地区内
    pub seq_id: String,           // 如 "hk01"
}
pub const COMMON_REGIONS: [&str; 5] = ["HK", "TW", "SG", "US", "JP"];
pub const MAX_DISTRIBUTE_DEFAULT: usize = 200;  // CONVENTIONS §2.6
```

`normalise_region` 与 Go 完全一致：`ToUpper + TrimSpace`，空串 → `"UN"`。

```rust
fn normalise_region(r: &str) -> String {
    let t = r.trim().to_uppercase();
    if t.is_empty() { "UN".to_string() } else { t }
}
fn seq_id(region: &str, seq: usize) -> String {
    format!("{}{:02}", region.to_lowercase(), seq)
}
```

#### 2. 入口：preset 分派（对应 `SelectWithPreset`）

```rust
pub fn select_with_preset(
    entries: &[Candidate], max_distribute: usize, preset: &str,
) -> Vec<SelectedNode> {
    if entries.is_empty() || max_distribute == 0 { return vec![]; }
    let raw = match preset {
        "other" => collect_all(&filter_out_common(entries), max_distribute),
        "all"   => collect_all(entries, max_distribute),
        _       => collect_common(entries, max_distribute), // default/"common"
    };
    assemble_output(&raw)
}
```

`filter_out_common` 对应 `SelectWithPreset` 的 `other` 分支：保留 `normalise_region(region)` 不在 `COMMON_REGIONS` 的条目。

#### 3. `collectAll` / `collectProportional`（对应 Go 同名函数）

```rust
// collectAll: 非 UN 走 proportional，剩余配额用 top-N 评分补 UN
fn collect_all(entries: &[Candidate], max: usize) -> Vec<Candidate> {
    let (non_un, un): (Vec<_>, Vec<_>) = entries.iter().cloned()
        .partition(|e| normalise_region(&e.region) != "UN");
    let mut sel = collect_proportional(&non_un, max);
    let rem = max.saturating_sub(sel.len());
    if rem > 0 { sel.extend(top_n_by_score(&un, rem)); }
    sel
}

// collectProportional: 按地区节点数比例分配 quota，超额分支用 weighted_sample
fn collect_proportional(entries: &[Candidate], max: usize) -> Vec<Candidate> {
    if entries.is_empty() { return vec![]; }
    let mut region_map: BTreeMap<String, Vec<Candidate>> = group_by_region(entries);
    let mut regions = region_map.keys().cloned().collect::<Vec<_>>();
    sort_regions_by_naming_order(&mut regions);
    let target = max.min(entries.len());
    let quotas = compute_quotas(&region_map, &regions, target);
    let mut selected = Vec::new();
    let mut remaining = Vec::new();
    let mut total_deficit = 0usize;
    for r in &regions {
        let pool = &region_map[r];
        let (got, deficit) = select_from_pool(pool, quotas[r]);
        selected.extend(got);
        total_deficit += deficit;
        if deficit > 0 {
            // 收集未被选中的节点用于 deficit fill
            let got_set: HashSet<_> = selected.iter().map(|c| c.id.clone()).collect();
            for c in pool { if !got_set.contains(&c.id) { remaining.push(c.clone()); } }
        }
    }
    if total_deficit > 0 {
        selected.extend(weighted_sample(&remaining, total_deficit));
    }
    selected
}
```

#### 4. `collectCommon`（对应 Go `collectCommon`）

关键等价点（逐条对齐 Go 实现）：

- `commonPool` / `otherPool` / `unPool` 三路分流；`targetCount = min(max, len(commonPool))`；若 `targetCount==0` **回退到 `collect_all`**（Go 行 112–115）。
- 按地区分组 `regionMap`，`activeRegions` 按 `sort_regions_by_naming_order` 排序。
- 配额分配（`compute_quotas` / Go `computeQuotas`）：`avg = targetCount / n`；前 `n-1` 个地区各得 `avg`，**最后一个地区吸收余数** `targetCount - assigned`。
- 每地区 `select_from_pool_random`（对应 `selectFromPoolRandom`）：`n≤quota` 全取，否则 `random_sample` 均匀随机取 `quota`。
- 累计 `totalDeficit`，把每个地区未选中的 common 节点收进 `commonRemaining`；`totalDeficit>0` 时 `random_sample(commonRemaining, totalDeficit)`（均匀随机补，避免高分集中，Go 行 170–172）。
- 仍不足：`top_n_by_score(otherPool, rem)` → `top_n_by_score(unPool, rem)`（Go 行 175–182）。

#### 5. 三分支池选择（对应 `selectFromPool`，§5.4）

```rust
fn select_from_pool(pool: &[Candidate], quota: usize) -> (Vec<Candidate>, usize) {
    let n = pool.len();
    if n == 0 { return (vec![], quota); }
    if n <= quota {                       // 分支 A: 全取
        (pool.to_vec(), quota - n)
    } else if (n as f64) <= (quota as f64) * 1.5 {  // 分支 B: ≤1.5× → top-N 评分
        (top_n_by_score(pool, quota), 0)
    } else {                             // 分支 C: >1.5× → A-Res 加权抽样
        (weighted_sample(pool, quota), 0)
    }
}
```

#### 6. Efraimidis-Spirakis A-Res 加权抽样（对应 `weightedSample`）

逐条对齐 Go 实现（Go 行 397–431）：

- 仅当 `k>0` 且 `len(pool)>k` 才抽样；否则返回全量（`len(pool)≤k` 时取全部）。
- 权重 `w = max(score, 0.01)`。
- 对每个条目：`u = rng.Float64()`；**若 `u==0` 置 `u=1e-10`**（与 Go 一致）；`key = u^(1/w)`。
- 维护一个**最小堆（size k）**保留 key 最大的 k 个；插入时若未达 k 直接入堆并在满 k 时排序成堆，否则仅当 `key > heap[0].key` 替换堆顶并下沉。

```rust
fn weighted_sample(pool: &[Candidate], k: usize, rng: &mut impl Rng) -> Vec<Candidate> {
    if k == 0 { return vec![]; }
    if pool.len() <= k { return pool.to_vec(); }
    // 最小堆（Reverse）保留 key 最大的 k 个
    let mut heap: BinaryHeap<Reverse<(f64, usize)>> = BinaryHeap::with_capacity(k);
    for (i, e) in pool.iter().enumerate() {
        let w = e.score.max(0.01);
        let mut u = rng.r#gen::<f64>();
        if u == 0.0 { u = 1e-10; }
        let key = u.powf(1.0 / w);
        if heap.len() < k {
            heap.push(Reverse((key, i)));
        } else if key > heap.peek().unwrap().0.0 {
            heap.pop();
            heap.push(Reverse((key, i)));
        }
    }
    // 按 Go 输出顺序：原 pool 顺序
    let mut idxs: Vec<usize> = heap.into_iter().map(|Reverse((_, i))| i).collect();
    idxs.sort_unstable();
    idxs.into_iter().map(|i| pool[i].clone()).collect()
}
```

> 注：Go 最终 `out` 按「堆内顺序」返回，而 `select_from_pool` 的返回值在 `assemble_output` 前只用于配额填充；最终顺序由 `assemble_output` 重排（见下）。等价性测试以「集合成员 + 数量」为主，顺序只在确定性分支断言。

#### 7. `assembleOutput`（对应 Go 同名函数，行 239–265）

- 按 `normalise_region` 分组 → `sort_regions_by_naming_order` 排序地区。
- 同地区内按 `score` 降序排序（**稳定排序**，同分时保留原顺序）。
- `seq_no = 1..`，`seq_id = seq_id(region, seq_no)`（如 `hk01`）。

```rust
fn assemble_output(nodes: &[Candidate]) -> Vec<SelectedNode> {
    // 分组 → 地区按 naming order → 组内 score 降序 → 编 seq
}
```

#### 8. `sortRegionsByNamingOrder`（对应 Go 同名）

`naming.RegionOrder()` 的顺序在 Rust 侧以常量数组给出（`COMMON_REGIONS` 之外还有完整命名顺序，见 `internal/naming/naming.go` 的 `regionOrder`：`HK,TW,JP,SG,US,UK,GB,KR,DE,FR,AU,CA,NL,RU,IN,BR,TR,TH,MY,PH,ID,VN,AR,MX,IT,ES,PL,SE,NO,FI,CH,AT,BE,DK,PT,CZ,HU,RO,ZA,AE,SA,IL,EG,NG,UA`）。已知地区按该数组索引排，未知地区按字母序排在已知之后。

### 验收 / Acceptance

下列命令通过即视为等价性达标（退出码 `0`）：

```bash
cargo test -p distribution selector::
```

**A. 确定性分支精确相等（必须逐条 `assert_eq!`）**

1. **`pool ≤ quota` 全取**：某地区 3 个节点、quota 5 → 选中 3，deficit 2。
2. **`pool ≤ 1.5×quota` top-N**：某地区 6 个节点、quota 5（`6 ≤ 7.5`）→ 选中评分最高的 5 个，且为 `top_n_by_score` 结果。
3. **`pool > 1.5×quota` 加权抽样分支被触发**：某地区 10 节点、quota 5（`10 > 7.5`）→ 走 `weighted_sample`，结果为 5 个、**成员来自该池**且数量恰为 5。
4. **`common` 配额分配**：5 个常用地区各 10 节点、`max_distribute=20` → `avg=4`，前 4 地区各 4、第 5 地区 4（20/5=4 整除），每地区各取 4。
5. **`common` 回退**：无任何常用地区节点、`max_distribute=20` → 结果等同 `collect_all`（all 行为）。
6. **`other` 排除 common**：含 HK/TW 与 DE/FR 节点、`preset=other` → 结果中 `normalise_region` 不含 HK/TW/SG/US/JP。
7. **UN 补足**：`all` 下非 UN 不足配额时，UN 池以 `top_n_by_score` 补足；`common` 下最后以 `top_n_by_score(unPool)` 补足。
8. **输出顺序**：`assemble_output` 输出地区顺序 == `sort_regions_by_naming_order`；同地区内 `score` 严格降序；`seq_id` 形如 `hk01`、`jp02`。

**B. 随机分支 —— 可种子化 + 统计性质（对照 `docs/distribution.md` 的「加权随机使用 A-Res，权重 max(评分,0.01)，评分较低概率较低但不被排除」）**

9. 用固定种子 `StdRng::seed_from_u64(SEED)` 跑 `weighted_sample` 1000 次（例如 10 节点、quota 1），统计每个节点被选中的频率 `f_i`；断言 `f_i` 与 `w_i=max(score_i,0.01)` 的 Spearman/秩相关显著正相关（高分节点被选概率更高），且最低分节点 `f>0`（不被完全排除，因为 `w≥0.01`）。
10. `select_from_pool_random` 在超额时做均匀随机：固定种子跑 1000 次、地区 10 节点 quota 5，每个节点被选频率 ≈ 0.5（±容差），且无节点永远不被选（除非 0）。

**C. 端到端对照 `docs/distribution.md` 的预设语义**

11. 构造一个包含 HK/TW/SG/US/JP + DE + UN 的代表性候选集（每地区若干节点、不同 score），分别用 `preset=common/all/other` 调用 `select_with_preset`，断言：
    - `common`：优先 HK/TW/SG/US/JP，且结果总数 ≤ `max_distribute`。
    - `all`：非 UN 按节点数比例，`UN` 仅在不足时补足（与 `docs/distribution.md` §all 描述一致）。
    - `other`：不含 common 五地区（与 `docs/distribution.md` §other 一致）。
12. 断言任意输入下选中数 ≤ `max_distribute`（对应 CONVENTIONS §2.6 `max_distribute` 上限语义）。

预期输出：`test result: ok. 12+ passed; 0 failed`。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo（Go，`internal/distribution/selector.go`） | Rust（`crates/distribution/src/selector.rs`） |
|---|---|
| `commonRegions` / `commonRegionSet` | `COMMON_REGIONS` 常量 |
| `SelectWithPreset` / `Select` | `select_with_preset`（`other`→`filter_out_common`+`collect_all`；`all`→`collect_all`；默认/`common`→`collect_common`） |
| `collectAll` | `collect_all`（非 UN proportional + UN top-N 补足） |
| `collectCommon` | `collect_common`（三路分流、回退、配额、随机补、otherPool/unPool 补） |
| `collectProportional` | `collect_proportional` |
| `computeQuotas`（末位吸收余数） | `compute_quotas`（同算法） |
| `selectFromPool`（≤quota / ≤1.5× / >1.5× 三分支） | `select_from_pool` |
| `selectFromPoolRandom`（超额均匀随机） | `select_from_pool_random` |
| `weightedSample`（E-S A-Res，`w=max(score,0.01)`，`u==0→1e-10`，min-heap size k） | `weighted_sample`（同上，Rust `BinaryHeap<Reverse>`） |
| `topNByScore` / `randomSample` | `top_n_by_score` / `random_sample` |
| `assembleOutput`（地区 naming order + 组内 score 降序 + seq） | `assemble_output` |
| `normaliseRegion` / `seqID` / `sortRegionsByNamingOrder` | 同名 Rust 函数 |
| preset 优先级在 `handler.go` 解析（`URL > tag.default_preset > 全局 default_preset > common`） | 在 `apps/api` 订阅路由解析后传入 `select_with_preset`，逻辑同 CONVENTIONS §2.8 |

> 关键算法常量均来自源码实证：`max(score,0.01)`、`u==0 ⇒ 1e-10`、`pool ≤ 1.5*quota` 阈值、末位地区吸收余数 `targetCount - assigned`、回退 `collect_all`、common 五地区集合 `["HK","TW","SG","US","JP"]`。

### CI 提示 / CI notes（仅描述，不落地 YAML）

- 单测：`cargo test -p distribution selector::`（确定性断言 + 统计性质，见 §验收 A/B/C）。
- 建议加 **snapshot/种子化测试**：固定 `SEED` 的 `weighted_sample` 与 `random_sample` 输出做 `assert_eq!`（跨语言浮点不保证一致，故 Rust 自洽快照即可，不强求与 Go 逐字节相同；确定性分支才强求精确相等）。
- 性能回归（可选）：对 10k 候选集跑 `select_with_preset` 计时，防止加权抽样堆实现退化。

### 风险 / Risks

- **抽样分支不可跨语言逐字节等价**：Go `math/rand` 与 Rust `rand` 的浮点/顺序不同，A-Res 的随机分支只能做「统计性质 + 种子化快照」等价，**不要**在 CI 里断言与 Go 完全一致（CONVENTIONS §2.8 也仅要求「忠实移植/重写」）。
- **`regionOrder` 顺序错配**：`sort_regions_by_naming_order` 必须复用 `internal/naming/naming.go` 的完整 `regionOrder` 数组；若只放 common 五地区，跨地区排序会与 Phlogo 不一致。
- **`max_distribute` 来源**：Go 从 `configs` 读（CONVENTIONS §2.6 默认 200），Rust 侧必须由 config crate 注入，p5-03 的订阅路由负责传入；若传 0 需在入口处返回空（与 Go `maxDistribute<=0` 一致）。
- **配额整数余数**：`compute_quotas` 的「末位吸收余数」必须与 Go 完全一致（前 n-1 取 `round`，末位=`target-assigned`），否则地区配额分布偏差。
