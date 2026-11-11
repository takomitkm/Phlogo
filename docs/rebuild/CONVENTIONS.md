# 重构文档约定与现状速查（CONVENTIONS & Phlogo Current-State Facts）

> 本文件是 `docs/rebuild/` 全部步骤页面的**唯一事实来源（single source of truth）**。
> 每个阶段的子步骤页面都必须引用本文件，保证术语、组件映射与验收口径一致。
> Bilingual: 正文以中文为主，关键英文术语（组件名、命令、字段名、API path）保留英文。

---

## 0. 目的（Purpose）

把 `takomitkm/Phlogo`（即 `openpaily/paily-core` 的一个 fork）这个 **Go 单体**重构为
`@S:\Downloads\paily-core-rust-rebuild-plan.md` 描述的 **Rust 多进程架构**（core-api + core-worker + 独立 MiaoSpeed + Vben 前端）。

本目录的每一页对应一个**可独立验收的大步骤的子步骤**。页面要写得足够详细，使后续可以**照着写 CI**——
因此每个页面都给出**具体命令、预期产物、验收标准**，并在末尾标注「CI 提示」。
**本期不写 CI 本身**，只把这些命令作为 CI 的种子沉淀下来。

---

## 1. 页面模板（Page Template）

每个子步骤 `.md` 必须包含以下章节（顺序固定）：

1. **标题** `## N-NN <中文标题> · <English Title>`
2. **目标 / Goal** — 这一步要达成什么（一句话 + 3~5 条要点）。
3. **范围 / Scope** — 新建/修改哪些 crate、文件、表；明确**不做**什么（与 MVP 不做项对齐）。
4. **前置 / Prerequisites** — 依赖的上一步产物、工具链版本、外部服务。
5. **步骤 / Steps** — 有序、可执行。每条尽量给具体命令或 Rust 代码片段；引用 fork 源的 Go 文件时用相对路径 `internal/...`（原文见根提交 `dda4e6f2`）。
6. **验收 / Acceptance** — 可机器校验的清单（命令 + 预期输出/退出码）。这是 CI 的核心输入。
7. **与 fork 源对照 / Mapping to the fork source** — 指出对应的上游 Go 文件与被改写的逻辑。**对照基准**：Go 源码已于收口后从工作树移除，引用以 fork 根提交 `dda4e6f2` 为准（`git show dda4e6f2:<path>` 查原文）。
8. **CI 提示 / CI notes** — 这一步将来在 CI 里对应哪条 job/命令；**只描述，不落地 YAML**。
9. **风险 / Risks** — 已知坑、需要 P0 验证的假设。

阶段目录下的 `index.md` 额外包含：阶段目标、子步骤清单（带链接）、阶段级验收。

---

## 2. Phlogo 现状速查（Phlogo Current-State Facts）

> 以下均来自对仓库 `S:\apps\llm\agentwork\repos\Phlogo` 的实际读取，是重构的「从哪来」基准。

### 2.1 技术栈（Tech stack）

| 项 | 现状（Phlogo / paily-core） | 重构目标（Rust） |
|---|---|---|
| 语言/运行时 | Go 1.24，单体 `cmd/server/main.go` | Rust + Cargo workspace，core-api / core-worker 两二进制 |
| HTTP | Gin | Axum |
| 异步 | goroutine + 手动 mutex | Tokio |
| ORM | GORM（`gorm.io/gorm`，同时带 postgres+sqlite 驱动） | SQLx（仅 PostgreSQL） |
| API 契约 | `oapi-codegen` v2.6.0 从 `api/openapi/spec.yaml` 生成 `internal/api/generated/server.gen.go` | utoipa 生成 OpenAPI，再生成 TS client |
| 表达式引擎 | `expr-lang/expr`（筛选、命名模板、sponsor 匹配） | 自研 Typed Filter AST（MVP 白名单字段/运算符） |
| 日志 | zerolog | tracing |
| 配置 | Viper，`PAILY_` 前缀环境变量覆盖 | Figment 或轻量自建 |
| 认证 | `golang-jwt/v5` + 自定义 bearer 中间件 | jsonwebtoken / OIDC |
| UUID / 加密 | `google/uuid`、`golang.org/x/crypto` | `uuid`、`rustcrypto` |
| 外部格式插件 | `openpaily/paily-fetch/format`（clash/singbox/base64） | VProxy Export 或自研 Mihomo/Sing-box 导出 |
| 许可证 | AGPL-3.0（继承 paily-core） | 同上游义务，需单独核 MiaoSpeed/Mihomo/VProxy |

