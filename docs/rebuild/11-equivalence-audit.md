# 11 上游等价性审计（B3 答复）· Equivalence Audit vs the Go baseline

> 用户问题：「我是否与原版的功能完全等价？原版有多个实现路径的，我只要有一个等价就可以。」
> **裁定口径**：对上游每条路径/能力，本实现只要存在**至少一条可用等价路径**即算通过；不要求同名同形。
> **基线**：fork 根提交 `dda4e6f2`（`git show dda4e6f2:<path>` 取原文）。
> **结论先说**：**不是完全等价**。26 条上游 API 路径中 15 条同形等价、4 条由架构取代（有等价能力、无同名端点）、
> **7 条真缺失**（1 个功能域 + 3 个端点族）；另有 **13 个配置键已在库里 seed 但代码从不读取**（改了没反应）。

---

## 1. 逐路径对照（上游 26 条 API 路径）

| # | 上游路径 | 本实现 | 判定 | 证据 |
|---|---|---|---|---|
| 1 | `POST /api/v1/auth/login` | 同名同形 | ✅ 等价 | `apps/api/src/handlers/auth.rs`（HS256、24h、`token_type=="access"`） |
| 2 | `POST /api/v1/auth/refresh` | 无 | ❌ **缺失** | 上游 `internal/auth/handler.go`；单管理员模型未做 refresh |
| 3 | `POST /api/v1/fetch/sources` | 无 HTTP 端点 | 🔁 架构取代 | 上游供外部 paily-fetch 拉源；本实现 worker 内部 `FetchJob` 直读 DB（`apps/worker/src/fetch_job.rs`） |
| 4 | `POST /api/v1/fetch/results` | 无 HTTP 端点 | 🔁 架构取代 | worker `run_fetch()` 直接落库+对账（`crates/registry/src/reconcile.rs`） |
| 5 | `POST /api/v1/check/nodes` | 无 HTTP 端点 | 🔁 架构取代 | 上游供外部 checker 拉待测节点；本实现 Apalis `CheckJob` + MiaoSpeed WS（`checker-miaospeed`） |
| 6 | `POST /api/v1/check/results` | 无 HTTP 端点 | 🔁 架构取代 | 结果经 `registry` 落库；幂等键 `check_run_id` |
| 7 | `GET/POST /api/v1/sources` | 同名同形 | ✅ 等价 | `handlers/source.rs` |
| 8 | `POST /api/v1/sources/search` | ✅ **同轮补齐** | `handlers/search.rs::sources_search`（AST + 分页 ≤200） |
| 14 | `POST /api/v1/nodes/search` | ✅ **同轮补齐** | `handlers/search.rs::nodes_search` |
| 18 | `POST /api/v1/filter/preview` | ✅ **同轮补齐** | `handlers/search.rs::preview`（≤100 条） |
| 25 | `GET /api/v1/format-configs` | ✅ **同轮补齐** | `handlers/format_config.rs::list` |
| 26 | `PUT /api/v1/format-configs/{format}` | ✅ **同轮补齐** | `handlers/format_config.rs::update`（写入前校验模板） |
| 19 | `GET/POST /api/v1/sponsors` | 无 | ❌ **缺失**（整域缺，**待你裁定是否恢复该功能**） |
| 9 | `GET/PATCH/DELETE /api/v1/sources/{id}` | 同名同形 | ✅ 等价 | `handlers/source.rs` |
| 10 | `GET /api/v1/sources/{id}/nodes` | 同名同形 | ✅ 等价（**无分页**） | `handlers/source.rs::nodes`；上游有 page/limit（≤200） |
| 11 | `POST /api/v1/sources/{id}/ignore-dead` | 同名同形 | ✅ 等价 | `handlers/source.rs::set_ignore_dead` |
| 12 | `GET /api/v1/checkers` | 同名同形 | ✅ 等价 | `handlers/views.rs::checkers` |
| 13 | `GET /api/v1/nodes` | 同名同形 | ✅ 等价（**无分页**） | `handlers/node.rs::list` |
| 14 | `POST /api/v1/nodes/search` | ✅ **同轮补齐** | `handlers/search.rs::nodes_search` |
| 15 | `GET/DELETE /api/v1/nodes/{id}` | 同名同形 | ✅ 等价 | `handlers/node.rs` |
| 16 | `GET /api/v1/nodes/{id}/checks/initial` | 同名同形 | ✅ 等价 | `handlers/views.rs` |
| 17 | `GET /api/v1/nodes/{id}/checks/deep` | 同名同形 | ✅ 等价 | `handlers/views.rs` |
| 18 | `POST /api/v1/filter/preview` | ✅ **同轮补齐** | `handlers/search.rs::preview`（≤100 条） |
| 19 | `GET/POST /api/v1/sponsors` | 无 | ❌ **缺失**（整域缺，**待你裁定是否恢复**） | 上游 `handler/sponsor.go` + `internal/sponsor`；无 sponsor 表 |
| 20 | `PUT/DELETE /api/v1/sponsors/{id}` | 无 | ❌ **缺失** | 同上 |
| 21 | `GET /api/v1/stats` | 同名同形 | ✅ 等价 | `handlers/views.rs::stats` |
| 22 | `GET/PUT /api/v1/config` | 同名同形 | ✅ 等价 | `handlers/config.rs` |
| 23 | `GET /api/v1/logs/fetch` | 同名同形 | ✅ 等价 | `handlers/views.rs::logs_fetch` |
| 24 | `GET /api/v1/logs/check` | 同名同形 | ✅ 等价 | `handlers/views.rs::logs_check` |
| 25 | `GET /api/v1/format-configs` | ✅ **同轮补齐** | `handlers/format_config.rs::list` |
| 26 | `PUT /api/v1/format-configs/{format}` | ✅ **同轮补齐** | `handlers/format_config.rs::update`（写入前校验模板） |
| — | `GET /:format`（上游订阅，**无 token**） | `GET /sub/{token}.{fmt}` | ✅ 等价（**异形**） | 本实现加 token 鉴权+版本失效（`handlers/subscription.rs`）；上游 `cmd/server/main.go:122` 公开 |

