# 10 Review Crosscheck · 旧版 Go 评审 × Rust 重构 交叉审查

> 目的：上游 Go 仓库收到一份五维评审（正确性/可靠性/安全性/架构/工程化）。本文以**当前 Rust 实现**为准，
> 逐节对照该评审，区分「已实现（附证据）/ 部分实现 / 真实缺口 / 已被新架构取代（不再讨论）」。
> **裁定（用户 2026-11-12）**：本文档入库 + P0-01（权限收紧）+ P0-02（登录限流）直接执行；
> ErrorResponse/分页等契约改进独立为 P1，不混入本轮；不得恢复 Go 代码或已取消的外部服务。

---

## 1. Review Scope 评审适用范围

**旧评审的前提**（写评审时的事实）：Go 单体 `cmd/server/main.go` + 外部 `paily-fetch`/`paily-check` 服务 +
expr-lang 表达式引擎 + SQLite/PostgreSQL 双支持 + OpenAPI spec-first（oapi-codegen 27 路径）。

**当前事实**（Rust 重构后，见 `docs/rebuild/09-self-completed-gaps.md` 与 ACTION-LOG S05–S38）：
- Go 源码已删除（commit `b7c75c4`），对照基准为 fork 根提交 `dda4e6f2`；
- 单一 Rust 进程对：`apps/curse`（Axum 管理面）+ `apps/worker`（抓取/测活/清理闭环）；
- 外部 Fetch/Checker 服务取消：worker 直连 PostgreSQL + MiaoSpeed WebSocket；
- expr-lang 消灭：`crates/filter` 类型化 AST（白名单字段/运算符 + 参数化 SQL）；
- PostgreSQL only（SQLx 0001–0009 迁移）；utoipa 契约 19 路径（commit `de57dcc`）。

**结论**：评审中所有以 Go/外部服务为前提的建议，先按 §6 裁决是否仍适用，再谈实现。

---

## 2. Finding-by-Finding Crosscheck 逐节对照

### 2.1 权限与认证（评审 §1，P0）

| 评审点 | 状态 | 证据 |
|---|---|---|
| 「管理接口同时接受 ServiceBearer」 | **⚠️ 评审对本实现仍成立（收口前）**——P1-02 起 `require_admin_or_service`（`apps/curse/src/auth.rs`）罩住整个 `/api/v1/*`，含 DELETE/PUT 管理写操作 | `auth.rs:45-66`；router.rs 单层 `middleware::from_fn_with_state(require_admin_or_service)` |
| 三层权限 Admin/Fetch/Checker | 部分：分层中间件接口已预留；但**本架构没有 checker HTTP 面**（MiaoSpeed WS，见 §6） | `auth.rs:1-2` 注释；worker 走 `checker-miaospeed` crate |
| Fetch/Checker 独立密钥+轮换 | ❌ service_secret 单值，无轮换 | `crates/config/src/lib.rs` AuthConfig |
| 登录速率限制/防爆破 | ❌ 登录端点 P7-02 才有；限流 layer 已写但**未挂载**（本轮 P0-02 修复） | `apps/curse/src/rate_limit.rs` 存在、router 未引用 |
| JWT 过期/算法/刷新 | 部分：HS256 + `token_type=="access"` + 24h 过期 ✅（`auth.rs:26-43`）；refresh ❌（单管理员模型暂缓） |
| NodeDetail 敏感字段暴露 | ✅ **优于上游**：上游 `GetNodes` 把含凭据的 `raw` 全量下发 checker（`dda4e6f2:internal/checker/handler.go`）；本实现的 API 面不返回 password/raw，节点 payload 经 `node_payloads` 加密层（B2 待办）才出网 | `handlers/node.rs` NodeView 无敏感字段；`CONVENTIONS.md` §2.4 |

**P0-01 修复**：见 §4 Accepted Changes AC-1；修复前的权限矩阵见 `apps/curse/src/auth.rs::auth_kind`（本轮新增）。

### 2.2 数据库可靠性（评审 §2，P0）

