## 2-04 Source/Node 基础 CRUD 与关系 · Source/Node CRUD & Relation

### 目标 / Goal

- 在 `database` / `registry` crate 定义 **repository 层**，实现 Source CRUD（按 `type+content` 去重、状态 `active`/`dead`、`ignore_dead` 标志）与 Node CRUD + Node-Source 多对多关系。
- 在 `apps/api` 暴露 P1 所需 API 路径（CONVENTIONS §2.10 中 Source/Node 子集），并复用 p1-02 的分层认证。
- 用 `#[tokio::test]` 集成测试覆盖两条核心不变量：
  1. 相同 `type+content` 连续创建 source 返回**已存在的那条**（`alreadyExisted=true`）。
  2. 一个 node 关联两个 source → **1 行 `nodes` + 2 行 `node_sources`**。

### 范围 / Scope

**新建 / 改写**：`crates/database/src/repositories/{source,node}.rs`（SQLx 实现）、`crates/registry/src/{source,node}.rs`（领域服务，对应 Phlogo `internal/source/service.go` / `internal/node/*`）、`apps/api/src/handlers/{source,node}.rs`、`apps/api/src/router.rs`（挂真实 handler）、`tests/integration/p1_crud.rs`。

**本阶段 API 路径（来自 CONVENTIONS §2.10）**：
- `POST /api/v1/sources`、`GET /api/v1/sources`、`GET /api/v1/sources/{id}`、`PATCH /api/v1/sources/{id}`、`DELETE /api/v1/sources/{id}`、`POST /api/v1/sources/{id}/ignore-dead`、`GET /api/v1/sources/{id}/nodes`
- `GET /api/v1/nodes`、`GET /api/v1/nodes/{id}`、`DELETE /api/v1/nodes/{id}`

**不做（后续阶段）**：fetch 触发（`POST /sources/{id}/fetch`、`/fetch/*`、`/fetch/results`，P2）、check（`/check/*`，P3）、评分/健康（`node_status`/`node_scores`，P4）、`node_payloads` 加密存凭据（P2）、订阅（P5）。`source_fetch_runs` 仅在创建/列表时可写只读统计，完整抓取流程在 P2。

### 前置 / Prerequisites

- 完成 [p1-01-workspace.md](./p1-01-workspace.md)、[p1-02-api-skeleton.md](./p1-02-api-skeleton.md)、[p1-03-migrations.md](./p1-03-migrations.md)：workspace 可编译、迁移已应用、`configs` 已 seed。
- 已读 `internal/source/service.go`（`Create` L27-51 去重、`List` L66-110、`Delete` L143-156、`SetIgnoreDead` L159-169）、`internal/node/upsert.go`（`UpsertFromFetchResult` L38-164 的「事务内 SELECT-then-INSERT 防并发重复」与 `removeSourceMappings` L169-198）、`internal/api/handler/source.go`、`internal/api/handler/node.go`、`internal/db/model/{source,node}.go`。
- `sqlx-data.json` 已由 p1-03 `cargo sqlx prepare` 生成（离线编译）。

### 步骤 / Steps

**1. Repository 层（`crates/database`）—— 纯 SQLx，按 p1-03 表结构。**