**本实现净增（上游没有）**：`POST /api/v1/fetch/trigger`、`/api/v1/subscriptions` 全套（profile/token）、`/metrics`、`/api-docs/openapi.json`。

### 汇总（2026-11-13 更新）

| 判定 | 数量 |
|---|---|
| ✅ 同形等价 | 20（含 2 条「无分页」形态差异） |
| ✅ 异形等价（架构取代 / 改了形态） | 5（fetch×2、check×2、订阅 URL） |
| ❌ 真缺失 | **3**（`auth/refresh`、sponsors×2） |

---

## 2. 真缺失的 3 个功能域 → **已于 2026-11-13 补齐 1 和 3（sponsor 仍待定）**

| 域 | 缺什么 | 状态 |
|---|---|---|
| **B6 高级搜索 + 预览** | `nodes/search`、`sources/search`、`filter/preview` | ✅ **已补齐**：`apps/api/src/handlers/search.rs`（AST JSON body + 分页 ≤200 + 预览 ≤100）；filter crate 扩 `parse_with` / `matches_source` / `SOURCE_FIELD_WHITELIST`；权限矩阵纳入（读面 AdminOrService） |
| **format-configs** | 每格式模板可自定义 | ✅ **已补齐**：`apps/api/src/handlers/format_config.rs` + `format_configs` 表（migration 0010）；写入前用渲染同款解析器校验（坏模板在门口被拒，不在每次订阅时 500） |
| **sponsors** | 整个 sponsor 域（表 + CRUD + 注入订阅） | ✅ **已补齐**（用户裁定恢复，2026-11-13）：`migrations/0011_sponsors`（`filter_ast` JSONB 替代上游 expr 字符串）+ `crates/registry/src/sponsor.rs`（priority 升序、后命中覆盖、`default_sponsor_text` 兜底——上游 `Service::Match` 语义逐条对齐）+ `apps/api/src/handlers/sponsor_admin.rs`（CRUD，AdminOnly 面）+ 订阅渲染注入（`/sub` 命名带 sponsor 文案）；`default_sponsor_text` 键随之生效 |

### 已补齐部分的已知限制（诚实记录）

