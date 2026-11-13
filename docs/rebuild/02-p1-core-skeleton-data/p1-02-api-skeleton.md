## 2-02 API 骨架与分层认证 · Axum API Skeleton & Layered Auth

### 目标 / Goal

- 在 `apps/curse` 立起 Axum + `tracing` + Figment 配置骨架，端口/配置形状对齐 `config.example.yaml`。
- 复刻 `cmd/server/main.go` 的**分层认证分发**逻辑（`/health`、`/auth/*` 公开；`/check/*` checker secret；`/fetch/*` service bearer；`POST /sources` admin-or-service；其余 admin JWT）。
- 决策：**OpenAPI 仍是唯一契约**（`api/openapi/spec.yaml` 保留），本阶段用 utoipa 描述 P1 涉及的 schema/paths，并挂载 `utoipa-swagger-ui`；既有 YAML 可作为 TS client 生成源（CONVENTIONS §2.1、§2.10）。
- `GET /health` 返回 200；未授权访问受保护路由返回 401；未知路径返回 404；`cargo test` 通过。

### 范围 / Scope

**新建 / 改写**：`apps/curse/src/main.rs`、`apps/curse/src/router.rs`、`apps/curse/src/auth.rs`、`apps/curse/src/error.rs`；`crates/config/src/lib.rs`（Figment 配置加载）；`crates/api-contract/src/lib.rs`（utoipa schema + 路由 trait 占位）。

**不做**：不实现真正的 Source/Node 业务 handler（在 p1-04）；不实现 `/check/*`、`/fetch/*` 业务（P2/P3）；不实现 auth login 的 bcrypt 校验细节（仅接 JWT 校验链路）。

### 前置 / Prerequisites

- 完成 [p1-01-workspace.md](./p1-01-workspace.md)，workspace 可编译。
- 已读 `cmd/server/main.go`（auth dispatch 在 L113-141）、`internal/auth/{bearer,jwt,handler}.go`、`config.example.yaml`、`api/openapi/spec.yaml`（schema 定义在 L19-138）。
- Rust 侧需 `axum` / `tracing-subscriber` / `figment` / `utoipa` / `utoipa-swagger-ui` / `jsonwebtoken`（已在 p1-01 `workspace.dependencies` 声明）。

### 步骤 / Steps

**1. 配置加载（Figment，对齐 `config.example.yaml` 结构 + `PHLOGO_` 前缀覆盖）。** Phlogo 用 Viper + `PHLOGO_` 前缀、嵌套键 `.`→`_`（见 `config.example.yaml` 注释 L6-7）。Rust 侧用 Figment 的 `Env::prefixed("PHLOGO_")` + `.separator("_")`：

```rust
// crates/config/src/lib.rs
use serde::Deserialize;
use figment::{Figment, providers::{Format, Yaml, Env}};

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub port: u16,                       // PHLOGO_SERVER_PORT
    pub cors_origins: Vec<String>,       // PHLOGO_SERVER_CORS_ORIGINS
    pub verbose: bool,                   // PHLOGO_SERVER_VERBOSE
}

#[derive(Debug, Clone, Deserialize)]
pub struct DatabaseConfig {
    #[serde(rename = "type")]
    pub db_type: String,                 // PHLOGO_DATABASE_TYPE
    pub dsn: String,                     // PHLOGO_DATABASE_DSN
    pub max_open_conns: u32,
    pub max_idle_conns: u32,
    pub conn_max_lifetime_minutes: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AuthConfig {
    pub admin_password: String,          // PHLOGO_AUTH_ADMIN_PASSWORD
    pub service_secret: String,          // PHLOGO_AUTH_SERVICE_SECRET
    pub jwt_secret: String,              // PHLOGO_AUTH_JWT_SECRET (>= 32 chars)
}

impl Config {
    pub fn load(path: Option<&str>) -> Result<Self, figment::Error> {
        let mut fig = Figment::new().merge(Yaml::file("config.yaml"));
        if let Some(p) = path {
            fig = fig.merge(Yaml::file(p));
        }
        // PHLOGO_ 前缀，嵌套键 "_" 分隔（server.port -> PHLOGO_SERVER_PORT）
        fig = fig.merge(Env::prefixed("PHLOGO_").separator("_"));
        fig.extract()
    }
}
```

