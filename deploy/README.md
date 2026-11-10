# deploy/ — production topology (p7-01)

- `compose.yaml` — postgres / api / worker / miaospeed / web / caddy (+ optional apalis-board via `--profile debug`).
  postgres and miaospeed publish **no** host ports (plan §16.10); caddy on 21380 is the only ingress.
- `Dockerfile.api` / `Dockerfile.worker` — multi-stage Rust builds (rust:1.83-slim builder → debian:bookworm-slim).
- `Caddyfile` — static UI + `/api/*`, `/sub/*`, `/health` reverse proxy.
- `.env.example` — copy to `.env` before `docker compose up`.

Backup/restore: `scripts/backup.sh` / `scripts/restore.sh` (single `pg_dump -Fc` covers business
tables + apalis job state; restore drill verifies row counts).
