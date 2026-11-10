//! API contract layer (utoipa OpenAPI). Served as JSON at `/api-docs/openapi.json`;
//! the exported document is the single source for the Vben TS client (p6-01).
//! Schema structs live in `schemas.rs` and are glob-imported here because utoipa's
//! `#[path]`/`#[openapi]` macros resolve type names in the enclosing scope.
mod schemas;

use schemas::*;
use utoipa::OpenApi;

/// Documented routes (HTTP wiring in `apps/api/src/router.rs`).
mod paths_mod {
    use super::*;

    #[utoipa::path(
        operation_id = "SourceCreate",
        post,
        path = "/api/v1/sources",
        request_body = CreateSourceReq,
        responses(
            (status = 200, description = "dedup hit", body = CreateSourceResp),
            (status = 201, description = "created", body = CreateSourceResp),
            (status = 401, description = "unauthorized"),
        )
    )]
    #[allow(dead_code)]
    pub fn create() {}

    #[utoipa::path(
        operation_id = "SourceList",
        get,
        path = "/api/v1/sources",
        responses(
            (status = 200, description = "list", body = [SourceView]),
            (status = 401, description = "unauthorized"),
        )
    )]
    #[allow(dead_code)]
    pub fn list() {}

    #[utoipa::path(
        operation_id = "SourceGet",
        get,
        path = "/api/v1/sources/{id}",
        params(("id" = Uuid, Path, description = "source id")),
        responses(
            (status = 200, description = "found", body = SourceView),
            (status = 404, description = "not found"),
        )
    )]
    #[allow(dead_code)]
    pub fn get() {}

    #[utoipa::path(
        operation_id = "SourcePatch",
        patch,
        path = "/api/v1/sources/{id}",
        request_body = PatchSourceReq,
        responses(
            (status = 200, description = "updated", body = SourceView),
            (status = 404, description = "not found"),
        )
    )]
    #[allow(dead_code)]
    pub fn patch() {}

    #[utoipa::path(
        operation_id = "SourceDelete",
        delete,
        path = "/api/v1/sources/{id}",
        responses(
            (status = 204, description = "deleted"),
            (status = 404, description = "not found"),
        )
    )]
    #[allow(dead_code)]
    pub fn delete() {}

    #[utoipa::path(
        operation_id = "SourceIgnoreDead",
        post,
        path = "/api/v1/sources/{id}/ignore-dead",
        responses((status = 200, description = "marked"), (status = 404))
    )]
    #[allow(dead_code)]
    pub fn set_ignore_dead() {}

    #[utoipa::path(
        operation_id = "SourceNodes",
        get,
        path = "/api/v1/sources/{id}/nodes",
        params(("id" = Uuid, Path)),
        responses((status = 200, description = "nodes", body = [NodeView]))
    )]
    #[allow(dead_code)]
    pub fn nodes() {}
}

/// Documented node routes.
pub mod node_paths {
    use super::*;

    #[utoipa::path(
        operation_id = "NodeList",
        get,
        path = "/api/v1/nodes",
        responses(
            (status = 200, description = "list", body = [NodeView]),
            (status = 401, description = "unauthorized"),
        )
    )]
    #[allow(dead_code)]
    pub fn list() {}

    #[utoipa::path(
        operation_id = "NodeGet",
        get,
        path = "/api/v1/nodes/{id}",
        params(("id" = Uuid, Path)),
        responses(
            (status = 200, description = "found", body = NodeView),
            (status = 404, description = "not found"),
        )
    )]
    #[allow(dead_code)]
    pub fn get() {}

    #[utoipa::path(
        operation_id = "NodeDelete",
        delete,
        path = "/api/v1/nodes/{id}",
        responses(
            (status = 204, description = "deleted"),
            (status = 404, description = "not found"),
        )
    )]
    #[allow(dead_code)]
    pub fn delete() {}
}

/// Documented health route.
pub mod health_path {
    #[utoipa::path(
        operation_id = "Health",
        get,
        path = "/health",
        responses((status = 200, description = "ok"))
    )]
    #[allow(dead_code)]
    pub fn health() {}
}

#[derive(OpenApi)]
#[openapi(
    info(
        title = "paily-core-rust API",
        version = "0.1.0",
        description = "Rust rebuild of paily-core (Phlogo). spec.yaml is the canonical contract."
    ),
    paths(
        paths_mod::create,
        paths_mod::list,
        paths_mod::get,
        paths_mod::patch,
        paths_mod::delete,
        paths_mod::set_ignore_dead,
        paths_mod::nodes,
        node_paths::list,
        node_paths::get,
        node_paths::delete,
        health_path::health,
    ),
    components(schemas(
        CreateSourceReq,
        PatchSourceReq,
        SourceView,
        CreateSourceResp,
        NodeView,
    ))
)]
pub struct ApiDoc;
