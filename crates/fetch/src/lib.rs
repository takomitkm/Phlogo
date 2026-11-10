//! Controlled fetch: manual redirects (each hop re-checked), size + timeout limits, SSRF guard.
//! Phlogo had no SSRF protection (fetch happened in an external service); this is net-new.
pub mod ssrf;

use std::time::Duration;

use bytes::Bytes;
use ipnetwork::IpNetwork;

#[derive(Debug, Clone)]
pub struct FetchPolicy {
    pub connect_timeout: Duration,
    pub total_timeout: Duration,
    pub max_response_bytes: usize,
    pub max_redirects: usize,
    /// IPs allowed to bypass the blocklist (tests pin a loopback mock server here).
    pub allowlist: Vec<IpNetwork>,
}

impl Default for FetchPolicy {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            total_timeout: Duration::from_secs(60),
            max_response_bytes: 16 * 1024 * 1024,
            max_redirects: 5,
            allowlist: Vec::new(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("ssrf: {0}")]
    Ssrf(#[from] ssrf::SsrfError),
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("timeout")]
    Timeout,
    #[error("too many redirects")]
    TooManyRedirects,
    #[error("redirect without location")]
    RedirectWithoutLocation,
    #[error("response too large: {0}")]
    ResponseTooLarge(usize),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
}

fn map_req(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Timeout
    } else {
        FetchError::Http(e)
    }
}

pub async fn fetch(url: &str, policy: &FetchPolicy) -> Result<Bytes, FetchError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(policy.connect_timeout)
        .timeout(policy.total_timeout)
        .build()?;

    let mut next = url.to_string();
    let mut hops = 0usize;
    loop {
        ssrf::guard(&next, &policy.allowlist).await?;
        let resp = client.get(&next).send().await.map_err(map_req)?;
        let status = resp.status();

        if status.is_redirection() {
            if hops >= policy.max_redirects {
                return Err(FetchError::TooManyRedirects);
            }
            let loc = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .ok_or(FetchError::RedirectWithoutLocation)?
                .to_str()
                .map_err(|_| FetchError::InvalidUrl("bad location header".into()))?
                .to_string();
            let base =
                reqwest::Url::parse(&next).map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
            let abs = base
                .join(&loc)
                .map_err(|e| FetchError::InvalidUrl(e.to_string()))?;
            next = abs.to_string();
            hops += 1;
            continue;
        }

        if !status.is_success() {
            return Err(FetchError::Http(
                resp.error_for_status().expect_err("non-success status"),
            ));
        }

        let mut out: Vec<u8> = Vec::new();
        let mut resp = resp;
        while let Some(chunk) = resp.chunk().await.map_err(map_req)? {
            if out.len() + chunk.len() > policy.max_response_bytes {
                return Err(FetchError::ResponseTooLarge(out.len() + chunk.len()));
            }
            out.extend_from_slice(&chunk);
        }
        return Ok(Bytes::from(out));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{response::Redirect, routing::get, Router};
    use std::net::SocketAddr;

    async fn spawn(app: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}")
    }

    fn allow_loopback() -> FetchPolicy {
        FetchPolicy {
            allowlist: vec!["127.0.0.1/32".parse().unwrap()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn benign_fetch_ok() {
        let app = Router::new().route("/", get(|| async { "hello" }));
        let base = spawn(app).await;
        let body = fetch(&base, &allow_loopback()).await.unwrap();
        assert_eq!(&body[..], b"hello");
    }

    #[tokio::test]
    async fn redirect_to_metadata_blocked() {
        let app = Router::new().route(
            "/",
            get(|| async { Redirect::temporary("http://169.254.169.254/") }),
        );
        let base = spawn(app).await;
        let err = fetch(&base, &allow_loopback()).await.unwrap_err();
        assert!(matches!(
            err,
            FetchError::Ssrf(ssrf::SsrfError::BlockedAddress(_))
        ));
    }

    #[tokio::test]
    async fn metadata_literal_blocked() {
        let err = fetch(
            "http://169.254.169.254/latest/meta-data/",
            &FetchPolicy::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Ssrf(ssrf::SsrfError::BlockedAddress(_))
        ));
    }

    #[tokio::test]
    async fn response_too_large() {
        let app = Router::new().route("/", get(|| async { "0123456789" }));
        let base = spawn(app).await;
        let policy = FetchPolicy {
            max_response_bytes: 4,
            ..allow_loopback()
        };
        let err = fetch(&base, &policy).await.unwrap_err();
        assert!(matches!(err, FetchError::ResponseTooLarge(_)));
    }

    #[tokio::test]
    async fn timeout_hit() {
        let app = Router::new().route(
            "/",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(500)).await;
                "late"
            }),
        );
        let base = spawn(app).await;
        let policy = FetchPolicy {
            total_timeout: Duration::from_millis(100),
            ..allow_loopback()
        };
        let err = fetch(&base, &policy).await.unwrap_err();
        assert!(matches!(err, FetchError::Timeout));
    }
}
