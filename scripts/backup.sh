#!/usr/bin/env bash
# p7-01 backup: single pg_dump covers all business data + apalis job state.
set -euo pipefail
cd "$(dirname "$0")/.."
OUT="${1:-./backups/paily-$(date +%Y%m%d-%H%M%S).dump}"
mkdir -p "$(dirname "$OUT")"
docker compose -f deploy/compose.yaml exec -T postgres \
  pg_dump -U paily -d paily -Fc > "$OUT"
echo "backup written: $OUT"
