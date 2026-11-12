//! API contract layer (utoipa OpenAPI). Served as JSON at `/api-docs/openapi.json`;
//! the exported document is the single source for the Vben TS client (p6-01).
//! Schema structs live in `schemas.rs` and are glob-imported here because utoipa's
//! `#[path]`/`#[openapi]` macros resolve type names in the enclosing scope.
mod paths2;
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
        paths2::views::stats,
        paths2::views::checkers,
        paths2::views::logs_check,
        paths2::views::logs_fetch,
        paths2::views::node_initial_checks,
        paths2::views::node_deep_checks,
        paths2::subscriptions_admin::list_profiles,
        paths2::subscriptions_admin::create_profile,
        paths2::subscriptions_admin::update_profile,
        paths2::subscriptions_admin::delete_profile,
        paths2::subscriptions_admin::list_tokens,
        paths2::subscriptions_admin::issue_token,
        paths2::subscriptions_admin::revoke_token,
        paths2::search::nodes_search,
        paths2::search::sources_search,
        paths2::search::preview,
        paths2::search::format_configs_list,
        paths2::search::format_config_get,
        paths2::search::format_config_update,
        paths2::sponsors::list,
        paths2::sponsors::create,
        paths2::sponsors::get,
        paths2::sponsors::update,
        paths2::sponsors::delete,
        paths2::misc::login,
        paths2::misc::trigger,
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
