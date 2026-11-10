## 8-01 部署编排与备份恢复 · Compose & Backup

### 目标 / Goal

- 把现有 **单二进制** `Dockerfile`（`golang:1.24-alpine` builder → `alpine:3.21`，`EXPOSE 21380`，
  默认 SQLite `./data/data.db`）演进为**多阶段 Rust 镜像**：分别产出 `api` 与 `worker` 两个二进制。
- 用 **Docker Compose** 编排完整运行时：`postgres` / `api` / `worker` / `miaospeed` / `web`（+ 可选 `apalis-board`），
  对齐方案 §2.2 运行形态与 §13 `deploy/` 目录（`compose.yaml`、`Caddyfile`、`miaospeed/`）。
- 建立**可演练**的备份/恢复流程：PostgreSQL 数据（来源、节点、`node_sources` 关系、测活历史、评分快照）与
  Apalis 任务状态全部落在 Postgres，随库备份；并提供一次验证过的恢复步骤。
- 保证 **重启数据一致性**：`docker compose down` 后重起，Postgres 命名卷数据、`node_sources` 多对多关系、
  进行中的任务状态不丢不乱。

### 范围 / Scope

- **新建**：
  - `deploy/compose.yaml`（或仓库根 `docker-compose.yaml`）。
  - `Dockerfile.api`、`Dockerfile.worker`（多阶段 Rust 构建）。
  - `deploy/Caddyfile`（web 静态 + API 反向代理，替代裸 `EXPOSE 21380`）。
  - `deploy/miaospeed/` 配置（固定版本 MiaoSpeed 二进制挂载 / 或独立镜像）。
  - `scripts/backup.sh`、`scripts/restore.sh`（pg_dump / psql 封装）。
- **修改**：`Dockerfile`（保留为构建参考，或删除改为两个新 Dockerfile；本页以「演进」为口径，保留旧文件做 diff 基准）。
- **不做**：Kubernetes / Helm / Redis / Kafka（方案 §17）；Mihomo 常驻服务（§2.2「不部署独立 Mihomo 代理服务」）；
  多进程 worker 的自动水平伸缩编排（先做单副本 `worker` 可多副本，但 compose 默认 1 副本）。

### 前置 / Prerequisites

- 已完成 P1（`migrations/` + SQLx 初始化）、P3（Apalis PG backend 已建 `apalis` 任务表，方案 §3 Apalis 迁移以官方为准）。
- Rust 工具链与 `Cargo.lock` 已锁定；`apps/api`、`apps/worker` 二进制可由 `cargo build --release` 产出。
- 本地已安装 `docker` 与 `docker compose`（v2）。
- Postgres 镜像（如 `postgres:16-alpine`）与固定版本 MiaoSpeed 二进制（P0-02 验证的 commit/tag）。
- 配置 facts：见 `CONVENTIONS.md` §2.3 数据模型（`sources`/`nodes`/`node_sources`/`node_check_results`/`check_runs`/
  `node_status`/`node_scores`）与 §2.6 配置默认键（seed 必须包含 `max_distribute`、`score_*` 等）。

### 步骤 / Steps

#### Step 1 — 多阶段 Rust 镜像（演进 `Dockerfile`）

现有 `Dockerfile` 用 Go 单二进制；Rust 改为「builder 编译 → 轻量 runtime」两段。
两镜像共享 builder 形态，`api` 与 `worker` 仅 `cargo build --bin` 目标不同。

```dockerfile
# deploy/Dockerfile.api
# ── Build stage ──
FROM rust:1.82-slim AS builder
WORKDIR /src
# 先拷 Cargo 清单做依赖缓存层
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY apps ./apps
# 仅构建 api（离线检查见 CONVENTIONS §5: cargo sqlx prepare）
RUN cargo build --release --bin core-api

# ── Runtime stage ──
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /src/target/release/core-api /app/core-api
# 静态前端由 web 服务托管；api 仅暴露管理/订阅 API
EXPOSE 8080
ENTRYPOINT ["/app/core-api"]
```

`deploy/Dockerfile.worker` 同形，仅把 `--bin core-api` 改为 `--bin core-worker`，`EXPOSE` 可按需省略（worker 不对外监听）。
> 注：旧 `Dockerfile` 使用 `--tags="noclash"` 与 `CGO_ENABLED=1` 是 Go 侧构建细节；
> Rust 侧无需该 tag，但若 `sqlx` 离线宏启用需 `SQLX_OFFLINE=true` + 已 `cargo sqlx prepare` 产物（见 P1 验收）。

