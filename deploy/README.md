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
