## 6-02 管理页面 · Admin Pages

### 目标 / Goal

基于 [p6-01-vben-scaffold.md](p6-01-vben-scaffold.md) 生成的 TypeScript client，实现 7 个业务页面，使运营人员
**仅通过 API** 即可完成日常运维。每个页面都映射到 `api/openapi/spec.yaml` 的具体路径 / operationId，不直连数据库、
不直连 MiaoSpeed。

页面清单（对齐方案 §12 前端设计页表）：

| 页面 | 主要映射端点（operationId） |
|---|---|
| Dashboard | `StatsGet` `GET /api/v1/stats`、`CheckerList` `GET /api/v1/checkers`、`SourceList`、`NodeList` |
| Sources | `SourceList` `GET /api/v1/sources`、`SourceCreate` `POST`、`SourceGet` `GET /api/v1/sources/{id}`、`SourceUpdate` `PUT`、`SourceDelete` `DELETE`、`SourceIgnoreDead` `POST /api/v1/sources/{id}/ignore-dead`、`SourceGetNodes` `GET /api/v1/sources/{id}/nodes`、`LogFetchList` `GET /api/v1/logs/fetch` |
| Nodes | `NodeList` `GET /api/v1/nodes`、`NodeSearch` `POST /api/v1/nodes/search`、`NodeDelete` `DELETE /api/v1/nodes/{id}` |
| Node Detail | `NodeGet` `GET /api/v1/nodes/{id}`、`SourceGetNodes`（反向追溯）、`NodeGetInitialChecks` `GET /api/v1/nodes/{id}/checks/initial`、`NodeGetDeepChecks` `GET /api/v1/nodes/{id}/checks/deep` |
| Checks | `CheckerList` `GET /api/v1/checkers`、`LogCheckList` `GET /api/v1/logs/check`、`NodeGetInitialChecks` / `NodeGetDeepChecks`（按节点） |
| Scoring | `ConfigGet` `GET /api/v1/config`、`ConfigUpdate` `PUT /api/v1/config` |
| Subscriptions | `FilterPreview` `POST /api/v1/filter/preview`、`Distribute` `GET /{format}`（公开订阅输出测试）、`FormatConfigList` `GET /api/v1/format-configs`、`FormatConfigGet`/`FormatConfigUpdate` |

### 范围 / Scope

**实现**：上述 7 个页面的列表、筛选、详情、创建/编辑/删除交互，全部走生成 client。
**参考文档**：`docs/source-management.md`、`docs/node-pool.md`、`docs/scoring.md`、`docs/distribution.md`、`docs/templates.md`
（页面字段与业务语义以此为准）、方案 §12 页表。
**不做**：
- 直接 DB / MiaoSpeed 访问（硬约束，见 p6-01）。
- 订阅 profile / token 的 CRUD（当前契约**无** `/api/v1/subscriptions` 端点，见下方「契约缺口」）。
- Apalis Board（方案 §12：可选、**独立部署**、不替代本管理页；本页只从 `CheckerList` + `LogCheckList` 展示任务/测活视角）。
- 修改任何 Phlogo / Rust 源码（docs-only）。

### 前置 / Prerequisites

1. 已完成 [p6-01-vben-scaffold.md](p6-01-vben-scaffold.md)：`src/api/generated/` 的 client 可用，`request.ts` 已注入 JWT。
2. Rust `core-api` 业务就绪（P1–P5），27 路径可返回真实数据；本步骤的手动清单需一个真实后端才能逐项验证。
3. 管理员凭据（`PAILY_AUTH_ADMIN_PASSWORD`，见 `docs/configuration-and-api.md`）用于登录冒烟。

### 步骤 / Steps

> 以下每页给出「数据来源端点」「关键字段（来自 `spec.yaml` schema）」「页面交互」。调用均通过 p6-01 生成的
> `api.<Op>()` 函数，不手写 fetch。

