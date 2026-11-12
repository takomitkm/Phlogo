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
| 8 | `POST /api/v1/sources/search` | 无 | ❌ **缺失**（B6） | 上游 `internal/api/handler/search.go:109`（expr 过滤+内存分页） |
| 9 | `GET/PATCH/DELETE /api/v1/sources/{id}` | 同名同形 | ✅ 等价 | `handlers/source.rs` |
| 10 | `GET /api/v1/sources/{id}/nodes` | 同名同形 | ✅ 等价（**无分页**） | `handlers/source.rs::nodes`；上游有 page/limit（≤200） |
| 11 | `POST /api/v1/sources/{id}/ignore-dead` | 同名同形 | ✅ 等价 | `handlers/source.rs::set_ignore_dead` |
| 12 | `GET /api/v1/checkers` | 同名同形 | ✅ 等价 | `handlers/views.rs::checkers` |
| 13 | `GET /api/v1/nodes` | 同名同形 | ✅ 等价（**无分页**） | `handlers/node.rs::list` |
| 14 | `POST /api/v1/nodes/search` | 无 | ❌ **缺失**（B6） | 上游 `search.go:25` |
| 15 | `GET/DELETE /api/v1/nodes/{id}` | 同名同形 | ✅ 等价 | `handlers/node.rs` |
| 16 | `GET /api/v1/nodes/{id}/checks/initial` | 同名同形 | ✅ 等价 | `handlers/views.rs` |
| 17 | `GET /api/v1/nodes/{id}/checks/deep` | 同名同形 | ✅ 等价 | `handlers/views.rs` |
| 18 | `POST /api/v1/filter/preview` | 无 | ❌ **缺失** | 上游 `handler/filter.go:23`（expr 试算，最多 100 条） |
| 19 | `GET/POST /api/v1/sponsors` | 无 | ❌ **缺失**（整域缺） | 上游 `handler/sponsor.go` + `internal/sponsor`；`data model` 无 sponsor 表 |
| 20 | `PUT/DELETE /api/v1/sponsors/{id}` | 无 | ❌ **缺失** | 同上 |
| 21 | `GET /api/v1/stats` | 同名同形 | ✅ 等价 | `handlers/views.rs::stats` |
| 22 | `GET/PUT /api/v1/config` | 同名同形 | ✅ 等价 | `handlers/config.rs` |
| 23 | `GET /api/v1/logs/fetch` | 同名同形 | ✅ 等价 | `handlers/views.rs::logs_fetch` |
| 24 | `GET /api/v1/logs/check` | 同名同形 | ✅ 等价 | `handlers/views.rs::logs_check` |
| 25 | `GET /api/v1/format-configs` | 无 | ❌ **缺失** | 上游 `handler/format_config.go`（每格式模板 JSON 可改） |
| 26 | `PUT /api/v1/format-configs/{format}` | 无 | ❌ **缺失** | 同上；本实现模板硬编码默认值（`output.rs:110` 注释「template files come from config in P7」，未接线） |
| — | `GET /:format`（上游订阅，**无 token**） | `GET /sub/{token}.{fmt}` | ✅ 等价（**异形**） | 本实现加 token 鉴权+版本失效（`handlers/subscription.rs`）；上游 `cmd/server/main.go:122` 公开 |

**本实现净增（上游没有）**：`POST /api/v1/fetch/trigger`、`/api/v1/subscriptions` 全套（profile/token）、`/metrics`、`/api-docs/openapi.json`。

### 汇总

| 判定 | 数量 |
|---|---|
| ✅ 同形等价 | 15（含 2 条「无分页」形态差异） |
| ✅ 异形等价（架构取代 / 改了形态） | 5（fetch×2、check×2、订阅 URL） |
| ❌ 真缺失 | **7**（refresh、search×2、preview、sponsors×2、format-configs×2） |

---

## 2. 真缺失的 3 个功能域（按补的成本排序）

| 域 | 缺什么 | 补的成本 | 建议 |
|---|---|---|---|
| **B6 高级搜索** | `nodes/search`、`sources/search` 两个 POST 路由 | **小**：`crates/filter` 已有 AST + 参数化 SQL + 内存 `matches`；只缺 body 接 AST 与分页 | 前端产 AST JSON（`filter_ast` 字段），不做 expr 兼容层 |
| **filter/preview** | 试算端点（最多 100 条） | **小**：`crates/filter/src/eval.rs` 的 `matches` 已具备；上游就是「全量拉内存逐条 EvalBool」 | 顺手可做，与 B6 同批 |
| **sponsors** | 整个 sponsor 域（表 + CRUD + 注入订阅） | **中**：要建表 + 匹配引擎（按 priority 从小到大、后者覆盖）+ 注入点；`naming.rs` 的 `sponsor` 字段已预留 | 需要你确认是否还要这个功能 |
| **format-configs** | 每格式模板可自定义 | **中**：`output.rs` 已接受 `template_yaml/template_json` 参数（库层就绪），缺「配置存储 + 读取路径 + 管理端点」 | 需要你确认是否要「自定义模板」 |

