## 2-01 Cargo 工作区与基线依赖 · Cargo Workspace & Baseline Dependencies

### 目标 / Goal

- 建立与方案 §13 一致的 Cargo workspace，区分 `apps/`（二进制）与 `crates/`（库）。
- 锁定工具链（rustup stable）、edition，沉淀 baseline 依赖清单。
- 使 `cargo build --workspace --all-targets`、`cargo fmt --check`、`cargo clippy -- -D warnings` 全绿，作为后续所有阶段的 CI 基线。

### 范围 / Scope

**新建**：workspace 根 `Cargo.toml`、`Cargo.lock`（首次构建生成）、`rust-toolchain.toml`、`apps/{api,worker}` 两个二进制 crate、`crates/{domain,database,config,api-contract}` 等骨架目录与空 `lib.rs`。

**不做**：不写业务逻辑（业务在 p1-02 / p1-03 / p1-04）。不在本步接入 MiaoSpeed / Apalis（P0 验证后于 P3 接入）。

### 前置 / Prerequisites

- 已安装 `rustup` + 稳定工具链：`rustup show`（建议 stable，版本 ≥ 1.80 以支持 `cargo` 原生 workspace 行为）。
- 已安装 `cargo` 子工具（CI 种子，P1 暂用 `fmt`/`clippy` 内置；`sqlx-cli`/`cargo-sqlx` 在 p1-03 安装）：
  - `cargo install cargo-edit`（用于 `cargo add`）——可选。
- 已读完 `../CONVENTIONS.md` §3 组件映射表（下表复用其事实）。

### 步骤 / Steps

**1. 工具链固定。** 根目录新建 `rust-toolchain.toml`，统一 edition 与 channel，避免本地/CI 漂移：

```toml
# rust-toolchain.toml
[toolchain]
channel = "stable"
components = ["rustfmt", "clippy"]
profile = "default"
```

**2. 建立 workspace 根 `Cargo.toml`。** 按方案 §13 布局（本阶段只需下列子集，其余 crate 在后续阶段补）：

```toml
# Cargo.toml (workspace root)
[workspace]
resolver = "2"
members = [
    "apps/api",
    "apps/worker",
    "crates/domain",
    "crates/database",
    "crates/config",
    "crates/api-contract",
]
exclude = ["web/vben-admin"]

[workspace.package]
edition = "2021"
version = "0.1.0"
license = "AGPL-3.0"          # 继承 paily-core，见 CONVENTIONS §2.11
rust-version = "1.80"

[workspace.dependencies]
# ── 运行时 / Web ──
tokio        = { version = "1", features = ["full"] }
axum         = { version = "0.7", features = ["macros"] }
# ── 数据库 ──
sqlx         = { version = "0.8", features = ["runtime-tokio", "postgres", "chrono", "uuid", "json"] }
# ── 观测 / 配置 ──
tracing      = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
figment      = { version = "0.10", features = ["env", "yaml"] }
# ── API 契约 ──
utoipa       = { version = "5", features = ["axum_extras"] }
utoipa-swagger-ui = { version = "8", features = ["axum"] }
# ── 认证 / 标识 ──
jsonwebtoken = "9"
uuid         = { version = "1", features = ["v4", "serde"] }
# ── 序列化 / 错误 ──
serde        = { version = "1", features = ["derive"] }
serde_json   = "1"
thiserror    = "1"
anyhow       = "1"
reqwest      = { version = "0.12", features = ["json", "rustls-tls"] }
apalis       = { version = "0.7", features = ["postgres"] }   # P3 启用，P1 仅声明
```

> 注：`apalis` 在 P0 验证其固定版本 PG backend 行为后再锁定（CONVENTIONS §4）。P1 仅把它放进 `workspace.dependencies` 占位，不在任何 crate 真正 `use`，避免未验证依赖进入编译图。

**3. 用 `cargo new` 生成骨架（edition 来自根 package 继承）：**

```powershell
# 在仓库根（与 Cargo.toml 同级）执行
cargo new apps/api --bin
cargo new apps/worker --bin
cargo new crates/domain --lib
cargo new crates/database --lib
cargo new crates/config --lib
cargo new crates/api-contract --lib
```

**4. 各 crate 的 `Cargo.toml` 引用 workspace 依赖。** 例如 `apps/api/Cargo.toml`：