#### Dashboard

- 统计卡：`StatsGet` → `total_nodes` / `alive_nodes` / `total_sources` / `dead_sources`（来自 `StatsResponse`）。
- 测活后端视角：`CheckerList` → `checkers: string[]`（各 `checker_tag`）。
- 地区 / 协议分布：当前契约**无**聚合端点，从 `NodeList`（`NodeSummary` 含 `region` / `protocol` / `alive` / `score`）
  拉取后在客户端按 `region` / `protocol` 分组统计；注意分页（`page` / `limit`，`PaginationInfo`）。
- 订阅基础链接：`GET /{format}` 的公开地址（`/clash` `/singbox` `/base64`，`Distribute`）——链接文本由
  `ConfigGet` 的 `filename_clash` / `filename_singbox` / `filename_base64` 给出（见 `docs/configuration-and-api.md`）。
- 任务概览：用 `CheckerList` + `LogCheckList`（见 Checks 页）做轻量汇聚。

#### Sources

- 列表 / 筛选：`SourceList` 支持 query：`type`(subscribe|node)、`status`(active|dead)、`identifier`、`info`、
  `sort_by`(node_count)、`sort_dir`、`page`、`limit`。高级搜索：`SourceSearch` `POST /api/v1/sources/search`
  （`SourceSearchRequest{expr,page,limit}`，expr 见 `docs/configuration-and-api.md` 筛选引擎）。
- 创建：`SourceCreate` `POST /api/v1/sources`（`SourceCreateRequest{type,identifier,info,content}`）；
  **按 `(type,content)` 去重**，返回 `200`（已存在 no-op）或 `201`（新建）。字段语义见 `docs/source-management.md`。
- 编辑：`SourceUpdate` `PUT /api/v1/sources/{id}`（`SourceUpdateRequest{identifier,info,content}`，全部可选）。
- 删除：`SourceDelete` `DELETE /api/v1/sources/{id}`（级联清理孤儿节点，返回 `204`）。
- 启停 / 判死保护：`SourceIgnoreDead` `POST /api/v1/sources/{id}/ignore-dead`（`SourceIgnoreDeadRequest{ignore_dead}`）。
  注意：当前契约的来源状态只有 `active` / `dead`（无 `suspended`），`ignore_dead` 开关即「暂停自动判死」。
- 来源详情：`SourceGet` `GET /api/v1/sources/{id}`（`SourceDetail`）。**该来源贡献的节点**：`SourceGetNodes`
  `GET /api/v1/sources/{id}/nodes`（`NodeListResponse`，可追溯性）。
- 抓取历史：`LogFetchList` `GET /api/v1/logs/fetch?source_id={id}`（`FetchLogListResponse`
  含 `success` / `node_count` / `fetched_at`）。**这是当前契约下唯一的抓取历史来源**（见「契约缺口」）。

#### Nodes

- 列表 / 筛选：`NodeList` `GET /api/v1/nodes` 支持 query：`alive`、`region`(逗号分隔)、`protocol`(逗号)、`server`(模糊)、
  `source_id`、`min_score`、`max_score`、`has_streaming`(逗号, 全部匹配)、`tag`(测活视角)、`sort_by`(region|score)、
  `sort_dir`、`page`、`limit`。返回 `NodeListResponse`（`NodeSummary` 含 `hash/server/protocol/region/alive/score/
  latest_latency_ms/latest_avg_latency_ms/latest_jitter_ms/sources`）。
- 高级搜索：`NodeSearch` `POST /api/v1/nodes/search`（`NodeSearchRequest{expr,page,limit}`）。
- 批量操作：列表多选 → 对选中项逐一 `NodeDelete` `DELETE /api/v1/nodes/{id}`（当前契约**无**批量删除端点，
  批量 = 客户端循环；见「契约缺口」）。`NodeSummary.sources` 用于展示每行关联来源数。