| 评审点 | 状态 | 证据 |
|---|---|---|
| 版本化 migration | ✅ | `migrations/0001..0009_{up,down}.sql`；每个集成测试 `setup()` 先 `database::migrate()` |
| 事务原子性 | ✅ | 对账+孤儿标记单事务：`registry/src/reconcile.rs::reconcile_fetch_result`（`pool.begin()/commit`）；失败保留旧关联（P2-04 集成测试 `failed_fetch_keeps_mappings`） |
| 并发防覆盖 | ✅ | `ON CONFLICT (type,content)/(identity_hash)/(node_id,check_run_id,checker_tag) DO NOTHING` + `WHERE EXCLUDED.last_checked_at > node_status.last_checked_at`（迟到不覆盖，P3-04 测试）+ `mark_orphaned_if_lonely` 原子 `NOT EXISTS`（P4-01） |
| 索引 | 部分 ✅ | 已有：PK/UNIQUE 各表、`idx_sources_type_status`、`idx_fetch_runs_source`、`idx_node_check_results_run`。**缺**：`node_check_results(node_id, checked_at)` 复合索引——本轮**经查询形态确认**时间线查询 `WHERE node_id=? ORDER BY checked_at DESC` 无覆盖索引（P1 待办，不混入本轮） |
| 历史分级保留 | ❌ | 仅孤儿节点有保留窗口（`orphan_retention_days`，P4-01/02）；测活历史只增不减。**需先定审计价值策略**（P1 待办） |
| 备份/恢复演练 | 部分 ✅ | `scripts/backup.sh`(pg_dump -Fc)/`restore.sh`(恢复+行数校验)；compose 拓扑 podman 实测（S34）；**恢复演练未实跑**（无容器环境，B4 残留） |
| 数据一致性定期检查 | 部分 ✅ | cleanup cycle 600s tick 已接线 worker（`apps/worker/src/main.rs`），孤儿→Retired→物理删；失效关联由外键 CASCADE 保证 |

### 2.3 测活与评分（评审 §3，P1）

**3.1 可解释评分**：

| 评审点 | 状态 | 证据 |
|---|---|---|
| 分项因子+权重记录 | ✅ | `crates/scoring/src/engine.rs::ScoreComponents{weighted_latency_ms, weighted_jitter_ms, latency_score, stability_score, quality_score, speed_score, sample_count}` 落 `node_scores.components`(JSONB) |
| 评分版本号 | ✅ | `version="ewma-v1"`（engine.rs `SCORE_VERSION`），golden 0.967722（14+5 测试） |
| 数据新鲜度衰减 | ✅（EWMA 本身即时间加权，`alpha^i`） |
| 延迟非线性 | ✅ | `1 - ln(1+k·x)/ln(1+k)` 对数曲线 |
| 抖动独立 | ✅ | 线性/平方分段（阈值 200ms 定稿） |
| 速度归一化 | ✅ | `sqrt(speed/163840)`；**分歧已定稿**：按 Go 代码基线始终混合（p4-03 页 §验证结果决策 1） |
| 存活率窗口统计 | 部分：`cross_tag_alive` 窗口合并（P4-02）；存活率**序列**统计 ❌ |
| 流媒体分别评分 | ❌（unlock 仅作布尔 flags，P2-02 `streaming` 原样入库） |
| 场景化配置（游戏/流媒体 profile） | ❌（P1 待办；需先与 docs/scoring.md 分歧定稿） |
| 历史评分重算 | ❌（`recompute_all_identities` 同款机制未建，P1 待办） |

**3.2 测活可信度**：

| 评审点 | 状态 | 证据 |
|---|---|---|
| 结果过期机制 | ✅ | `cross_tag_alive` 的 `node_alive_check_window_minutes`(240) 窗口（P4-02 `cross_tag_alive_map`） |
| 迟到/重复防护 | ✅ | `should_overwrite`（p3-03）+ `ON CONFLICT`（p3-04），集成测试钉死 |
| 多 Checker 交叉验证/置信度 | ❌ | 跨 tag 仅做 alive OR 合并（`bool_or`）；置信度与异常识别未建（**P1 待办**，Degraded 规则一并冻结于 LIFECYCLE.md） |
| 连续 vs 偶发失败区分 | ❌ 同上 |

**3.3 调度**：评审建议（分批领取/租约/优先级/心跳）针对**外部 Checker HTTP 模式**——本架构已取消外部 checker，**Apalis PG backend 天然提供租约/超时回收/崩溃恢复**（P3-01 真机验证；官方 migration 表）。已被新架构取代，唯一可取项：
- **增量测活**（只测有变化节点）——❌ 现为每轮全量（P1 待办）。

### 2.4 API 契约（评审 §4，P1）

