# Phlogo → Rust 重构步骤文档总览（Rebuild Step Docs — Index）

> 本目录是 `takomitkm/Phlogo`（=`openpaily/paily-core` 的 fork，Go 单体）向
> `paily-core-rust-rebuild-plan.md` 描述的 Rust 多进程架构重构的**步骤拆解文档**。
> 所有页面以「可独立验收的大步骤的子步骤」为粒度，写得足够详细，以便后续**照着写 CI**。
> **本期只写文档，不写 CI。**

---

## 1. 这份文档解决什么问题（What this is for）

Phlogo 当前是一个 Go 单体（Gin + GORM + oapi-codegen + expr-lang），由 `cmd/server/main.go` 装配，
通过 HTTP 回调接收独立 `paily-fetch` / `paily-check` 服务的抓取与测活结果。

重构目标（见 `paily-core-rust-rebuild-plan.md`）是拆成：

- `core-api`（Rust + Axum）：管理 API、认证、查询。
- `core-worker`（Rust + Tokio + Apalis）：抓取、解析、测活编排、评分、清理。
- `postgres`：统一数据层（SQLx）。
- `miaospeed`：独立 Go 二进制，通过 WebSocket 被 worker 调用（替代 paily-check）。
- `web`：Vben Admin 静态前端。

本目录把方案里的 **P0–P7 八个阶段**再拆成更细的子步骤，每个子步骤一页，含目标、范围、步骤、验收、
与 Phlogo 现状对照、CI 提示、风险。

---

## 2. 阶段地图（Phase map）

| 阶段 | 标题 | 目录 | 子步骤 |
|---|---|---|---|
| **P0** | 上游技术验证 | `01-p0-upstream-verification/` | vproxy · miaospeed · apalis · vben · licensing |
| **P1** | Core 骨架与数据层 | `02-p1-core-skeleton-data/` | workspace · api-skeleton · migrations · source-node-crud |
| **P2** | 抓取与解析闭环 | `03-p2-fetch-parse/` | fetch-ssrf · vproxy-adapter · fingerprint · reconciliation |
| **P3** | MiaoSpeed 测活闭环 | `04-p3-check-miaospeed/` | apalis-check-job · ws-adapter · result-mapping · history-persistence |
| **P4** | 生命周期与评分 | `05-p4-lifecycle-scoring/` | state-transitions · health-orphan · scoring-engine |
| **P5** | 筛选与订阅发布 | `06-p5-filter-subscription/` | filter-ast · quota-limit · subscription-output |
| **P6** | 管理前端 | `07-p6-admin-ui/` | vben-scaffold · pages |
| **P7** | 生产化 | `08-p7-productionization/` | compose-backup · observability · security-hardening |

实施顺序严格遵循：**P0 → P1 → P2 → P3 → P4 → P5 → P6 → P7**。P0 未通过前不锁定 Cargo 依赖。
完整验收清单见方案 §15；各阶段验收见对应 `index.md`。

---

## 3. 与 Phlogo 现状的关系（Current state baseline）

重构的「从哪来」基准记录在 [`CONVENTIONS.md`](CONVENTIONS.md)（数据模型、评分公式、指纹、分发算法、
测活契约、配置默认值、API 面、许可证）。写/读任何子步骤前都应以该文件为准，保证术语与口径一致。

关键差异（务必在 P0 决策）：
- `paily-fetch/format` → **VProxy** Parser/Export（P0 验证轻依赖可行性）；
- 独立 `paily-check` HTTP 回调 → **MiaoSpeed WebSocket Adapter**（方案 §8）；
- GORM（含 SQLite）→ **SQLx + PostgreSQL only**；
- `expr-lang` 动态表达式 → **Typed Filter AST**（方案 §9.2）；
- `oapi-codegen` → **utoipa**；
- 单体 → **core-api + core-worker** 多进程（fetch/check 收编为 worker Job）。

---

## 4. 如何使用（How to use）

1. 先读 [`CONVENTIONS.md`](CONVENTIONS.md) 掌握事实来源与页面模板。
2. **全程维护 [`ACTION-LOG.md`](ACTION-LOG.md)**：总行为清单，每一步、每条命令都以 UTC 时间戳开头记录；重量级编译在校机执行（见该文件「执行环境」）。
3. 按 P0→P7 顺序逐阶段推进；每阶段的 `index.md` 给出子步骤链接与阶段级验收。
3. 每完成一个子步骤，按页面「验收 / Acceptance」逐条核对；这些命令就是未来 CI 的种子。
4. **CI 不在此目录落地**：每个页面的「CI 提示 / CI notes」只描述将来对应哪条命令，
   由你后续照此编写 workflow / pipeline（详见 `CONVENTIONS.md` §5）。

---

## 5. 与最终规格文件的关系（Frozen specs, plan §18）

方案 §18 要求的冻结规格文件（`SPEC.md`/`ARCHITECTURE.md`/`DATA_MODEL.md`/`NORMALIZED_NODE.md`/
`LIFECYCLE.md`/`MIAOSPEED_PROTOCOL.md`/`SUBSCRIPTION_SPEC.md`/`SECURITY.md`/`THIRD_PARTY.md`/`ADR/`）
应在 P0/P1 阶段产出，并作为本步骤文档的上游依据。本目录的步骤页面是**执行层**，规格文件是**定义层**；
两者通过 `CONVENTIONS.md` 衔接。