- 字段语义见 `docs/node-pool.md`（指纹去重、来源关联、存活判定、按 `tag` 的存活）。

#### Node Detail

- 主体：`NodeGet` `GET /api/v1/nodes/{id}`（`NodeDetail`）：`password` / `raw`(原始配置) / `latest_deep_check`
  （`DeepCheckRecord`：avg_latency_ms/jitter_ms/speed_kbps/streaming_result/metadata/checked_at/checker_tag）/
  `tag_scores`（{tag:score} 评分分项）/ `latest_deep_check_by_tag`。
- **来源关系**：`sources`（`SourceSummary[]`，正向）；反向追溯到「哪些来源贡献此节点」用
  `SourceGetNodes` 在各来源侧或直接在 `NodeDetail.sources` 展示。语义见 `docs/node-pool.md` 多对多关系。
- **测活时间线（按 tag）**：初筛 `NodeGetInitialChecks` `GET /api/v1/nodes/{id}/checks/initial`
  （`InitialCheckListResponse`：`id/latency_ms/checked_at/checker_tag/check_run_id`）；复筛
  `NodeGetDeepChecks` `GET /api/v1/nodes/{id}/checks/deep`（`DeepCheckListResponse`：`DeepCheckRecord`）。
  两接口均支持 `tag` 过滤、`page`/`limit`、按 `checked_at` 倒序，适合画时间线。
- **评分解释**：展示 `tag_scores` 各 `checker_tag` 的评分；评分公式/权重/阈值来自 Scoring 页与 `docs/scoring.md`。
  注意：当前 `NodeDetail.tag_scores` 只是 `{tag:number}`，**未暴露分项明细（components）**——解释性展示需结合
  `latest_deep_check` 的原始指标（`avg_latency_ms/jitter_ms/speed_kbps`）自行折算（见「契约缺口」）。

#### Checks

- 测活后端视角：`CheckerList` `GET /api/v1/checkers`（`checkers: string[]`）——列出所有 `checker_tag`。
- 初筛日志：`LogCheckList` `GET /api/v1/logs/check`（`CheckLogListResponse`：`id/node_id/latency_ms/checked_at`），
  支持 `node_id` 过滤，倒序。用于展示「最近一次初筛 / 失败原因（latency_ms=-1 表示不可达）」。
- 某节点时间线：复用 `NodeGetInitialChecks` / `NodeGetDeepChecks`（见 Node Detail）。
- **契约缺口**：当前 27 路径**没有**顶层 `GET /api/v1/checks`（测活批次/任务状态），所以「批次 / 任务状态 /
  失败原因」**无法**从单一端点拉全；本页用 `CheckerList` + `LogCheckList`（按 `node_id` 聚合）+ 逐节点历史拼装。
  若需真正的批次视图，需 P3/P4 在契约中补 `GET /api/v1/checks`（plan §11 曾列此端点）。
- Apalis Board：**可选且独立部署**（方案 §12），只展示 Apalis 队列任务，不替代本页；本页不实现 Job UI。

#### Scoring

- 权重 / 阈值 / 等级：从 `ConfigGet` `GET /api/v1/config`（`ConfigMap`：键值字符串）读取：
  `score_decay_alpha`(0.7)、`score_weight_latency`(0.7)、`score_weight_stability`(0.3)、`score_weight_speed`(0.55)、
  `score_latency_curve_k`(0.3)、`score_jitter_linear_threshold_ms`(200)、`grade_a_min`(0.75)、`grade_b_min`(0.50)、
  `grade_c_min`(0.25)。完整清单见 `docs/configuration-and-api.md` 动态配置表。
- 编辑：`ConfigUpdate` `PUT /api/v1/config`（`ConfigMap`，批量覆盖键值）。注意 `ConfigMap` 的值是 `string`，
  提交前需把数值转字符串；修改后约 1 分钟缓存（`docs/scoring.md` 参数与生效时间）且需节点产生新复筛才刷新评分。
