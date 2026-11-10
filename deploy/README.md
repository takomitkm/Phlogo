# deploy/ — production topology (p7-01, podman-first)

- `compose.yaml` — postgres / api / worker / miaospeed / web / caddy (+ optional apalis-board via `--profile debug`).
  postgres and miaospeed publish **no** host ports (plan §16.10); caddy on 21380 is the only ingress.
- `Dockerfile.api` / `Dockerfile.worker` — multi-stage Rust builds (rust:1.83-slim builder → debian:bookworm-slim).
  **Buildah/podman compatible**: no BuildKit-only directives (the `# syntax` line is ignored by Buildah).
- `Caddyfile` — static UI + `/api/*`, `/sub/*`, `/health` reverse proxy; `/metrics` answers 403 at the edge (p7-03).
- `.env.example` — copy to `.env` before starting the stack.

## Runtime: podman first, docker as fallback

`scripts/runtime.sh` resolves the compose command in this order:
1. `podman compose` (podman 4.7+/6.x built-in provider forwarding)
2. `podman-compose`
3. `docker compose`

`backup.sh` / `restore.sh` source it — they work unchanged on either runtime.
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

Backup/restore: `scripts/backup.sh` / `scripts/restore.sh` (single `pg_dump -Fc` covers business
tables + apalis job state; restore drill verifies row counts).
