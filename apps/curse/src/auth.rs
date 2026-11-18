//! Layered auth (mirrors `cmd/server/main.go` dispatch, tightened per review §1).
//! Three identities:
//! - Public:  /health, /metrics, /sub/{token}, /api-docs, login
//! - AdminJWT: full management surface (all reads and writes)
//! - ServiceBearer: **exactly two** approved actions — `POST /sources` (fetch intake)
//!   and `POST /fetch/trigger` (orchestration). Everything else is AdminOnly.
//!
//! **Minimal-privilege ruling (user, 2026-11-18)**: the previous matrix granted
//! ServiceBearer broad read access through prefix wildcards (`GET sources/*`,
//! `GET logs/*`, `GET config`, `GET stats`, ...). Those endpoints expose
//! subscription content (`SourceView.content`), the entire config KV table and run
//! history, so "read-only" is **not** harmless. The matrix is now an exact
//! Method + full-path whitelist (no `_` wildcards, no prefix inheritance); any route
//! not listed below — including every future route — is AdminOnly until someone
//! explicitly adds it here with a proven service consumer.
//!
//! Fail-closed: any /api/v1 route not present in the whitelist requires AdminJWT.
use axum::{
    extract::{Request, State},
    http::{header::AUTHORIZATION, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

#[derive(Clone)]
pub struct AuthState {
    pub service_secret: String,
    pub jwt_secret: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Public,
    AdminOrService,
    AdminOnly,
}

fn bearer_token(req: &Request) -> Option<String> {
    let h = req.headers().get(AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = h.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    Some(token.trim().to_string())
}

/// HS256 access-token verification (mirrors `internal/auth/jwt.go`: token_type == "access").
pub fn verify_jwt(secret: &str, token: &str) -> bool {
    use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Claims {
        token_type: Option<String>,
    }

    match decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::new(Algorithm::HS256),
    ) {
        Ok(data) => data.claims.token_type.as_deref() == Some("access"),
        Err(_) => false,
    }
}

/// Route permission matrix (P0-01, tightened 2026-11-18).
///
/// Path normalization: middleware inside `nest("/api/v1", ..)` sees the prefix
/// STRIPPED (`/sources`), while callers/tests may pass the full path
/// (`/api/v1/sources`) — both are handled by stripping the optional prefix.
///
/// Matching is **exact**: the method and the complete segment vector must both
/// match. No `_` wildcards, no prefix inheritance — `POST /sources/{id}/anything`
/// therefore falls through to AdminOnly instead of inheriting `POST /sources`.
/// Everything unmatched is AdminOnly (fail-closed).
pub fn auth_kind(method: &str, path: &str) -> AuthKind {
    let p = path.strip_prefix("/api/v1").unwrap_or(path);
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    let m = method.to_ascii_uppercase();

    // Public: the only credential-free endpoint inside /api/v1 (it issues the JWT).
    if m == "POST" && segs == ["auth", "login"] {
        return AuthKind::Public;
    }

    // ServiceBearer whitelist — exactly two actions, full method + full path.
    // A new service consumer must be approved explicitly before being added here.
    if m == "POST" && segs == ["sources"] {
        return AuthKind::AdminOrService;
    }
    if m == "POST" && segs == ["fetch", "trigger"] {
        return AuthKind::AdminOrService;
    }

    // Everything else: AdminJWT only. This includes all reads — the management
    // surface deliberately does not separate "read" from "write" for services.
    AuthKind::AdminOnly
}

/// Middleware enforcing the matrix. 401 = missing/bad credentials; 403 = ServiceBearer
/// hitting an AdminOnly route.
pub async fn require_api_auth(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let kind = auth_kind(req.method().as_str(), req.uri().path());
    if kind == AuthKind::Public {
        return next.run(req).await;
    }
    let token = bearer_token(&req);
    let ok_service = token
        .as_deref()
        .map(|t| !auth.service_secret.is_empty() && t == auth.service_secret)
        .unwrap_or(false);
    let ok_admin = token
        .as_deref()
        .map(|t| !auth.jwt_secret.is_empty() && verify_jwt(&auth.jwt_secret, t))
        .unwrap_or(false);

    let admin = ok_admin || (ok_service && kind == AuthKind::AdminOrService);
    if admin {
        return next.run(req).await;
    }
    if ok_service && kind == AuthKind::AdminOnly {
        return (
            StatusCode::FORBIDDEN,
            "service identity may not perform admin writes",
        )
            .into_response();
    }
    (StatusCode::UNAUTHORIZED, "missing or invalid credentials").into_response()
}