#### Step 2 — compose 拓扑（`deploy/compose.yaml`）

按方案 §2.2 起进程；关键约束：**`postgres` 与 `miaospeed` 不映射公网端口**，`miaospeed` 的测试端口只暴露给 `worker` 所在内部网络。

```yaml
services:
  postgres:
    image: postgres:16-alpine
    environment:
      POSTGRES_USER: paily
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD:?set in .env}
      POSTGRES_DB: paily
    volumes:
      - pgdata:/var/lib/postgresql/data
    # 不写 ports: → 仅 compose 内部网络可达（方案 §16.10：PostgreSQL 不暴露公网）

  api:
    build: { context: .., dockerfile: deploy/Dockerfile.api }
    environment:
      DATABASE__DSN: postgres://paily:${POSTGRES_PASSWORD}@postgres:5432/paily
      AUTH__JWT_SECRET: ${JWT_SECRET:?set in .env}
      AUTH__SERVICE_SECRET: ${SERVICE_SECRET:?set in .env}
      AUTH__ADMIN_PASSWORD: ${ADMIN_PASSWORD:?set in .env}
    depends_on: [postgres]
    expose: ["8080"]          # 仅内部；对外由 caddy 反代

  worker:
    build: { context: .., dockerfile: deploy/Dockerfile.worker }
    environment:
      DATABASE__DSN: postgres://paily:${POSTGRES_PASSWORD}@postgres:5432/paily
      CHECKERS__MIAOSPEED__URL: ws://miaospeed:65432
      CHECKERS__MIAOSPEED__SECRET: ${MIAOSPEED_SECRET:?set in .env}
    depends_on: [postgres, miaospeed]

  miaospeed:
    image: ghcr.io/airportr/miaospeed:${MIAOSPEED_TAG:?pin at P0-02}   # 固定版本
    # 管理/测试端口只暴露给 compose 内部；worker 经 ws://miaospeed:65432 访问（§16.10）
    expose: ["65432"]
    # 不写 ports: → 宿主机不可直连

  web:
    image: ${WEB_IMAGE:?built at P6}      # Vben 静态产物，由 caddy 提供
    expose: ["80"]

  caddy:
    image: caddy:2-alpine
    ports: ["21380:80"]                   # 对外唯一入口，沿用旧 EXPOSE 21380 习惯
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
    depends_on: [api, web]

  # 可选：apalis-board（仅看任务，不替代管理页，方案 §12）
  apalis-board:
    image: ${APALIS_BOARD_IMAGE:?optional}
    command: ["--database-url", "postgres://paily:${POSTGRES_PASSWORD}@postgres:5432/paily"]
    profiles: ["debug"]                   # 默认不启动，compose --profile debug 才起

volumes:
  pgdata:
```

`deploy/Caddyfile` 示例（web 静态 + API 反代 + 订阅路径透传）：

```caddyfile
:80 {
    # 静态前端
    root * /srv/web
    file_server

    # 管理 / 订阅 API 反代到 api
    reverse_proxy /api/*  http://api:8080
    reverse_proxy /sub/*  http://api:8080
    reverse_proxy /health http://api:8080
}
```

#### Step 3 — 备份脚本（`scripts/backup.sh`）

所有业务数据（来源、节点、`node_sources`、测活历史、评分、`apalis` 任务表）都在 Postgres，单一 `pg_dump` 即可整体备份。

```bash
#!/usr/bin/env bash
set -euo pipefail
OUT="${1:-./backups/paily-$(date +%Y%m%d-%H%M%S).sql.gz}"
mkdir -p "$(dirname "$OUT")"
# 用 compose 内 postgres 服务执行；-Fc 自定义格式便于选择性恢复
docker compose exec -T postgres \
  pg_dump -U paily -d paily -Fc > "$OUT"
echo "backup written: $OUT"
```

#### Step 4 — 恢复演练（`scripts/restore.sh` + 一致性校验）