### 2.2 进程与依赖装配（Composition root）

`cmd/server/main.go` 在单进程内装配：
`node.Service` → `node.UpsertService` → `source.Service` → `fetcher.Handler`
→ `scoring.Service` → `cache.Cache`（`aliveCache`）→ `checker.Handler` → `sponsor.Service`。
另起 goroutine：`cleanup.Start(...)`、初始 `aliveCache.TriggerRebuild(...)`、格式配置 seed。

**关键差异**：Phlogo 是单体，fetch 由独立 `paily-fetch` 服务回调 `POST /api/v1/fetch/*`，
check 由独立 `paily-check` 服务回调 `POST /api/v1/check/*`。Rust 方案把 fetch 收编为 core-worker 内的
Fetch Worker，把 check 收编为通过 **MiaoSpeed WebSocket Adapter** 调用的测活（替代 paily-check）。

### 2.3 数据模型（GORM models → 目标表）

来源 `internal/db/model/*.go`。GORM 自动复数化表名。

| 现有表（Phlogo） | 关键字段 | 对应方案表（plan §5.1） | 重构注意 |
|---|---|---|---|
| `nodes` | `id uuid PK`, `hash uniqueIndex`(sha256(type\|server:port\|auth) 前16字节), `server`, `password`, `protocol`, `raw`(clash JSON), `region`, `alive bool`, `score float64[0,1]` | `nodes` + 新增 `node_payloads`、`node_status`、`node_scores` | 拆分：payload/凭据独立加密存储；健康与评分改为按 `checker_tag` 的快照表 |
| `node_sources` | `node_id`,`source_id` 复合 PK（**无时间戳**） | `node_sources` | 补 `first_seen_at`/`last_seen_at` |
| `node_initial_checks` | `node_id`,`check_run_id`,`checker_tag`,`latency_ms`(-1=dead),`checked_at` | 并入 `node_check_results` | 合并初筛/复筛为统一结果表，带 `result_status` |
| `node_deep_checks` | `avg_latency_ms`,`jitter_ms`,`speed_kbps`,`streaming_result`(JSON),`metadata`(JSON) | 同上 | JSON 字段改为强类型 `metrics` |
| `checker_runs` | `id uuid`,`checker_tag`,`node_count`,`completed_at` | `check_runs` | 加 `external_job_id`、`status` |
| `node_tag_scores` | `node_id`,`checker_tag` 复合 PK, `score`, `updated_at` | `node_scores` | 加 `components`/`calculated_at`/版本 |
| `sources` | `id`,`type`(subscribe\|node),`identifier`,`info`,`content`,`status`(active\|dead),`ignore_dead` | `sources` | 加 `fetch_policy`(Replace/Accumulate)、`created_at` 已有 |
| `source_fetch_histories` | `source_id`,`success`,`node_count`,`fetched_at` | `source_fetch_runs` | 加 `status`/`started_at`/`finished_at`/`counts`/`error` |
| `configs` | KV：`key`,`value`,`updated_at` | （运行时配置，可保留或迁入 config crate） | 见 2.6 默认键 |
| `format_configs` | `format_name`(clash\|singbox\|base64),`config` JSON | `subscription_profiles` + `subscription_tokens` | 订阅筛选/授权拆出 |
| `sponsors` | `name`,`text`,`filter_expr`,`priority` | （命名/筛选规则，可保留） | 迁移到 Filter AST |

### 2.4 节点指纹（Node fingerprint）— `internal/node/upsert.go`

```
canonical = "type|server|port_int|auth_credential"
digest    = hex(sha256(canonical)[:16])   // 32-char hex
```
- `nodeAuth` 按协议取主凭据：vmess/vless→`uuid`；trojan/anytls/hysteria/hysteria2→`password`；
  tuic→`uuid:password`(V5)或`password`(V4)；ss/ssr→`password`；wireguard→`private-key`；
  socks5/http/mieru→`username:password`；snell→`psk`；sudoku→`key`。
