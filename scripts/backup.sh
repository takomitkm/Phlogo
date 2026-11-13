#!/usr/bin/env bash
# p7-01 backup: single pg_dump covers all business data + apalis job state.
# Podman-first (scripts/runtime.sh), docker as fallback.
set -euo pipefail
cd "$(dirname "$0")/.."
# shellcheck source=runtime.sh
source "$(dirname "$0")/runtime.sh"
OUT="${1:-./backups/phlogo-$(date +%Y%m%d-%H%M%S).dump}"
mkdir -p "$(dirname "$OUT")"
runtime_compose -f deploy/compose.yaml exec -T postgres \
  pg_dump -U phlogo -d phlogo -Fc > "$OUT"
echo "backup written: $OUT"
