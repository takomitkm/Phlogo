//! API contract layer (utoipa OpenAPI). P1-02: minimal document served as JSON.
//! spec-first: `api/openapi/spec.yaml` remains the canonical contract; utoipa documents routes.
//! Note: utoipa-swagger-ui static assets are deferred (upstream build.rs vs zip 2.6 incompatibility);
//! the OpenAPI JSON is served at `/api-docs/openapi.json`.
use utoipa::OpenApi;

#[derive(OpenApi)]
#[openapi(info(
    title = "paily-core-rust API",
    version = "0.1.0",
    description = "Rust rebuild of paily-core (Phlogo). spec.yaml is the canonical contract."
))]
pub struct ApiDoc;