- `normalizeCredential` 对凭据做**反复 percent-decode** 直到稳定（防止多层 %25 转义产生不同 hash）。
- 重构要点：保留算法可解释性，但改为带 `fingerprint_version` 的 `NormalizedNode` 身份计算（方案 §7）；
  名称**不**参与指纹，凭据参与。

### 2.5 评分公式（Scoring）— `internal/scoring/scoring.go` + `docs/scoring.md`

对（节点, checker_tag）独立计算，取最近 20 条 deep check（最新 i=0）。权重 `w_i = alpha^i`，`alpha` 默认 0.7。

```
加权延迟 = Σ(w_i·lat·i) / Σw_i          # 不可达 lat/jitter 按 3000ms/1000ms 计
加权抖动 = Σ(w_i·jit_i) / Σw_i
latencyX = clamp(加权延迟/3000, 0,1)
延迟分   = 1 - ln(1+k·latencyX)/ln(1+k)      # k=score_latency_curve_k 默认 0.3
jitterX  = clamp(加权抖动/1000, 0,1)
t        = clamp(score_jitter_linear_threshold_ms/1000,0,1)   # 默认 200ms→0.2
稳定性分 = (jitterX<=t) ? 1-jitterX : (1-t)·sqrt((1-jitterX)/(1-t))
质量分   = 延迟分·wLat + 稳定性分·wStab      # wLat=0.7, wStab=0.3（归一后）
速度分   = sqrt(clamp(speed_kbps/163840,0,1)) # 仅最新一条有速度时参与
总分     = 质量分·(1-wSpeed) + 速度分·wSpeed  # wSpeed=0.55
节点.score = AVG(所有 tag 的 score>0)         # 跨 tag 算术平均
结果保留 6 位小数
```
等级（仅展示/命名用）：A≥0.75, B≥0.50, C≥0.25, 否则 D。
参数在内存缓存最多 ~1 分钟（TTL=1min），改参数后需等新复筛才生效。
**重构**：把公式、单位、缺失值处理、样本量门槛、版本号写入 `Scoring` crate 文档；先复现此基线再迭代。

### 2.6 配置默认值（Config KV）— `internal/db/model/config.go`

`configs` 表默认键（CI 与迁移 seed 必须包含）：
`max_distribute=200`、`dead_latency_ms=3000`、`fetch_dead_window_days=7`、
`node_dead_window_days=4`、`fetch_history_window_days=7`、
`score_decay_alpha=0.7`、`score_weight_latency=0.7`、`score_weight_stability=0.3`、
`score_weight_speed=0.55`、`score_latency_curve_k=0.3`、`score_jitter_linear_threshold_ms=200`、
`grade_a_min=0.75`、`grade_b_min=0.50`、`grade_c_min=0.25`、
`node_alive_check_window_minutes=240`、`check_run_history_days=7`、`cleanup_interval_minutes=30`、
`default_sponsor_text=""`、`node_distribute_filter=""`、
`node_name_template="(unlock_flags != \"\" ? \"[\" + unlock_flags + \"] \" : \"\") + node.grade + \" - \" + node.region_flag + \" \" + node.region + \" \" + node.seq + (node.sponsor != \"\" ? \" \" + node.sponsor : \"\")"`、
`filename_clash/singbox/base64`。
另：`config.example.yaml` 给出 `server`/`database`/`auth`/`checkers`/`default_preset` 的 YAML 结构，
环境变量用 `PAILY_` 前缀、`_` 替换 `.`（如 `PAILY_SERVER_PORT`）。

### 2.7 测活回调契约（Checker contract）— `internal/checker/handler.go`

- `POST /api/v1/check/nodes`：拉取全部节点（hash/id/protocol/raw/server）给 checker 去测。
- `POST /api/v1/check/results`：`CheckResultsRequest{ results: CheckResult[] }`，
  `CheckResult{ node_id, initial:{latency_ms}, deep?:{avg_latency_ms,jitter_ms,speed_kbps,region,streaming,metadata} }`。