```rust
// crates/database/src/repositories/source.rs
use sqlx::{PgPool, types::Uuid};
use crate::models::SourceRow;

#[derive(Debug, Clone)]
pub struct SourceRepository { pub pool: PgPool }

impl SourceRepository {
    /// 对应 internal/source/service.go Create：按 type+content 去重。
    /// 命中则返回 (existing_id, true)；否则插入 (new_id, false)。
    pub async fn create_dedup(
        &self, typ: &str, identifier: &str, info: &str, content: &str,
    ) -> anyhow::Result<(Uuid, bool)> {
        // 先查后插，事务保证并发下不双插（对应 Go service.go L28-33 + upsert.go L107 事务）
        let mut tx = self.pool.begin().await?;
        if let Some(existing) = sqlx::query_as::<_, SourceRow>(
            "SELECT id,type,identifier,info,content,status,ignore_dead,created_at,updated_at \
             FROM sources WHERE type=$1 AND content=$2 FOR UPDATE")
            .bind(typ).bind(content).fetch_optional(&mut *tx).await?
        {
            return Ok((existing.id, true));
        }
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO sources (id,type,identifier,info,content,status,ignore_dead,created_at,updated_at) \
             VALUES ($1,$2,$3,$4,$5,'active',false,now(),now())")
            .bind(id).bind(typ).bind(identifier).bind(info).bind(content)
            .execute(&mut *tx).await?;
        tx.commit().await?;
        Ok((id, false))
    }

    pub async fn get(&self, id: Uuid) -> anyhow::Result<Option<SourceRow>> {
        Ok(sqlx::query_as::<_, SourceRow>(
            "SELECT id,type,identifier,info,content,status,ignore_dead,created_at,updated_at \
             FROM sources WHERE id=$1")
            .bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn delete(&self, id: Uuid) -> anyhow::Result<bool> {
        let n = sqlx::query("DELETE FROM sources WHERE id=$1")
            .bind(id).execute(&self.pool).await?.rows_affected();
        Ok(n > 0)
    }
}
```

**2. Node-Source 关系仓库（核心不变量 #2）。** 对应 `internal/node/upsert.go` 的事务内 SELECT-then-INSERT（`nodeHash` 命中则复用、否则新建）+ 幂等 `OnConflict DoNothing` 映射（L109-138）：

```rust
// crates/database/src/repositories/node.rs
impl NodeRepository {
    /// 按 identity_hash upsert 节点；返回 (node_id, already_existed)。
    pub async fn upsert_by_hash(
        &self, identity_hash: &str, fingerprint_version: i32,
        protocol: &str, server: &str, port: i32,
    ) -> anyhow::Result<(Uuid, bool)> {
        let mut tx = self.pool.begin().await?;
        let existing: Option<Uuid> = sqlx::query_scalar(
            "SELECT id FROM nodes WHERE identity_hash=$1 FOR UPDATE")
            .bind(identity_hash).fetch_optional(&mut *tx).await?;
        let (id, existed) = match existing {
            Some(id) => (id, true),
            None => {
                let new_id = Uuid::new_v4();
                sqlx::query("INSERT INTO nodes \
                    (id,identity_hash,fingerprint_version,protocol,server,port,first_seen_at,last_seen_at,created_at,updated_at) \
                    VALUES ($1,$2,$3,$4,$5,$6,now(),now(),now(),now())")
                    .bind(new_id).bind(identity_hash).bind(fingerprint_version)
                    .bind(protocol).bind(server).bind(port)
                    .execute(&mut *tx).await?;
                (new_id, false)
            }
        };
        tx.commit().await?;
        Ok((id, existed))
    }

    /// 幂等建立 node↔source 关系（对应 upsert.go L136 OnConflict DoNothing）。
    /// 返回受影响的行数（0 表示已存在，不重复插入）。
    pub async fn link(&self, node_id: Uuid, source_id: Uuid) -> anyhow::Result<u64> {
        let n = sqlx::query(
            "INSERT INTO node_sources (node_id,source_id,first_seen_at,last_seen_at) \
             VALUES ($1,$2,now(),now()) ON CONFLICT (node_id,source_id) DO NOTHING")
            .bind(node_id).bind(source_id)
            .execute(&self.pool).await?.rows_affected();
        Ok(n)
    }

    pub async fn count_node_sources(&self, node_id: Uuid) -> anyhow::Result<i64> {
        Ok(sqlx::query_scalar("SELECT count(*) FROM node_sources WHERE node_id=$1")
            .bind(node_id).fetch_one(&self.pool).await?)
    }
}
```