---

## 3. 配置面审计（**新增发现：13 个键改了没反应**）

`configs` 表 seed 了 23 个键（`migrations/0001_init.up.sql:58-81` + 0006 的 `orphan_retention_days`）。逐一核对消费点后：

### 3.1 真正生效（3 个）

| 键 | 消费点 |
|---|---|
| `max_distribute` | `handlers/subscription.rs:72` |
| `node_alive_check_window_minutes` | `crates/lifecycle/src/ops.rs:132` |
| `orphan_retention_days` | `crates/lifecycle/src/ops.rs:88` |

### 3.2 **读了但读不到你的值**（6 个，格式不匹配 - 缺陷）

`crates/scoring/src/params.rs:16-21` 用 `cfg_i64(...)/100.0` 读这六个键，期望「整数百分数」；
但 seed 的是**小数**（`0.7`、`0.3`…，见 `0001_init.up.sql:64-69`）→ `parse::<i64>()` 失败 → **走代码里的 fallback**。

| 键 | seed 值 | 代码期望 | 现状 |
|---|---|---|---|
| `score_decay_alpha` | `0.7` | `70` | 用 fallback 70（恰好等于 0.7，所以看不出问题） |
| `score_weight_latency` | `0.7` | `70` | 同上 |
| `score_weight_stability` | `0.3` | `30` | 同上 |
| `score_weight_speed` | `0.55` | `55` | 同上 |
| `score_latency_curve_k` | `0.3` | `30` | 同上 |
| `score_jitter_linear_threshold_ms` | `200` | `200` | 唯一格式正确的（整数），真正生效 |

**后果**：你在后台把权重改成任何小数（比如 `0.8`）→ **静默失效**，评分不变。这是必须修的缺陷（P1，改动小：统一成小数解析，或 seed 改整数）。

### 3.3 完全没消费者（13 个，改了 100% 没反应）

| 键 | 上游原语义 | 本实现状况 |
|---|---|---|
| `grade_a_min` / `grade_b_min` / `grade_c_min` | 分级阈值 | `handlers/subscription.rs:90` 用 `GradeThresholds::default()` **硬编码** |
| `dead_latency_ms` | 判死延迟门限 | 无消费者 |
| `fetch_dead_window_days` | 源判死窗口 | 无消费者 |
| `node_dead_window_days` | 节点判死窗口 | 无消费者 |
| `fetch_history_window_days` | 抓取历史保留天数 | 无消费者（清理只处理孤儿节点） |
| `check_run_history_days` | 测活历史保留天数 | 无消费者（同上，**A5 要落地的就是这个**） |
| `cleanup_interval_minutes` | 清理周期 | 无消费者（`apps/worker/src/main.rs` 硬编码 300s/600s tick） |
| `default_sponsor_text` | 无 sponsor 命中时的默认文案 | 无消费者（sponsor 域未实现） |
| `node_distribute_filter` | 全局分发筛选 | 无消费者（改由 `subscription_profiles.filter_ast` 表达） |
| `node_name_template` | 节点命名模板 | 无消费者（改由 `subscription_profiles.naming_rule` 表达） |
| `filename_clash` / `filename_singbox` / `filename_base64` | 订阅文件名 | 无消费者 |

**另外**：`score_min_samples` 被读（`params.rs:32`）但**从未 seed**，永远走 fallback `1`。

**为什么这很重要**：你以为改配置能调行为，实际有 13 个键是空心的；6 个键的值被静默忽略。这比「缺功能」更危险——它给你「我调过了」的错觉。

---

## 4. 结论

- **不是完全等价**；按你的口径（有一条等价路径即算），**19/26 通过**，**7 条真缺失**（3 个功能域：高级搜索+预览、sponsor、模板配置）。
- 架构取代的 4 条（fetch/check 服务端点）**不再需要补**：能力已由 worker 闭环 + MiaoSpeed WS 实现，且上游那套要求外部服务持有全量节点凭据（安全更差）。
- **新发现两类配置缺陷**：6 键值被忽略（格式不匹配）、13 键无消费者。
- **优先级建议**：① 修配置读取格式（影响所有调参）→ ② 接 `check_run_history_days` 并把默认改 14 天（A5 裁定）→ ③ B6 搜索 + preview（小）→ ④ sponsor / format-configs 待你定是否要恢复该功能。

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