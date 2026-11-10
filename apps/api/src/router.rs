//! Router assembly (mirrors `cmd/server/main.go` dispatch tree).
use axum::{
    http::{header, StatusCode},
    middleware,
    response::IntoResponse,
    routing::{get, post},
    Router,
};
use utoipa::OpenApi;

use app_config::Config;

use crate::auth::{require_admin_or_service, AuthState};
use crate::handlers::{config, metrics, node, source, subscription};
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
    let state = AppState::new(pool);

    let auth = AuthState {
        service_secret: cfg.auth.service_secret.clone(),
        jwt_secret: cfg.auth.jwt_secret.clone(),
    };

    let api_v1 = Router::new()
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
        .route("/nodes/:id", get(node::get).delete(node::delete))
        .route("/config", get(config::get_config).put(config::put_config))
        .layer(middleware::from_fn_with_state(
            auth,
            require_admin_or_service,
        ))
        .with_state(state.clone());

    Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .route("/metrics", get(metrics::metrics).with_state(state.clone()))
        .route("/sub/:spec", get(subscription::sub))
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
        let req = Request::builder()
            .method("POST")
            .uri("/api/v1/sources")
            .body(Body::empty())
            .unwrap();
        assert_eq!(status_of(req).await, StatusCode::UNAUTHORIZED);
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
