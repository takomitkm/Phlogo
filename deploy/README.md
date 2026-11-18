# deploy/ — production topology (p7-01, podman-first)

- `compose.yaml` — postgres / curse / worker / miaospeed / web / caddy (+ optional apalis-board via `--profile debug`).
  postgres and miaospeed publish **no** host ports (plan §16.10); caddy on 21380 is the only ingress.
- `Dockerfile.curse` / `Dockerfile.worker` — multi-stage Rust builds (rust:1.83-slim builder → debian:bookworm-slim).
  **Buildah/podman compatible**: no BuildKit-only directives (the `# syntax` line is ignored by Buildah).
- `Caddyfile` — static UI + `/api/*`, `/sub/*`, `/health` reverse proxy; `/metrics` answers 403 at the edge (p7-03).
  Each reverse proxy to the `curse` service uses `transport http { proxy_protocol v1 }` (A1): the real client
  address is carried at the TCP layer, so the API's login rate limiter can bucket per client
  IP instead of seeing one shared proxy address — and it cannot be forged like `X-Forwarded-For`.
- `.env.example` — copy to `.env` before starting the stack.

## Runtime: podman first, docker as fallback

`../scripts/runtime.sh` (repo root, **not** `deploy/scripts/`) resolves the compose command in this order:
1. `podman compose` (podman 4.7+/6.x built-in provider forwarding)
2. `podman-compose`
3. `docker compose`

`../scripts/backup.sh` / `../scripts/restore.sh` source it — they work unchanged on either runtime.
Start the stack with podman:

```bash
podman machine start            # Windows/macOS only; Linux runs rootless directly
cp deploy/.env.example deploy/.env   # fill real secrets
podman compose -f deploy/compose.yaml --env-file deploy/.env up -d --wait
curl -s http://127.0.0.1:21380/health
```

Notes:
- rootless podman: published port 21380 is unprivileged, no root needed; the named volume
  `pgdata` lives in the user's storage, so `restore.sh`'s `down -v` only drops that user-owned volume.
- `podman compose` delegates to an external provider; if none is installed, `podman-compose` is the fallback.
- `docker compose` still works for CI images that ship docker (the compose file is spec-compliant).

Backup/restore: `../scripts/backup.sh` / `../scripts/restore.sh` (single `pg_dump -Fc` covers business
tables + apalis job state; restore drill verifies row counts).

## PROXY protocol (A1)

`curse` publishes **no host port** — it is reachable only from the compose network through caddy.
Caddy prepends a PROXY v1 header with the real client address; the API reads it at the TCP layer
(`apps/curse/src/proxy_protocol.rs`) and rejects connections without it when
`PHLOGO_SERVER_EXPECT_PROXY=1` (the default in `compose.yaml`).

If you run the API **without** caddy (e.g. `cargo run` for local development), leave
`PHLOGO_SERVER_PROXY_PROTOCOL` unset — the API then uses the TCP peer address and a plain
`axum::serve` path.

## 限流与反向代理的边界（部署前必读）

登录限流的桶键来自 `ConnectInfo<SocketAddr>`：PROXY 模式下是 **PROXY 头声明的真实 IP**，
直连模式（默认）是 **TCP peer**。它**从不读取** `X-Forwarded-For`（HTTP 头，边缘可伪造）。

| 你的入口 | 桶键 | 后果 |
| --- | --- | --- |
| 本仓 compose（caddy + `proxy_protocol v1` + `EXPECT_PROXY=1`） | 真实客户端 IP | ✅ 每客户端独立桶 |
| 代理**不支持 PROXY v1**（nginx 默认、Cloudflare、多数托管 LB）且 `PROXY_PROTOCOL` 关闭 | 代理 IP | ⚠️ **所有客户端共用一个桶**：10 次/分钟是**全局**额度，一个人就能把所有人挡在登录外 |
| 同上但 `EXPECT_PROXY=1` | —— | 连接在 TCP 层被拒（fail-closed），表现为 502 / 连接被断 |

**为什么现在不引入"可信代理"机制**：读 `X-Forwarded-For` 需要可信 CIDR 配置，配错等于允许
任何人伪造来源 IP（见 `docs/rebuild/10-review-crosscheck.md` §9 方案②）。PROXY 头同理——只有
在"API 端口不可直连"时才可信，所以 compose 里 `curse` 不发布宿主端口。

**如果你的代理不会说 PROXY v1**，三条不改代码的路：

1. 接受全局一个桶，调大 `PHLOGO_LOGIN_RATE_LIMIT_PER_MIN`（牺牲防爆破强度）；
2. 在代理/边缘做登录限流（如 caddy-ratelimit），API 侧不依赖该桶；
3. 换/加一层支持 `proxy_protocol` 的代理（Caddy：`transport http { proxy_protocol v1 }`）。

> 当前只有 `POST /api/v1/auth/login` 挂了限流；其余 `/api/v1/*` 的通用限流尚未挂载
> （登记于 `docs/rebuild/09-self-completed-gaps.md` §D）。
