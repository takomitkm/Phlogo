# P7 生产化 · Productionization

> 阶段编号：**P7**（文档目录 `08-p7-productionization/`）。
> 上游依据：`paily-core-rust-rebuild-plan.md` §2.2（运行形态）、§13（仓库/部署目录）、
> §14（P7 阶段）、§16（安全/许可证/供应链）；`CONVENTIONS.md` §2.3（数据模型）、§2.11（许可证与依赖）。
> 现状基准：`Phlogo` 当前为 Go 单体，由 `Dockerfile` 构建为**单二进制** `paily-core`
> （`golang:1.24-alpine` → `alpine:3.21`，`EXPOSE 21380`），无 compose、无备份流程、无结构化日志。

---

## 1. 阶段目标（Phase Goal）

把单体重写后的 Rust 多进程服务（`core-api` + `core-worker` + `postgres` + `miaospeed` + `web`）
从「本地能跑」推进到「可运维投产」：

- 用 **Docker Compose** 描述整套运行时拓扑，替换现有单二进制 `Dockerfile` 镜像。
- 建立可演练的 **备份/恢复** 流程（PostgreSQL 数据 + 来源/节点关系 + 任务状态）。
- 提供 **结构化日志（tracing 替换 zerolog）**、**指标端点**、以及 **worker 重启后 Apalis 任务恢复**。
- 落实 **API 限流 + 认证分层 + SSRF 防护 + 依赖与 SBOM 扫描**。
- 重申 **进程隔离不是许可证豁免**：MiaoSpeed（AGPL）/Mihomo（GPL）/VProxy（MIT）义务仍须履行。

本阶段不引入 Redis / Kafka / ClickHouse / Kubernetes（方案 §17、§19 MVP 不作事项）。

---

## 2. 子步骤清单（Sub-steps）

| 编号 | 标题 | 文件 | 要点 |
|---|---|---|---|
| **8-01** | 部署编排与备份恢复 · Compose & Backup | [p7-01-compose-backup.md](p7-01-compose-backup.md) | 多阶段 `api`/`worker` 镜像、compose 拓扑、pg_dump/restore 演练、重启数据一致性 |
| **8-02** | 可观测性 · Observability | [p7-02-observability.md](p7-02-observability.md) | tracing 结构化日志（无秘密）、metrics 端点、Apalis 任务恢复 |
| **8-03** | 安全加固 · Security Hardening | [p7-03-security-hardening.md](p7-03-security-hardening.md) | 限流 + 认证分层（service bearer / admin JWT / checker secret）、SSRF、cargo-deny + cargo-audit |

---

## 3. 阶段级验收（Phase-level Acceptance）

以下三条是 P7 的**总验收门槛**，需逐条用命令或脚本证明；它们分别由 8-01 / 8-02 / 8-03 落细：

1. **重启一致性（Restart consistency）** — `docker compose down` 后再 `docker compose up`，
   数据库（来源、节点、`node_sources` 多对多关系、测活历史、评分快照）与**进行中的 Apalis 任务状态**
   保持一致：已入队的 Job 可被重新认领并继续，未完成的 fetch/check 不丢、不重复产生节点关联。
   - 机器校验：`docker compose down -v` 之外的数据落 `postgres` 命名卷；重起后
     `docker compose exec postgres psql -U paily -d paily -c "SELECT count(*) FROM nodes;"` 行数不变；
     `apalis` 内部任务表无孤儿 `Pending`/`Running` 卡死（重起后 worker 认领并推进）。

2. **日志无秘密（No secrets in logs）** — 对 `check/nodes`、`check/results`、`fetch/*` 等携带秘密的端点
   与 MiaoSpeed WebSocket 建连，**日志记录中不得出现**：节点原始凭据（密码/UUID/`raw` clash JSON 中的 `psk`/`private-key`/`password`）、
   订阅 URL 中的 token、subscription Token、checker secret、MiaoSpeed secret。
   - 机器校验：跑一次 fetch + check 流程并 `docker compose logs api worker 2>&1 | grep -Ei "<secret-value-pattern>"`，
     退出码 `1`（未命中）即为通过；同时 `cargo test` 内含「凭据/URL/token/secret 被 redact」断言（见 8-02 验收）。