```toml
[package]
name = "api"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
tokio.workspace      = true
axum.workspace       = true
tracing.workspace    = true
config-crate        = { path = "../crates/config" }      # 见下方 rename 说明
api-contract        = { path = "../crates/api-contract" }
# ...
```

> 为避免与 `config` crate 生态重名，本阶段将自有配置 crate 命名为 `app-config`（在根 `Cargo.toml` 用 `[workspace.dependencies]` 的 key 命名自由）。下方 p1-02 同此。

**5. 各 crate 写入最小可编译内容**，让 `cargo build --workspace --all-targets` 通过：

- `apps/api/src/main.rs`：一个 `fn main() {}`（p1-02 再填充 Axum）。
- `crates/*/src/lib.rs`：一个 `//! 占位文档` 注释 + `pub mod prelude {}` 或空。

**6. 统一 `.cargo/config.toml`（可选，便于镜像/离线）：**

```toml
# .cargo/config.toml
[build]
# rustflags = ["-D", "warnings"]   # 本地开发不建议常开；CI 用 clippy -D warnings
```

### 验收 / Acceptance

| 命令 | 预期退出码 / 输出 |
|---|---|
| `rustup show` | 显示 `stable` toolchain 已安装 |
| `cargo build --workspace --all-targets` | 退出码 `0` |
| `cargo fmt --all -- --check` | 退出码 `0`（无 diff；提交前先 `cargo fmt --all`） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 `0`（0 warnings） |
| `cargo tree -e no-dev` | 输出依赖树且包含 `axum`、`sqlx`、`tracing`、`utoipa` 等基线包 |

> 提示：`cargo build` 首次会生成 `Cargo.lock`，应提交到仓库（保证可复现构建，CONVENTIONS §16 供应链要求）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Rust 交付物 | Phlogo（Go）对应 | 说明 |
|---|---|---|
| `apps/api` + `apps/worker` | `cmd/server/main.go`（单体） | 拆分 `cmd/server/main.go` L39-157 的单进程装配为 api/worker 两二进制（CONVENTIONS §3 映射表第 1 行） |
| `crates/api-contract` | `internal/api/generated/server.gen.go` + `api/openapi/spec.yaml` | oapi-codegen → utoipa（CONVENTIONS §2.1 技术栈表、§2.10 API 面） |
| `crates/database` | `internal/db/**` + `internal/db/model/**` | GORM → SQLx（CONVENTIONS §2.1、§3 第 4 行） |
| `crates/config` | `internal/config/**` + `config.example.yaml` | Viper（`PAILY_` 前缀）→ Figment（CONVENTIONS §2.1、§2.6） |
| `crates/domain` | `internal/source/**`、`internal/node/**`、`internal/scoring/**` 等领域实体 | 业务实体与领域服务（CONVENTIONS §3 第 5-11 行） |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **构建 job**：`cargo build --workspace --all-targets`（等价于 `cargo build` + `cargo test --no-run`）。
- **格式 gate**：`cargo fmt --all -- --check`，失败即阻断 PR。
- **Lint gate**：`cargo clippy --workspace --all-targets -- -D warnings`。
- **依赖审计（P1 起建议并行）**：`cargo audit`；许可证：`cargo deny check licenses`（CONVENTIONS §5）。
- **缓存**：CI 缓存 `~/.cargo` 与 `target/`，并按 `Cargo.lock` 哈希做 cache key。

### 风险 / Risks

- **license 一致性**：`workspace.package.license = "AGPL-3.0"` 须与 Phlogo 的 AGPL-3.0 一致（CONVENTIONS §2.11）。任何新增第三方 crate 都需经 `cargo deny` 复核许可证兼容性（MiaoSpeed GPL / VProxy MIT / Mihomo GPL 不得链接进 Core，仅进程隔离）。
- **edition 漂移**：所有 crate 必须继承 `workspace.package.edition`，禁止个别 crate 写死 `edition = "2024"` 之类造成不一致。
- **rust-toolchain 与 CI 镜像不一致**：CI 镜像若未预装对应 channel，需在 CI 中 `rustup toolchain install` 或让 `rust-toolchain.toml` 自动触发安装。
- **未验证依赖（`apalis`）**：P1 仅占位声明；在 P0 完成 Apalis PG backend 验证（CONVENTIONS §4 风险之一）前，不要让它进入真实编译路径。
