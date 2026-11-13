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

- `gnosis-api`（Rust + Axum）：管理 API、认证、查询。
- `gnosis-worker`（Rust + Tokio + Apalis）：抓取、解析、测活编排、评分、清理。
- `postgres`：统一数据层（SQLx）。
- `miaospeed`：独立 Go 二进制，通过 WebSocket 被 worker 调用（替代 paily-check）。
- `web`：Vben Admin 静态前端。

本目录把方案里的 **P0–P7 八个阶段**再拆成更细的子步骤，每个子步骤一页，含目标、范围、步骤、验收、
与 fork 源对照、CI 提示、风险。

---

## 2. 阶段地图（Phase map）

| 阶段 | 标题 | 目录 | 子步骤 |
|---|---|---|---|
| **P0** | 上游技术验证 | `01-p0-upstream-verification/` | vproxy · miaospeed · apalis · vben · licensing |
| **P1** | Gnosis 骨架与数据层 | `02-p1-gnosis-skeleton-data/` | workspace · api-skeleton · migrations · source-node-crud |
| **P2** | 抓取与解析闭环 | `03-p2-fetch-parse/` | fetch-ssrf · vproxy-adapter · fingerprint · reconciliation |
| **P3** | MiaoSpeed 测活闭环 | `04-p3-check-miaospeed/` | apalis-check-job · ws-adapter · result-mapping · history-persistence |
| **P4** | 生命周期与评分 | `05-p4-lifecycle-scoring/` | state-transitions · health-orphan · scoring-engine |
| **P5** | 筛选与订阅发布 | `06-p5-filter-subscription/` | filter-ast · quota-limit · subscription-output |
| **P6** | 管理前端 | `07-p6-admin-ui/` | vben-scaffold · pages |
| **P7** | 生产化 | `08-p7-productionization/` | compose-backup · observability · security-hardening |
| **收口** | 自补记录 + **缺口总账 / MUST-DO** | `09-self-completed-gaps.md` | 收口后自查补齐的缺口（fetch 闭环接线/调度/登录/触发）+ 推迟项 + 换构架评估输入；**§D 生产就绪只读审计**：上游「非直接生产可用」警告原文与含义、5 个必确认项逐条结论、**M1–M22 未完成清单（用户裁定全部要做）**、§D.5 生产级能力 5 项（按业务选出口/用户地理调度/SLA/故障恢复/审计合规）、部署入口一致性（README/compose/旧入口名）；**§D.6 计划对照**：M23–M27（评分历史/冻结规格/SBOM/演练/两条验收）|
| **计划对照** | 最初计划逐节核查（`paily-core-rust-rebuild-plan.md`） | `09-self-completed-gaps.md` §D.6 | §14 P0–P7 逐阶段判定（P3/P4/P5/P7 四个 ⚠️）；§15 核心验收 15/17 有测试但**零件级**、#15/#17 缺（M27）；§16 四条 ⚠️（SBOM/deny/ScanCode/凭据加密）；**§18 冻结规格 9/10 缺（M24）**；**P4 评分历史 ❌（M23）**；备份演练 ❌（M26）；新增 **M23–M27**，总账 **M1–M27 全部要做**，分两道门禁 |
| **评审** | 旧 Go 评审 × Rust 实现交叉审查 | `10-review-crosscheck.md` | 逐节对照（权限/DB/评分/契约/表达式/分发/工程化/架构）+ 真实缺口 + 已废弃建议 + P0 验收 |
| **等价性** | 上游等价性审计（B3 答复） | `11-equivalence-audit.md` | 26 条上游路径逐条判定（等价/架构取代/缺失）+ **配置键悬挂审计**（13 键无消费者、6 键值被静默忽略） |
| **协议覆盖** | 最新 mihomo / sing-box 协议支持度审计 | `12-protocol-coverage-audit.md` | 校机真实订阅样本（mihomo v1.19.32，156 节点）实测 keep_rate **69.2% → 100%**；解析层补齐上游 `nodeAuth` 全 11 类协议；sing-box 发射层从硬编码简版改为全保真转换 |
| **转换对比** | Sub-Store 对比 + sing-box v1.14.4 实测 | `13-substore-compare-and-singbox-test.md` | 我们用什么做转换（自研三段式，无外部依赖）；与 Sub-Store 的定位差异；**校机装 sing-box v1.14.4 实跑**：hysteria2 与 REALITY vless 经我们转换的配置**真实出网 204**；实测抓出并修复 REALITY 缺 uTLS 映射；**S54 缺口全补**：13 协议 × 双二进制校验（sing-box 12/12、mihomo 13/13 PASS） |
| **格式扩充** | 引入 Sub-Store 转换旁挂服务（决策 A 落地） | `14-substore-sidecar.md` | 校机环境实测（无 Docker/Bun/npm、有 Node 22 + pnpm）；spike 确定**输入必须是 Clash YAML**（URI 列表 0 节点）；62 KB `data:` URL 塞不进 GET → 自写 POST 薄壳；**真实 52 节点 E2E 11/11 格式通过**；commit + SHA-256 固化；含可照抄进 CI 的部署门禁与后续施工图 |

### 实施状态总览（2026-11-13 更新）

**P0–P7 全部子步骤已实现、验证并推送**（实现 + 各页「验证结果」实测记录 + ACTION-LOG S05–S47）。