- 处理是**异步**的，按 `checker_tag` 加 per-tag mutex 串行化，四阶段：
  1. 原子事务：批量插入 initial/deep checks + `checker_runs`；
  2. 批量存活判定：`DISTINCT ON (node_id, checker_tag)` 取窗口内（默认 240min）最新 initial，
     任一 checker 报告存活即 `alive=true`（跨 tag 合并）；
  3. 异步评分：对每个有 deep 的节点触发 `ComputeAndUpdateTagScore`；
  4. 死源评估 + 触发分发缓存重建 `cache.TriggerRebuild`。
- 认证：`CheckerTagMiddleware` 用 checker secret 校验并注入 `checker_tag`。
- **重构**：Rust 侧用 `Checker` trait（`submit`/`collect`/`status`）抽象，Adapter 把 MiaoSpeed WebSocket
  协议映射到该 trait（方案 §8）。迟到结果不能覆盖更新的状态快照（见 2.9）。

### 2.8 分发与选节点（Distribution）— `internal/distribution/selector.go` + `docs/distribution.md`

- 公开订阅：`/clash`、`/singbox`、`/base64`（无需认证），文件名来自 `filename_*` 配置。
- 查询参数：`preset`(common\|all\|other)、`tag`(测活视角)、`filter_unlock`(\| 分隔)、`filter_mode`(select\|mark)。
  preset 优先级：`URL preset > tag.default_preset > 全局 default_preset > common`。
- 选节点算法（**核心，必须忠实移植/重写**）：
  - `common`：HK/TW/SG/US/JP 优先；按活跃地区平分配额；超额地区**均匀随机抽样**避免集中高分；
    不足时从其他常用地区随机补→按评分补其他地区→补 UN。
  - `all`：非 UN 按地区节点数比例分配配额；池≤配额全取；池≤1.5×配额取 top-N 评分；
    池>1.5×配额用 **Efraimidis-Spirakis A-Res 加权抽样**（权重 `max(score,0.01)`）。
  - `other`：排除 common 地区，其余同 `all`。
  - 输出按命名地区顺序排序，同地区内按评分降序编号 `seq`（如 `hk01`）。
- 命名：`internal/naming/naming.go` 用 expr 模板 + `flags.json`（ISO→emoji/全称）+ 等级。
- **重构**：Filter AST 替代 expr（方案 §9.2）；Mihomo/Sing-box 输出复用或自研（方案 §10）；
  新增 `subscription_profiles`（筛选 AST/地区/健康/最低分/上限/排序/命名）与 `subscription_tokens`（仅存哈希、可撤销）。

### 2.9 来源与节点生命周期（Lifecycle）— `internal/source/*.go` + `internal/node/upsert.go`

- Source 去重：`Create` 按 `type+content` 去重；状态 `active`/`dead`；`ignore_dead` 标志。
- 死源判定：`CheckFetchDead`/`CheckNodeDead`（基于 `fetch_dead_window_days`/`node_dead_window_days`，
  连续失败/无存活节点超过窗口即判死）；`MarkDeadAndClean` 用条件 UPDATE（`status != 'dead'`）防并发重复清理。
- 节点 upsert（`UpsertFromFetchResult`）：
  - 失败 → no-op（保留现有节点）；
  - 成功但空 → 删除该来源全部映射并清理孤儿节点；
  - 成功有节点 → 按 hash upsert 节点 + 确保 source→node 映射（事务内 SELECT-then-INSERT 防并发重复）；
    移除本来源已消失节点的映射；孤儿节点（`NOT EXISTS node_sources`）连带删除其测活历史后物理删除。
  - **死源保护**：source 已 dead 时拒绝提交，避免复活僵尸映射。
- **重构**：方案 §6 要求明确的 `SourceStatus`(Active/Suspended/Dead)、`FetchPolicy`(Replace/Accumulate)、
  `NodeHealth`(Unknown/Alive/Degraded/Dead)、`NodeMembership`(Linked/Orphaned/Retired)，
  所有关联清理在事务内完成，迟到/重试幂等。Phlogo 当前没有 Replace/Accumulate 区分，也没有 Orphaned/Retired 状态——
  这是 P1/P2/P4 的重点新增。

