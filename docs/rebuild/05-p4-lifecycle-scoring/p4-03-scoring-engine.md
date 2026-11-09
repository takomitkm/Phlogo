## 5-03 评分引擎 · Scoring Engine (EWMA baseline)

### 目标 / Goal

1. 忠实复现 `internal/scoring/scoring.go` 的 EWMA 评分基线（plan §9.1「先复现可解释基线，再迭代」）。
2. 评分按 `(node, checker_tag)` 独立计算，取最近 20 条 deep check（最新 `i=0`），权重 `w_i = alpha^i`。
3. 在基线之上新增（net-new）：`version` 字段、`components/explanations` 分项解释、
   样本量不足时返回 `insufficient_data`。
4. 提供可复现的单元测试，对给定 fixture 复现精确 EWMA 数值（移植 `computeScore`）。

> 复现口径以 **`internal/scoring/scoring.go` 代码**为准，不是以 `docs/scoring.md` / `CONVENTIONS.md §2.5`
> 的文档意图为准（两者存在分歧，见 §风险）。

### 范围 / Scope

- **新建**：`crates/scoring/src/engine.rs`（`compute_score` 纯函数）、
  `crates/scoring/src/params.rs`（`ScoreParams` + 默认值 + 配置键映射）、
  `crates/scoring/src/model.rs`（`ScoreResult { score, components, version, status }`）。
- **修改**：`ComputeAndUpdateTagScore` 等价逻辑（写 `node_scores`，再 `AVG(score>0)` 回写 `nodes.score`）。
- **修改**：`node_scores` 表加 `components`(JSON) / `version` / `calculated_at`（plan §5.1）。
- **不做**：实际网络测活（P3）；节点指纹（P2）；订阅筛选（P5）。
- **迭代项（标注，不在基线强制）**：`insufficient_data` 样本门槛默认值、`Degraded` 联动。

### 前置 / Prerequisites

- P3 已落 `node_check_results`（或等价 `node_deep_checks` 迁移）且带 `avg_latency_ms`、`jitter_ms`、
  `speed_kbps`、`checked_at`，按 `(node_id, checker_tag)` 可查最近 20 条。
- 配置键已 seed（见 `CONVENTIONS.md §2.6`）：`score_decay_alpha`、`score_weight_latency`、
  `score_weight_stability`、`score_weight_speed`、`score_latency_curve_k`、
  `score_jitter_linear_threshold_ms`。

### 步骤 / Steps

#### 步骤 1 — 复现 EWMA 算法（对照 internal/scoring/scoring.go）

**不可配置常量**（scoring.go:19-23，`const`，不是 config）：

```
maxLatencyMs = 3000
maxJitterMs  = 1000
maxSpeedKbps = 163840
```

**参数默认值**（scoring.go:210-217；若对应 config 行缺失则使用；config seed 见 §2.6）：

```
alpha              = 0.7    # score_decay_alpha
wLatency           = 0.7    # score_weight_latency
wStability         = 0.3    # score_weight_stability
wSpeed             = 0.55   # score_weight_speed
latencyCurveK      = 0.3    # score_latency_curve_k
jitterThresholdMs  = 70     # loadParams 兜底值；config seed 实际为 200（见 §风险）
```

**`compute_score` 精确步骤**（逐行对应 scoring.go:113-188）：