**2. tracing 初始化。** 对应 Phlogo `main.go` L44 `zerolog.ConsoleWriter`，Rust 用 `tracing-subscriber` 的 `fmt` + `EnvFilter`（`RUST_LOG` 或 config `server.verbose` 控制级别）：

```rust
// apps/curse/src/telemetry.rs
pub fn init(level: &str) {
    use tracing_subscriber::{fmt, EnvFilter, prelude::*};
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
```

**3. 分层认证中间件（复刻 `main.go` L117-141 的 switch）。** Phlogo 三层：`ServiceBearerMiddleware`、`AdminJWTMiddleware`、`AdminOrServiceMiddleware`，外加 `CheckerTagMiddleware`（见 `internal/auth/bearer.go`、`internal/auth/jwt.go`）。本阶段实现前两层 + admin-or-service；checker 层在 P3 补：

```rust
// apps/curse/src/auth.rs
use axum::{
    extract::Request,
    middleware::Next,
    response::{IntoResponse, Response},
    http::{StatusCode, header::AUTHORIZATION},
};

fn bearer_token(req: &Request) -> Option<String> {
    let h = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = h.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") { return None; }
    Some(token.trim().to_string())
}

pub async fn require_service_bearer(
    service_secret: String, req: Request, next: Next,
) -> Response {
    match bearer_token(&req) {
        Some(t) if t == service_secret => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "missing or invalid service secret")
               .into_response(),
    }
}

pub async fn require_admin_jwt(
    jwt_secret: String, req: Request, next: Next,
) -> Response {
    match bearer_token(&req).and_then(|t| verify_jwt(&jwt_secret, &t)) {
        Some(true) => next.run(req).await,
        _ => (StatusCode::UNAUTHORIZED, "missing or invalid JWT")
               .into_response(),
    }
}

// 对应 internal/auth/jwt.go：HS256，access token 15min，refresh 7d
fn verify_jwt(secret: &str, token: &str) -> Option<bool> {
    use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
    let data = decode::<serde_json::Value>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    ).ok()?;
    Some(data.claims.get("token_type").and_then(|v| v.as_str()) == Some("access"))
}
```

**4. 路由装配（复刻 `main.go` 的分发树）。** 用 `axum::middleware::from_fn_with_state` 按路径前缀挂不同 auth：

```rust
// apps/curse/src/router.rs
use axum::Router;
use axum::routing::get;
use crate::auth::{require_service_bearer, require_admin_jwt};
use crate::config::Config;

pub fn build(cfg: Config) -> Router {
    let shared = cfg.clone();
    let service_mw = from_fn_with_state(shared.clone(), move |r, n| {
        require_service_bearer(cfg.service_secret.clone(), r, n)
    });
    let admin_mw = from_fn_with_state(shared.clone(), move |r, n| {
        require_admin_jwt(cfg.jwt_secret.clone(), r, n)
    });

    Router::new()
        .route("/health", get(|| async { "ok" }))
        // 公开：/auth/* 在 main 中单独挂载，不加任何 auth
        // service bearer：/fetch/* （P2 接入）
        // admin jwt：其余 /api/v1/* （P1 仅 stub）
        .route("/api/v1/sources", get(stub_list).post(stub_create))
        .layer(admin_mw)            // 默认 admin JWT（对应 main.go default 分支）
        // 注：/fetch/* 与 POST /sources 的 admin-or-service 分支在 P1 用 stub 验证 401
        .with_state(shared)
}
```

> 对照 `main.go` L120-137：`p == ""` / `/:format` / `/health` / `/api/v1/auth/*` 公开；`/api/v1/check/*` → checker；`/api/v1/fetch/*` → service；`POST /api/v1/sources` → admin-or-service；其余 → admin。P1 仅在受保护路由上验证「缺 token 返 401、未知路径返 404」，业务 stub 返回 501 或固定 JSON。

**5. OpenAPI 契约（spec-first）。** 保留 `api/openapi/spec.yaml`（Phlogo 27 path 契约源，CONVENTIONS §2.10），同时用 utoipa 描述 P1 schema。两种落地方式二选一（CONVENTIONS §2.1）：

