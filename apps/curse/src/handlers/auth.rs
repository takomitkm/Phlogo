//! Auth endpoints (P6 dependency): admin login issues a HS256 access token.
//! Mirrors Phlogo `internal/api/handler/auth.go` login semantics.
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct LoginReq {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Serialize)]
struct Claims {
    sub: String,
    token_type: String,
    exp: usize,
}

/// POST /api/v1/auth/login — admin login (PHLOGO_AUTH_ADMIN_PASSWORD check).
pub async fn login(State(st): State<AppState>, Json(req): Json<LoginReq>) -> Response {
    // Single-admin model (plan §17): username is informational; password is the gate.
    let expected = st.admin_password();
    if expected.is_empty() || req.password != expected {
        return (StatusCode::UNAUTHORIZED, "invalid credentials").into_response();
    }
    let exp = chrono::Utc::now().timestamp() as usize + 24 * 3600;
    let claims = Claims {
        sub: req.username,
        token_type: "access".into(),
        exp,
    };
    let jwt = match encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(st.jwt_secret().as_bytes()),
    ) {
        Ok(t) => t,
        Err(e) => return (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
    };
    Json(serde_json::json!({ "token": jwt, "token_type": "bearer", "expires_in": 86400 }))
        .into_response()
}
