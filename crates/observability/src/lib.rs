//! Observability (P7-02): structured JSON logging + global secret redaction.
//! Plan §16.8: node credentials, subscription URL tokens, subscription tokens,
//! checker/MiaoSpeed secrets must never reach the logs.
pub mod redact;

pub use redact::{redact_secret, redact_subscription_url, RedactionLayer, SECRET_FIELD_NAMES};

use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};

/// Initialize the global JSON subscriber (api + worker share this).
pub fn init(service: &str) {
    let fmt_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_current_span(true)
        .with_span_list(false)
        .with_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")));
    tracing_subscriber::registry()
        .with(fmt_layer)
        .with(redact::RedactionLayer)
        .init();
    tracing::info!(service, "observability initialized");
}
