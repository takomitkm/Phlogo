# P4 生命周期与评分 · Lifecycle & Scoring

> 阶段目录：`docs/rebuild/05-p4-lifecycle-scoring/`
> 上游依据：`S:\Downloads\paily-core-rust-rebuild-plan.md` §6 / §9 / P4 / §15
> 现状基准：`docs/rebuild/CONVENTIONS.md` §2.3 / §2.5 / §2.7 / §2.9
> 代码基准：`internal/source/service.go`、`internal/node/upsert.go`、`internal/checker/handler.go`、`internal/scoring/scoring.go`

## 阶段目标 / Phase Goal

把 Phlogo 单体里分散在 `internal/source`、`internal/node`、`internal/checker`、`internal/scoring`
四处的生命周期与评分逻辑，重写为 Rust Core 的 `crates/registry` + `crates/lifecycle` + `crates/scoring`
中的明确状态机与可解释评分引擎：

1. **状态转换**：定义 `SourceStatus`(Active/Suspended/Dead)、`FetchPolicy`(Replace/Accumulate)、
   `NodeHealth`(Unknown/Alive/Degraded/Dead)、`NodeMembership`(Linked/Orphaned/Retired) 及合法转换；
   所有关联清理放在数据库事务内。
2. **按 tag 健康**：`node_status` 以 `node_id + checker_tag` 维度维护健康快照
   （替代 Phlogo 单一的 `nodes.alive bool`）；跨 tag 存活合并；不可达节点不删除来源关联。
3. **孤儿/退役清理**：引入 `Orphaned → Retired → 物理删除` 的保留窗口
   （Phlogo 当前是「孤儿即立即物理删除」，这是 net-new）。
4. **评分引擎**：忠实复现 `internal/scoring/scoring.go` 的 EWMA 基线，再迭代
   （加 `version` 字段、`components/explanations`、`insufficient_data` 样本门槛）。

## 子步骤清单 / Sub-steps

- [p4-01-state-transitions.md](p4-01-state-transitions.md) — 状态枚举 `SourceStatus` /
  `FetchPolicy` / `NodeHealth` / `NodeMembership` 与合法转换，关联清理事务化。
- [p4-02-health-orphan.md](p4-02-health-orphan.md) — 按 tag 的 `node_status` 快照、
  跨 tag 存活合并、孤儿/退役保留窗口、迟到结果保护。
- [p4-03-scoring-engine.md](p4-03-scoring-engine.md) — EWMA 评分引擎忠实移植 + 版本/分项。
  **本步骤是整个 P4 的关键路径**：其余两步都依赖评分输出的结构。

## 阶段级验收 / Phase Acceptance

下列三条是 P4 的核心验收（plan §14 P4、plan §15 核心验收测试），需有可机器校验的测试覆盖：

1. **不同 tag 独立（different tags independent）**
   - `node_status` 与 `node_scores` 以 `node_id + checker_tag` 为维度；对 tag `A` 的写入/重算
     不得改变 tag `B` 的健康或评分快照。
   - 验收命令（示例）：
     ```bash
     cargo test -p lifecycle -p scoring --test per_tag_independence
     # 期望退出码 0，且用例证明：tag B 的 score/health 在 tag A 更新后不变
     ```

2. **不可达节点不删除来源关联（an unreachable node does NOT delete its source mapping）**
   - 某节点被某 tag 判为 Dead（延迟 `-1`）时，`node_sources` 中该节点与其它来源（或同一来源）的映射
     **保持不变**；孤儿清理只发生在「无任何来源引用」之后（见 p4-02）。
   - 验收命令（示例）：
     ```bash
     cargo test -p lifecycle unreachable_node_keeps_source_mapping
     # 期望退出码 0，且断言 node_sources 行数不变、节点未被物理删除
     ```

3. **迟到结果不覆盖新状态（a late check result does NOT overwrite a newer state snapshot）**
   - 绑定 `check_run_id + checked_at`；当一条旧的 `checked_at` 晚到时，状态重算以「窗口内最新
     `checked_at`」为准，不能回退已写入的较新 `node_status` 快照（见 p4-02 §2.7 / §6.4）。
   - 验收命令（示例）：
     ```bash
     cargo test -p checker-miaospeed --test late_result_no_overwrite
     # 期望退出码 0，且断言旧 check_run 提交后 node_status.last_checked_at 仍为较新值
     ```

