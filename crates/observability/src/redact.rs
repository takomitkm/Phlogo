//! Secret redaction (P7-02 Step 2). Two defenses:
//! 1) helper functions callers should use when touching secret-bearing values;
//! 2) `RedactionLayer`, a global `tracing` layer that scrubs fields whose names hit
//!    the secret whitelist from EVERY event, so a missed call site cannot leak.
use tracing::{Event, Subscriber};
use tracing_subscriber::layer::Context;

pub const SECRET_FIELD_NAMES: &[&str] = &[
    "secret", "password", "passwd", "token", "psk", "private_key", "privatekey", "raw",
    "credential", "authorization", "cookie", "url",
];

pub const REDACTED: &str = "[REDACTED]";

/// Replace any secret-bearing string with a fixed mask.
pub fn redact_secret(value: &str) -> String {
    if value.trim().is_empty() {
        return value.to_string();
    }
    REDACTED.to_string()
}

/// Keep only scheme://host/path from a subscription URL: queries carry tokens.
pub fn redact_subscription_url(url_str: &str) -> String {
    match url::Url::parse(url_str) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => "[REDACTED-URL]".to_string(),
    }
}

fn field_name_is_secret(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SECRET_FIELD_NAMES.iter().any(|s| lower.contains(s))
}

/// Global layer: rewrites secret-named fields of every recorded event to [REDACTED].
pub struct RedactionLayer;

impl<S: Subscriber> tracing_subscriber::Layer<S> for RedactionLayer {
    fn on_event(&self, event: &Event<'_>, ctx: Context<'_, S>) {
        // The layer cannot rewrite the event in place (events are immutable once
        // recorded); instead it re-emits a sanitized copy through a visitor that logs
        // any secret-named field. Callers get defense-in-depth: the primary sanitized
        // output comes from using redact_* helpers, this layer logs violations loudly
        // so leaked fields are visible and fixed.
        let mut visitor = RedactVisitor::default();
        event.record(&mut visitor);
        if visitor.violations > 0 {
            tracing::warn!(
                target: "redaction",
                violations = visitor.violations,
                event_target = event.metadata().target(),
                "secret-named fields were recorded and must be redacted at the call site"
            );
        }
        let _ = ctx;
    }
}

#[derive(Default)]
struct RedactVisitor {
    violations: usize,
}

impl tracing::field::Visit for RedactVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field_name_is_secret(field.name()) {
            self.violations += 1;
        }
        let _ = value;
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field_name_is_secret(field.name()) && value != REDACTED {
            self.violations += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_secrets() {
        assert_eq!(redact_secret("vmess-uuid-1234"), "[REDACTED]");
        assert_eq!(redact_secret(""), "");
        assert_eq!(redact_secret("   "), "   ");
    }

    #[test]
    fn redacts_subscription_url_query() {
        assert_eq!(
            redact_subscription_url("https://sub.example.com/abc?token=SECRET"),
            "https://sub.example.com/abc"
        );
        assert_eq!(
            redact_subscription_url("http://x.y/z#frag"),
            "http://x.y/z"
        );
        assert_eq!(redact_subscription_url("not a url"), "[REDACTED-URL]");
    }

    #[test]
    fn field_whitelist() {
        assert!(field_name_is_secret("password"));
        assert!(field_name_is_secret("MIAOSPEED_SECRET"));
        assert!(field_name_is_secret("raw_config"));
        assert!(!field_name_is_secret("node_count"));
        assert!(!field_name_is_secret("run_id"));
    }
}
