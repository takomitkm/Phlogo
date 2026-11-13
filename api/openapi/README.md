# api/openapi — canonical contract

- `spec.json` — the OpenAPI 3.1 document exported from the Rust API
  (`GET /api-docs/openapi.json` on a running `gnosis-api`). Single source for the
  Vben TypeScript client (`web/vben-admin/src/api/generated/schema.d.ts`).
- Regenerate: boot the API and `curl /api-docs/openapi.json > spec.json`.
- utoipa paths/schemas are declared in `crates/api-contract/src/lib.rs`
  (HTTP wiring in `apps/curse/src/router.rs` must stay in sync).
