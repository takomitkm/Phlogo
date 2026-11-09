## 2-03 初始迁移与配置种子 · SQLx Migrations & Config Seed

### 目标 / Goal

- 用 SQLx + PostgreSQL 写下 **P1 初始迁移**，覆盖 Source/Node/Node-Source 关系与抓取历史，并以 KV 种子写入 CONVENTIONS §2.6 的**全部默认配置键**（与 Phlogo `configs` 表 seed 对齐）。
- 建立离线编译（SQLx `query!`/`query_as!` 需 `sqlx-data.json`）与测试数据库迁移校验机制。
- 明确**不手写** Apalis 内部表（方案 §5.1 警告）。

### 范围 / Scope

**新建**：`migrations/0001_init/up.sql` + `down.sql`、`crates/database/src/lib.rs`（PgPool 初始化）、`crates/database/src/migrate.rs`（启动迁移 + seed 入口）、`sqlx-data.json`（由 `cargo sqlx prepare` 生成，提交仓库）、`.env` / `.env.example`（`DATABASE_URL`）。

**本迁移创建的业务表**：`sources`、`nodes`、`node_sources`、`source_fetch_runs`，以及 `configs`（KV 种子）。

**不在本迁移创建**（由后续阶段迁移补，CONVENTIONS §2.3 / 方案 §5.1）：`node_payloads`（P2）、`check_runs`/`node_check_results`/`node_status`/`node_scores`（P3/P4）、`subscription_profiles`/`subscription_tokens`（P5）。这些表的列在 P1 仅规划，避免一次性大迁移难以 review。

**不做**：SQLite 兼容（CONVENTIONS §2.1 已决定仅 PostgreSQL）；Apalis `apalis_*` 表（必须用 Apalis 官方 migration，P0 验证后由 worker 启动自动写入）。

### 前置 / Prerequisites

- 完成 [p1-01-workspace.md](./p1-01-workspace.md) 与 [p1-02-api-skeleton.md](./p1-02-api-skeleton.md)。
- 已读 `internal/db/model/source.go`、`internal/db/model/node.go`、`internal/migrate/migrate.go`、`config.example.yaml`。
- 已安装 `sqlx-cli`（任选其一）：
  - `cargo install sqlx-cli --features postgres --no-default-features`（或 `cargo install sqlx-cli --features native-tls,postgres`）。
  - 或用 `cargo sqlx` 通过 `cargo install cargo-sqlx`（别名相同）。
- 本地或 CI 有可达的 PostgreSQL（`DATABASE_URL` 形如 `postgres://paily:change-me@localhost:5432/paily`）。CI 用容器或 CI 托管 Postgres。

### 步骤 / Steps

**1. 初始化 SQLx 离线元数据目录。** 在根 `Cargo.toml` 的 `crates/database` 加入：

```toml
sqlx = { workspace = true }   # 已含 runtime-tokio + postgres
```

并在根目录准备 `.env`（被 `sqlx-cli` 与 `cargo sqlx` 自动读取）：

```dotenv
# .env (本地；不要提交真实密钥)
DATABASE_URL=postgres://paily:change-me@localhost:5432/paily
```

> `.env.example` 提交仓库，`DATABASE_URL` 由 CI/部署注入。

**2. 写初始迁移 `migrations/0001_init/up.sql`。** 字段来源：方案 §5.1（P1 涉及表）+ CONVENTIONS §2.3（Phlogo GORM 字段）+ §2.6（config 键）。