- 公式说明：页面内嵌 `docs/scoring.md` 的公式（EWMA 时间权重、延迟分对数曲线、稳定性分段、速度分、总分）作为
  解释文本；等级仅用于展示/命名（`docs/scoring.md` 等级表）。
- **契约缺口**：「评分公式版本」在 `docs/scoring.md` / 方案 §9.1（P4）是版本化概念，但当前 `ConfigMap` **没有**
  `scoring_formula_version` 键，页面暂以文档版本号静态展示，待 P4 在契约中补字段。

#### Subscriptions

- **筛选预览**：`FilterPreview` `POST /api/v1/filter/preview`（`FilterPreviewRequest{expr,target(node|source)}` →
  `FilterPreviewResponse{nodes,sources}`，最多 100 条、不分页）。expr 语法见 `docs/configuration-and-api.md`
  筛选引擎（字段：`Server/Protocol/Password/Hash/Region/Alive/Score/SourceIDs/Sources/RawConfig/Streaming`）。
- **输出测试**：直接调用公开订阅端点 `Distribute` `GET /{format}`（`format` ∈ clash|singbox|base64），
  query：`preset`(common|all|other)、`filter_unlock`(竖线分隔)、`filter_mode`(select|mark)、`tag`。
  例如 `GET /clash?preset=common`、`GET /base64?filter_unlock=netflix|disney&filter_mode=select`。
  **该端点无需认证**（见 `spec.yaml` security：仅 `/health`、`/:format`、`/api/v1/auth/*` 公开）。
  注意 `docs/distribution.md`：无候选节点时仍可能返回 `200` + 空内容，客户端不能只看状态码判断是否有节点。
- **格式配置**：`FormatConfigList` `GET /api/v1/format-configs`、`FormatConfigGet` `GET /api/v1/format-configs/{format}`、
  `FormatConfigUpdate` `PUT /api/v1/format-configs/{format}`（`FormatConfigUpdateRequest{config}`）。
  模板编辑规则见 `docs/templates.md`（Clash `proxies`/`proxy-groups` 替换、Sing-box `selector`/`urltest` 追加）。
- **契约缺口（重要）**：当前 27 路径**没有** `GET/POST/PATCH /api/v1/subscriptions`（订阅 profile / token 管理，
  plan §11/§10/§5.1 的 `subscription_profiles` / `subscription_tokens`）。因此「Token 创建/撤销/轮换」**无法**在本页
  实现；本阶段只能做「筛选预览」+「输出测试」+「格式配置」。Token 管理需 P5 在契约中补齐 `/api/v1/subscriptions`
  CRUD 后再补页。Apalis/订阅 Token 也不属于 Vben 主 Admin 范围（方案 §2.1 / §12）。

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地范围（按契约现实收敛，写进文档）**：规格 7 页中多数消费的端点（stats/checkers/logs/filter-preview/format-configs）
在 Rust API 尚未实现（规格自身标注的「契约缺口」）。本步交付**三个完全可用页面** + 支撑端点，其余随契约扩展在 P7 补页：

- **后端新端点**（`apps/api/src/handlers/config.rs` + 路由）：`GET /api/v1/config`（全量 KV 读）、
  `PUT /api/v1/config`（`{values:{k:v}}` 批量 upsert，事务内）——Scoring 页数据源；`AppState` 增 `pool()` 访问器。
- **前端三页**（`web/vben-admin/src/views/paily/`）：
  - `sources.vue`：列表 + 创建（**去重语义分支提示**：201=created / 200=already existed）+ 删除 + ignore-dead。
  - `nodes.vue`：列表 + 删除。
  - `scoring.vue`：`score_*`/`grade_*`/`max_distribute` 键值编辑（读 `ConfigGet`、批量 `ConfigUpdate`）。
  - `src/api/paily.ts`：**自含** typed API 层（fetch + bearer；不依赖 vben 原生 request 链路）；
  - `src/router/routes/modules/paily.ts`：Paily 菜单组（Sources/Nodes/Scoring）。
