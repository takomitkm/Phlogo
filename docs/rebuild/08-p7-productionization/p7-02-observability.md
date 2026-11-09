## 8-02 可观测性 · Observability

### 目标 / Goal

- 用 **tracing** 替换 Phlogo 现有 **zerolog**（CONVENTIONS §2.1：`日志 | zerolog → tracing`），输出结构化日志，
  便于集中采集与检索。
- **日志永不记录秘密**：节点原始凭据（密码/UUID/`raw` clash JSON 中的 `psk`/`private-key`/`password`）、
  订阅 URL 中的 token、subscription Token、checker secret、MiaoSpeed secret 一律 redact（方案 §16.8）。
- 提供 **metrics 端点**（如 Prometheus `/metrics`），暴露请求数、延迟、存活率、Job 吞吐等。
- 保证 **worker 重启后 Apalis 任务恢复**：进行中的 fetch/check/score/cleanup Job 由 PostgreSQL-backed Apalis
  持久化，worker 重启后能被重新认领并继续（方案 §3 Apalis、§6.4 幂等要求）。

### 范围 / Scope

- **新建**：
  - `crates/observability/`（或合并进 `config`/`api-contract`）封装 tracing 初始化 + 全局 redaction layer。
  - API 中间件：`tracing` request span + 结构化字段；metrics middleware。
  - `crates/worker` 内 Apalis `Monitor` 配置（Redis/Postgres backend 已用 Postgres，见 P3）。
  - 集成测试：日志 redaction 断言、metrics 端点断言、任务恢复断言。
- **修改**：`apps/api/src/main.rs`、`apps/worker/src/main.rs`（替换 zerolog、接 tracing）。
- **不做**：自建 APM 后端；把 Apalis 任务状态机与 Node Lifecycle 状态混用（方案 §3 明确二者严格分开）；
  任意 SQL 查询日志化（避免把 `raw`/凭据顺手写进慢查询日志）。

### 前置 / Prerequisites

- P1 已完成：`migrations/` + SQLx，Postgres 为唯一数据层（CONVENTIONS §2.3）。
- P3 已完成：Apalis PostgreSQL backend 已建官方迁移表（方案 §3「以固定版本 Apalis 官方 migration 为准」）。
- P2-01 已完成：Fetch Worker 的 SSRF 防护已落地（方案 §16.9）—— 本页只接管「日志脱敏」与「指标/恢复」，
  不重复 SSRF 实现。
- 依赖：`tracing`、`tracing-subscriber`、`opentelemetry`(可选)、`metrics` / `axum-prometheus`。
- 现状事实：`internal/checker/handler.go` 的 `GetNodes` 把 `node.Raw`（clash JSON，含凭据）原样序列化返回给 checker；
  `main.go` 的认证分层（service bearer / admin JWT / checker secret）是 secret 出现的另一处上下文。

### 步骤 / Steps

#### Step 1 — tracing 初始化（替换 zerolog）

Phlogo：`main.go` 用 `log.Logger = log.Output(zerolog.ConsoleWriter{...}).Level(zerolog.InfoLevel)`。
Rust 侧在 `apps/api` 与 `apps/worker` 启动处统一装配 `tracing-subscriber`，
输出 JSON 而非 human 文本，便于采集：

```rust
// crates/observability/src/lib.rs (示意)
use tracing_subscriber::{fmt, EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

pub fn init(service: &str) {
    let fmt_layer = fmt::layer()
        .json()                       // 结构化 JSON 日志
        .with_current_span(true)
        .with_span_list(false);
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::registry()
        .with(filter)
        .with(fmt_layer)
        .init();
    tracing::info!(service, "observability initialized");
}
```

#### Step 2 — 全局 redaction layer（secret 不得入日志）

方案 §16.8：「不记录节点原始凭据、订阅 URL 中的秘密、订阅 Token 和 MiaoSpeed Secret」。
做法：在**序列化层统一兜底**，不依赖各 handler 手动脱敏（index.md 风险已强调 `check/nodes` 的 `raw` 含凭据）。