```sql
-- migrations/0001_init/up.sql
-- ── sources ───────────────────────────────────────────────────────────────
-- 对应 internal/db/model/source.go + 方案 §5.1（新增 fetch_policy；status 扩展见 §2.9）
CREATE TABLE sources (
    id            uuid PRIMARY KEY,
    type          varchar(32) NOT NULL,          -- "subscribe" | "node"
    identifier    text NOT NULL DEFAULT '',
    info          text NOT NULL DEFAULT '',
    content       text NOT NULL,                 -- URL(subscribe) | raw text(node)
    status        varchar(16) NOT NULL DEFAULT 'active',  -- active|suspended|dead
    ignore_dead   boolean NOT NULL DEFAULT false,
    fetch_policy  varchar(16) NOT NULL DEFAULT 'replace', -- replace|accumulate (方案 §6.1)
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_sources_type_status ON sources (type, status);

-- ── nodes ──────────────────────────────────────────────────────────────────
-- 对应 internal/db/model/node.go + 方案 §5.1（identity_hash + fingerprint_version）
-- 注：raw/password/credentials 的加密存储属 node_payloads（P2），本表仅放身份与连接字段
CREATE TABLE nodes (
    id                 uuid PRIMARY KEY,
    identity_hash      varchar(64) NOT NULL UNIQUE,  -- sha256(canonical)[:16] hex（见 p1-04）
    fingerprint_version integer NOT NULL DEFAULT 1,  -- 支持算法迁移（方案 §5.1 规则 4）
    protocol           varchar(32) NOT NULL,
    server             varchar(255) NOT NULL,
    port               integer NOT NULL,
    first_seen_at      timestamptz NOT NULL DEFAULT now(),
    last_seen_at       timestamptz NOT NULL DEFAULT now(),
    created_at         timestamptz NOT NULL DEFAULT now(),
    updated_at         timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_nodes_server ON nodes (server);

-- ── node_sources（多对多，补时间戳）──────────────────────────────────────────
-- 对应 internal/db/model/node.go NodeSource（复合 PK，无时间戳）→ 方案 §5.1 补 first/last_seen_at
CREATE TABLE node_sources (
    node_id      uuid NOT NULL REFERENCES nodes (id) ON DELETE CASCADE,
    source_id    uuid NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    first_seen_at timestamptz NOT NULL DEFAULT now(),
    last_seen_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (node_id, source_id)
);

-- ── source_fetch_runs（抓取历史）────────────────────────────────────────────
-- 对应 internal/db/model/source.go SourceFetchHistory（success/node_count）
--   → 方案 §5.1 扩展为 status/started_at/finished_at/counts/error
CREATE TABLE source_fetch_runs (
    id           uuid PRIMARY KEY,
    source_id    uuid NOT NULL REFERENCES sources (id) ON DELETE CASCADE,
    status       varchar(16) NOT NULL,            -- "success" | "failed"
    started_at   timestamptz NOT NULL,
    finished_at  timestamptz,
    counts       jsonb NOT NULL DEFAULT '{}'::jsonb,  -- 按视角/阶段的节点计数汇总
    error        text,
    created_at   timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX idx_fetch_runs_source ON source_fetch_runs (source_id, started_at DESC);

-- ── configs（KV 运行时配置，seed 见下）───────────────────────────────────────
-- 对应 internal/db/model/config.go + config.example.yaml
CREATE TABLE configs (
    key         varchar(128) PRIMARY KEY,
    value       text NOT NULL DEFAULT '',
    updated_at  timestamptz NOT NULL DEFAULT now()
);
```

**3. `down.sql`（可回滚，便于本地 reset）：**

```sql
-- migrations/0001_init/down.sql
DROP TABLE IF EXISTS node_sources;
DROP TABLE IF EXISTS source_fetch_runs;
DROP TABLE IF EXISTS nodes;
DROP TABLE IF EXISTS sources;
DROP TABLE IF EXISTS configs;
```

**4. KV 种子（CONVENTIONS §2.6 全部默认键）。** 用 `INSERT ... ON CONFLICT (key) DO NOTHING` 保证幂等（对应 `internal/migrate/migrate.go` 的 `OnConflict{DoNothing}` 语义），放在 `up.sql` 末尾或单独的 seed 步骤。下列键与默认值**逐字来自 CONVENTIONS §2.6**：

```sql
-- 配置默认值种子（CONVENTIONS §2.6 全文；CI 与迁移必须包含）
INSERT INTO configs (key, value) VALUES
    ('max_distribute',                 '200'),
    ('dead_latency_ms',                '3000'),
    ('fetch_dead_window_days',         '7'),
    ('node_dead_window_days',          '4'),
    ('fetch_history_window_days',      '7'),
    ('score_decay_alpha',              '0.7'),
    ('score_weight_latency',           '0.7'),
    ('score_weight_stability',         '0.3'),
    ('score_weight_speed',             '0.55'),
    ('score_latency_curve_k',          '0.3'),
    ('score_jitter_linear_threshold_ms','200'),
    ('grade_a_min',                    '0.75'),
    ('grade_b_min',                    '0.50'),
    ('grade_c_min',                    '0.25'),
    ('node_alive_check_window_minutes','240'),
    ('check_run_history_days',         '7'),
    ('cleanup_interval_minutes',       '30'),
    ('default_sponsor_text',           ''),
    ('node_distribute_filter',         ''),
    ('node_name_template',             '(unlock_flags != "" ? "[" + unlock_flags + "] " : "") + node.grade + " - " + node.region_flag + " " + node.region + " " + node.seq + (node.sponsor != "" ? " " + node.sponsor : "")'),
    ('filename_clash',                 ''),
    ('filename_singbox',               ''),
    ('filename_base64',                '')
) ON CONFLICT (key) DO NOTHING;
```

