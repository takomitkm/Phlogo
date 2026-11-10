//! tracing initialization (mirrors Phlogo zerolog ConsoleWriter setup).
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

pub fn init(level: &str) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(level));
    tracing_subscriber::registry()
        .with(fmt::layer())
        .with(filter)
        .init();
}