```rust
// crates/observability/src/redact.rs (示意)
pub fn redact_secret(value: &str) -> String {
    if value.trim().is_empty() { return value.to_string(); }
    // 凭据/令牌一律替换为固定掩码；保留长度上限便于排障但不暴露内容
    "[REDACTED]".to_string()
}

/// 对订阅 URL 仅保留 host/path，剥离 query 中的 token
pub fn redact_subscription_url(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => { u.set_query(None); u.set_fragment(None); u.to_string() },
        Err(_) => "[REDACTED-URL]".to_string(),
    }
}
```

强制约束（写进 crate 文档）：任何进入 `tracing::info!`/`warn!`/`error!` 的字段，
若字段名命中 `{secret, password, token, psk, private_key, raw, url}` 等白名单，必须经过 `redact_*`，
否则在 `tracing` 的 `on_event` 自定义 layer 中拦截并替换为 `[REDACTED]`。
特别地，`internal/checker/handler.go::GetNodes` 对应的 Rust 版「向 checker 推送节点」逻辑，
其 `raw` 字段在日志里只允许记录 `hash`/`protocol`/`server`，不得记录 `raw` 原文。

#### Step 3 — metrics 端点

在 `apps/api` 暴露 Prometheus 指标（沿用对外端口，路径 `/metrics`；也可走 Caddy 反代）：

```rust
// apps/api/src/main.rs (示意)
use axum::Router;
use axum_prometheus::PrometheusMetricLayer;

let (prom_layer, metric_handle) = PrometheusMetricLayer::pair();
let app = Router::new()
    .route("/health", get(health))
    .route("/metrics", get_service(metric_handle))   // 指标端点
    .layer(prom_layer);
```

建议初始指标：`http_requests_total{route,status}`、`http_request_duration_seconds`、
`nodes_total`、`nodes_alive`、`sources_total`、`apalis_jobs_enqueued_total{job}`、
`apalis_jobs_finished_total{job,status}`、`check_runs_total{checker_tag}`（表名与字段见 CONVENTIONS §2.3）。

#### Step 4 — Apalis 任务恢复（worker 重启接管）

Phlogo 单进程用 goroutine + `aliveCache.TriggerRebuild`；Rust 把 fetch/check/score/cleanup 收编为 Apalis Job。
Postgres-backed Apalis 把 Job 状态落库，worker 崩溃/重启后由新 worker 认领未完成 Job：

```rust
// apps/worker/src/main.rs (示意)
use apalis::prelude::*;
use apalis::postgres::PostgresStorage;

let pg = PostgresStorage::new(pool.clone()).await?;
// 确保官方 migration 已执行（P3）；WorkerBuilder 自动认领 Pending/Running 任务
Monitor::new()
    .register_with_count(2, move |_| FetchJob::worker(pg.clone()))
    .register_with_count(2, move |_| CheckJob::worker(pg.clone()))
    .run()
    .await?;
```

要求：每个 Job 的 `perform` **幂等**（方案 §6.4：旧 run 不覆盖新 run、迟到测活结果不覆盖新状态、重试不产生重复关联）。
`CheckJob` 对应 `internal/checker/handler.go` 的四阶段（事务插入 → 批量存活判定 → 异步评分 → 死源评估+缓存重建），
其中「迟到结果不能覆盖更新的状态快照」由 `node_status` 按 `node_id+checker_tag` 维护（CONVENTIONS §2.7/§2.9）。

### 验收 / Acceptance

1. **redaction 单元测试（最核心，对应阶段验收 #2）**：
   ```bash
   cargo test -p observability redact
   ```
   预期：`redact_secret("vmess-uuid-xxxx")` == `"[REDACTED]"`；
   `redact_subscription_url("https://sub.example.com/abc?token=SECRET")` == `"https://sub.example.com/abc"`；
   且有一个「`tracing` on_event 拦截命中 secret 字段名即替换」的集成断言，退出码 0。

2. **运行期日志无秘密（端到端）**：
   ```bash
   # 起栈（见 8-01），跑一次 fetch + check
   docker compose up -d
   # 触发抓取与测活（具体命令依赖 P1/P3 的管理 API，此处只校验日志）
   docker compose logs api worker 2>&1 | grep -Ei 'password=|psk=|private-key|vmess://|token=|secret='
   echo "exit=$?"
   ```
   预期：`grep` 退出码 `1`（未命中任何秘密模式）即通过；若命中则阶段验收 #2 失败。