### 2.10 API 面（27 paths，来自 `api/openapi/spec.yaml`）

`/api/v1/auth/login`、`/auth/refresh`、`/fetch/sources`、`/fetch/results`、
`/check/nodes`、`/check/results`、`/sources`、`/sources/search`、`/sources/{id}`、
`/sources/{id}/nodes`、`/sources/{id}/ignore-dead`、`/checkers`、`/nodes`、`/nodes/search`、
`/nodes/{id}`、`/nodes/{id}/checks/initial`、`/nodes/{id}/checks/deep`、`/filter/preview`、
`/sponsors`、`/sponsors/{id}`、`/stats`、`/config`、`/logs/fetch`、`/logs/check`、
`/format-configs`、`/format-configs/{format}`、`/{format}`（订阅）。
认证分层：公开(`/health`,`/:format`,`/api/v1/auth/*`)；`/check/*`→checker secret；
`/fetch/*`→service bearer；`POST /sources`→admin-or-service；其余→admin JWT。

### 2.11 许可证与依赖（License & deps）

- Phlogo 继承 paily-core 的 **AGPL-3.0**。重构若继续分发，AGPL 义务延续。
- 上游需单独核许可证：MiaoSpeed（AGPL）、Mihomo（GPL）、VProxy（MIT，可复用但保留版权）、
  paily-fetch/paily-check（AGPL，仅参考架构、勿直接移植代码）。
- `go.mod` 间接依赖里有 `metacubex/mihomo`、`enfein/mieru` 等大量代理协议库——Rust 侧**不**链接这些，
  测活交给独立 MiaoSpeed 二进制（进程边界，不是许可证豁免）。
- 供应链：固定上游 commit/tag；保存二进制 SHA-256、构建信息、SBOM；用 `cargo-deny` 查许可证与安全公告。

---

## 3. Phlogo → Rust 组件映射（Component Mapping）

| Phlogo（Go） | Rust crate / 模块 | 说明 |
|---|---|---|
| `cmd/server/main.go` | `apps/api/src/main.rs` + `apps/worker/src/main.rs` | 拆分单体为 api/worker |
| `internal/api/**` + `api/openapi/spec.yaml` | `api-contract` + `apps/api` | utoipa 替代 oapi-codegen |
| `internal/auth/**` | `api-contract` / `config` 内 auth 模块 | JWT + service bearer + checker secret |
| `internal/db/**` + `internal/db/model/**` | `database` + `domain` + `migrations/` | SQLx + 迁移，替代 GORM |
| `internal/source/**` | `registry` + `lifecycle` | 来源生命周期/去重/死源 |
| `internal/node/**` | `registry` + `lifecycle` | 节点实体/去重/映射/孤儿清理 |
| `internal/fetcher/**` | `fetch` | 收编为 worker 内 Fetch Worker |
| `internal/checker/**` | `checker-miaospeed`（Adapter）+ `worker` Job | 替代 paily-check，走 MiaoSpeed WS |
| `internal/scoring/**` | `scoring` | EWMA 评分（复现基线） |
| `internal/filter/**` + `internal/naming/**` | `filter` + `distribution` 内命名 | Typed AST 替代 expr |
| `internal/distribution/**` + `internal/templates/**` | `distribution` | 选节点 + 格式输出 |
| `internal/cache/**` | `distribution` / `registry` 内缓存 | 分发缓存重建 |
| `internal/cleanup/**` | `lifecycle` / `worker` 清理 Job | 孤儿/过期清理 |
| `internal/config/**` + `config.example.yaml` | `config` | Figment 或轻量自建 |
| `internal/migrate/**` | `migrations/` + `database` | SQLx migrate 替代 GORM AutoMigrate |
| `internal/sponsor/**` | `filter` / `distribution` | sponsor 匹配 → Filter AST |
| `docs/*.md`（design intent） | `docs/rebuild/` + 冻结规格文件 | 见方案 §18 |

---

## 4. 与原方案的关键差异（Divergences to resolve in P0）

1. **paily-fetch → VProxy**：Phlogo 用 `openpaily/paily-fetch/format` 插件做格式解析/导出；
   方案要求改用 VProxy Parser/Fingerprint/Export（P0 验证能否轻依赖独立编译）。
