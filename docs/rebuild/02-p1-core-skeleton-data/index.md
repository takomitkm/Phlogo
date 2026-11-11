# 阶段一 · P1 Core 骨架与数据层（Core Skeleton & Data Layer）

> 本目录属于 `docs/rebuild/`，事实来源与页面模板见 `../CONVENTIONS.md`。
> 本阶段只做「骨架 + 数据层 + Source/Node 基础 CRUD」，不实现抓取、测活、评分、订阅发布。
> 本期不含任何 CI/Workflow 文件，仅在每页「CI 提示」中沉淀将来 CI 的命令种子。

## 阶段目标 / Phase Goal

把 `openpaily/paily-core`（Phlogo fork）的 Go 单体，在 Rust 侧立起**可编译、可启动、可迁移、可 CRUD** 的最小骨架：

1. 建立 Cargo workspace，按方案 §13 划分 `apps/{api,worker}` 与 `crates/{domain,database,config,api-contract,...}`。
2. 立 Axum + `tracing` + 配置加载（Figment，`PAILY_` 前缀 env 覆盖）的服务骨架，OpenAPI 仍为唯一契约。
3. 用 SQLx + PostgreSQL 写下初始 migration（方案 §5.1 + CONVENTIONS §2.3 / §2.6）。
4. 实现 Source CRUD（按 `type+content` 去重）、Node CRUD 与 Node-Source 多对多关系（CONVENTIONS §2.9 / `internal/source/service.go` / `internal/node/upsert.go`）。

## 子步骤清单 / Sub-steps

| # | 页面 | 内容 |
|---|---|---|
| 1 | [p1-01-workspace.md](./p1-01-workspace.md) | Cargo workspace 布局、工具链、baseline 依赖、fmt/clippy 基线 |
| 2 | [p1-02-api-skeleton.md](./p1-02-api-skeleton.md) | Axum + utoipa 骨架、分层认证中间件、tracing、配置加载 |
| 3 | [p1-03-migrations.md](./p1-03-migrations.md) | SQLx + PostgreSQL 初始 migration（sources/nodes/node_sources/source_fetch_runs + KV seed） |
| 4 | [p1-04-source-node-crud.md](./p1-04-source-node-crud.md) | Source 去重 CRUD、Node CRUD + Node-Source 关系、repository 层与集成测试 |

## 阶段级验收 / Phase Acceptance

以下条件**全部满足**才视为 P1 完成（后续均可直接转写为 CI `run:`）。**状态：已完成，P1 PASS（2026-11-10，真实 PG 18.6 验证）**：

1. **Workspace 可编译**：`cargo build --workspace --all-targets` 退出码 0，`cargo fmt --all -- --check` 与 `cargo clippy --workspace --all-targets -- -D warnings` 均 0 警告。
2. **迁移可应用**：`cargo sqlx migrate run` 在干净的测试 PostgreSQL 上成功；`SQLX_OFFLINE=true cargo sqlx prepare` 离线编译通过。
3. **API 可启动并暴露契约**：`cargo run -p api` 后 `GET /health` 返回 200；未授权访问 `POST /api/v1/sources` 返回 401；未知路径返回 404；OpenAPI 文档（`utoipa-swagger-ui` 或既有 `api/openapi/spec.yaml`）可访问。
4. **Source & Node CRUD 经 API 可用**：创建/读取/列表/更新/删除 source 与 node、节点关联来源均符合 CONVENTIONS §2.10 的 27 个 path 中本阶段应实现的部分。
5. **去重正确性**（核心验收，来自方案 §14 P1 验收 & CONVENTIONS §2.9）：
   - 用**完全相同 `type+content`** 连续创建两个 source，第二次返回**已存在的那条**（HTTP 200，`alreadyExisted=true`），不产生第二条 `sources` 行。
   - 创建一个 node 并把它关联到**两个 source**，结果只有 **1 行 `nodes`** 与 **2 行 `node_sources`**（`#![cfg(test)]` 或 `#[tokio::test]` 集成测试覆盖）。

## 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| P1 交付物 | 对应 Phlogo（Go）代码 |
|---|---|
| workspace / 两个二进制 | `cmd/server/main.go`（单体装配） → 拆为 `apps/api` + `apps/worker`（CONVENTIONS §3 映射表） |
| API 骨架 + 分层认证 | `cmd/server/main.go` 的 auth dispatch（L117-141）+ `internal/auth/{bearer,jwt,handler}.go` |
| 初始 migration | `internal/db/model/{source,node}.go`（GORM 模型）+ `internal/migrate/migrate.go`（GORM AutoMigrate → SQLx migrations） |
| Source CRUD | `internal/source/service.go`（`Create` 按 `type+content` 去重，`List/Get/Update/Delete/SetIgnoreDead`） |
| Node CRUD + Node-Source 关系 | `internal/node/upsert.go`（`UpsertFromFetchResult`、`removeSourceMappings`）+ `internal/db/model/node.go`（`NodeSource` 复合 PK） |

## 阶段不做 / Out of Scope（与方案 §17 MVP 不做项对齐）

- 不实现 `fetch`/`fetcher`（P2）、MiaoSpeed 测活（P3）、评分（P4）、Filter/Subscription（P5）。
- 不实现 `node_payloads` 的加密存储细节、`node_status`/`node_scores`/测活历史表的写入（P3/P4）。
- 不实现 `subscription_profiles` / `subscription_tokens`（P5）。
- 不引入 Redis/Kafka/ClickHouse/Kubernetes（方案 §4）。

## 风险 / Risks

- **Apalis 内部表名未知**：方案 §5.1 明确警告「不要在业务表里直接假设 Apalis 内部任务表的名称」。P1 的 migration 只创建本系统业务表，Apalis 的 `apalis_*` 表由 **Apalis 官方 migration 在 P0 验证后**写入，**不得手写**。
- **SQLx 离线模式**：所有带 `?` 占位符的查询必须能在 `SQLX_OFFLINE=true` 下编译；需要 `cargo sqlx prepare` 生成 `sqlx-data.json`，CI 默认开启离线校验。
- **指纹算法（identity_hash）不在 P1 完整实现**：P1 只落 `nodes` 表结构与基本的 Node 实体 CRUD；完整 `NormalizedNode` 指纹（`internal/node/upsert.go` 的 `nodeHash`/`nodeAuth`）属 P2，但表结构须预留 `identity_hash` + `fingerprint_version`（方案 §5.1 / CONVENTIONS §2.3）。