3. **metrics 端点响应**：
   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/metrics
   curl -s http://127.0.0.1:21380/metrics | grep -c 'http_requests_total'
   ```
   预期：首条返回 `200`；第二条计数 `>= 1`（指标已暴露）。

4. **worker 重启任务恢复**：
   ```bash
   # 入队一个长 check/fetch Job 后，强制重启 worker
   docker compose exec worker sh -c 'kill -TERM 1'   # 或 docker compose restart worker
   docker compose logs worker 2>&1 | grep -Ei 're-enqueue|resume|ack|job .* started'
   ```
   预期：日志显示 worker 重启后重新认领（resume/enqueue）了先前未完成的 Job，且最终 `check_runs` 行数等于入队次数
   （无重复计数，证明幂等），`SELECT count(*) FROM check_runs;` 不自增产生重复。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `main.go` 第 44 行 `log.Output(zerolog.ConsoleWriter...)` → 本页 Step 1 的 `tracing-subscriber` JSON 初始化。
- `internal/checker/handler.go`：多处 `log.Info()/Warn()/Error()`（如 `PostResults` 的 `run_id`、`result.count`）
  与 `GetNodes` 把 `raw` 返回给 checker——其中 `raw` 含 credentials（CONVENTIONS §5.3 第 5 条「凭据加密存储，日志不得输出原文」）。
  Rust 版在 Step 2 用 redaction layer 兜底，保证 `raw`/`password`/`psk`/`private-key` 不落日志。
- `main.go` 认证分层（`ServiceBearerMiddleware`/`AdminJWTMiddleware`/`CheckerTagMiddleware`）携带的
  checker secret / service secret 是另一处 secret 上下文，同样走 Step 2 redact。
- 单进程 goroutine（`cleanup.Start`、`aliveCache.TriggerRebuild`）→ Apalis Job（Step 4），任务状态持久化到 Postgres，
  实现「重启恢复」（CONVENTIONS §3 映射 `internal/cleanup/**` → `lifecycle`/`worker` 清理 Job）。

### CI 提示 / CI notes

- **单元/集成测试 gate**：`cargo test --workspace` 必须包含 Step 2 的 redaction 断言与 Step 3 的 metrics 断言；
  其中「日志无 secret」建议做成 `cargo test --test integration` 的独立用例（跑真实 fetch+check 后 grep 日志）。
- **日志 redaction 静态检查（可选）**：CI 可加一个轻量 lint，扫描源码中 `tracing::` 调用是否对 secret 类字段名
  直接透传（warning），作为人工 review 之外的兜底。
- **任务恢复演练**：在集成测试环境起 worker → 注入未完成 Job → `kill -TERM` worker → 重启 → 断言任务被接管且不重复。
  该用例可并入 8-01 的重启一致性演练或独立 nightly job。
- **指标采集冒烟**：CI 内 `curl /metrics` 断言 200 与关键指标存在，防止 metrics 中间件被误删。

### 风险 / Risks

- **redaction 漏网**：若某 handler 把 `raw`/凭据作为 `Debug`/默认字段打印，绕过字段名白名单则会漏；
  必须在 `tracing` `on_event` 层做全局兜底，而不能只靠约定（index.md 已列为风险）。
- **Apalis 非幂等重放**：worker 重启后重放未完成 Job，若 Job `perform` 不幂等会重复创建 `node_sources` 关联
  或重复触发清理（方案 §6.4）—— 必须与 P3/P4 的幂等约束同步完成，否则「恢复」反而制造脏数据。
- **指标端点暴露面**：`/metrics` 若被 Caddy 反代到公网会泄露节点规模/拓扑；应按 8-03 在边界做访问控制或仅内部暴露。
- **MiaoSpeed secret 在 WebSocket 建连日志**：worker 与 MiaoSpeed 握手的 secret 也算 secret（§16.8），
  redaction layer 必须覆盖 WebSocket 客户端日志，不能只覆盖 HTTP 层。
- **Postgres 任务表与业务表耦合**：`pg_dump` 会连带备份 `apalis` 内部表；恢复「运行中快照」时，
  重放的 `Running` 任务依赖 worker 幂等接管（与 8-01 风险呼应）。
