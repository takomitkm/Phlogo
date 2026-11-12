//! Router assembly (mirrors `cmd/server/main.go` dispatch tree).
use axum::{
    http::{header, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{delete, get, post, put},
    Router,
};
use utoipa::OpenApi;

use app_config::Config;

use crate::auth::{require_api_auth, AuthState};
use crate::handlers::{
    auth, config, fetch_trigger, format_config, metrics, node, search, source, sponsor_admin,
    subscription, subscriptions_admin, views,
};
use crate::state::AppState;

async fn health() -> &'static str {
    "ok"
}

async fn openapi_json() -> impl IntoResponse {
    match api_contract::ApiDoc::openapi().to_json() {
        Ok(j) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            j,
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain")],
            e.to_string(),
        ),
    }
}

pub fn build(cfg: Config) -> Router {
    // Lazy pool: no connection until a handler issues a query, so health/auth tests work without DB.
    let pool = database::connect_lazy(&cfg.database.dsn).expect("invalid database dsn");
    let state = AppState::new(&cfg, pool);

    let auth = AuthState {
        service_secret: cfg.auth.service_secret.clone(),
        jwt_secret: cfg.auth.jwt_secret.clone(),
    };

    // P0-02: login-specific limiter (tight threshold, peer-addr keyed; XFF is not trusted).
    // Login lives OUTSIDE /api/v1 so the matrix middleware doesn't wrap it; the limiter
    // is applied via route_layer on this mini-router only.
    let login_limit = std::env::var("PAILY_LOGIN_RATE_LIMIT_PER_MIN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let login_state = crate::rate_limit::RateLimitState::new(login_limit);
    let login = Router::new()
        .route("/api/v1/auth/login", post(auth::login))
        .route_layer(middleware::from_fn_with_state(
            login_state,
            crate::rate_limit::rate_limit,
        ))
        .with_state(state.clone());

    let api_v1 = Router::new()
        .route("/fetch/trigger", post(fetch_trigger::trigger))
        .route("/sources", get(source::list).post(source::create))
        .route(
            "/sources/:id",
            get(source::get)
                .patch(source::patch_source)
                .delete(source::delete),
        )
        .route("/sources/:id/ignore-dead", post(source::set_ignore_dead))
        .route("/sources/:id/nodes", get(source::nodes))
        .route("/nodes", get(node::list))
        .route("/nodes/search", post(search::nodes_search))
        .route("/sources/search", post(search::sources_search))
        .route("/filter/preview", post(search::preview))
        .route("/nodes/:id", get(node::get).delete(node::delete))
        .route("/nodes/:id/checks/initial", get(views::node_initial_checks))
        .route("/nodes/:id/checks/deep", get(views::node_deep_checks))
        .route("/stats", get(views::stats))
        .route("/checkers", get(views::checkers))
        .route("/logs/check", get(views::logs_check))
        .route("/logs/fetch", get(views::logs_fetch))
        .route(
            "/subscriptions",
            get(subscriptions_admin::list_profiles).post(subscriptions_admin::create_profile),
        )
        .route(
            "/subscriptions/:id",
            put(subscriptions_admin::update_profile).delete(subscriptions_admin::delete_profile),
        )
        .route(
            "/subscriptions/:id/tokens",
            get(subscriptions_admin::list_tokens).post(subscriptions_admin::issue_token),
        )
        .route(
            "/subscriptions/tokens/:id",
            delete(subscriptions_admin::revoke_token),
        )
        .route("/config", get(config::get_config).put(config::put_config))
        .route(
            "/sponsors",
            get(sponsor_admin::list).post(sponsor_admin::create),
        )
        .route(
            "/sponsors/:id",
            get(sponsor_admin::get)
                .patch(sponsor_admin::update)
                .delete(sponsor_admin::delete),
        )
        .route(
            "/format-configs",
            get(format_config::list),
        )
        .route(
            "/format-configs/:format",
            get(format_config::get).put(format_config::update),
        )
        .with_state(state.clone())
        // P0-01: matrix-enforcing auth (fail-closed AdminOnly for unmatched routes).
        // Applied AFTER with_state so the middleware stack isn't reset by the
        // state remap (axum 0.7 quirk observed: layer-then-with_state dropped it).
        .layer(middleware::from_fn_with_state(
            auth,
            require_api_auth,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/metrics", get(metrics::metrics).with_state(state.clone()))
        .route("/sub/:spec", get(subscription::sub))
        .merge(login)
        .nest("/api/v1", api_v1)
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
        .with_state(state)
}

#[cfg(test)]
mod smoke {
    use super::build;
    use app_config::Config;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn status_of(req: Request<Body>) -> StatusCode {
        build(Config::default())
            .oneshot(req)
            .await
            .unwrap()
            .status()
    }

    #[tokio::test]
    async fn health_returns_200() {
        let req = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::OK);
    }

    #[tokio::test]
    async fn protected_without_token_returns_401() {
        let get_req = Request::builder()
            .uri("/api/v1/stats")
            .body(Body::empty())
            .unwrap();
        let resp = build(Config::default()).oneshot(get_req).await.unwrap();
        let (parts, body) = resp.into_parts();
        let bytes = axum::body::to_bytes(body, 8192).await.unwrap_or_default();
        assert_eq!(
            parts.status,
            StatusCode::UNAUTHORIZED,
            "GET /stats body={}",
            String::from_utf8_lossy(&bytes)
        );
    }

    #[tokio::test]
    async fn unknown_path_returns_404() {
        let req = Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn openapi_json_returns_200() {
        let req = Request::builder()
            .uri("/api-docs/openapi.json")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::OK);
    }
}