2. **paily-check → MiaoSpeed**：Phlogo 由独立 paily-check 通过 HTTP 回调；方案改为 core-worker 经
   MiaoSpeed WebSocket Adapter 直接测活。
3. **GORM → SQLx**：放弃 SQLite 支持，仅 PostgreSQL；模型从 struct tag 改为手写 migration + `sqlx::FromRow`。
4. **expr-lang → Typed Filter AST**：筛选/命名/sponsor 从动态 expr 改为白名单字段+运算符的结构化 AST（方案 §9.2）。
5. **oapi-codegen → utoipa**：API 契约生成方式变更，但 `spec.yaml` 仍是契约来源（先翻译为 OpenAPI 再 utoipa）。
6. **单体 → 多进程**：fetch/check 从外部服务回调收编为 core-worker 内的 Job（Apalis 调度）。

P0 必须验证：VProxy 可独立编译、MiaoSpeed 固定版本 WebSocket 协议、Apalis PG backend 行为、Vben 固定版本构建。

---

## 5. CI 就绪说明（CI-readiness guidance）

- 每个页面「CI 提示」只描述**将来 CI 对应哪条命令**，不落地任何 YAML/Workflow。
- 推荐 CI 命令种子（后续据此写 job）：
  - 构建：`cargo build --workspace --all-targets`
  - 单测：`cargo test --workspace`
  - 格式：`cargo fmt --all -- --check`
  - Lint：`cargo clippy --workspace --all-targets -- -D warnings`
  - 安全审计：`cargo audit`；许可证：`cargo deny check licenses`
  - 迁移检查：`sqlx migrate run --check` / `cargo sqlx prepare`（离线查询校验）
  - 集成：`cargo test --test integration`（需要 Postgres + 可选 MiaoSpeed stub）
- 验收标准尽量写成「命令 + 退出码/输出」，便于直接变成 `run:`，但**本目录不含 CI 文件**，由用户后续照此编写。

---

## 6. 目录结构（This doc set layout）

```
docs/rebuild/
├── CONVENTIONS.md            # 本文件（事实来源 + 约定）
├── 00-index.md               # 总览 + 阶段地图 + 如何使用 + 与 CI 的关系
├── ACTION-LOG.md             # 总行为清单：每步/每命令 + UTC 时间戳；校机执行环境
├── 01-p0-upstream-verification/
│   ├── index.md
│   ├── p0-01-vproxy.md
│   ├── p0-02-miaospeed.md
│   ├── p0-03-apalis.md
│   ├── p0-04-vben.md
│   └── p0-05-licensing-supply-chain.md
├── 02-p1-core-skeleton-data/
│   ├── index.md
│   ├── p1-01-workspace.md
│   ├── p1-02-api-skeleton.md
│   ├── p1-03-migrations.md
│   └── p1-04-source-node-crud.md
├── 03-p2-fetch-parse/
│   ├── index.md
│   ├── p2-01-fetch-ssrf.md
│   ├── p2-02-vproxy-adapter.md
│   ├── p2-03-fingerprint.md
│   └── p2-04-reconciliation.md
├── 04-p3-check-miaospeed/
│   ├── index.md
│   ├── p3-01-apalis-check-job.md
│   ├── p3-02-ws-adapter.md
│   ├── p3-03-result-mapping.md
│   └── p3-04-history-persistence.md
├── 05-p4-lifecycle-scoring/
│   ├── index.md
│   ├── p4-01-state-transitions.md
│   ├── p4-02-health-orphan.md
│   └── p4-03-scoring-engine.md
├── 06-p5-filter-subscription/
│   ├── index.md
│   ├── p5-01-filter-ast.md
│   ├── p5-02-quota-limit.md
│   └── p5-03-subscription-output.md
├── 07-p6-admin-ui/
│   ├── index.md
│   ├── p6-01-vben-scaffold.md
│   └── p6-02-pages.md
└── 08-p7-productionization/
    ├── index.md
    ├── p7-01-compose-backup.md
    ├── p7-02-observability.md
    └── p7-03-security-hardening.md
```
