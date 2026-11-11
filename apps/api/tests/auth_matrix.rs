//! P0-01 unit tests: the route permission matrix itself.
use api::auth::{auth_kind, AuthKind};

#[test]
fn login_is_public() {
    assert_eq!(auth_kind("POST", "/api/v1/auth/login"), AuthKind::Public);
}

#[test]
fn reads_are_admin_or_service() {
    for (m, p) in [
        ("GET", "/api/v1/sources"),
        ("GET", "/api/v1/sources/6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f"),
        ("GET", "/api/v1/sources/6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f/nodes"),
        ("GET", "/api/v1/nodes"),
        ("GET", "/api/v1/nodes/6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f"),
        ("GET", "/api/v1/nodes/6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f/checks/initial"),
        ("GET", "/api/v1/nodes/6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f/checks/deep"),
        ("GET", "/api/v1/stats"),
        ("GET", "/api/v1/checkers"),
        ("GET", "/api/v1/logs/check"),
        ("GET", "/api/v1/logs/fetch"),
        ("GET", "/api/v1/config"),
    ] {
        assert_eq!(auth_kind(m, p), AuthKind::AdminOrService, "{m} {p}");
    }
}

#[test]
fn service_intake_is_admin_or_service() {
    assert_eq!(auth_kind("POST", "/api/v1/sources"), AuthKind::AdminOrService);
    assert_eq!(
        auth_kind("POST", "/api/v1/fetch/trigger"),
        AuthKind::AdminOrService
    );
}

#[test]
fn admin_writes_are_admin_only() {
    let uuid = "6f96f9e0-8d1d-4c1a-9d3f-1a2b3c4d5e6f";
    for (m, p) in [
        ("PATCH", "/api/v1/sources/x"),
        ("DELETE", "/api/v1/sources/x"),
        ("POST", "/api/v1/sources/x/ignore-dead"),
        ("DELETE", "/api/v1/nodes/x"),
        ("PUT", "/api/v1/config"),
        ("GET", "/api/v1/subscriptions"),
        ("POST", "/api/v1/subscriptions"),
        ("PUT", "/api/v1/subscriptions/x"),
        ("DELETE", "/api/v1/subscriptions/x"),
        ("GET", "/api/v1/subscriptions/x/tokens"),
        ("POST", "/api/v1/subscriptions/x/tokens"),
        ("DELETE", "/api/v1/subscriptions/tokens/x"),
    ] {
        assert_eq!(auth_kind(m, p), AuthKind::AdminOnly, "{m} {p}");
        let _ = uuid;
    }
}

#[test]
fn unmatched_api_routes_fail_closed() {
    // Unknown verb on a known path, unknown path, anything weird -> AdminOnly.
    assert_eq!(auth_kind("TRACE", "/api/v1/sources"), AuthKind::AdminOnly);
    assert_eq!(auth_kind("GET", "/api/v1/does-not-exist"), AuthKind::AdminOnly);
    assert_eq!(
        auth_kind("POST", "/api/v1/nodes/x/checks/deep"),
        AuthKind::AdminOnly
    );
}

#[test]
fn non_api_routes_never_reach_the_matrix() {
    // /health, /metrics, /sub are mounted OUTSIDE the /api/v1 nest, so the matrix
    // middleware is never invoked for them (structural guarantee, not matrix output).
    // The matrix itself only asserts /api/v1 shapes; relative paths below are what
    // the middleware actually sees.
    assert_eq!(auth_kind("GET", "health"), AuthKind::AdminOnly, "unmatched relative path fails closed");
    assert_eq!(auth_kind("GET", "/api/v1/stats"), AuthKind::AdminOrService);
}

#[test]
fn uuid_shape_required_for_node_get_read_split() {
    // A non-uuid third segment on GET /nodes/{id} fails closed to AdminOnly.
    assert_eq!(auth_kind("GET", "/api/v1/nodes/not-a-uuid"), AuthKind::AdminOnly);
}