| 评审点 | 状态 | 证据 |
|---|---|---|
| spec-first | ✅ 且比上游强：utoipa 为**源头**（`crates/api-contract`），`GET /api-docs/openapi.json` 导出 19 路径（commit `de57dcc`，B5 关闭） | `api/openapi/spec.json` |
| 幂等性 | ✅ | `check_run_id` 键控 + 三层唯一约束 + `ON CONFLICT`（P3-04 验收；`no_duplicate_work`/`check_persistence_idempotent` 测试） |
| 统一 ErrorResponse | ❌ 现为纯文本错误体（各 handler `err500`）——**P1**（用户裁定不混入本轮） |
| 分页 + limit 上限 | ❌ 列表端点全量返回——**P1** |
| ConfigMap 类型/范围 | ❌ KV 字符串——**P1** |
| 请求长度/格式校验 | 部分：filter AST 白名单+深度/叶子数限制 ✅；其余端点长度/范围 ❌ **P1** |
| API 兼容策略 | ❌ 文档缺失（**P1**） |

### 2.5 表达式引擎（评审 §5，P1）

| 评审点 | 状态 | 证据 |
|---|---|---|
| Expression → AST → Type Check → Query Plan → SQL | ✅ **除 Type Check 外全部** | `crates/filter`：`ast.rs`（类型化 AST，手写反序列化贴 plan §9.2）→ `parse.rs`（字段/运算符双白名单 + 深度8/叶子32限制）→ `sql.rs`（**$n 参数化，零字符串拼接**，EXISTS 子查询）→ `eval.rs`（内存求值） |
| 白名单约束 | ✅ | `FIELD_WHITELIST` 7 字段 / 10 运算符；password 等敏感字段拒绝（测试 `rejects_unknown_field`） |
| 静态类型检查 | ❌ `score gte "abc"` 类型错配在求值时才拒（**P1 待办**） |
| 编译缓存 | ❌ 每请求重新 parse（上游 expr 有 `compiled()` 缓存；P1 待办） |
| 错误位置定位 | 部分：`FilterError` 带字段/运算符名，无行列位置 |
| 资源限制 | ✅ 深度/叶子数/结果集上限（`MAX_AST_DEPTH/MAX_CONDITION_COUNT/MAX_RESULT_SIZE`） |
| **不要把表达式转 SQL 字符串** | ✅ 且已超额：**expr-lang 整个被消灭**（评审以为还在用）——上游 `internal/filter/filter.go` 的 expr 引擎不存在于本实现 |

### 2.6 分发系统（评审 §6，P1）

| 评审点 | 状态 | 证据 |
|---|---|---|
| 独立格式适配层 | ✅ | `crates/distribution/src/output.rs`：clash/singbox/base64 三 converter；**sing-box schema 独立性测试钉死**（`converters_independent`：clash 用 `port`、singbox 用 `server_port`，字段集不重叠） |
| 每 adapter 独立测试 | ✅ | `mihomo_yaml_fills_proxies_and_groups` / `singbox_is_independent_schema` / `base64_roundtrip` |
| 节点数量限制 | ✅ | `min(profile.limit_count, max_distribute)` + `never_exceeds_max` 测试（200/37/0） |
| ETag/Last-Modified/Cache-Control | ❌（**P1 待办**） |
| 生成缓存（version 键） | 部分：`subscription_profiles.version` 列与失效语义已设计（S35），HTTP 层未实装 |
| 失败降级 | ❌（空候选当前 404，无降级内容） |
| 兼容性测试（mihomo check） | ❌ 留 compose 环境（B4 残留） |

### 2.7 工程化（评审 §7）