**3. 领域服务（`crates/registry`）—— 对应 Phlogo `internal/source/service.go`。** 仅做编排与去重决策，数据库细节下沉到 repository：

```rust
// crates/registry/src/source.rs
pub struct SourceService { repo: SourceRepository }
pub struct CreatedSource { pub id: Uuid, pub already_existed: bool }

impl SourceService {
    pub async fn create(&self, typ:&str, identifier:&str, info:&str, content:&str)
        -> anyhow::Result<CreatedSource> {
        let (id, already) = self.repo.create_dedup(typ, identifier, info, content).await?;
        Ok(CreatedSource { id, already_existed: already })
    }
}
```

**4. API handler（`apps/api/src/handlers/source.rs`）—— 复刻 `internal/api/handler/source.go`。** 关键点：去重命中返回 **200**（非 201），与 Go `SourceHandler.Create` 的 `existed` 分支（handler/source.go L44-48）一致：

```rust
// POST /api/v1/sources
async fn create(State(svc): State<Arc<SourceService>>, Json(req): Json<SourceCreateRequest>) -> impl IntoResponse {
    match svc.create(&req.type_, &req.identifier.unwrap_or_default(),
                     &req.info.unwrap_or_default(), &req.content).await {
        Ok(c) if c.already_existed => (StatusCode::OK, ...).into_response(),       // 已存在
        Ok(_) => (StatusCode::CREATED, ...).into_response(),                       // 新建
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "failed to create source").into_response(),
    }
}
```

**5. router 挂真实 handler**（替代 p1-02 的 stub），复用 p1-02 分层认证：`POST /sources` 走 `admin-or-service` 层；其余 `/sources/*` 走 admin JWT（见 `cmd/server/main.go` L132-136）。

**6. 集成测试（核心验收）。** `tests/integration/p1_crud.rs` 用 test 容器或 CI Postgres：

```rust
#[tokio::test]
async fn same_type_content_returns_existing() {
    let pool = test_pool().await;                 // 迁移已应用（见 p1-03）
    let repo = SourceRepository { pool };
    let (id1, existed1) = repo.create_dedup("subscribe","","https://x").await.unwrap();
    let (id2, existed2) = repo.create_dedup("subscribe","","https://x").await.unwrap();
    assert!(!existed1);                            // 第一条新建
    assert!(existed2);                             // 第二条命中去重
    assert_eq!(id1, id2);                          // 返回同一条
    let cnt: i64 = sqlx::query_scalar("SELECT count(*) FROM sources WHERE type='subscribe' AND content='https://x'")
        .fetch_one(&repo.pool).await.unwrap();
    assert_eq!(cnt, 1);                            // 不产生第二条 sources 行
}

#[tokio::test]
async fn node_linked_to_two_sources_one_node_two_relations() {
    let repo = NodeRepository { pool: test_pool().await };
    let (sid1,_) = source_repo.create_dedup("node","","s1").await.unwrap();
    let (sid2,_) = source_repo.create_dedup("node","","s2").await.unwrap();
    let (nid,_)  = repo.upsert_by_hash("hashABC",1,"vmess","1.2.3.4",443).await.unwrap();
    repo.link(nid, sid1).await.unwrap();
    repo.link(nid, sid2).await.unwrap();
    let node_rows: i64  = sqlx::query_scalar("SELECT count(*) FROM nodes").fetch_one(&repo.pool).await.unwrap();
    let rel_rows:  i64  = repo.count_node_sources(nid).await.unwrap();
    assert_eq!(node_rows, 1);                      // 1 行 nodes
    assert_eq!(rel_rows, 2);                       // 2 行 node_sources
}
```

### 验证结果 / Verified Results（已实测，2026-11-10）

> 环境：校机 `rustc 1.99.1` + **PostgreSQL 18.6**（MSYS2）；`DATABASE_URL=postgres://paily:change-me@127.0.0.1:5432/paily`。

