# Paily Connect Core（Rust 重构版）

> 本仓库是 [`openpaily/paily-core`](https://github.com/openpaily/paily-core) 的 **Rust 重构 fork**。
> 原始 Go 实现已从本仓库移除，其行为以 fork 根提交 **`dda4e6f2`** 为对照基准
> （`git show dda4e6f2:<path>` 可查看任何原始文件）。逐路径等价性审计见
> [`docs/rebuild/11-equivalence-audit.md`](docs/rebuild/11-equivalence-audit.md)。

Paily Connect 是一个免费订阅项目：从互联网上的订阅源和节点源收集节点，经过解析、去重和测活后，
根据节点的各种表现综合评分，最后按地区和评分筛选，生成 Clash、Sing-box 或 Base64 订阅。

## 与上游的架构差异

上游由三个协作服务组成（paily-core / paily-fetch / paily-check，经 HTTP API + `service_secret` 通信）。
本重构将它们收敛为**一个进程对**，外部服务面被显式取消：

| 上游 | 本实现 |
| --- | --- |
| `paily-core`（Go + Gin + GORM） | `apps/api`（**Rust** + Axum + SQLx）— 管理面、订阅分发、评分、生命周期 |
| `paily-fetch`（外部抓取服务） | **取消** — `apps/worker` 的 Fetch 闭环直读数据库（Apalis 队列驱动），并新增上游没有的 **SSRF 防护**（`crates/fetch`） |
| `paily-check`（外部测活服务） | **取消** — worker 经 **MiaoSpeed WebSocket Adapter**（`crates/checker-miaospeed`）主动测活；协议认证签名与上游 Go 实现**逐字节一致**（golden 测试钉死） |
| expr-lang 字符串表达式 | **类型化 Filter-AST JSON**（`crates/filter`）：字段/运算符双白名单、参数化 SQL、深度/数量限制 |
| OpenAPI spec-first（oapi-codegen 生成） | **utoipa 为源头**：代码即契约，`GET /api-docs/openapi.json` 导出 27 条路径 |

 Worker 与数据库直连；队列使用独立的 `paily_jobs` 数据库（Apalis 内嵌迁移与业务迁移隔离，方案 §3.1）。

## 工作流程

```text
  订阅源 / 节点（管理 API 或 fetch intake 提交）
        │
        ▼
  worker · Fetch 闭环        SSRF 守卫 → 解析 → 指纹 → 对账（原子替换/累积）
        │
        ▼
  api · 节点池               去重（fingerprint v1）· 节点↔来源关联 · 生命周期状态机
        │
        ▼
  worker · 测活闭环          Apalis CheckJob → MiaoSpeed WS（初筛 + 复筛）
        │
        ▼
  api · 评分与分发           EWMA 评分（ewma-v1，分项可解释）→ Filter-AST 筛选 → 地区配额
        │
        ▼
  /sub/{token}.base64  /sub/{token}.yaml  /sub/{token}.json
```

- **抓取**：周期调度（worker 内置 300s tick）或 `POST /api/v1/fetch/trigger` 手动触发；`type=url` 源走 SSRF 守卫的 HTTP 抓取，`type=content` 源直接解析内联内容。
- **测活**：CheckJob 幂等（`check_run_id` 键控），结果迟到不覆盖、跨 tag 合并；历史按 `check_run_history_days`（默认 14 天）自动清理。
- **评分**：EWMA 时间加权，分项（延迟/抖动/稳定性/质量/速度）写入 `node_scores.components`，权重全部可配（`configs` 表）。
- **分发**：订阅经**哈希校验的一次性 token** 访问（可吊销、即时 401）；按档案（`subscription_profiles`）筛选——地区、健康、最低分、Filter-AST、地区配额（common/all/other 预设）、`{{field}}` 白名单命名模板 + **sponsor 文案注入**（priority 升序、后命中覆盖、`default_sponsor_text` 兜底）。

## 主要能力

- 源库管理：订阅源/节点源去重、手动触发抓取、抓取历史、失效源忽略。
- 节点池：fingerprint 去重、多来源关联、原子对账（失败保留旧关联）、孤儿节点保留窗口后物理删除。
- 测活与评分：MiaoSpeed WS 初筛+复筛、跨 tag 存活合并、EWMA 可解释评分、分级阈值可配。
- 筛选引擎：类型化 AST（白名单字段/运算符）、SQL 下推（`nodes/search`、`sources/search`、`filter/preview`）。
- 分发：Clash/Sing-box/Base64 三格式、每格式模板可自定义（`format-configs`，写入时校验）、ETag 语义的档案版本失效。
- 管理 API + Web 前端（Vben）：Dashboard / Sources / Nodes / Node Detail / Checks / Scoring / Subscriptions / **Sponsors** / 配置。
- 可观测性：`/metrics`（Prometheus）、JSON 结构化日志 + 敏感字段 redaction。
- 安全：三层身份（Public / AdminJWT / ServiceBearer）按**路由权限矩阵**强制（fail-closed，管理写仅 AdminJWT）、登录限流（IP 桶、PROXY protocol 真实 IP、不信任 XFF）、服务面最小化（仅 GET 读 + 源 intake + fetch trigger）。

## 部署

> [!IMPORTANT]
> **仅支持 PostgreSQL**（SQLx 迁移 0001–0011）。建议数据库配 NVMe、至少 2 核 4G。

容器化部署（podman 优先，docker 兼容）：

```bash
cd deploy
cp .env.example .env        # 填 POSTGRES_PASSWORD / SERVICE_SECRET / ADMIN_PASSWORD / JWT_SECRET
podman machine start        # 仅 Windows/macOS 需要
podman compose -f compose.yaml up -d --no-build postgres api worker caddy
curl -s http://127.0.0.1:21380/health
```

- caddy 是**唯一入口**（21380），并经 **PROXY protocol v1** 向 api 透传真实客户端 IP（限流按真实 IP 分桶；api 不发布宿主端口）。
- 镜像构建：`podman build -f deploy/Dockerfile.api -t paily-api:local .`（worker 同理）。
- 备份/恢复：`scripts/backup.sh` / `scripts/restore.sh`（`pg_dump -Fc` 覆盖业务表 + 队列库）。

本地开发（不经反代）：

```bash
export DATABASE_URL=postgres://paily:change-me@localhost:5432/paily
cargo run -p api        # 管理面（默认 21380）
cargo run -p worker     # 抓取/测活/清理闭环
```

登录：`POST /api/v1/auth/login`（`ADMIN_PASSWORD`）→ JWT（24h）。完整接口见运行时的
`/api-docs/openapi.json`，权限矩阵见 `apps/api/src/auth.rs::auth_kind`。

## 仓库结构

```text
apps/api       管理面：路由、权限矩阵、处理器、订阅分发
apps/worker    抓取/测活/清理闭环（Apalis PG 队列）
crates/        domain · database · registry · fetch · vproxy-adapter · checker-miaospeed
               lifecycle · scoring · filter · distribution · observability · config · api-contract
web/vben-admin 管理前端（Vben 5 + Element Plus）
deploy/        compose 拓扑 · Dockerfile · Caddyfile · postgres-init
migrations/    SQLx 版本化迁移（0001–0011）
docs/          领域文档 + rebuild/（重构全程记录）
```

## 开发

```bash
cargo build --workspace
cargo test --workspace                  # 单测 + 真实 PG 集成（需 DATABASE_URL）
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
```

- API 契约以 `crates/api-contract` 的 utoipa 注解为源头，`api/openapi/openapi.json` 为导出物（改动后重新生成）。
- 重构全程的分步实现记录、验收标准与踩坑：[`docs/rebuild/`](docs/rebuild/00-index.md)（00 索引 → 11 等价性审计）。

## 文档

| 文档 | 内容 |
| --- | --- |
| [源库与来源管理](docs/source-management.md) | 订阅源与节点、源库去重、死源判定、管理界面操作。 |
| [节点池](docs/node-pool.md) | 节点去重、节点与来源的关联、存活判定。 |
| [评分机制](docs/scoring.md) | 评分参数与计算（EWMA，`configs` 可配）。 |
| [分发](docs/distribution.md) | 订阅地址、档案、地区策略与选节点算法。 |
| [订阅模板](docs/templates.md) | 分发时的订阅模板配置与制作方法。 |
| [配置与 API](docs/configuration-and-api.md) | 启动配置、动态参数（含生效状态标注）、筛选引擎（Filter-AST）、命名与 Sponsor、API 权限。 |
| [rebuild/](docs/rebuild/00-index.md) | Rust 重构全程：11 个阶段页 + 交叉审查 + 等价性审计 + 行为总账。 |

## 许可证

本项目基于 GNU Affero General Public License v3.0（AGPL-3.0）发布，详见 [LICENSE](LICENSE)。
第三方组件义务见 [docs/THIRD_PARTY.md](docs/THIRD_PARTY.md)（MiaoSpeed 为外部进程，协议隔离不豁免 AGPL 义务）。