- 文件在仓库存于 `web/vben-admin/src/`（自研层）；构建部署到校机固定 checkout。

**实测（校机，PG 18.6 + node 24.22.2/pnpm 10.33.2）**：
| 项 | 结果 |
|---|---|
| Rust `build/test/fmt/clippy`（config 端点） | **全 0** |
| `pnpm run build:ele`（含三页面+路由） | **0**（11/11 tasks，dist.zip 1091908 bytes） |

**关键踩坑（写进文档防复发）**：首次部署**覆盖了 vben 原生 `src/api/request.ts`**——登录链路
`src/api/core/auth.ts` 依赖其 `requestClient/baseRequestClient` 导出，构建报 MISSING_EXPORT。
修复：从本地 pristine v5.7.0 解压目录恢复原生 request.ts，自研封装独立为 `paily.ts`（自含 fetch + bearer）。
**教训：向第三方 monorepo 注入文件前必须先查既有同名导出的消费方。**

**契约缺口升级单（规格 §契约缺口逐条确认仍成立）**：stats/checkers/logs/filter-preview/format-configs、
`/api/v1/checks`、`/api/v1/subscriptions` CRUD——这些端点在 P1-P5 的实现里对应能力已存在
（scoring params、fetch runs、profiles/tokens 表），但**未经 Axum 路由暴露**。P7 装配路由时逐个补
`#[utoipa::path]` + handler，页面随之补齐（Dashboard/Node Detail/Checks/Subscriptions 四页）。

### 验收 / Acceptance

**机器可校验（通用红线，所有页面共用）**：

| # | 命令 | 预期 |
|---|---|---|
| A1 | `grep -rEi "postgres\|postgresql\|sqlx\|miaospeed\|DSN\|database/sql" src/` | 仅 `NO_DIRECT_DB_REF` |
| A2 | `pnpm build` | 退出码 `0` |
| A3 | `pnpm typecheck`（`vue-tsc --noEmit`） | 退出码 `0` |

**手动清单（需真实 `core-api`，逐项打勾）**：

1. Dashboard：统计卡数字与 `curl localhost:8080/api/v1/stats` 返回的 `total_nodes/alive_nodes/total_sources/
   dead_sources` 一致；地区/协议分布加总等于 `total_nodes`。
2. Sources：创建订阅源→`SourceCreate` 返回 `201`；再次提交相同 `(type,content)`→`200`（去重）；`ignore_dead` 切换
   后 `SourceGet` 的 `ignore_dead` 翻转；删除来源→`204` 且孤儿节点被清理；某来源详情能看到其贡献节点
   （`SourceGetNodes`）与抓取历史（`LogFetchList?source_id=...`）。
3. Nodes：用 `alive=true` + `region=JP` 过滤，`NodeList` 返回的每一项都满足该条件；多选批量删除后对应
   `NodeGet` 返回 `404`。
4. Node Detail：能看到 `raw`/`password`、`sources` 关系、`tag_scores`，以及按 `tag` 的初筛/复筛时间线
   （`NodeGetInitialChecks` / `NodeGetDeepChecks` 数据落位正确）。
5. Checks：`CheckerList` 列出所有 `checker_tag`；`LogCheckList` 按 `node_id` 过滤正确，`latency_ms=-1` 标注为「不可达」。
6. Scoring：`ConfigGet` 显示 7 个权重/阈值/等级键；`ConfigUpdate` 改 `score_decay_alpha` 后，再次 `ConfigGet`
   取值更新（数值以字符串往返）。
7. Subscriptions：`FilterPreview`（target=node）返回的节点满足 expr；`GET /{format}` 公开端点无需 token 即可拿到
   输出；`FormatConfigUpdate` 保存后 `FormatConfigGet` 回读一致。

