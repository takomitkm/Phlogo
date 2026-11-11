//! utoipa path stubs for the P6/P7 endpoints (B5 closure): stats, checkers, logs,
//! node check timelines, subscriptions admin. HTTP wiring in `router.rs` must stay
//! in sync; schemas are inline JSON (Value) so no ToSchema components needed here.
//! NOTE: these use `body = Value` — typed schemas land with the next contract pass.

/// Documented view routes.
pub mod views {
    #[utoipa::path(
        operation_id = "StatsGet",
        get,
        path = "/api/v1/stats",
        responses((status = 200, description = "dashboard counters", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn stats() {}

    #[utoipa::path(
        operation_id = "CheckerList",
        get,
        path = "/api/v1/checkers",
        responses((status = 200, description = "checker tags", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn checkers() {}

    #[utoipa::path(
        operation_id = "LogCheckList",
        get,
        path = "/api/v1/logs/check",
        params(("node_id" = Option<uuid::Uuid>, Query, description = "filter by node")),
        responses((status = 200, description = "recent initial checks", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn logs_check() {}

    #[utoipa::path(
        operation_id = "LogFetchList",
        get,
        path = "/api/v1/logs/fetch",
        params(("source_id" = Option<uuid::Uuid>, Query, description = "filter by source")),
        responses((status = 200, description = "fetch run history", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn logs_fetch() {}

    #[utoipa::path(
        operation_id = "NodeGetInitialChecks",
        get,
        path = "/api/v1/nodes/{id}/checks/initial",
        params(("id" = super::Uuid, Path)),
        responses((status = 200, description = "initial check timeline", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn node_initial_checks() {}

    #[utoipa::path(
        operation_id = "NodeGetDeepChecks",
        get,
        path = "/api/v1/nodes/{id}/checks/deep",
        params(("id" = super::Uuid, Path)),
        responses((status = 200, description = "deep check timeline", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn node_deep_checks() {}
}

/// Documented subscription-admin routes.
pub mod subscriptions_admin {
    #[utoipa::path(
        operation_id = "SubscriptionProfileList",
        get,
        path = "/api/v1/subscriptions",
        responses((status = 200, description = "profiles", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn list_profiles() {}

    #[utoipa::path(
        operation_id = "SubscriptionProfileCreate",
        post,
        path = "/api/v1/subscriptions",
        request_body = serde_json::Value,
        responses((status = 201, description = "created", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn create_profile() {}

    #[utoipa::path(
        operation_id = "SubscriptionProfileUpdate",
        put,
        path = "/api/v1/subscriptions/{id}",
        params(("id" = super::Uuid, Path)),
        request_body = serde_json::Value,
        responses((status = 200, description = "updated (version bumped)"))
    )]
    #[allow(dead_code)]
    pub fn update_profile() {}

    #[utoipa::path(
        operation_id = "SubscriptionProfileDelete",
        delete,
        path = "/api/v1/subscriptions/{id}",
        params(("id" = super::Uuid, Path)),
        responses((status = 204, description = "deleted"))
    )]
    #[allow(dead_code)]
    pub fn delete_profile() {}

    #[utoipa::path(
        operation_id = "SubscriptionTokenList",
        get,
        path = "/api/v1/subscriptions/{id}/tokens",
        params(("id" = super::Uuid, Path)),
        responses((status = 200, description = "token statuses", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn list_tokens() {}

    #[utoipa::path(
        operation_id = "SubscriptionTokenIssue",
        post,
        path = "/api/v1/subscriptions/{id}/tokens",
        params(("id" = super::Uuid, Path)),
        responses((status = 201, description = "plaintext token (one-time)", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn issue_token() {}

    #[utoipa::path(
        operation_id = "SubscriptionTokenRevoke",
        delete,
        path = "/api/v1/subscriptions/tokens/{id}",
        params(("id" = super::Uuid, Path)),
        responses((status = 204, description = "revoked"))
    )]
    #[allow(dead_code)]
    pub fn revoke_token() {}
}

/// Documented auth + fetch-trigger routes.
pub mod misc {
    #[utoipa::path(
        operation_id = "AuthLogin",
        post,
        path = "/api/v1/auth/login",
        request_body = serde_json::Value,
        responses(
            (status = 200, description = "JWT issued", body = serde_json::Value),
            (status = 401, description = "invalid credentials"),
        )
    )]
    #[allow(dead_code)]
    pub fn login() {}

    #[utoipa::path(
        operation_id = "FetchTrigger",
        post,
        path = "/api/v1/fetch/trigger",
        responses((status = 200, description = "fetch jobs enqueued", body = serde_json::Value))
    )]
    #[allow(dead_code)]
    pub fn trigger() {}
}