| 评审点 | 状态 | 证据 |
|---|---|---|
| 单元测试 P0 | ✅ | fingerprint 14 / filter 7 / selector 12 / scoring 5 / ws-adapter 7 / token 3 / naming 3 / output 4 / redaction 3 / transitions 2 |
| 集成测试 P0（PG+API） | ✅ | PG：pg_crud 2 / reconcile 6 / check_persist 2 / p401 2 / p402 4 / fetch_loop e2e 1；API：smoke 4 + views e2e（stats/checkers/logs 200、订阅链 201→token→/sub 200） |
| 并发测试 P0 | 部分 | 幂等重投（同 run 4 次 1 行）✅；真并发压测 ❌ |
| CI P0 | ❌ **用户裁定先不写**；各页「CI 提示」节为现成种子 |
| 迁移自动化测试 P0 | ✅ | 每集成 setup `migrate!` 全量跑（0001→0009 链） |
| Docker Compose P1 | ✅ | `deploy/compose.yaml` 六服务；podman 6.1.4 实测（解析/postgres/端口语义，S34） |
| OpenAPI 校验 P1 | ✅ | utoipa 源头生成（spec-first 反向：代码即契约），19 路径 |
| Go 静态分析 P1 | **已过时**：Go 已删；等价物 `cargo clippy --workspace --all-targets -- -D warnings` 全绿 + `cargo audit`（B4，离线匹配器 4 命中甄别） |
| 可观测性 P1 | ✅ | `/metrics`（进程计数+DB 量规）、JSON tracing、`RedactionLayer`+`redact_*`（P7-02） |
| 结构化日志（请求/任务 ID） | 部分：JSON 结构化 ✅；请求 ID/任务 ID 字段 ❌（**P1**） |
| 性能基准 P2 / 发布流程 P2 | ❌ |

### 2.8 架构（评审 §8）——已由既定架构决策取代

评审建议「保留 Go、内部模块化、保留 Phlogo Fetch/Checker」。**裁定：不再讨论**（用户 2026-11-12）：
- Go → Rust 全面重写已完成（Go 已删，`dda4e6f2` 为对照基准）；
- Phlogo Fetch 取消 → worker fetch 闭环 + **net-new SSRF 防护**（`crates/fetch`，6 测试；上游无此安全面）；
- Phlogo Checker 取消 → MiaoSpeed WS Adapter（`checker-miaospeed`，golden 签名与 Go `utils.SignRequest` **逐字节一致**）；
- 评审的内部模块化清单（Source/Node/Check/Scoring/Filter/Distribution/Persistence）与本实现的 crate 切分**一一对应**——fetch 闭环接线（S37）零接口改动即验证了边界。

---

## 3. Confirmed Gaps 真实缺口（按本轮裁定分层）

**P0（本轮执行）**：
1. 管理写操作接受 ServiceBearer（2.1 首行）→ **P0-01 修复**。
2. 登录无限流 → **P0-02 修复**。

**P1（已排期，不混入本轮）**：
3. 统一 ErrorResponse；4. 列表分页+limit 上限；5. ConfigMap 类型化；6. 请求长度/范围校验；7. API 兼容策略文档；
8. `node_check_results(node_id, checked_at)` 索引（先看查询计划）；9. 测活历史分级保留（先定审计价值）；
10. 分发 ETag/Cache-Control/version 缓存实装；11. 评分场景化+重算（先与 docs/scoring.md 分歧定稿）；
12. Degraded 派生规则 + 置信度/异常识别（LIFECYCLE.md 冻结）；13. 增量测活；14. 请求/任务 ID 日志字段；15. 静态类型检查+编译缓存（filter）。

**P2**：性能基准、发布流程/CHANGELOG、mihomo check 兼容性测试（容器环境）。

---

## 4. Accepted Changes 本轮接受的改动（P0）

### AC-1 权限矩阵 + 管理写收紧（P0-01）
- **矩阵**（代码化于 `auth.rs::auth_kind(method, path)`，segment 匹配 + **未匹配一律 AdminOnly（fail-closed）**）：