```
1. 若 records 为空 -> 返回 0.0
2. 若 alpha <= 0 或 alpha > 1 -> alpha = 0.7
3. 遍历 i=0..n-1（最新在前）：
     latency = r.avg_latency_ms; 若 == -1 -> 3000
     jitter  = r.jitter_ms;      若 == -1 -> 1000
     w = alpha^i
     sumW      += w
     sumLat    += w * latency
     sumJit    += w * jitter
4. weightedLatency = sumLat / sumW
   weightedJitter  = sumJit / sumW
5. latencyX  = clamp01(weightedLatency / 3000)
   若 latencyCurveK <= 0 -> 0.3
   scoreLatency = 1 - ln(1 + k*latencyX) / ln(1 + k)
6. jitterX = clamp01(weightedJitter / 1000)
   t = clamp01(jitterThresholdMs / 1000)      # 注意：代码除以 1000（=maxJitterMs）
   若 t <= 0 或 t >= 1 -> t = 70 / 1000
   若 jitterX <= t -> scoreStability = 1 - jitterX
   否则 scoreStability = (1 - t) * sqrt((1 - jitterX) / (1 - t))
7. wLat = clamp01(wLat); wStab = clamp01(wStab); wSpeed = clamp01(wSpeed)
   totalQ = wLat + wStab; 若 totalQ <= 0 -> wLat,wStab=0.7,0.3; totalQ=1
   wLAdj = wLat / totalQ; wSAdj = wStab / totalQ
   qualityScore = scoreLatency*wLAdj + scoreStability*wSAdj
8. speed = records[0].speed_kbps            # 仅取最新一条
   若 speed >= 0:                            # 代码恒为真（缺速度默认 0）
     speedX = clamp01(speed / 163840)
     scoreSpeed = sqrt(speedX)
     score = qualityScore*(1 - wSpeed) + scoreSpeed*wSpeed
   否则 score = qualityScore
9. 返回 round(score * 1e6) / 1e6            # 6 位小数
```

Rust 端口（faithful port，类型对齐 Go 的 `int`/`i32`，`-1` 表示不可达）：

```rust
pub struct ScoreParams {
    pub alpha: f64,
    pub w_latency: f64,
    pub w_stability: f64,
    pub w_speed: f64,
    pub latency_curve_k: f64,
    pub jitter_linear_threshold_ms: f64,
}

pub struct DeepCheck {
    pub avg_latency_ms: i32, // -1 = 不可达
    pub jitter_ms: i32,      // -1 = 不可达
    pub speed_kbps: i32,     // 缺速度时 Go 默认 0（见 §风险：与文档意图分歧）
}

pub fn compute_score(records: &[DeepCheck], p: &ScoreParams) -> f64 {
    if records.is_empty() { return 0.0; }
    let mut alpha = p.alpha;
    if alpha <= 0.0 || alpha > 1.0 { alpha = 0.7; }

    let (mut sum_w, mut sum_lat, mut sum_jit) = (0.0_f64, 0.0, 0.0);
    for (i, r) in records.iter().enumerate() {
        let mut latency = r.avg_latency_ms as f64;
        let mut jitter  = r.jitter_ms as f64;
        if latency == -1.0 { latency = 3000.0; }
        if jitter  == -1.0 { jitter  = 1000.0; }
        let w = alpha.powi(i as i32);
        sum_w   += w;
        sum_lat += w * latency;
        sum_jit += w * jitter;
    }
    let w_lat = sum_lat / sum_w;
    let w_jit = sum_jit / sum_w;

    let latency_x = clamp01(w_lat / 3000.0);
    let mut k = p.latency_curve_k;
    if k <= 0.0 { k = 0.3; }
    let score_latency = 1.0 - (1.0 + k * latency_x).ln() / (1.0 + k).ln();

    let jitter_x = clamp01(w_jit / 1000.0);
    let mut t = clamp01(p.jitter_linear_threshold_ms / 1000.0);
    if t <= 0.0 || t >= 1.0 { t = 70.0 / 1000.0; }
    let score_stability = if jitter_x <= t {
        1.0 - jitter_x
    } else {
        (1.0 - t) * ((1.0 - jitter_x) / (1.0 - t)).sqrt()
    };

    let w_lat = clamp01(p.w_latency);
    let w_stab = clamp01(p.w_stability);
    let w_speed = clamp01(p.w_speed);
    let mut total_q = w_lat + w_stab;
    let (w_lat_adj, w_stab_adj) = if total_q <= 0.0 {
        total_q = 1.0; (0.7, 0.3)
    } else { (w_lat / total_q, w_stab / total_q) };
    let quality = score_latency * w_lat_adj + score_stability * w_stab_adj;

    let mut score = quality;
    let speed = records[0].speed_kbps as f64;   // 仅最新一条
    if speed >= 0.0 {                            // Go 基线恒为真（见 §风险）
        let speed_x = clamp01(speed / 163840.0);
        let score_speed = speed_x.sqrt();
        score = quality * (1.0 - w_speed) + score_speed * w_speed;
    }
    (score * 1e6).round() / 1e6
}

fn clamp01(v: f64) -> f64 { if v < 0.0 { 0.0 } else if v > 1.0 { 1.0 } else { v } }
```

