## 8-03 安全加固 · Security Hardening

### 目标 / Goal

- **API 限流（rate limiting）**：对管理 API 与订阅拉取按客户端/IP 限流，防止滥用与刷订阅。
- **认证分层强制（auth enforcement）**，镜像 `cmd/server/main.go` 的 dispatch 语义：
  - 公开：`/health`、`/:format`（注意订阅需 token）、`/api/v1/auth/*`；
  - `/api/v1/check/*` → **checker secret**；
  - `/api/v1/fetch/*` → **service bearer**；
  - `POST /api/v1/sources` → admin-or-service；
  - 其余管理路由 → **admin JWT**；
  - 订阅 `/sub/{token}` → **subscription Token**（哈希校验，可撤销，见 CONVENTIONS §2.8/§2.3）。
- **SSRF 防护（fetch）**：限制环回/内网/链路本地/云元数据地址，重定向目标重校验，限响应大小与超时（方案 §16.9 / P2-01）。
- **依赖与 SBOM 扫描**：`cargo-deny` 查许可证、`cargo-audit` 查安全公告；生成并保存 SBOM。

### 范围 / Scope

- **新建**：
  - `crates/api-contract`（或 `crates/config` 内 auth 模块）的 Axum 认证中间件集合：
    `service_bearer` / `admin_jwt` / `admin_or_service` / `checker_secret` / `subscription_token`。
  - 限流中间件（如 `tower-governor` 或自研 `tower` layer），按 route/identity 维度配置。
  - SSRF 守卫（已在 P2-01 实现，本页负责在「生产化」层把它纳入验收与默认开启，不重复实现）。
  - `deny.toml`（`cargo-deny` 配置）、CI 用的 `cargo audit` 步骤、SBOM 生成（`cargo cyclonedx` / `cargo sbom`）。
- **修改**：`apps/api/src/main.rs` 路由装配（从单进程 `main.go` dispatch 翻译为 Axum middleware 嵌套）。
- **不做**：多租户 / 复杂 RBAC / 公开用户注册（方案 §17）；Mihomo 常驻转发服务（§2.2）；自由执行筛选脚本（§17）。

### 前置 / Prerequisites

- P1 已完成认证相关配置（`AUTH__JWT_SECRET`/`SERVICE_SECRET`/`ADMIN_PASSWORD`，沿用 `main.go` 的同名字段）。
- P2-01 已完成 SSRF 防护实现（方案 §16.9）；本页引用而非重写。
- P5 已完成 subscription Token（哈希存储、可撤销，CONVENTIONS §2.3 `subscription_tokens`）。
- 工具：`cargo-deny`、`cargo-audit`、`cargo-cyclonedx`（或等价 SBOM 工具）；可选 `trivy`/`docker scout` 镜像扫描。
- 现状事实：`cmd/server/main.go` 第 117–141 行的 dispatch 是认证分层的**权威来源**；本页翻译它。

### 步骤 / Steps

#### Step 1 — 认证中间件（翻译 `main.go` dispatch）

Phlogo `main.go` 的 dispatch（公开 / `/check/*` / `/fetch/*` / `POST /sources` / 其余 admin JWT）
翻译为 Axum 的 route-level 嵌套 layer：

```rust
// apps/api/src/main.rs (示意：对应 main.go 的 switch dispatch)
use axum::routing::{get, post};
use axum::Router;

let public = Router::new()
    .route("/health", get(health))
    .route("/api/v1/auth/login", post(login))
    .route("/api/v1/auth/refresh", post(refresh));

let checker = Router::new()
    .route("/api/v1/check/nodes", post(check_nodes))
    .route("/api/v1/check/results", post(check_results))
    .layer(checker_secret_layer(cfg.auth.checker_secret.clone()));   // 注入 checker_tag

let fetch = Router::new()
    .route("/api/v1/fetch/sources", post(fetch_sources))
    .route("/api/v1/fetch/results", post(fetch_results))
    .layer(service_bearer_layer(cfg.auth.service_secret.clone()));

let admin = Router::new()
    .route("/api/v1/sources", get(list_sources).post(admin_or_service_layer(create_source)))
    .route("/api/v1/nodes", get(list_nodes))
    .route("/api/v1/checks", get(list_checks))
    // … 其余管理路由
    .layer(admin_jwt_layer(cfg.auth.jwt_secret.clone()));

let subscription = Router::new()
    .route("/sub/{token}.yaml", get(sub_yaml))
    .route("/sub/{token}.base64", get(sub_base64))
    .layer(subscription_token_layer(pool.clone()));   // 哈希校验 + 撤销

Router::new()
    .merge(public)
    .merge(checker)
    .merge(fetch)
    .merge(admin)
    .merge(subscription)
```

