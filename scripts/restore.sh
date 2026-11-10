#!/usr/bin/env bash
# p7-01 restore drill: destroy the volume, restore from a dump, verify row counts.
set -euo pipefail
cd "$(dirname "$0")/.."
IN="${1:?usage: restore.sh <file.dump>}"
docker compose -f deploy/compose.yaml down -v
docker compose -f deploy/compose.yaml up -d postgres
until docker compose -f deploy/compose.yaml exec -T postgres pg_isready -U paily -d paily >/dev/null 2>&1; do
  sleep 1
done
docker compose -f deploy/compose.yaml exec -T postgres \
  pg_restore -U paily -d paily --clean --if-exists < "$IN"
echo "---- row counts after restore ----"
docker compose -f deploy/compose.yaml exec -T postgres psql -U paily -d paily -tAc \
  "SELECT 'sources',count(*) FROM sources UNION ALL
   SELECT 'nodes',count(*) FROM nodes UNION ALL
   SELECT 'node_sources',count(*) FROM node_sources UNION ALL
   SELECT 'check_runs',count(*) FROM check_runs;"