1. **`unlock` 条件永远不匹配**：没有 streaming 表（`node_streaming` 待随 deep-check 接线），`NodeView.streaming` 恒空 → 「解锁 Netflix」的筛选暂时筛不出东西；
2. **节点 `region` 取自最近一次测活的 `metrics->>'region'`**（`nodes` 表没有 region 列），没有测活记录的节点 region 为空；
3. **搜索是内存过滤**（上游同款语义）——但节点量级大时应改走 `filter::to_sql` 下推；本轮先保持语义等价；
4. **`auth/refresh` 仍未做**（单管理员模型，24h token 过期后重新登录即可）。

---

## 3. 配置面审计（**新增发现：13 个键改了没反应**）

`configs` 表 seed 了 23 个键（`migrations/0001_init.up.sql:58-81` + 0006 的 `orphan_retention_days`）。逐一核对消费点后：

### 3.1 真正生效（3 个）

| 键 | 消费点 |
|---|---|
| `max_distribute` | `handlers/subscription.rs:72` |
| `node_alive_check_window_minutes` | `crates/lifecycle/src/ops.rs:132` |
| `orphan_retention_days` | `crates/lifecycle/src/ops.rs:88` |

### 3.2 **读了但读不到你的值**（6 个，格式不匹配 — 缺陷）✅ **已修（s41）**

`crates/scoring/src/params.rs:16-21` 原用 `cfg_i64(...)/100.0` 读这六个键，期望「整数百分数」；
但 seed 的是**小数**（`0.7`、`0.3`…，见 `0001_init.up.sql:64-69`）→ `parse::<i64>()` 失败 → **走代码里的 fallback**。

| 键 | seed 值 | 代码期望 | 现状 |
|---|---|---|---|
| `score_decay_alpha` | `0.7` | `70` | ✅ 已修（统一按小数解析，兼容百分数写法） |
| `score_weight_latency` | `0.7` | `70` | ✅ 已修 |
| `score_weight_stability` | `0.3` | `30` | ✅ 已修 |
| `score_weight_speed` | `0.55` | `55` | ✅ 已修 |
| `score_latency_curve_k` | `0.3` | `30` | ✅ 已修 |
| `score_jitter_linear_threshold_ms` | `200` | `200` | 唯一格式正确的（整数），一直生效 |

**修复后**：`params.rs` 新增 `cfg_ratio()`——小数 `0.7` → 0.7；兼容百分数 `70` → 0.7（值 ≥ 2 视作百分数）；
单测 `decimals_and_percents_agree` 钉死这条规则。

**后果（已消除）**：以前在后台把权重改成任何小数，评分都不会变；现在**改了就生效**。

### 3.3 无消费者 / 已接线的配置键（13 个 → 逐步接线）

| 键 | 上游原语义 | 本实现状况（s41 后） |
|---|---|---|
| `grade_a_min` / `grade_b_min` / `grade_c_min` | 分级阈值 | ✅ **已接线**：`handlers/config.rs::grade_thresholds()`，订阅渲染不再用硬编码默认值 |
| `check_run_history_days` | 测活历史保留天数 | ✅ **已接线**（A5）：`lifecycle::prune_check_history` 清理周期删除超期记录；**默认改 14 天**（migration 0010，仅改未被改过的 seed 值 7）；`≤0` = 永不删 |
| `cleanup_interval_minutes` | 清理周期 | ✅ **已接线**：worker 调度周期读该键（默认 30 分钟），不再硬编码 600s |
| `score_min_samples` | 评分最少样本数 | ✅ **已接线**（补 seed，migration 0010） |
| `dead_latency_ms` | 判死延迟门限 | ⏸ 仍无消费者（待 B1 真测活后按实际分布定） |
| `fetch_dead_window_days` | 源判死窗口 | ⏸ 同上 |
| `node_dead_window_days` | 节点判死窗口 | ⏸ 同上 |
| `fetch_history_window_days` | 抓取历史保留天数 | ⏸ 仍无消费者（清理只处理孤儿节点 + 测活历史） |
| `default_sponsor_text` | 无 sponsor 命中时的默认文案 | ⏸ sponsor 域未实现 |
| `node_distribute_filter` | 全局分发筛选 | ⏸ 仍无消费者（改由 `subscription_profiles.filter_ast` 表达——**语义已覆盖，但键本身空心**；用户裁定 2026-11-13：**先留着并记录，全部功能做完后再统一复查确认无用了再删**） |
| `node_name_template` | 节点命名模板 | ⏸ 同上（由 `subscription_profiles.naming_rule` 表达；**先留着**，收尾时复查） |
| `filename_clash` / `filename_singbox` / `filename_base64` | 订阅文件名 | ⏸ 仍无消费者（现由 `/sub/{token}.{fmt}` 的 fmt 段决定格式）；**先留着**，收尾时复查 |