> 注：CONVENTIONS §2.6 末行写 `filename_clash/singbox/base64`，此处按三个独立 KV 键处理（clash/singbox/base64 三种订阅文件名）。若后续发现是单个复合键，以 `config.example.yaml` 的 format 默认值逻辑为准。

**5. Rust 侧接入迁移 + 连接池。** `crates/database/src/lib.rs`：

```rust
// crates/database/src/lib.rs
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

pub async fn connect(dsn: &str) -> anyhow::Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(6)        // 对应 config.example.yaml database.max_open_conns
        .idle_timeout(std::time::Duration::from_secs(60 * 2))
        .connect(dsn).await?;
    Ok(pool)
}

// 启动期执行迁移；SQLx 内置 migrate! 宏会编译期校验 migrations/ 目录
pub async fn migrate(pool: &PgPool) -> anyhow::Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}
```

> 若 `migrations/` 不在 `crates/database` 包根，需把宏路径指到仓库根（如 `sqlx::migrate!("../migrations")`），并保证编译期 `DATABASE_URL` 可达（或 `SQLX_OFFLINE=true` + 已 prepare 的 `sqlx-data.json`）。

**6. 离线查询准备（CI 关键）。** 任何 `#[sqlx::query]` / `query_as!` 都必须先 prepare：

```powershell
# 在仓库根，DATABASE_URL 指向任一可达 Postgres（含本地）
$env:DATABASE_URL = "postgres://paily:change-me@localhost:5432/paily"
cargo sqlx prepare            # 生成/更新 sqlx-data.json
cargo sqlx prepare --check    # CI：校验 sqlx-data.json 是否最新
```

### 验收 / Acceptance

| 命令 | 预期 |
|---|---|
| `cargo sqlx database create` 然后 `cargo sqlx migrate run` | 退出码 `0`；Postgres 出现 `sources`/`nodes`/`node_sources`/`source_fetch_runs`/`configs` 五张表 |
| `psql $DATABASE_URL -c "SELECT count(*) FROM configs;"` | 返回 `22`（§2.6 共 22 个默认键：上述 21 个显式键 + node_name_template 计 1；见下方说明） |
| `SQLX_OFFLINE=true cargo sqlx prepare` | 生成 `sqlx-data.json` 且 `cargo build --workspace` 离线编译通过（退出码 0） |
| `cargo sqlx prepare --check` | 退出码 `0`（查询与缓存一致；CI 用此阻止未 prepare 的查询 PR） |
| 集成测试（`cargo test --test integration`，用 test 容器或 CI Postgres） | 退出码 `0`；迁移在该库成功应用，且 `configs` 行数符合预期 |
| `cargo sqlx migrate revert`（本地回滚验证） | 五张表被 `down.sql` 删除，退出码 0 |

> 关于 config 计数：§2.6 列出的键 = `max_distribute`、`dead_latency_ms`、`fetch_dead_window_days`、`node_dead_window_days`、`fetch_history_window_days`、`score_decay_alpha`、`score_weight_latency`、`score_weight_stability`、`score_weight_speed`、`score_latency_curve_k`、`score_jitter_linear_threshold_ms`、`grade_a_min`、`grade_b_min`、`grade_c_min`、`node_alive_check_window_minutes`、`check_run_history_days`、`cleanup_interval_minutes`、`default_sponsor_text`、`node_distribute_filter`、`node_name_template`、`filename_clash`、`filename_singbox`、`filename_base64` —— 共 **22** 个 `configs` 行（若把 `filename_*` 视为 1 个复合键则相应减 2）。

测试容器（推荐 CI/本地一致）示意：