#### 步骤 2 — 节点级聚合（对照 scoring.go:96-109）

```rust
// ComputeAndUpdateTagScore：写 node_scores(node_id, checker_tag, score, components, version)
// 再 nodes.score = AVG(score) WHERE node_id=? AND score > 0
// 注意：与 Go 一致，只有 score>0 的 tag 参与 AVG；无 tag 时 AVG=0。
```

#### 步骤 3 — 新增 version / components / insufficient_data（net-new）

```rust
pub struct ScoreResult {
    pub score: f64,
    pub status: ScoreStatus,           // Ok | InsufficientData
    pub version: String,               // 例如 "ewma-v1"（net-new，Go 无）
    pub components: ScoreComponents,   // net-new：分项解释
}

pub struct ScoreComponents {
    pub weighted_latency_ms: f64,
    pub weighted_jitter_ms: f64,
    pub latency_score: f64,
    pub stability_score: f64,
    pub quality_score: f64,
    pub speed_score: Option<f64>,      // 仅当 speed>=0（见 §风险）
    pub sample_count: usize,
}
```

样本量门槛（plan §9.1「无足够样本应返回 insufficient_data」）为 **net-new**：Go 基线无此门槛
（空记录仅返回 0）。Rust 引入可配置 `score_min_samples`（建议默认如 1 或 3），低于则
`status = InsufficientData`、`score` 返回 0 或 `None`，不写 `nodes.score`。默认值需在 P4 验收固定。

### 验收 / Acceptance

#### A. EWMA 数值复现（移植 compute_score 的黄金 fixture）

fixture（最新在前，`avg_latency_ms`/`jitter_ms`/`speed_kbps`）：

```
i=0: lat=100, jit=20,  speed=163840
i=1: lat=200, jit=40,  speed=0
i=2: lat=500, jit=100, speed=0
params: alpha=0.7, w_lat=0.7, w_stab=0.3, w_speed=0.55, k=0.3,
        jitterLinearThresholdMs=200   # config seed 实际值，t=0.2
```

手算预期（按 §步骤 1 算法，6 位四舍五入）：

```
sumW   = 1 + 0.7 + 0.49 = 2.19
w_lat  = (1*100 + 0.7*200 + 0.49*500)/2.19 = 485/2.19      ≈ 221.461187
w_jit  = (1*20 + 0.7*40 + 0.49*100)/2.19   = 97/2.19       ≈ 44.292237
latX   = 221.461187/3000                              ≈ 0.0738204
scoreLat = 1 - ln(1+0.3*0.0738204)/ln(1.3)            ≈ 0.916499
jitX   = 44.292237/1000                               ≈ 0.0442922  (<= t=0.2)
scoreStab = 1 - 0.0442922                             ≈ 0.955708
quality = 0.916499*0.7 + 0.955708*0.3                 ≈ 0.928262
speed  = 163840 -> speedX=1 -> scoreSpeed=1
score  = 0.928262*(1-0.55) + 1*0.55                   ≈ 0.967718
```

```bash
cargo test -p scoring ewma_baseline_fixture
# 期望退出码 0；断言 (compute_score(&fixture, &params) - 0.967718).abs() < 1e-6
# 注：精确参考值应以「运行移植后的 compute_score」为准；手算用于交叉验证，建议保留 1e-6 容差。
```

#### B. 不可达记录按最差处理（-1 → 3000/1000）

```bash
cargo test -p scoring unreachable_record_worst_case
# 期望退出码 0：单条 lat=-1,jit=-1 时，weightedLatency=3000 -> latencyX=1 -> scoreLatency=0；
# 断言 score 接近 0（quality≈0），且不发生 panic / 除零。
```

#### C. 6 位小数四舍五入

```bash
cargo test -p scoring rounds_to_6_decimals
# 期望退出码 0：任意产生 7+ 位小数值的输入，断言返回值的 1e6 倍为整数。
```

#### D. 节点级 AVG（仅 score>0 参与）

