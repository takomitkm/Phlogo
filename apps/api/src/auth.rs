//! Layered auth (mirrors `internal/auth/{bearer,jwt}.go` + `cmd/server/main.go` dispatch).
//! P1-02 implements service-bearer and admin-JWT; checker layer lands in P3.
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

/// Middleware: accept if a valid service bearer OR a valid admin access JWT is present.
/// (P1-02 applies this to `/api/v1/*` stubs; finer checker/fetch split lands in P2/P3.)
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