3. **未授权不可达（Unauthenticated cannot reach management / subscription）** —
   无 `Authorization`/service bearer 时，`POST /api/v1/sources`、`GET /api/v1/nodes`、`GET /api/v1/checks`
   等管理接口返回 `401`/`403`；订阅拉取（`/sub/{token}`）在无有效 token 时返回 `401`/`403`（撤销/失效 token 同理）。
   公开面仅 `/health`、`/:format`(无 token 的裸订阅除外)、`/api/v1/auth/*`。
   - 机器校验：见 8-03 验收的 curl 断言（管理 `401/403`、订阅 `401/403`）。

---

## 4. 与 Phlogo 现状对照（Mapping from current Phlogo）

| 现状（Phlogo） | P7 目标（Rust 多进程） |
|---|---|
| `Dockerfile` 单二进制 `paily-core`，`EXPOSE 21380`，SQLite 默认 `./data/data.db` | 多阶段 `api`/`worker` 镜像（Rust）；统一 PostgreSQL；compose 编排 `postgres`/`api`/`worker`/`miaospeed`/`web`(+`apalis-board`) |
| `cmd/server/main.go` 单进程装配全部 service + 起 goroutine（`cleanup.Start`、`aliveCache.TriggerRebuild`） | `apps/api/src/main.rs` 仅 API；`apps/worker/src/main.rs` 承担 fetch/check/score/cleanup Job（Apalis PG backend） |
| 无备份流程、数据在容器内 SQLite | `pg_dump` / 恢复演练；Postgres 数据命名卷持久化 |
| `internal/checker/handler.go` 用 zerolog；`main.go` 用 zerolog.ConsoleWriter | `tracing` 结构化日志，secret 字段经 redaction layer |
| 认证分层在 `main.go` 中间件 dispatch（service bearer/admin JWT/checker secret） | 同分层落到 Axum middleware，并叠加限流；checker secret 仅用于 `/api/v1/check/*` |
| 无 SSRF 约束（fetch 由外部 `paily-fetch` 回调） | Rust Fetch Worker 内置 SSRF 防护（方案 §16.9 / P2-01） |

---

## 5. CI 提示（CI notes）

本阶段对应的未来 CI job（**只描述，不落地 YAML**）：

- **部署冒烟（deploy smoke）**：构建 `api`/`worker` 镜像，`docker compose up -d --wait`，
  断言 `/health` 返回 200，断言 8-03 的未授权 `401/403` 检查通过。
- **备份恢复演练（backup drill）**：在 CI 内起 postgres + 注入样例数据，`pg_dump` → 销毁卷 → `psql` 恢复 →
  校验行数与外键关系一致。
- **日志 redaction 测试**：`cargo test --test integration` 内含「secret 不落日志」断言（见 8-02）。
- **安全扫描 gate**：`cargo deny check licenses` 与 `cargo audit` 在 merge gate 中必须零异常退出；
  `cargo-deny` 的 `advisories` 与 `bans`/`sources` 配置见 8-03。

---

## 6. 风险（Risks）

- **Postgres 暴露面**：compose 默认会把 `5432` 顺手映射到宿主机；必须显式只放内部网络，避免公网可达（方案 §16.10）。
- **MiaoSpeed 端口**：其管理/测试端口只应被 `worker` 访问，需在 compose 网络层隔离（§16.10）。
- **Apalis 任务幂等**：worker 重启后任务恢复的前提是 Job 实现幂等（方案 §6.4）；非幂等 Job 重放会产生重复关联/清理。
- **许可证误判**：进程隔离 `miaospeed` 不等于免除 AGPL/GPL 义务（§16.1/§16.2）；分发时仍需附源码/许可证文本。
- **日志误记**：`check/nodes` 的 `raw` 字段含完整 clash 配置（含密码/UUID），redaction 必须在序列化层统一兜底，不能依赖各 handler 手动脱敏。
