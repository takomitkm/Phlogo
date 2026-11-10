//! Router assembly (mirrors `cmd/server/main.go` dispatch tree).
use axum::{
    extract::Path,
    http::{header, StatusCode},
    middleware,
    response::IntoResponse,
    routing::get,
    Router,
};

use app_config::Config;
use utoipa::OpenApi;

use crate::auth::{require_admin_or_service, AuthState};

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

// P1-02 stubs (real handlers land in p1-04): 501 when authenticated.
async fn stub_list_sources() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "sources list not implemented (p1-04)",
    )
}

async fn stub_create_source() -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "sources create not implemented (p1-04)",
    )
}

async fn stub_node_get(Path(_id): Path<String>) -> impl IntoResponse {
    (
        StatusCode::NOT_IMPLEMENTED,
        "node get not implemented (p1-04)",
    )
}

pub fn build(cfg: Config) -> Router {
    let auth = AuthState {
        service_secret: cfg.auth.service_secret.clone(),
        jwt_secret: cfg.auth.jwt_secret.clone(),
    };

    let api_v1 = Router::new()
        .route("/sources", get(stub_list_sources).post(stub_create_source))
        .route("/nodes/:id", get(stub_node_get))
        .layer(middleware::from_fn_with_state(
            auth,
            require_admin_or_service,
        ));

    Router::new()
        .route("/health", get(health))
        .route("/api-docs/openapi.json", get(openapi_json))
        .nest("/api/v1", api_v1)
        .fallback(|| async { (StatusCode::NOT_FOUND, "not found") })
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