对应 `main.go`：`/api/v1/check/*`→`CheckerTagMiddleware(cfg)`；`/api/v1/fetch/*`→`svcMW`；
`POST /api/v1/sources`→`sourceCreateMW`；default→`adminMW`（`CONVENTIONS` §2.10 同口径）。

#### Step 2 — 限流中间件

用 `tower-governor`（基于 `governor` 的令牌桶）按 route + 客户端标识（JWT sub / bearer / IP）限流：

```rust
// 示意：对 admin 路由加 60 req/min/IP，订阅路由加更严格配额
use tower_governor::{GovernorConfigBuilder, GovernorLayer};

let admin_limit = GovernorConfigBuilder::default()
    .per_second(1).burst_size(60).finish().unwrap();
let admin = admin.route_layer(GovernorLayer { config: Arc::new(admin_limit) });
```

阈值作为配置项（放 `config.example.yaml` 的 `server.rate_limit`），默认值在文档中明确，CI 不硬编码。

#### Step 3 — SSRF 防护（引用 P2-01，默认开启）

方案 §16.9：限制环回（`127.0.0.0/8`、`::1`）、内网（`10/8`、`172.16/12`、`192.168/16`、`169.254/16` 链路本地）、
云元数据（`169.254.169.254`）；对重定向目标**重新校验**；限制响应大小与超时。
Rust Fetch Worker 已在 P2-01 实现该守卫；本页要求：
- 守卫默认开启，且不能通过普通配置绕过；
- 提供单元测试覆盖「请求环回/内网/元数据地址被拒」「重定向到内网被二次拦截」。

#### Step 4 — 依赖与 SBOM 扫描

`deny.toml`（放仓库根，供 `cargo-deny` 读取）：

```toml
# deny.toml (示意)
[advisories]
version = 2
yanked = "deny"

[licenses]
version = 2
allow = [
    "MIT", "Apache-2.0", "BSD-2-Clause", "BSD-3-Clause",
    "ISC", "MPL-2.0", "Unicode-DFS-2016", "Zlib",
]
# AGPL/GPL 需显式评估（见 §2.11 / 方案 §16）；未评估前列入 deny 或 warn 并人工复核
confidence-threshold = 0.8

[bans]
multiple-versions = "warn"
wildcards = "deny"

[sources]
unknown-registry = "deny"
unknown-git = "warn"
```

生成 SBOM 并保存（方案 §16.6「保存二进制 SHA-256、构建信息、SBOM」）：

```bash
# SBOM（CycloneDX JSON）
cargo cyclonedx --format json -o deploy/sbom.cdx.json
# 二进制哈希（发布产物）
sha256sum target/release/core-api target/release/core-worker
```

### 验收 / Acceptance

1. **未授权管理接口被拒（阶段验收 #3 细化）**：
   ```bash
   curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:21380/api/v1/sources
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/api/v1/nodes
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/api/v1/checks
   ```
   预期：三条均返回 `401` 或 `403`（无 Authorization）。

2. **checker/fetch 端点校验 secret**：
   ```bash
   # 无 checker secret
   curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:21380/api/v1/check/nodes
   # 无 service bearer
   curl -s -o /dev/null -w "%{http_code}\n" -X POST http://127.0.0.1:21380/api/v1/fetch/sources
   ```
   预期：均 `401`/`403`；带上正确 secret/bearer 后 `2xx`。