**另外**：`score_min_samples` 被读（`params.rs:32`）但**从未 seed**，永远走 fallback `1`。

**为什么这很重要**：你以为改配置能调行为，实际有 13 个键是空心的；6 个键的值被静默忽略。这比「缺功能」更危险——它给你「我调过了」的错觉。

### 3.4 收尾复查（步骤 3，2026-11-13 用户裁定的「全部做完后复查」）

对 3.3 中「先留着」的三组键逐键 grep 全仓 `.rs` 后的终态判定：

| 键 | 复查结果 | 处置 |
|---|---|---|
| `node_distribute_filter` | **确认无消费者**（仅 `naming.rs` 文档注释提及）。语义已 100% 由 `subscription_profiles.filter_ast` 覆盖（同一条 filter 引擎， stronger：AST+参数化 SQL） | **保留**（ configs 是运行时 KV，删行要 migration；等 sponsor 域裁定一起定 schema 变更）。标记：改它无效果 |
| `node_name_template` | **确认无消费者**。语义由 `subscription_profiles.naming_rule` 覆盖（`{{field}}` 白名单模板，默认模板等价于该键的 seed 值） | 同上：保留 + 标记无效 |
| `filename_clash/singbox/base64` | **确认无消费者**。文件名现由 URL 格式后缀决定；上游语义是「每种格式的下载文件名」——若未来要自定义 Content-Disposition 才需要 | 同上：保留 + 标记无效 |
| `dead_latency_ms` / `fetch_dead_window_days` / `node_dead_window_days` / `fetch_history_window_days` | **确认无消费者**，但这些是**生命周期阈值**——B1 真测活落地后必须接线（不是删） | 保留，等 B1 |
| `default_sponsor_text` | sponsor 域未实现 | 保留，等 sponsor 裁定 |
| `score_min_samples` | 已 seed（0010）+ 已读（params.rs）| ✅ 生效 |

**最终清点**：23 seed 键 + 1 补 seed = 24；**生效 10**（6 权重 + max_distribute + node_alive_check_window + orphan_retention_days + check_run_history_days + cleanup_interval_minutes + score_min_samples + grade_*×3 —— 计 12）；**空心 8**（上表 5 组）全部有明确的存在理由与重启条件，无"改了没反应"的静默陷阱（在 configuration-and-api.md 补标注后完全消除）。

---

## 4. 结论（2026-11-13 最终更新，sponsor 恢复后）

- **等价性收口**：26 条上游路径 **25/26 通过**，唯一剩余 `auth/refresh`（单管理员场景重新登录即可覆盖，暂不做）。
- 架构取代的 4 条（fetch/check 服务端点）**不再需要补**：能力已由 worker 闭环 + MiaoSpeed WS 实现，且上游那套要求外部服务持有全量节点凭据（安全更差）。
- 配置面：**12 键生效、8 键空心但有标注**（见 §3.4）——`default_sponsor_text` 随 sponsor 域恢复而生效；`node_distribute_filter`/`node_name_template`/`filename_*` 保留待清理；`dead_latency_ms` 等生命周期阈值等 B1 真数据。
- sponsor 语义与上游逐条对齐：priority 升序遍历、后命中覆盖前者、无命中落 `default_sponsor_text`；差异仅在存储（AST JSON 替代 expr 字符串，与全仓筛选引擎统一）。
- 搜索已在 SQL 下推（S44），比上游的内存全量过滤更强。

---

## 5. 复现命令（校机或本机有 PG 时）

```sql
-- 看 seed 全貌
SELECT key, value FROM configs ORDER BY key;
-- 验证权重键是否被忽略：改成 50 后重算一次，看 node_scores.components 是否变化
UPDATE configs SET value='50' WHERE key='score_weight_latency';
-- 验证 grade 阈值是否硬编码：改成 0.1 后看订阅里的 grade 是否变化（预期：不变 = 缺陷证实）
UPDATE configs SET value='0.1' WHERE key='grade_a_min';
```