**落地结构**：
- 新 crate **`crates/registry`**（`SourceService`/`NodeService`，领域服务，镜像 `internal/source/service.go`、`internal/node/*`）。
- `crates/database`：`models::{SourceRow,NodeRow}` + `repositories::{source,node}`（`create_dedup`、`list/get/update/delete`、`set_ignore_dead`、`nodes_of_source`、`upsert_by_hash`、`link`、`count_node_sources`）；`connect_lazy`（API 启动/测试无需真库）。
- `apps/api`：`AppState`（含 lazy `PgPool` + services）+ `handlers/{source,node}`，`router::build` 挂真实路由（`/sources`、`/sources/:id`、`/sources/:id/ignore-dead`、`/sources/:id/nodes`、`/nodes`、`/nodes/:id`），沿用 P1-02 认证层。

**去重竞态（评审结论）**：新增迁移 `0002_uq_sources` 给 `sources` 加 `UNIQUE(type,content)`；`create_dedup` 用
`INSERT ... ON CONFLICT (type,content) DO NOTHING RETURNING id`，冲突则回查既有 id → **并发安全、无需 `FOR UPDATE`**。

**实测**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0** |
| `cargo test --workspace` | **0** |
| `cargo test -p database --test pg_crud` | **0**，`2 passed`（两条不变量） |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |
| **API e2e（同 body 两次 POST）** | **`create1=201 create2=200`**；b1 `alreadyExisted=false`，b2 `alreadyExisted=true`，**同一 id** |
| 鉴权 | `list_noauth=401`、`list_auth=200`（service bearer） |

**不变量验证**：
1. **去重**：API 两次相同 `type+content` → 201→200，同一 id，`sources` 仅 1 行（e2e + 集成测试双证）。
2. **关系**：1 node ↔ 2 source → `nodes` 1 行 + `node_sources` 2 行（`pg_crud::node_linked_to_two_sources_one_node_two_relations` 通过）。

**说明/偏差**：P1 未提供建 node 的 API（node 由 P2 fetch 产生），故不变量 #2 用 repository 集成测试验证；`nodes` 的 API 仅 list/get/delete。
`result_large_err`（handler `parse_id` 返回大 `Response`）用 `#[allow]` 显式标注。

### 验收 / Acceptance

| 命令 / 行为 | 预期 |
|---|---|
| `cargo test --workspace` | 退出码 `0` |
| `cargo test --test integration` | 退出码 `0`，含上面两条 `#[tokio::test]` |
| `SQLX_OFFLINE=true cargo build --workspace` | 退出码 `0`（p1-03 已 prepare） |
| 经 API：`POST /api/v1/sources` 同 `type+content` ×2 | 第 1 次 `201 Created`；第 2 次 `200 OK`（body 为同一条），数据库 `sources` 仅 1 行 |
| 经 API：建 node 后关联到 2 个 source | `GET /api/v1/sources/{id}/nodes` 与 `node_sources` 计数分别为 1 节点 / 2 关系 |
| 缺 token 访问 `GET /api/v1/sources` | `401 Unauthorized`（p1-02 认证层仍生效） |
| `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 `0` |

示例（经 API 去重）：

```text
$ curl -i -X POST localhost:21380/api/v1/sources -H 'Authorization: Bearer <admin>' \
    -d '{"type":"subscribe","content":"https://same"}'
HTTP/1.1 201 Created            # 第一条
$ curl -i -X POST localhost:21380/api/v1/sources -H 'Authorization: Bearer <admin>' \
    -d '{"type":"subscribe","content":"https://same"}'
