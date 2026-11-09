# P3 · MiaoSpeed 测活闭环 · MiaoSpeed Check Loop

> 本阶段属于 `takomitkm/Phlogo`（=`openpaily/paily-core` fork）向
> `paily-core-rust-rebuild-plan.md` 描述的 Rust 多进程架构重构的 **P3 阶段**（方案 §14 / §15）。
> 页面模板遵循 [`CONVENTIONS.md`](../../docs/rebuild/CONVENTIONS.md) §1；事实口径以 §2.3 / §2.7 / §2.9 / §2.11 为准。
> **本期只写文档，不写 CI、不修改 Phlogo Go 源码、不落地任何 workflow YAML。**

---

## 阶段目标 / Phase Goal

用一条 **Rust WebSocket Adapter** 收尾 MiaoSpeed 测活闭环，**替代** Phlogo 当前由外部 `paily-check`
通过 HTTP 回调 `POST /api/v1/check/results` 的测活链路（见 `internal/checker/handler.go`、`CONVENTIONS.md` §2.7 / §4 差异 #2）。

具体目标：

1. 由 `core-worker` 内的 **Apalis Check Job** 触发并编排一次测活批次（取代 Phlogo 被动等待外部回调）。
2. 通过 `checker-miaospeed` crate 实现方案 §8 的 `Checker` trait（`submit` / `collect` / `status`），
   经 WebSocket 与独立 MiaoSpeed 二进制通信。
3. 把 MiaoSpeed 返回的结果做**单位规范化**并**精准映射回内部 `node_id`**。
4. 将测活历史持久化到 `check_runs` / `node_check_results` / `node_status`（方案 §5.1），并支持**幂等重试**。

设计要点：Apalis Job 状态与 Node Lifecycle 状态**严格分开**（方案 §3.1）；迟到结果**不得覆盖**更新的状态快照
（`CONVENTIONS.md` §2.7、§2.9；方案 §6.4）；**跨 tag 合并**——任一 checker 近期报告存活即视为 alive（`internal/checker/handler.go` `batchAliveDecision`）。

---

## 子步骤清单 / Sub-steps

- [p3-01 Apalis Check Job](p3-01-apalis-check-job.md) — 测活任务的入队、Worker 领取、重试/超时/恢复；Job 状态与节点生命周期解耦。
- [p3-02 MiaoSpeed WebSocket Adapter](p3-02-ws-adapter.md) — 实现 `Checker` trait：握手、auth 签名/挑战、vendor+node payload、单/批量提交；对接 MiaoSpeed stub。
- [p3-03 Result Mapping & Node ID 映射](p3-03-result-mapping.md) — 结果解析、单位规范、映射回 `node_id`；partial/timeout/disconnect/duplicate 处理；跨 tag 存活合并。
- [p3-04 History Persistence](p3-04-history-persistence.md) — 持久化 `check_runs` / `node_check_results` / `node_status` 与按 tag 状态；幂等重试。

---

## 阶段级验收 / Phase Acceptance

以下为本阶段整体验收（各子步骤另有细化验收，见对应页面）：

1. **批量结果精准映射到内部 `node_id`**：一次批量测活返回的每一条结果都能准确对应到提交时的内部 `node_id`，
   无错位、无遗漏（`core-worker` 测试可断言 `node_check_results.node_id` 与提交集合一一对应）。
   - 命令种子：`cargo test --test integration check_loop_maps_node_ids`
   - 预期：测试通过（exit 0），且 `SELECT COUNT(*) FROM node_check_results WHERE check_run_id = $run` == 提交节点数（成功+失败计全）。

2. **断连与重试不造成重复或错误覆盖**：Worker 崩溃/重连后重跑同一 `run_id`，
   `check_runs` / `node_check_results` 行数保持稳定（幂等），且**不会用迟到结果覆盖更新的 `node_status` 快照**。
   - 命令种子：`cargo test --test integration check_loop_idempotent_on_retry`
   - 预期：同 `run_id` 二次处理前后行数不变；较旧的 `checked_at` 结果被丢弃（不覆盖 `last_checked_at` 更新的行）。