```bash
cargo test -p scoring node_avg_ignores_zero_scores
# 期望退出码 0：构造 node 有 tagA score=0.8、tagB score=0.0；
# 断言 nodes.score = 0.8（不是 0.4）。
```

#### E. version / insufficient_data（net-new）

```bash
cargo test -p scoring returns_version_and_insufficient_data
# 期望退出码 0：
#  (1) 正常计算的 ScoreResult.version == "ewma-v1"（或选定的版本串）；
#  (2) 样本数 < score_min_samples 时 status == InsufficientData 且不写 nodes.score。
```

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `internal/scoring/scoring.go` `computeScore`（scoring.go:113）：纯计算函数 → 移植为 `compute_score`。
- `internal/scoring/scoring.go` `ComputeAndUpdateTagScore`（scoring.go:59）：写 `node_tag_scores` +
  `AVG(score>0)` 回写 `nodes.score` → 移植为 `ComputeAndUpdateTagScore` + `node_scores` 表。
- `internal/scoring/scoring.go` `loadParams`（scoring.go:200）：6 个 config 键 + TTL 1min 缓存 →
  映射为 `ScoreParams` 加载（Rust 侧建议保留「参数变更需等新复筛才生效」语义，或在迁移里明确）。
- **缺失对照（net-new）**：`version` 字段、`components/explanations`、`insufficient_data` 样本门槛、
  结构化 `ScoreResult` —— Go 基线只返回 `float64` 直写，均无。

### CI 提示 / CI notes

- 单测：`cargo test -p scoring`（含 EWMA 黄金 fixture + 不可达/舍入/AVG/版本门槛）。
- 集成（Postgres）：`cargo test --test integration scoring_recompute`。
- 配置漂移检查：CI 里断言 seed 配置含全部 6 个 `score_*` 键（`CONVENTIONS.md §2.6`），
  且 `score_jitter_linear_threshold_ms` 与「评分引擎实际读取的兜底/默认值」一致（见 §风险）。
- 基准回归：把本页 §验收 A 的 fixture 当 golden test，防止后续迭代无意改变基线数值。

### 风险 / Risks

- **speed 分支与文档意图分歧（重要）**：Go 代码 `speed = records[0].speed_kbps` 且 `if speed >= 0`
  恒成立（缺速度时 `buildCheckRecords` 把 `speedKbps` 默认填 `0`，见 `internal/checker/handler.go:238`）。
  因此 Go 基线**始终**混入 speed 权重：缺速度时 `scoreSpeed=0`，实际 `score = quality * 0.45`。
  但 `docs/scoring.md` 与 `CONVENTIONS.md §2.5` 的意图是「缺速度则不参与计算」。
  本步骤以**代码**为基线（始终混合）；「缺速度真的不混合」属迭代决策——若采用，Rust 需把
  `speed_kbps` 改为 `Option<i32>`，`None` 时跳过 wSpeed 分支。需在 P4 验收明确选用哪种。
- **抖动阈值默认值冲突**：`scoring.go` `loadParams` 硬编码兜底 `jitterThresholdMs=70`，
  而 `CONVENTIONS.md §2.5/§2.6` 与 `docs/scoring.md` 的文档默认是 `200ms`（config seed
  `score_jitter_linear_threshold_ms=200`，使 `t=0.2`）。P4 须定一个权威值并写进迁移 seed /
  `DATA_MODEL.md`，否则 Rust 端口与 Go 行为漂移。
- **`ORDER BY checked_at DESC` 缺 tie-break**：`scoring.go:66` 取最近 20 条仅按 `checked_at DESC`，
  同毫秒多记录顺序不定 → EWMA 权重分配不确定。Rust 建议加 `, id DESC` 保证确定性（影响黄金 fixture 复现）。
- **参数缓存 TTL 语义**：Go 用 `sync.RWMutex` + 1min TTL（scoring.go:43, 200-256），改参后需等新复筛才生效。
  Rust 端口要么保留该语义，要么改为显式失效；否则「改配置立即重算」会与 Go 行为不一致。
- **`Degraded` 与评分**：plan §6.2 的 `NodeHealth::Degraded` 是否影响评分（如降权）未定义，
  当前基线完全不读健康状态，仅读 deep check 数值；若迭代要联动，需另立规则。