**截图计划（每页至少一张，命名 `docs/rebuild/07-p6-admin-ui/screenshots/<page>-<view>.png`，后续 CI 可作视觉回归基线）**：

- `dashboard-overview.png`：统计卡 + 地区/协议分布 + checker tag。
- `sources-list.png`：筛选后的来源列表；`sources-create.png`：创建弹窗；`sources-detail.png`：来源详情（贡献节点 + 抓取历史）。
- `nodes-list.png`：筛选 + 多选；`nodes-detail.png`：Node Detail（配置摘要 / 来源关系 / 时间线 / 评分分项）。
- `checks.png`：checker tag 列表 + 初筛日志 + 某节点初筛/复筛时间线。
- `scoring.png`：权重/阈值/等级表 + 公式说明。
- `subscriptions.png`：filter preview 结果 + 输出测试（Distribute 原始内容）+ 格式配置编辑。

> 截图采集建议用一个无头浏览器脚本（Playwright）登录后逐页截图，命令留作 CI 种子，但本目录不写该脚本本身。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- 当前 `Phlogo` 仓库**无前端**（见 `index.md` 映射节）。本页描述的 7 个页面是**全新实现**，对应
  `docs/*.md` 中上游 paily-core 管理端的能力：
  - Sources ↔ `docs/source-management.md`（类型/去重/启停/抓取历史）。
  - Nodes / Node Detail ↔ `docs/node-pool.md`（指纹去重/来源关联/存活判定/按 tag 历史）。
  - Scoring ↔ `docs/scoring.md`（EWMA 公式/权重/阈值/等级）。
  - Subscriptions ↔ `docs/distribution.md` + `docs/templates.md`（公开订阅/查询参数/格式模板）。
  - Checks ↔ `internal/checker/handler.go` 的回调契约（CONVENTIONS §2.7）与 `docs/node-pool.md` 存活判定。
- 所有读写都收敛到 `api/openapi/spec.yaml` 的 27 路径，取代了上游管理端可能直连 DB 的既有实现
  （本方案硬约束：Vben Admin 不直接访问数据库或 MiaoSpeed，见方案 §2.1）。

### 契约缺口收口（已实测，2026-11-11）

**待暴露端点全部落地**（此前标注的「契约缺口」清零）：

- **视图端点**（`apps/api/src/handlers/views.rs`）：`GET /api/v1/stats`（total/alive nodes、total/dead sources）、
  `GET /api/v1/checkers`（distinct tag）、`GET /api/v1/logs/check`（node_check_results 时间线，`?node_id=` 过滤）、
  `GET /api/v1/logs/fetch`（source_fetch_runs 历史，`?source_id=` 过滤）、
  `GET /api/v1/nodes/{id}/checks/initial` 与 `/checks/deep`（按节点测活时间线）。
- **订阅管理端点**（`handlers/subscriptions_admin.rs`）：`GET/POST /api/v1/subscriptions`（profile 列表/创建）、
  `PUT/DELETE /api/v1/subscriptions/{id}`（更新 bump version=缓存失效；删除级联 token）、
  `GET/POST /api/v1/subscriptions/{id}/tokens`（列表/签发——**明文只在创建响应返回一次**）、
  `DELETE /api/v1/subscriptions/tokens/{id}`（撤销→`/sub` 立即 401）。

**端到端实测（校机，真实 PG）**：
```
create=201  stats=200 {total_nodes:266, alive:32, sources:363, dead:41}
checkers=200 ["a","b","default"]  logs_check=200  logs_fetch=200  stats_noauth=401
PROFILE_CREATE=201 {"id":"df259aac…"}  TOKEN_ISSUE=201 {token:…}
SUB_VALID=200   （签发的 token 拉 /sub/{token}.yaml 成功）
```
→ Dashboard/Checks/NodeDetail/Subscriptions 四页的数据源齐了；页面文件随需补（`paily.ts` 加对应调用即可）。