> ⚠️ **但「阶段完成」≠「可部署生产」**：2026-11-13 只读审计发现测活/评分链路仍是桩、
> `/sub` 仍输出占位载荷（见 `09-self-completed-gaps.md` §D）。**M1–M27 全部要做**，分两道门禁：
> 可部署门禁（M1–M5 + M15）/ 生产级门禁（M6–M14 + M18–M27）。
> 上线总门禁是「抓取 → 真测活 → deep 落库 → 评分更新 → /sub 出真实节点 → mihomo -t 通过 → 真代理出网 204」。
> 关键决策与偏差均写入对应页面：

- **P0**：VProxy 最小化移植（不作依赖）；MiaoSpeed 自建 `4.7.9`（sha256 记录）；Apalis `0.7.4` PG backend；Vben `v5.7.0`。
- **P1**：workspace + Axum/SQLx/utoipa 骨架；迁移按 sqlx 文件约定（`.up/.down`）；`configs` seed 23 键；Source/Node CRUD + 真实 PG 集成测试。
- **P2**：SSRF 守卫（6 测试）；Clash/Singbox/Base64 解析（DB 层不依赖 adapter）；指纹 v1（canonical 含 transport/tls，14 golden）；Replace/Accumulate 对账 + 孤儿标记（6 场景）。
- **P3**：Apalis CheckJob（**独立 `phlogo_jobs` 库**——setup 内嵌 migration 与业务迁移冲突的解法）；MiaoSpeed WS Adapter（**golden 签名与 Go 逐字节一致**）；结果规范化（迟到不覆盖/跨 tag 合并）；历史持久化（幂等重跑、JSONB roundtrip）。
- **P4**：状态机 + 事务化清理（仅赢家清理）；cleanup cycle + 跨 tag 聚合；EWMA 评分（**golden 0.967722 更正规格手算误差**；speed 分支/jitter 阈值/tie-break 三决策定稿）。
- **P5**：类型化 Filter AST（白名单 + 参数化 SQL + 手写反序列化）；selector.go 逐函数移植（E-S A-Res 加权抽样，统计验收）；订阅输出（三格式 + **白名单命名模板**替代 expr + token 哈希）。
- **P6**：OpenAPI 契约（utoipa 实 paths）+ spec.json；Vben 页面（Sources/Nodes/Scoring + Dashboard/Checks/NodeDetail/Subscriptions/**Sponsors**）+ `build:ele` 绿（vben monorepo 不 vendor，构建用固定 checkout）。
- **P7**：compose 六服务拓扑 + 双 Dockerfile + 备份/恢复脚本；observability（JSON 日志 + redaction + `/metrics`）；`/sub/{token}` 订阅路由（哈希校验/撤销即 401）+ 限流层 + supply-chain 脚本。

**收口后批次（S34–S47）**：
- **S34–S37 自补**：fetch 闭环接线（FetchJob+run_fetch）、worker 双循环调度、登录端点、fetch/trigger、SSRF 测试逃生口、前端三页补齐。
- **S38–S39**：Go 源清除（对照基准 `dda4e6f2`）；旧评审交叉审查（`10-review-crosscheck.md`）；**P0 安全修复**——路由权限矩阵（fail-closed，nest 内路径归一化）+ 登录限流（IP 桶、XFF 不信任）。
- **S40–S41**：等价性审计（`11-equivalence-audit.md`，26 路径逐条判定）；**配置键修复**（6 键值被静默忽略→小数/百分数兼容解析；`grade_*` 阈值/`check_run_history_days` 14 天/`cleanup_interval_minutes` 接线）；B3 缺口 #1（搜索三端点）+ #3（format-configs）；migration 0010。
- **S42**：**A1 PROXY protocol v1**（自建 accept loop + hyper；Caddy `proxy_protocol v1`；api 不发布宿主端口；顺带修限流桶键真 bug——原读裸 SocketAddr 恒为 0.0.0.0）。
- **S43–S44**：**B4 本机 compose 全栈演练 ✅**（5 服务 Up、PROXY 链路 + 订阅全链路 + 权限矩阵实测；修 `type=content` 源被当 URL 的真 bug；`postgres-init` 建队列库）；搜索 **SQL 下推**（`to_search_sql` 聚合视图 + contains→ILIKE；删内存复检）。
- **S45–S47**：配置键收尾复查（12 生效/8 空心有标注）；**sponsor 域恢复**（0011 + registry 引擎上游语义对齐 + AdminOnly CRUD + 订阅注入 + Vben 页面，`build:ele` 11/11 绿）；领域文档全面更新（README 重写 + 六份 docs 去陈旧）。

**已知收口项**（后续迭代/CI，非阻塞）：`auth/refresh`（单管理员重新登录覆盖）；`node_payloads` 凭据加密（XChaCha20 方向已裁定）；`unlock` 流媒体筛选（待 `node_streaming` 表随 deep-check 接线）；sponsor 档案外的管理前端细节打磨；CI（用户裁定暂不写，各页「CI 提示」+ `11 §5` 复现命令为种子）。

实施顺序严格遵循：**P0 → P1 → … → P7 → 收口 → 评审修复 → 等价性收口**。
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
- 单体 → **gnosis-api + gnosis-worker** 多进程（fetch/check 收编为 worker Job）。

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