```rust
// tests/integration/common.rs（示意，不必在 P1 完整实现）
use testcontainers_modules::postgres::Postgres;
// 或在 CI 直接复用 provider 的 Postgres service
let pool = PgPoolOptions::new().connect(&ci_database_url).await.unwrap();
sqlx::migrate!("../migrations").run(&pool).await.unwrap();
```

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Rust 迁移对象 | Phlogo（Go）对应 | 说明 |
|---|---|---|
| `migrations/0001_init` | `internal/migrate/migrate.go`（`dstDB.AutoMigrate`） | GORM AutoMigrate → 手写 SQL 迁移（CONVENTIONS §3 映射表第 13 行、`internal/migrate/migrate.go` L37-44） |
| `sources` 表 | `internal/db/model/source.go` `Source` + `SourceFetchHistory` | 拆分 `SourceFetchHistory` → `source_fetch_runs`（CONVENTIONS §2.3 表行） |
| `nodes` 表 | `internal/db/model/node.go` `Node` | 身份字段 `identity_hash`+`fingerprint_version`（方案 §5.1）；`raw`/`password` 推迟到 `node_payloads`（CONVENTIONS §2.3） |
| `node_sources` 表 | `internal/db/model/node.go` `NodeSource` | 复合 PK + 新增 `first_seen_at`/`last_seen_at`（方案 §5.1、CONVENTIONS §2.3） |
| `configs` 种子 | `internal/db/model/config.go` + `config.example.yaml` | §2.6 默认键全量 seed，幂等 `ON CONFLICT DO NOTHING` |

### CI 提示 / CI notes

> 仅描述将来 CI 命令，**不落地 YAML**。

- **迁移应用 job**：CI 起一个 Postgres（GitHub Actions `services: postgres` 或 testcontainers），注入 `DATABASE_URL`，运行 `cargo sqlx migrate run --database-url $DATABASE_URL`，退出码 0。
- **离线查询 gate**：默认 `SQLX_OFFLINE=true` 构建；PR 必须包含最新 `sqlx-data.json`，CI 跑 `cargo sqlx prepare --check` 校验，未 prepare 的 SQL 查询会被阻止合并。
- **集成测试 job**：`cargo test --test integration`（CONVENTIONS §5），需要 Postgres + 可选 MiaoSpeed stub（P1 仅需 Postgres）。
- **迁移幂等 / 回滚演练**（P7 前建议）：CI 偶发跑 `cargo sqlx migrate revert` 验证 `down.sql` 可逆。
- **不要手写 Apalis 表**：worker 侧的 `apalis_*` 表由 Apalis 官方迁移在 P3 写入；CI 迁移 job 只覆盖 `migrations/` 下的本系统表，不得假设或硬编码 Apalis 表名（方案 §5.1 与 CONVENTIONS §4 明确警告）。

### 风险 / Risks

- **Apalis 表名未知（高危）**：方案 §5.1 明令「不要在业务表里直接假设 Apalis 内部任务表的名称」。P1 迁移**绝不**手写 `apalis_jobs` 之类；由 P0 验证后的 Apalis 版本在 worker 启动自动迁移。
- **SQLx 离线校验门槛**：一旦引入 `#[sqlx::query]`，CI 必须 `SQLX_OFFLINE=true` 且 `sqlx-data.json` 最新，否则构建失败。P1 起就要把 `cargo sqlx prepare` 纳入「改完 DB 代码后的必做步骤」。
- **`configs` 键数量漂移**：§2.6 的 `filename_*` 到底是 3 键还是 1 复合键存在歧义（见步骤 4 注）。验收里用「行数 == 22 或 == 20」两种口径，需以 `config.example.yaml` 实际 format 枚举（`clash`/`singbox`/`base64`）为准，避免 seed 后 `count` 不满足。
- **时区**：Phlogo `config.example.yaml` 用 `TimeZone=Asia/Shanghai`；迁移用 `now()`（服务器时区）与 `timestamptz`，需确认 Postgres 连接时区与业务一致（`server` 连接参数或 `SET TIME ZONE`）。
- **`node_payloads` 推迟**：本迁移不建 `node_payloads`，P1 的 node CRUD 暂不含 raw/password 存储；若 p1-04 集成测试需要持久化节点配置，须明确这只是「身份层」，完整 payload 在 P2 补齐，避免误以为凭据已落库。