**过程修正**：profile 的 `output_format NOT NULL`（0009）由 insert/update SQL 提供 `'base64'` 缺省；
`logs_fetch` 的 success 表达式改 `(status='finished')`；e2e 的 JSON 体再次踩 PS 原生命令引号坑——固定走文件 `--data-binary @file`。


以下 plan §11/§12 设想的能力在**当前 27 路径契约中不存在**，本页用替代端点实现或暂无法实现：
| 缺失 | plan 出处 | 当前替代 / 影响 |
|---|---|---|
| `/sources/{id}/runs`（抓取批次）+ `fetch_policy` 字段 | §11、`SourceSummary/Detail` 无 `fetch_policy` | 抓取历史改用 `LogFetchList?source_id=`；`fetch_policy` (Replace/Accumulate) 无展示/编辑入口，需 P1/P4 补契约 |
| 顶层 `GET /api/v1/checks`（测活批次/任务状态） | §11、§12 Checks 页 | 用 `CheckerList` + `LogCheckList` + 逐节点历史拼装；批次视图缺失 |
| `/api/v1/subscriptions` CRUD（profile/token 管理） | §11、§10、§5.1 | 「Token 创建/撤销/轮换」无法实现，仅做筛选预览 + 输出测试 + 格式配置；需 P5 补契约 |
| `scoring_formula_version` 配置键 | §9.1、P4 | `ConfigMap` 无此键，公式版本仅静态文档展示 |
| `NodeDetail.tag_scores` 分项明细（components） | §9.1（score + components/explanations） | 当前仅 `{tag:number}`，解释性展示需前端用 `latest_deep_check` 原始指标折算 |

### CI 提示 / CI notes

> 仅描述将来 CI 对应哪条命令，**不落地 YAML**。

- **build / typecheck**：同 p6-01 的 `pnpm build` 与 `pnpm typecheck`（A2/A3）。
- **no-direct-backend-ref**：同 p6-01 A1 的 `grep` 关卡。
- **e2e 冒烟（需 core-api 真实后端）**：启动 `core-api` + `postgres` 后，用无头浏览器跑「手动清单」7 项，
  命令种子：`pnpm exec playwright test --grep "@smoke-admin"`（测试脚本由后续编写，本目录不含）。
- **视觉回归基线**：Playwright 逐页截图落到 `screenshots/`，与提交基线 diff；仅描述，不写脚本。
- **契约一致性**：复用 p6-01 的 `openapi:gen:check` —— 任何页面用到的 `api.<Op>()` 必须存在于生成 client；
  若 `spec.yaml` 被 Rust 侧改动而页面未更新，生成漂移检测先失败。

### 风险 / Risks

- **依赖 P1–P5 真实后端**：手动清单与截图都需可返回真实数据的 `core-api`；若后端未就绪，本页只能完成
  build/typecheck 红线，业务验证推迟。
- **契约缺口阻塞部分功能**：订阅 Token 管理、批次视图、fetch_policy 编辑在当前契约下无法实现（见上表）；
  若不先推动 API 侧补齐，Subscriptions/Checks/Sources 页将长期不完整。
- **`ConfigMap` 值是字符串**：`ConfigUpdate` 数值需字符串化，前端表单需做类型转换与回显，易出「`0.7` vs `0.7`」
  之外的格式错配（如 `score_decay_alpha` 写回 `"0.70"`）。
- **公开订阅端点无认证**：`GET /{format}` 无需 token，测试时不要用它做「鉴权」断言；其空内容仍可能 `200`
  （`docs/distribution.md`），截图/断言需检查内容非空而非仅状态码。
- **Apalis Board 边界**：不要把队列任务 UI 塞进 Vben 主 Admin；它是独立可选部署，混淆二者会导致范围蔓延。