- **方式 A（推荐）**：把 `spec.yaml` 作为单一契约，TS client 用 `openapi-typescript` / `openapi-generator` 从 YAML 生成；Rust 侧 `utoipa` 仅用于生成 Swagger UI 文档与校验路由。
- **方式 B**：把 P1 涉及的 `SourceSummary`/`SourceDetail`/`SourceCreateRequest` 等（spec.yaml L67-113）翻译为 `#[derive(ToSchema)]` 结构体，逐步替代 YAML。

本阶段先用 `utoipa-swagger-ui` 挂载文档，保证契约可见：

```rust
// apps/curse/src/main.rs (片段)
let (router, api) = ApiDoc::split_for_parts();
router.merge(SwaggerUi::new("/swagger-ui").url("/api-docs/openapi.json", api))
```

**6. `main.rs` 组合根（对应 `cmd/server/main.go`）。**

```rust
// apps/curse/src/main.rs
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::Config::load(None)?;
    telemetry::init(if cfg.server.verbose { "debug" } else { "info" });
    let app = router::build(cfg.clone());
    let addr = format!("0.0.0.0:{}", cfg.server.port);
    tracing::info!(%addr, "Phlogo Wish Gnosis api starting");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

### 验证结果 / Verified Results（已实测，2026-11-10）

> 环境：校机 `rustc 1.99.1`；`PATH` 含 cargo bin + `D:\yzd\MSYS2\mingw64\bin`。

**落地结构**：
- `apps/curse` 改为 **lib + bin**：`src/lib.rs`（`pub mod auth/router/telemetry`）+ 薄 `src/main.rs`（组合根）；测试放 crate 内 `#[cfg(test)] mod smoke`（可访问 axum/tower）。
- `crates/config`（包名 `app-config`）：`Config`（server/database/auth），`Config::load` = 默认值 → 可选 YAML → **显式 `PHLOGO_` env 覆盖**（`PHLOGO_SERVER_PORT`/`PHLOGO_DATABASE_DSN`/`PHLOGO_AUTH_*` 等），不依赖 Figment env 分隔符魔法，行为确定。
- `apps/curse/src/auth.rs`：`bearer_token` + `verify_jwt`（HS256、校验 `token_type=="access"`）+ 中间件 `require_admin_or_service`（service bearer **或** admin access JWT）。应用于 `/api/v1/*`。
- `apps/curse/src/router.rs`：`/health` 与 `/api-docs/openapi.json` 公开；`/api/v1/*` 统一挂 `require_admin_or_service`；`fallback` 404。
- OpenAPI：`/api-docs/openapi.json` 直接输出 utoipa `ApiDoc::openapi().to_json()`。

**实测（校机）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **BUILD_EXIT=0**（34.09s） |
| `cargo test -p api` | **TEST_EXIT=0**，`4 passed`（health 200 / protected 401 / unknown 404 / openapi 200） |
| `cargo fmt --all -- --check` | **FMT_EXIT=0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **CLIPPY_EXIT=0** |
| 运行期冒烟（`PHLOGO_SERVER_PORT=28099` + `curl`） | **`health=200 post_sources=401 unknown=404`**（env 覆盖生效，服务监听 28099） |

**偏差记录（诚实标注）**：
- **`utoipa-swagger-ui` 静态 UI 暂缺**：`utoipa-swagger-ui 8.1.0` 的 `build.rs` 与解析到的 `zip 2.6.1` API 不兼容（`ZipError::InvalidArchive` 需 `Cow<'static,str>`），构建失败。**处置**：本轮去掉 swagger-ui 静态资源，改由 `/api-docs/openapi.json` 提供 OpenAPI JSON（文档仍“可访问”）；后续如需 Swagger UI，钉一组兼容的 `utoipa-swagger-ui`+`zip` 版本再启用。
- **细粒度分层（`/check/*` checker、`/fetch/*` service）未在 P1 拆**：P1-02 对 `/api/v1/*` 统一用 `admin-or-service`（满足 401 验收）；checker/fetch 专属层在 P2/P3 接入时补。

### 验收 / Acceptance

