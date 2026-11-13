//! HTTP handlers (P1: sources + nodes; P6-02: config; P7-02: metrics; P7-03: subscriptions; views).
pub mod auth;
pub mod config;
pub mod fetch_trigger;
pub mod format_config;
pub mod metrics;
pub mod node;
pub mod search;
pub mod source;
pub mod sponsor_admin;
pub mod subscription;
pub mod subscriptions_admin;
pub mod views;