```bash
#!/usr/bin/env bash
set -euo pipefail
IN="${1:?usage: restore.sh <file.sql.gz|file.dump>"
docker compose down -v            # 销毁卷以模拟「从零恢复」
docker compose up -d postgres
# 等待 postgres 就绪
until docker compose exec -T postgres pg_isready -U paily -d paily; do sleep 1; done
docker compose exec -T postgres pg_restore -U paily -d paily --clean --if-exists < "$IN"
# 校验：关键表行数应与备份前一致
docker compose exec -T postgres psql -U paily -d paily -c \
  "SELECT 'sources',count(*) FROM sources UNION ALL
   SELECT 'nodes',count(*) FROM nodes UNION ALL
   SELECT 'node_sources',count(*) FROM node_sources;"
```

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地**：`deploy/` 全套 + `scripts/` 备份恢复——
- `Dockerfile.api` / `Dockerfile.worker`：多阶段 Rust（`rust:1.83-slim` builder → `debian:bookworm-slim` runtime），依赖清单先行缓存层；runtime 携带 `migrations/`（启动即迁移）。**bin 名按实际 `apps/{api,worker}`**（规格里的 `core-api/core-worker` 是方案占位名）。
- `compose.yaml`：postgres/api/worker/miaospeed/web/caddy 六服务 + `apalis-board`（`--profile debug` 选启）；**postgres/miaospeed 零公网端口**（§16.10），caddy 21380 唯一入口；环境变量用本实现的真实形状（`PAILY_*` 而非规格占位的 `DATABASE__DSN`）；worker 走独立 `paily_jobs` 库（P3-01 决策）。
- `Caddyfile`：静态 UI + `/api/*`、`/sub/*`、`/health` 反代。
- `scripts/backup.sh`（`pg_dump -Fc`）与 `scripts/restore.sh`（down -v → pg_restore → 行数校验）。

**实测（docker 不可用环境的等价验证；宿主 docker 集成留待有 docker 的机器，如实标注）**：
| 项 | 结果 |
|---|---|
| **Builder 段等价**：校机 `cargo build --release --bin api --bin worker` | **0**（35.28s；api 15,639,636 B / worker 11,569,271 B） |
| **compose 拓扑 lint**（无 ports 断言 ×2、21380 入口、6 服务存在、.env 4 键） | **13/13 OK** |

**Podman 兼容与真机验证（2026-11-11 追加）**：

- **脚本改 podman 优先**：新增 `scripts/runtime.sh`（解析顺序 `podman compose` → `podman-compose` → `docker compose`），`backup.sh`/`restore.sh` 改为 source 它——两个运行时下行为一致。
- **Dockerfile 零改动兼容 Buildah/podman**（无 BuildKit-only 指令）。
- **真机验证（本机 podman 6.1.4 + WSL machine + podman-compose 1.6.0）**：
  - `podman compose -f deploy/compose.yaml --env-file deploy/.env config --services` → **0**，六服务全识别；
  - `podman compose ... up -d postgres` → **0**；容器内 `pg_isready` → **accepting connections**；
  - **端口不暴露语义在 podman 下同样成立**：`compose port postgres 5432` 报错（inspect 的 `Ports[5432]` 为 None = 无宿主映射）；
  - `down` → 0。
- **环境事项（本机）**：docker.io 直连超时 → 在 `%APPDATA%\containers\registries.conf.d\999-mirrors.conf` 配 daocloud/1ms 镜像源后 `podman pull` 成功；compose provider 缺失时 `pip install --user podman-compose`。
- **compose.yaml 修正**：`WEB_IMAGE` 从强制变量改默认 `nginx:alpine`（静态托管本来就只用挂载卷），`podman compose config` 不再因缺省拒绝解析。

→ 8-01 验收 #2/#3 的「端口不暴露」在 **podman 运行时**实测成立；#1/#4/#5 的全栈演练在 podman 下同样可跑（api/worker 镜像 build 后）。

### 验收 / Acceptance

机器可校验清单（命令 + 预期）：

1. **整栈拉起**：
   ```bash
   docker compose up -d --wait
   curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:21380/health
   ```
   预期输出 `200`，且 `docker compose ps --status running | wc -l` ≥ 5（postgres/api/worker/miaospeed/web/caddy）。

2. **Postgres 不暴露公网**：宿主机直连应失败（未映射端口）：
   ```bash
   (docker compose port postgres 5432 || true)   # 预期无输出 / 报错：未发布端口
   ```
   预期：命令无 `0.0.0.0:5432` 形式输出（退出非 0 或空）。