| 命令 / 行为 | 预期 |
|---|---|
| `cargo run -p api`（默认 config.yaml 或 env） | 进程监听 `cfg.server.port`（默认 21380） |
| `curl -i http://localhost:21380/health` | `HTTP/1.1 200 OK`，body `ok` |
| `curl -i -X POST http://localhost:21380/api/v1/sources -d '{}'` | `HTTP/1.1 401 Unauthorized`（无 Bearer） |
| `curl -i http://localhost:21380/nonexistent` | `HTTP/1.1 404 Not Found` |
| 设置 `PHLOGO_SERVER_PORT=28099` 后启动 | 监听 `28099`（验证 Figment env 覆盖） |
| `cargo test -p api` | 退出码 `0`，含对 stub 路由 401/404 的 `#[tokio::test]` |

示例（本地运行后）：

```text
$ curl -i http://localhost:21380/health
HTTP/1.1 200 OK
content-type: text/plain; charset=utf-8
content-length: 2
ok

$ curl -i -X POST http://localhost:21380/api/v1/sources -H 'content-type: application/json' -d '{"type":"subscribe","content":"https://x"}'
HTTP/1.1 401 Unauthorized
content-length: 35
missing or invalid JWT
```

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Rust 交付物 | Phlogo（Go）对应 | 说明 |
|---|---|---|
| `config::Config` + Figment | `internal/config/**` + `config.example.yaml` | Viper `PHLOGO_` 前缀 → Figment `Env::prefixed("PHLOGO_").separator("_")`（config.example.yaml L6-7） |
| `auth::require_*`（3 层） | `internal/auth/bearer.go`（L16-69）+ `main.go` L113-141 | `ServiceBearerMiddleware`/`AdminJWTMiddleware`/`AdminOrServiceMiddleware` 分发树 |
| `auth::verify_jwt` | `internal/auth/jwt.go`（`IssueAccessToken` 15min / refresh 7d，HS256） | 须复刻 `accessTokenDuration=15m`、`refreshTokenDuration=7d`、`token_type` 校验 |
| `router::build` | `main.go` L117-144 的 `engine.Use(...)` + `api.RegisterHandlers` | `/health` 公开、`/auth/*` 公开、`/check/*` checker、`/fetch/*` service、其余 admin |
| OpenAPI 契约 | `api/openapi/spec.yaml`（L19-138 schema，L460+ paths） | spec-first，utoipa 生成 Swagger UI |

### CI 提示 / CI notes

> 仅描述将来 CI 命令，**不落地 YAML**。

- **启动冒烟（smoke）**：CI 中 `cargo run -p api &`，待端口就绪后 `curl -f http://localhost:PORT/health` 校验 200。
- **契约检查**：保留 `api/openapi/spec.yaml` 作为契约，CI 跑 `spectral` 或 `openapi-generator validate` 校验 YAML；若走方式 B，CI 跑 `cargo test` 中 utoipa 文档生成测试。
- **auth 回归**：`cargo test -p api` 的集成用例覆盖 401（缺 token）/ 404（未知路径）/ 200（`/health`），可在 CI 中用 `cargo nextest run -p api` 提速。
- **lint/format**：同 p1-01（`cargo fmt --check` + `cargo clippy -D warnings`）。

### 风险 / Risks

- **Auth 分发顺序**：Axum 的 `Router` layer 作用域与 Gin 的全局 `Use` 不同——Phlogo 用单一 `engine.Use` 内按 `FullPath()` 分支；Axum 必须用 `nest`/嵌套 Router + 按前缀挂 layer 才能等价。错配会导致「公开路由被 admin 拦截」或「受保护路由裸奔」。
- **JWT `token_type` 校验**：Phlogo `parseToken` 强校验 `c.TokenType == expected`（jwt.go L75-77）。Rust 侧若忽略 `token_type`，refresh token 可被当作 access token 用，属安全缺陷。
- **`PHLOGO_` env 分隔符**：Figment `separator("_")` 把 `PHLOGO_SERVER_CORS_ORIGINS` 映射到 `server.cors_origins`，但 Figment 默认对 `Vec` 的处理需要确认（可能需 `.split(" ")` 或逗号）。需实测 env 数组覆盖行为。
- **契约源唯一性**：若同时维护 `spec.yaml` 与 utoipa 生成两份，易漂移。明确「spec.yaml 为单一来源」，utoipa 仅消费或单向生成。
