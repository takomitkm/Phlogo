//! Layered auth (mirrors `cmd/server/main.go` dispatch, tightened per review §1).
//! Three identities:
//! - Public:  /health, /metrics, /sub/{token}, /api-docs, login
//! - AdminJWT: full management surface (all writes)
//! - ServiceBearer: read-only + the two legitimate service actions
//!   (POST /sources for fetch intake, POST /fetch/trigger for orchestration)
//!
//! Fail-closed: any /api/v1 route not present in the matrix requires AdminJWT.
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

/// Route permission matrix (P0-01). Method+path -> required identity.
/// Path normalization: middleware inside `nest("/api/v1", ..)` sees the prefix
/// STRIPPED (`/stats`), while callers/tests may pass the full path
/// (`/api/v1/stats`) — both are handled by stripping the optional prefix.
/// Everything unmatched is AdminOnly (fail-closed).
pub fn auth_kind(method: &str, path: &str) -> AuthKind {
    let p = path.strip_prefix("/api/v1").unwrap_or(path);
    let p = p.strip_prefix('/').unwrap_or(p);
    let segs: Vec<&str> = p.split('/').filter(|s| !s.is_empty()).collect();
    if segs.is_empty() {
        return AuthKind::AdminOnly;
    }
    use AuthKind::*;
    let m = method.to_ascii_uppercase();
    let (s0, s1, s2) = (
        segs.first().copied(),
        segs.get(1).copied(),
        segs.get(2).copied(),
    );
    match (m.as_str(), s0, s1, s2) {
        ("POST", Some("auth"), Some("login"), _) => Public,
        // sources
        ("GET", Some("sources"), None, _) => AdminOrService,
        ("POST", Some("sources"), None, _) => AdminOrService, // fetch intake (upstream sourceCreateMW)
        ("POST", Some("sources"), Some("search"), _) => AdminOrService, // AST search (read)
        ("GET", Some("sources"), Some(_), None) => AdminOrService,
        ("GET", Some("sources"), Some(_), Some("nodes")) => AdminOrService,
        ("PATCH", Some("sources"), Some(_), _) => AdminOnly,
        ("DELETE", Some("sources"), Some(_), _) => AdminOnly,
        ("POST", Some("sources"), Some(_), Some("ignore-dead")) => AdminOnly,
        // nodes
        ("GET", Some("nodes"), None, _) => AdminOrService,
        ("POST", Some("nodes"), Some("search"), _) => AdminOrService, // AST search (read)
        ("GET", Some("nodes"), Some(id), None) if UuidLike::is(id) => AdminOrService,
        ("GET", Some("nodes"), Some(_), Some("checks")) => AdminOrService,
        ("DELETE", Some("nodes"), Some(_), _) => AdminOnly,
        // filter preview (read-only dry run)
        ("POST", Some("filter"), Some("preview"), _) => AdminOrService,
        // reads
        ("GET", Some("stats"), _, _) => AdminOrService,
        ("GET", Some("checkers"), _, _) => AdminOrService,
        ("GET", Some("logs"), _, _) => AdminOrService,
        // config
        ("GET", Some("config"), _, _) => AdminOrService,
        ("PUT", Some("config"), _, _) => AdminOnly,
        // format templates: read for any service identity, write is admin-only
        ("GET", Some("format-configs"), _, _) => AdminOrService,
        ("PUT", Some("format-configs"), _, _) => AdminOnly,
        // fetch trigger: the one legitimate service-identity write
        ("POST", Some("fetch"), Some("trigger"), _) => AdminOrService,
        // subscriptions admin surface: AdminOnly for every method
        (_, Some("subscriptions"), _, _) => AdminOnly,
        // fail-closed default for anything else under /api/v1
        _ => AdminOnly,
    }
}

/// Minimal UUID shape check (8-4-4-4-12 hex segments) without pulling a dep into auth.
#[allow(non_snake_case)]
mod UuidLike {
    pub fn is(s: &str) -> bool {
        let parts: Vec<&str> = s.split('-').collect();
        if parts.len() != 5 {
            return false;
        }
        let lens = [8usize, 4, 4, 4, 12];
        parts
            .iter()
            .zip(lens)
            .all(|(p, l)| p.len() == l && p.chars().all(|c| c.is_ascii_hexdigit()))
    }
}

/// Middleware enforcing the matrix. 401 = missing/bad credentials; 403 = ServiceBearer
/// hitting an AdminOnly route.
pub async fn require_api_auth(
    State(auth): State<AuthState>,
    req: Request,
    next: Next,
) -> Response {
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
        return (StatusCode::FORBIDDEN, "service identity may not perform admin writes")
            .into_response();
    }
    (StatusCode::UNAUTHORIZED, "missing or invalid credentials").into_response()
}

/// Kept for compatibility with existing callers/tests that only need the combined check.
pub async fn require_admin_or_service(
    State(auth): State<AuthState>,
    req: Request,
    next: Next,
) -> Response {
    let token = bearer_token(&req);
    let ok_service = token
        .as_deref()
        .map(|t| !auth.service_secret.is_empty() && t == auth.service_secret)
        .unwrap_or(false);
    let ok_admin = token
        .as_deref()
        .map(|t| !auth.jwt_secret.is_empty() && verify_jwt(&auth.jwt_secret, t))
        .unwrap_or(false);
    if ok_service || ok_admin {
        next.run(req).await
    } else {
        (StatusCode::UNAUTHORIZED, "missing or invalid credentials").into_response()
    }
}