3. **跨 tag 存活合并正确**：某 `node_id` 在 `node_alive_check_window_minutes`（默认 240）窗口内只要任一 `checker_tag` 报告 alive，
   其按 tag 快照与跨 tag 聚合都标记为 alive。
   - 命令种子：`cargo test -p checker-miaospeed cross_tag_alive_merge`
   - 预期：部分 tag dead、部分 tag alive 时，节点最终 alive = true。

4. **CI 种子全部可跑**：`cargo build --workspace --all-targets`、`cargo test --workspace`、`cargo clippy --workspace --all-targets -- -D warnings`
   退出码均为 0（见各页 CI 提示）。

---

## 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo（现状） | P3 Rust 对应 | 说明 |
|---|---|---|
| `internal/checker/handler.go` `PostResults`：被动接收 `POST /api/v1/check/results` 异步 goroutine（30min 超时）+ per-tag mutex | `apps/worker` 的 Apalis Check Job + `crates/checker-miaospeed` Adapter | 由被动回调改为主动 WebSocket 提交/收集 |
| `internal/checker/handler.go` 四阶段：事务插入 → `batchAliveDecision` → 异步 `ComputeAndUpdateTagScore` → 死源评估 + `cache.TriggerRebuild` | p3-01（触发）/ p3-03（映射+存活判定）/ p3-04（持久化）/ 评分与缓存重建在 P4/P5 | 逻辑分段重写，跨 tag 存活合并规则保留 |
| `internal/checker/handler.go` `GetNodes`：pull 全量节点给 checker | p3-02：Adapter `submit` 主动把节点 payload 推给 MiaoSpeed | 方向反转（push 而非 pull），见 p3-02 |
| `internal/db/model/node.go` `CheckerRun` / `NodeInitialCheck` / `NodeDeepCheck` | `check_runs` / `node_check_results` / `node_status`（方案 §5.1） | 初筛+复筛合并为统一结果表，见 p3-04 映射 |

---

## 风险 / Risks

- **MiaoSpeed 协议未冻结**：握手、auth 挑战、vendor/node payload、结果帧字段以固定版本 MiaoSpeed 为准，
  必须先用 **P0-02 spike** 验证（方案 §3.1、§8.2）。本阶段所有具体帧格式均以「P0-02 验证后回填」标注，**不硬编码未经验证的 Matrix 名称**（方案 §8.1）。
- **Apalis 固定版本行为未冻结**：PostgreSQL backend、retry/timeout、任务恢复依赖 P0-03 spike（§3.1）。
  本阶段 Apalis 代码结构为预期形态，确切 API 名以 P0-03 为准。
- **进程边界非许可证豁免**：MiaoSpeed 为 AGPL（见 `CONVENTIONS.md` §2.11）。Adapter 只走 WebSocket，不链接其 Go 代码；
  AGPL 义务仍需单独履行，`cargo-deny` / SBOM 须覆盖（p3-04 风险）。
- **迟到结果顺序**：跨 tag 存活合并 + 快照不覆盖，需要 `checked_at` 比较与窗口查询协同，实现易错，必须有 p3-03 的 late-result 测试保护。

---

## 上游与参考 / References

- `paily-core-rust-rebuild-plan.md`：§2 架构流（Worker → Q → CA → MS）、§3 MiaoSpeed/Apalis 行、§5.1 表、§8 MiaoSpeed 接口边界、§14 P3、§15 验收。
- `internal/checker/handler.go`：`PostResults` 四阶段、`buildCheckRecords`、`batchAliveDecision`、`applyRegionUpdates`。
- `internal/checker/port.go`：`ScoringPort` / `CacheRebuildPort`（P4/P5 消费）。
- `internal/db/model/node.go`：`CheckerRun` / `NodeInitialCheck` / `NodeDeepCheck`。
- [`CONVENTIONS.md`](../../docs/rebuild/CONVENTIONS.md) §2.3 / §2.7 / §2.9 / §2.11 / §5。