| 路由（/api/v1 下） | 方法 | 权限 | 理由 |
|---|---|---|---|
| /auth/login | POST | Public | 登录 |
| /sources | GET/POST | AdminOrService | 上游 main.go `sourceCreateMW` 语义；fetch 服务身份合法面 |
| /sources/{id} | GET | AdminOrService | 读 |
| /sources/{id} | PATCH/DELETE | **AdminOnly** | 管理写 |
| /sources/{id}/ignore-dead | POST | **AdminOnly** | 管理写 |
| /sources/{id}/nodes | GET | AdminOrService | 读 |
| /nodes | GET | AdminOrService | 读 |
| /nodes/{id} | GET | AdminOrService；DELETE | **AdminOnly** |
| /nodes/{id}/checks/{initial,deep} | GET | AdminOrService | 读 |
| /stats /checkers /logs/{check,fetch} | GET | AdminOrService | 读 |
| /config | GET | AdminOrService；PUT | **AdminOnly** |
| /fetch/trigger | POST | AdminOrService | **唯一合法的服务身份写操作**（内部编排触发） |
| /subscriptions（全部方法） | * | **AdminOnly** | profile/token 全管理面 |
| 其余未匹配 /api/v1/* | * | **AdminOnly** | fail-closed |

- **实现**：`require_api_auth` 中间件替换整罩式 `require_admin_or_service`；401=缺/坏凭据，403=ServiceBearer 打 AdminOnly 面。
- **不动**：`/sub/{token}`（订阅 token 自证）、MiaoSpeed WS（worker 侧独立协议认证）、worker↔DB 直连。

### AC-2 登录限流（P0-02）
- 现有 `rate_limit.rs` 固定窗口 layer 挂到 `POST /api/v1/auth/login`（独立 `RateLimitState`，`PHLOGO_LOGIN_RATE_LIMIT_PER_MIN` 默认 10）。
- **IP 来源 = peer addr（`ConnectInfo`），不读 `X-Forwarded-For`**（防伪造）；caddy 反代下所有请求共享一个桶（保守方向），边界信任配置留 P1。
- **不做账户维度锁定**：单管理员模型下账户锁定 = 自我 DoS（评审建议的合理组合在单账户场景即纯 IP 限流）。
- `main.rs` 改 `into_make_service_with_connect_info::<SocketAddr>()` 使真实 peer 可用。

### AC-3 验收标准
1. AdminJWT 可执行全部管理操作（含此前被 service 也可执行的写）。
2. ServiceBearer 打管理写（PATCH/DELETE sources、DELETE nodes、PUT config、subscriptions 面、ignore-dead）→ **403**。
3. 未认证 → 401。
4. ServiceBearer 在合法面（GET 读、POST /sources、POST /fetch/trigger）→ 非 401/403。
5. `/sub`、`/health`、`/metrics`、login 不受影响。
6. `auth_kind` 矩阵函数单测覆盖正反向；集成测试覆盖「admin 通过 / service 403 / 无凭据 401」三态。
7. 登录限流：阈值内 401（凭据错仍计桶）、超限 429、XFF 不改判、并发正确、限流不影响其他端点（/health 200）。
8. 全套测试（build/test/fmt/clippy）0。

---

## 5. Deferred Items 暂缓项（§3 P1/P2 清单 + 用户裁定「不顺手开展」）

ErrorResponse/分页/ConfigMap 类型化/请求校验/兼容策略/索引/历史保留/缓存头/评分场景化与重算/Degraded/置信度/增量测活/日志 ID 字段/静态类型检查/编译缓存/CI/压测/发布流程——**全部只在本页登记，等待单独安排**。每项的验收标准已在 §3 引用评审原文并按本架构修正。

---

## 6. Rejected Recommendations 已被新架构取代（不再讨论）

1. 「保留 Go、不要全部重写成 Rust」——已重写并删除 Go（对照基准 `dda4e6f2`）。
2. 「保留 Phlogo Fetch / Phlogo Checker 外部服务」——已取消（worker 闭环 + MiaoSpeed WS）。
3. 「Checker 任务租约/心跳/分批领取」——Apalis PG backend 原生提供；唯一遗留可取项「增量测活」入 P1。
4. 「golangci-lint / govulncheck」——等价物 clippy -D warnings + cargo audit（B4 离线匹配器 4 命中甄别）。
5. 「SQLite 支持不完整」——PostgreSQL only，不适用。

---

## 7. 执行记录（P0-01 / P0-02 已完成，2026-11-12）

**P0-01 权限矩阵收紧**（`apps/curse/src/auth.rs` + `router.rs`）：
- `auth_kind(method, path)`：代码化矩阵；**路径归一化关键实现细节**——nest 内中间件收到的是剥掉 `/api/v1` 前缀的相对路径（smoke 实证：无凭据 GET /stats 曾直接 200 吐数据），`strip_prefix` 归一后按业务段匹配；
- **fail-closed**：未匹配一律 AdminOnly；403=ServiceBearer 打管理写（区别于 401 无凭据）；
- `require_api_auth` 替换整罩式 `require_admin_or_service`（后者保留供旧调用）；
- `login` 挂到 nest **外**（root `/api/v1/auth/login`），带独立限流 route_layer——矩阵 middleware 不包它。

**P0-02 登录限流**（`apps/curse/src/rate_limit.rs` + router）：
- **桶键改 peer IP**（剥端口，防换连接绕过；原 SocketAddr 键每次连接换桶——实测发现并修复）；XFF 从不读取；
- 挂载：`/api/v1/auth/login` 独立 `route_layer`（阈值 `PHLOGO_LOGIN_RATE_LIMIT_PER_MIN` 默认 10/min）；
- `main.rs` 改 `into_make_service_with_connect_info::<SocketAddr>()` 供真实 peer。

**验证**：
- `auth_matrix` 单测 7/7（login Public、读 AdminOrService、服务写 intake AdminOrService、12 项管理写 AdminOnly、未匹配 fail-closed、UUID 形状分流）；
- `login_limit` 集成 4/4（真实 TCP：触发 429、health 不受扰、XFF 不可换桶、并发锤 429>0）；
- **全套 workspace 测试（真实 PG）`FULL_TEST_EXIT=0`**，含全部既有 golden/PG 集成；
- `cargo fmt/clippy -D warnings` 0。

**修复过程中的额外发现**：
1. `lifecycle/tests/p402.rs` 用**绝对日期**造数据——跨日后 `cross_tag_map_window` 静默出窗（本轮跑挂），改相对 `now` 偏移（测试基建缺陷，非产品 bug）；
2. axum 0.7 `nest` 内 middleware 收到剥前缀路径——权限/日志类中间件做 path 判定时**必须归一化**（写入 auth.rs 文档注释）。

**验收对照（AC-1/AC-3）**：1✓ 2✓（403 语义）3✓ 4✓（读/intake/trigger 保持）5✓（sub/health/metrics/login 不经矩阵）6✓（正反向）7✓；AC-2 全项 ✓。P0 关闭。

---

## 8. 用户裁定登记（2026-11-13）

| # | 事项 | 裁定 | 落地状态 |
|---|---|---|---|
| A1 | 反代下限流语义 | **PROXY protocol**（选项③，用户裁定 2026-11-13） | ✅ **已实现**（s42）：`apps/curse/src/proxy_protocol.rs`（v1 解析 + 自己的 accept 循环 + hyper 驱动）；Caddy `transport http { proxy_protocol v1 }`；`api` 不再发布宿主端口；`expect_proxy` 强制要求该头 |
| A2 | 凭据加密方案 | **XChaCha20-Poly1305 + `PHLOGO_PAYLOAD_KEY`** 方向确认（量子面见 §9 说明） | 待实现（B2） |
| A3 | 高级搜索形态 | **① 前端产 Filter-AST JSON**（不做 expr 兼容层） | 待实现（B6，见 11 §2） |
| A4 | 评分 speed 混合口径 | **保持现状**（按上游 Go 代码基线混合；已记录于 p4-03 决策 1） | 已定稿，无需改动 |
| A5 | 测活历史保留 | **明细留 14 天**；**保留天数必须可配**（走 `configs`，禁止硬编码） | 待实现（P1：接 `check_run_history_days` 默认值改 14；注意该键当前**无消费者**） |
| A6 | Degraded 判定 | **先不定**，等 B1 真实测活数据出来后按分布定阈值 | 冻结中 |
| A7 | CI | **暂不急**；要求**记录做好**，随时可据此写完整 CI | 各页「CI 提示」+ 11 §5 复现命令为种子 |
| A8 | 容器演练 / 真测活环境 | **容器用本机**（本机 podman 已验）；真测活环境待 B1 落地时再定 | B4 容器部分可转本机执行 |
| B1 | 权限矩阵（P0-01） | **通过** | 已推送 `ed641b3` |
| B2 | 登录限流（P0-02） | **通过** | 已推送 `ed641b3` |
| B3 | 与上游等价性 | 口径：**一条等价路径即可** → 见 `11-equivalence-audit.md`（19/26 通过，7 条真缺失） | 审计完成，缺失项排期 |
| C | 三个自检失误 | **修复** | 本轮修复（见 ACTION-LOG S40） |

---

## 9. A1 备选方案对比（供裁定）

| 方案 | 做法 | 优点 | 缺点 | 工作量 |
|---|---|---|---|---|
| ① 现状 | 桶键=peer IP；反代后即全局桶 | 零改动、绝不可伪造 | 多用户互相挤（10/min 总量） | 0 |
| ② 可信代理 CIDR | `PHLOGO_TRUST_PROXY_CIDRS` 配置；只有来自该网段的请求才采纳 `X-Forwarded-For` | 保留 Caddy；标准做法（nginx/HAProxy/traefik 同） | 配置错=可伪造；需保证 API 端口不对外 | 半天 |
| ③ PROXY protocol | Caddy `listener_wrappers { proxy_protocol }`；API 直接拿真实 TCP peer | **无需信任任何头**（地址由内核给出）；最安全 | 需 Caddy 与 API 都开；直连 API 必须被封 | 半天~1天 |
| ④ 边缘限流 | Caddy 装 `caddy-ratelimit` 插件，在入口按真实 IP 限 | API 完全不关心 IP | 需自编译 Caddy（xcaddy）；多一处配置面 | 1天 |
| ⑤ 去掉反代 | API 直接暴露 TLS（axum+rustls） | peer 即真实 | 丢掉 http→https 跳转/静态资源/单端口便利 | 半天 |

**推荐**：③ PROXY protocol（不引入"可信头"这一类可伪造面）；若想少改 Caddy，则 ②（可信 CIDR）+ 防火墙保证 API 端口不可直连。**换掉 Caddy 本身不解决问题**——问题不在代理是谁，而在于「API 如何得知真实客户端地址」，换 nginx/HAProxy 同样是这一组选项。

### A1 落地记录（s42，2026-11-13）

**已实现 PROXY protocol v1**（用户裁定）：

| 组件 | 实现 |
|---|---|
| 解析 | `apps/curse/src/proxy_protocol.rs::parse_v1` — `PROXY TCP4/TCP6` 提取真实源 IP:port；`PROXY UNKNOWN` 无地址；非 PROXY 前缀不误判。单测 5 项 |
| 服务端 | axum 0.7 的 `axum::serve` **没有**消费前置头的钩子（内部直接建连交给 hyper），所以本模块自带 accept 循环 + `hyper::server::conn::http1`（照 axum `serve.rs` 的写法）；非 PROXY 连接的首批字节用 `PrefixedStream` **回放**给 hyper，不会丢请求 |
| 真实 IP 落点 | `ConnectInfo<SocketAddr>` 注入（与 axum 原生路径同构）→ `proxy_protocol::peer_addr/peer_ip` 统一取用 |
| 限流桶键 | `rate_limit.rs` 改用 `peer_ip()` |
| 代理侧 | `deploy/Caddyfile` 三个 `reverse_proxy` 各加 `transport http { proxy_protocol v1 }`（**caddy validate 实测通过**） |
| 部署面 | `deploy/compose.yaml`：`api` **取消宿主端口**（只 `expose`），`PHLOGO_SERVER_PROXY_PROTOCOL=1` + `PHLOGO_SERVER_EXPECT_PROXY=1`（缺头即拒） |
| 配置 | `PHLOGO_SERVER_PROXY_PROTOCOL` / `PHLOGO_SERVER_EXPECT_PROXY`（`crates/config`，默认关，向后兼容） |
| 测试 | `apps/curse/tests/proxy_protocol.rs`：带 PROXY 头→200；无头（不强制）→200；无头（强制）→拒绝；`PROXY UNKNOWN`→200 不报错；头被正确消费（否则 hyper 会把 `PROXY TCP4 …` 当请求行→400） |

**顺带修掉的真 bug（重要）**：`rate_limit.rs` 原本读**裸 `SocketAddr`** 扩展，而 axum 插入的是 `ConnectInfo<SocketAddr>` → **永远取不到**，桶键恒为 `0.0.0.0`（全局一个桶）。也就是说"按 IP 分桶"此前**没有真正生效**。现已改为经 `peer_ip()` 读取 `ConnectInfo`。

**遗留**：`expect_proxy` 只在 PROXY 模式下有意义；直连模式（默认）仍以 TCP peer 为准。

### A2 量子安全说明（供裁定）

- **对称加密**（XChaCha20-Poly1305，256 位密钥）：量子威胁只有 Grover 搜索，强度折半 → 有效 **128 位**，仍是通行安全水平；**不属于「会被量子计算机破掉」的类别**。我们用它保护的是「数据库被拖走」这一类静态风险，场景完全够用。
- **真正会被量子破的是公钥**（RSA/ECC，Shor 算法）。如果我们用 AGE，它的收件人加密基于 X25519（椭圆曲线）→ **这半边不是后量子安全**（除非用较新版本的混合后量子收件人 `age1pq…`，属实验/新特性）。
- 结论：**选 XChaCha20 反而比 AGE 更"后量子友好"**，因为全程不需要公钥；密钥管理靠环境变量/密钥轮换即可。唯一要写好的是 nonce 不可重用（每次加密随机 24B nonce）与 AAD 绑定记录 id。