HTTP/1.1 200 OK                 # 第二条：返回已存在的 source
```

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Rust 交付物 | Phlogo（Go）对应 | 说明 |
|---|---|---|
| `SourceRepository::create_dedup` | `internal/source/service.go` `Create` L27-51 | 按 `type+content` 去重；`status='active'`、`ignore_dead=false` 初值 |
| `SourceService::create` | `internal/source/service.go` `Create`（返回 `(src, alreadyExisted, err)`） | 去重命中返回 `alreadyExisted=true`（handler/source.go L44-48 返 200） |
| `NodeRepository::upsert_by_hash` | `internal/node/upsert.go` L105-139（事务内 SELECT-then-INSERT） | `hash` 命中复用、`FOR UPDATE` 防并发双插 |
| `NodeRepository::link` | `internal/node/upsert.go` L136 `OnConflict{DoNothing}` | 幂等映射 |
| `SourceRepository::delete` | `internal/source/service.go` `Delete` L143-156 | 删 source + 其 `source_fetch_runs` + 调用 cleanup 端口（P1 仅删 source 与 fetch 历史，孤儿清理在 P2 完成） |
| API handler | `internal/api/handler/source.go`（L22-194）、`internal/api/handler/node.go`（L26-158） | 路径/响应形状对齐 `api/openapi/spec.yaml` |

### CI 提示 / CI notes

> 仅描述将来 CI 命令，**不落地 YAML**。

- **单元 / 集成分离**：`cargo test --workspace`（纯单元，可离线）、`cargo test --test integration`（需 Postgres，CONVENTIONS §5）。
- **集成测试数据库**：CI 用 `services: postgres` 或一个一次性数据库，注入 `DATABASE_URL`；迁移由 p1-03 的 `cargo sqlx migrate run` 先应用，再跑测试。
- **去重与关系不变量**：把上面两条 `#[tokio::test]` 作为**回归必过项**；任何改动 `create_dedup` / `upsert_by_hash` 的 PR 必须保持这两条通过（这是方案 §14 P1 验收 + 核心验收测试 §15 的前两条）。
- **lint/format**：`cargo fmt --check` + `cargo clippy -D warnings`（`--all-targets` 含 tests）。
- **nextest（可选提速）**：`cargo nextest run --workspace` 替代 `cargo test`，CI 更稳定。

### 风险 / Risks

- **去重竞态**：Phlogo 用事务内 `FOR UPDATE` + `OnConflict` 防并发双插（`upsert.go` L107、L136）。Rust 侧必须同样在事务里 `SELECT ... FOR UPDATE` 再 `INSERT`，否则并发创建同一 `type+content` 可能双插（破坏验收 #1）。单纯 `ON CONFLICT` 在 `sources` 没有唯一约束（只有 `type+content` 复合语义）时不够，需显式加 `UNIQUE(type, content)` 或事务锁——本页未加该唯一约束，需在评审时确认采用哪种方案。
- **`identity_hash` 算法未定**：P1 仅用占位 hash；完整 `NormalizedNode` 指纹（`internal/node/upsert.go` 的 `nodeHash`/`nodeAuth`/`normalizeCredential`）属 P2。验收 #2 用任意稳定 hash 验证关系不变量即可，但上线前须替换为方案 §7 的版本化指纹，否则不同协议字段变化不会触发新身份。
- **孤儿清理推迟**：P1 的 `SourceRepository::delete` 只删 source + fetch 历史，不删 `node_sources` 与孤儿 node（Phlogo 在 `Delete` 调 `cleanup.RemoveSourceMappings`，service.go L147）。孤儿/Orphaned/Retired 状态是 P4 重点，P1 暂不在 delete 时级联，需明确告知以免误以为关联已清理。
- **`status` 取值漂移**：Phlogo 当前只有 `active`/`dead`（model/source.go L16），方案 §6.1 扩展为 `Active`/`Suspended`/`Dead`。P1 迁移已预留 `suspended`，但 handler 输入校验须与最终枚举一致，避免新旧混用。
- **`configs` 读取路径**：p1-04 若需读评分/窗口配置，应走 `crates/config` 从 `configs` 表加载（而非硬编码），保持与 §2.6 种子一致。