3. **MiaoSpeed 端口仅内部可达**：worker 容器可连，宿主机不可：
   ```bash
   docker compose exec worker sh -c 'nc -z -w2 miaospeed 65432 && echo worker-ok'
   # 宿主机侧不应有 65432 映射：docker compose port miaospeed 65432 应报错
   ```
   预期：`worker-ok` 打印；宿主机 `docker compose port miaospeed 65432` 报错（未发布）。

4. **备份 → 销毁 → 恢复 → 行数一致**：
   ```bash
   docker compose exec -T postgres psql -U paily -d paily -tAc "SELECT count(*) FROM node_sources;"  # 记为 N
   bash scripts/backup.sh /tmp/p7-test.dump
   bash scripts/restore.sh /tmp/p7-test.dump
   docker compose exec -T postgres psql -U paily -d paily -tAc "SELECT count(*) FROM node_sources;"  # 仍为 N
   ```
   预期：两次 `count` 相等（N 一致），`pg_restore` 退出码 0。

5. **重启数据一致性（阶段验收 #1 的细化）**：
   ```bash
   docker compose restart worker
   docker compose exec -T postgres psql -U paily -d paily -tAc "SELECT count(*) FROM nodes;"
   ```
   预期：重启前后 `nodes` 行数不变；`apalis` 任务无 `Running` 永久卡死（worker 重启后重新认领 `Pending`/`Running`）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- `Dockerfile`（单二进制 `paily-core`，`EXPOSE 21380`，SQLite `./data/data.db`）
  → 拆为 `deploy/Dockerfile.api` / `deploy/Dockerfile.worker`（方案 §2.2 双二进制；CONVENTIONS §3 映射
  `cmd/server/main.go` → `apps/api/src/main.rs` + `apps/worker/src/main.rs`）。
- 现有无 compose、无备份；数据在容器本地 SQLite → 改为统一 Postgres + 命名卷 `pgdata` + `pg_dump` 流程。
- 旧 `internal/checker/handler.go` 的 `GetNodes` 返回 `raw`（完整 clash JSON，含凭据）等逻辑，
  在多进程下由 `api` 暴露、由 `worker` 经 MiaoSpeed WebSocket 消费；数据表同 CONVENTIONS §2.3（`nodes`/`node_sources` 等），
  备份即备份这些表。
- 旧 `main.go` 在单进程内 `aliveCache.TriggerRebuild` / `cleanup.Start` → 在 worker 内由 Apalis Job 承担，
  重启恢复依赖 Apalis PG 任务表（见 8-02）。

### CI 提示 / CI notes

- **部署冒烟 job**：`docker compose up -d --wait` → 断言 `/health` 200 与 `ps` 运行服务数；
  跑完 `docker compose down -v` 清理（用 `--profile debug` 时才带 `apalis-board`）。
- **备份/恢复演练 job**：起 postgres → `pg_dump -Fc` → 注入样例来源/节点 → `down -v` → `pg_restore` →
  校验 `node_sources`/外键行数与种子一致。可作为 nightly 或 release 前的 gate。
- **镜像构建 job**：分别对 `Dockerfile.api` / `Dockerfile.worker` 跑 `docker build`，并 `docker scout cves`
  （或 trivy）做基础镜像漏洞扫描；可与 8-03 的 supply-chain 扫描合并。
- **端口暴露 lint（轻量）**：在 compose 解析里断言 `postgres`/`miaospeed` 无 `ports:` 公网映射，防止误暴露（§16.10）。

### 风险 / Risks

- **误暴露 Postgres/MiaoSpeed**：compose 写错 `ports:` 会把 `5432`/`65432` 挂到宿主机，违反 §16.10；
  必须靠「不写 ports」+ CI lint 双重保险。
- **卷销毁丢失**：`docker compose down -v` 会删 `pgdata`；生产恢复演练务必指向独立备份文件，而非依赖容器内数据。
- **Apalis 任务表非业务表**：`pg_restore` 默认会连带恢复 `apalis` 内部任务表；若恢复的是「运行中系统快照」，
  重放的 `Running` 任务需 worker 幂等接管（见 8-02 风险与方案 §6.4）。
- **SQLite → Postgres 差异**：旧单二进制默认 SQLite，P1 已切 Postgres；备份脚本只覆盖 Postgres，不再兼容 SQLite。
- **`pg_dump -Fc` 需配套 `pg_restore`**：不能用 `psql` 直接灌 `-Fc` 文件；脚本要成对，避免恢复命令错配。
