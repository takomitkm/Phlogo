#!/usr/bin/env bash
# p7-03 supply-chain checks: licenses, advisories, SBOM, binary hashes.
set -euo pipefail
cd "$(dirname "$0")/.."

echo "== cargo deny (licenses + bans) =="
cargo deny check licenses bans

echo "== cargo audit =="
cargo audit

echo "== SBOM (CycloneDX) =="
cargo cyclonedx --format json -o deploy/sbom.cdx.json 2>/dev/null \
  || echo "cargo-cyclonedx not installed; run: cargo install cargo-cyclonedx"

echo "== release binary hashes =="
for bin in api worker; do
  if [ -f "target/release/$bin" ]; then
    sha256sum "target/release/$bin"
  fi
done
echo "supply-chain checks done"