3. **订阅 token 校验（撤销即失效）**：
   ```bash
   # 有效 token
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/sub/<VALID_TOKEN>.yaml
   # 伪造/已撤销 token
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/sub/<REVOKED_TOKEN>.yaml
   ```
   预期：有效 `200`，撤销/`REVOKED` `401`/`403`（subscription_tokens 表 `status`/`revoked_at` 见 §2.3）。

4. **SSRF 测试通过（引用 P2-01）**：
   ```bash
   cargo test -p fetch ssrf
   ```
   预期：退出码 0；环回/内网/链路本地/元数据地址请求被拒，重定向到内网被二次拦截。

5. **依赖/供应链扫描通过**：
   ```bash
   cargo deny check licenses
   cargo deny check advisories
   cargo audit
   ```
   预期：三条命令退出码均为 `0`（`cargo deny` 无 license/advisory 违例；`cargo audit` 无未忽略公告）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `cmd/server/main.go` 第 113–141 行（`ServiceBearerMiddleware`/`AdminJWTMiddleware`/`AdminOrServiceMiddleware`
  + `CheckerTagMiddleware` 的 dispatch）→ Step 1 的 Axum 认证中间件集合（CONVENTIONS §3 映射
  `internal/auth/**` → `api-contract`/`config` 内 auth 模块）。
- `internal/checker/handler.go` 的 `CheckerTagMiddleware(cfg)` 注入 `checker_tag` → Step 1 `checker_secret_layer`
  （CONVENTIONS §2.7「认证：CheckerTagMiddleware 用 checker secret 校验并注入 checker_tag」）。
- 旧无 SSRF 约束（fetch 由外部 `paily-fetch` 回调，`main.go` 仅做 service bearer）→ Step 3 引用 P2-01 的
  Rust Fetch Worker 守卫（方案 §16.9）。
- 旧无依赖/SBOM 扫描 → Step 4 引入 `cargo-deny` + `cargo-audit` + SBOM，对齐 CONVENTIONS §2.11 与方案 §16.6/§16.7。

### CI 提示 / CI notes

- **auth/限流 gate**：把验收 #1–#3 的 curl 断言做成「部署冒烟」job 的一部分（与 8-01 的 `docker compose up` 联动），
  在 merge gate 跑，确保认证分层不被误改宽松。
- **SSRF gate**：`cargo test -p fetch ssrf`（P2-01）作为常规单元测试 job，每次 push 必跑。
- **supply-chain gate**：`cargo deny check licenses` 与 `cargo audit`（可加 `--deny warnings`）在
  merge/release 前必须零异常退出；`deny.toml` 的 license 白名单变更需人工 review（AGPL/GPL 尤其）。
- **SBOM 产物**：release job 生成 `deploy/sbom.cdx.json` 并归档，同时记录 `core-api`/`core-worker` 二进制 SHA-256；
  可与 8-01 的镜像漏洞扫描（trivy/docker scout）合并为「supply-chain」job。

### 风险 / Risks

- **认证回归**：Axum middleware 嵌套层级若配错，可能把管理路由误设为公开（直接违反阶段验收 #3）；
  必须用 #1–#3 的自动化断言守住，而非人工 review。
- **限流误伤**：过严的 `burst_size` 会拖垮正常管理操作；阈值走配置且默认值在文档明确，CI 不硬编码。
- **SSRF 绕过**：仅校验首跳不够，必须校验重定向目标（§16.9「对重定向目标重新校验」）；若 P2-01 未覆盖重定向二次校验，本页验收会暴露缺口。
- **许可证误判（重申）**：进程隔离 `miaospeed`（AGPL）/Mihomo（GPL）/VProxy（MIT）**不是许可证豁免**
  （CONVENTIONS §2.11、方案 §16.1/§16.2/§16.3）。`cargo-deny` 的 `licenses.allow` 不能把 AGPL/GPL 静默放行；
  分发时仍须履行对应义务（附源码/许可证文本）。
- **subscription token 哈希**：`subscription_tokens` 只存哈希（§2.3）；校验路径若误存明文或比对明文，既违反设计也增加泄露面，
  验收 #3 必须走哈希校验 + 撤销逻辑。