### 边界一致性（补充，源自 plan §15）

- 删除一个来源不影响其它来源引用的节点（`removeSourceMappings` 带 `NOT EXISTS` 原子守卫，见 p4-02）。
- 所有来源关系消失后节点进入 `Orphaned`，而非立即删除（net-new，见 p4-02）。
- 订阅输出不得包含 `Retired` 节点（由 P5 订阅生成消费 `NodeMembership`，此处只保证状态正确）。
- `Job` 重试幂等：不重复创建来源关联、不重复触发删除（见 p4-01 事务 + 条件 UPDATE）。

## 与 Phlogo 现状对照 / Mapping from current Phlogo

| 概念 | Phlogo 现状 | Rust 重构（net-new 标记） |
|---|---|---|
| Source 状态 | `sources.status` 仅 `active`/`dead`；`ignore_dead` 标志（`internal/source/service.go`） | + `Suspended`（net-new）；`FetchPolicy`(Replace/Accumulate)（net-new，Phlogo 无区分） |
| Node 健康 | `nodes.alive bool` 单字段，无 tag、无 Degraded | `node_status`(node_id+checker_tag) + 4 态枚举（net-new） |
| Node 归属 | 孤儿节点在 `removeSourceMappings` 里**立即**物理删除（`internal/node/upsert.go`） | `Orphaned → Retired → 物理删除` 保留窗口（net-new） |
| 评分 | `internal/scoring/scoring.go` 单函数 EWMA，返回 `float64` 直写 | 同公式 + `version`/`components`/`insufficient_data`（net-new） |

> 所有上面标 **net-new** 的项，在 Phlogo 当前代码中**不存在**，是 P4 的重点新增；
> 切勿假定 Phlogo 已有对应实现，移植时按需新建表/枚举/事务。

## CI 提示 / CI notes

- 构建：`cargo build --workspace --all-targets`
- 单测：`cargo test -p lifecycle -p registry -p scoring -p checker-miaospeed`
- 集成（需 Postgres + MiaoSpeed stub）：`cargo test --test integration lifecycle_scoring`
- 静态：`cargo clippy --workspace --all-targets -- -D warnings`
- 迁移校验：`sqlx migrate run --check`（新增 `node_status` / `node_scores` / 状态列时必须通过）

## 风险 / Risks

- **评分 speed 分支与文档意图不一致**：`internal/scoring/scoring.go` 中 `speed = records[0].SpeedKbps`
  且 `if speed >= 0` 恒成立（缺速度时 `buildCheckRecords` 把 `speedKbps` 默认填 `0`），
  因此 Go 基线**始终**混入 speed 权重（缺速度时 `scoreSpeed=0`，实际 = `qualityScore * 0.45`）。
  而 `docs/scoring.md` 与 `CONVENTIONS.md §2.5` 的意图是「缺速度则不参与」。复现基线须忠实代码；
  `insufficient_data` / 真正「无速度不混合」属于迭代决策（见 p4-03）。
- **抖动阈值默认值冲突**：`scoring.go` 的 `loadParams` 硬编码 `jitterThresholdMs: 70` 作为
  配置行缺失时的兜底，但 `CONVENTIONS.md §2.5`、§2.6、以及 `docs/scoring.md` 均把
  `score_jitter_linear_threshold_ms` 的**文档默认值**写作 `200ms`。P4 须先定一个权威值并写进
  `DATA_MODEL.md` / migration seed，避免 Rust 端口与 Go 行为漂移。
- **`ORDER BY checked_at DESC` 缺 tie-break**：`scoring.go` 取最近 20 条仅 `ORDER BY checked_at DESC`，
  同一毫秒多条记录时顺序不确定 → EWMA 权重分配不确定。Rust 端口建议加 `, id DESC` 保证确定性。
- **`Degraded` 派生规则未定**：plan §6.2 列出 `NodeHealth::Degraded` 但 sources 中未给计算口径，
  需在 `LIFECYCLE.md`（plan §18）冻结前补齐，否则 p4-02 无法验收。
