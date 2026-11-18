//! Permission matrix tests.
//!
//! P0-01 introduced the matrix; the 2026-11-18 minimal-privilege ruling replaced
//! prefix wildcards with an exact Method + full-path whitelist. These tests pin the
//! **shape** of that whitelist: exactly two ServiceBearer actions, everything else
//! AdminOnly, and extra path segments never inherit a parent's permission.
use curse::auth::{auth_kind, AuthKind};

const UUID: &str = "6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f";

#[test]
fn login_is_public() {
    assert_eq!(auth_kind("POST", "/api/v1/auth/login"), AuthKind::Public);
    // The login path is public for POST only, and only at its exact length.
    assert_eq!(auth_kind("GET", "/api/v1/auth/login"), AuthKind::AdminOnly);
    assert_eq!(
        auth_kind("POST", "/api/v1/auth/login/x"),
        AuthKind::AdminOnly
    );
    assert_eq!(auth_kind("POST", "/api/v1/auth"), AuthKind::AdminOnly);
}

#[test]
fn service_whitelist_is_exactly_two_actions() {
    // Absolute form (what tests callers use).
    assert_eq!(
        auth_kind("POST", "/api/v1/sources"),
        AuthKind::AdminOrService
    );
    assert_eq!(
        auth_kind("POST", "/api/v1/fetch/trigger"),
        AuthKind::AdminOrService
    );
    // Relative form — what the nest middleware actually sees (prefix stripped).
    assert_eq!(auth_kind("POST", "sources"), AuthKind::AdminOrService);
    assert_eq!(auth_kind("POST", "fetch/trigger"), AuthKind::AdminOrService);
}

#[test]
fn service_cannot_read_the_management_surface() {
    // Every GET on the management surface is AdminOnly: reads expose subscription
    // content (`SourceView.content`), the config KV table and run history.
    let reads = [
        "/api/v1/stats".to_string(),
        "/api/v1/checkers".to_string(),
        "/api/v1/config".to_string(),
        "/api/v1/logs/check".to_string(),
        "/api/v1/logs/fetch".to_string(),
        "/api/v1/sources".to_string(),
        format!("/api/v1/sources/{UUID}"),
        format!("/api/v1/sources/{UUID}/nodes"),
        "/api/v1/nodes".to_string(),
        format!("/api/v1/nodes/{UUID}"),
        format!("/api/v1/nodes/{UUID}/checks/initial"),
        format!("/api/v1/nodes/{UUID}/checks/deep"),
        "/api/v1/format-configs".to_string(),
        "/api/v1/format-configs/clash".to_string(),
        "/api/v1/subscriptions".to_string(),
        format!("/api/v1/subscriptions/{UUID}/tokens"),
        "/api/v1/sponsors".to_string(),
        format!("/api/v1/sponsors/{UUID}"),
    ];
    for p in &reads {
        assert_eq!(auth_kind("GET", p), AuthKind::AdminOnly, "GET {p}");
    }

    // POST-shaped reads (search / preview) are AdminOnly too: no service consumer
    // has been proven, so they stay on the admin surface.
    for p in [
        "/api/v1/sources/search",
        "/api/v1/nodes/search",
        "/api/v1/filter/preview",
    ] {
        assert_eq!(auth_kind("POST", p), AuthKind::AdminOnly, "POST {p}");
    }
}

#[test]
fn admin_writes_are_admin_only() {
    let writes = [
        ("PUT", "/api/v1/config".to_string()),
        ("PATCH", format!("/api/v1/sources/{UUID}")),
        ("DELETE", format!("/api/v1/sources/{UUID}")),
        ("POST", format!("/api/v1/sources/{UUID}/ignore-dead")),
        ("DELETE", format!("/api/v1/nodes/{UUID}")),
        ("POST", "/api/v1/subscriptions".to_string()),
        ("PUT", format!("/api/v1/subscriptions/{UUID}")),
        ("DELETE", format!("/api/v1/subscriptions/{UUID}")),
        ("POST", format!("/api/v1/subscriptions/{UUID}/tokens")),
        ("DELETE", format!("/api/v1/subscriptions/tokens/{UUID}")),
        ("POST", "/api/v1/sponsors".to_string()),
        ("PATCH", format!("/api/v1/sponsors/{UUID}")),
        ("PUT", "/api/v1/format-configs/clash".to_string()),
    ];
    for (m, p) in &writes {
        assert_eq!(auth_kind(m, p), AuthKind::AdminOnly, "{m} {p}");
    }
}

#[test]
fn extra_segments_never_inherit_parent_permission() {
    // `POST /sources` is allowed; the same prefix with any extra segment is not.
    assert_eq!(
        auth_kind("POST", &format!("/api/v1/sources/{UUID}")),
        AuthKind::AdminOnly
    );
    assert_eq!(
        auth_kind("POST", &format!("/api/v1/sources/{UUID}/anything")),
        AuthKind::AdminOnly
    );
    assert_eq!(
        auth_kind("POST", "/api/v1/fetch/trigger/extra"),
        AuthKind::AdminOnly
    );
    assert_eq!(auth_kind("POST", "/api/v1/fetch"), AuthKind::AdminOnly);
    assert_eq!(
        auth_kind("GET", "/api/v1/fetch/trigger"),
        AuthKind::AdminOnly
    );
    assert_eq!(auth_kind("HEAD", "/api/v1/sources"), AuthKind::AdminOnly);
    // Prefix tricks cannot re-enter the whitelist.
    assert_eq!(
        auth_kind("POST", "/api/v1/api/v1/sources"),
        AuthKind::AdminOnly
    );
    assert_eq!(auth_kind("POST", "/api/v1/../sources"), AuthKind::AdminOnly);
}

#[test]
fn unmatched_and_empty_paths_fail_closed() {
    assert_eq!(auth_kind("TRACE", "/api/v1/sources"), AuthKind::AdminOnly);
    assert_eq!(
        auth_kind("GET", "/api/v1/does-not-exist"),
        AuthKind::AdminOnly
    );
    assert_eq!(auth_kind("GET", "/api/v1"), AuthKind::AdminOnly);
    assert_eq!(auth_kind("GET", "/"), AuthKind::AdminOnly);
    // Outside the nest the middleware never runs, but the function must still be safe.
    assert_eq!(auth_kind("GET", "health"), AuthKind::AdminOnly);
}
