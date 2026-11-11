//! Rate limiting (P7-03 Step 2): per-IP fixed-window limiter as an Axum layer.
//! Self-contained to avoid a heavyweight dependency; thresholds are config-driven
//! (`PAILY_SERVER_RATE_LIMIT_PER_MIN`, default 300) with a document floor.
use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Instant, Duration};

#[derive(Clone)]
pub struct RateLimitState {
    pub per_minute: u32,
    windows: Arc<Mutex<HashMap<std::net::IpAddr, (u32, Instant)>>>,
}

impl RateLimitState {
    pub fn new(per_minute: u32) -> Self {
        Self {
            per_minute,
            windows: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

pub async fn rate_limit(
    State(st): State<RateLimitState>,
    req: Request,
    next: Next,
) -> Response {
    // Identity: peer IP only (strip the ephemeral port) so connection rotation cannot
    // evade the bucket; X-Forwarded-For is never read (spoofable at the edge).
    let ip = req
        .extensions()
        .get::<SocketAddr>()
        .map(|a| a.ip())
        .unwrap_or_else(|| "0.0.0.0".parse().expect("valid ip"));
    let decision = {
        let mut map = st.windows.lock().expect("rate windows poisoned");
        let now = Instant::now();
        let entry = map.entry(ip).or_insert((0u32, now));
        if now.duration_since(entry.1) >= Duration::from_secs(60) {
            *entry = (0, now);
        }
        entry.0 += 1;
        entry.0 <= st.per_minute
    };
    if decision {
        next.run(req).await
    } else {
        (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response()
    }
}
