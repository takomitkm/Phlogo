#!/usr/bin/env bash
# p7-01 restore drill: destroy the volume, restore from a dump, verify row counts.
# Podman-first (scripts/runtime.sh), docker as fallback.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=runtime.sh
source "$(dirname "$0")/runtime.sh"
IN="${1:?usage: restore.sh <file.dump>}"
runtime_compose -f deploy/compose.yaml down -v
runtime_compose -f deploy/compose.yaml up -d postgres
until runtime_compose -f deploy/compose.yaml exec -T postgres pg_isready -U phlogo -d phlogo >/dev/null 2>&1; do
  sleep 1
done
runtime_compose -f deploy/compose.yaml exec -T postgres \
  pg_restore -U phlogo -d phlogo --clean --if-exists < "$IN"
echo "---- row counts after restore ----"
runtime_compose -f deploy/compose.yaml exec -T postgres psql -U phlogo -d phlogo -tAc \
  "SELECT 'sources',count(*) FROM sources UNION ALL
   SELECT 'nodes',count(*) FROM nodes UNION ALL
   SELECT 'node_sources',count(*) FROM node_sources UNION ALL
   SELECT 'check_runs',count(*) FROM check_runs;"
