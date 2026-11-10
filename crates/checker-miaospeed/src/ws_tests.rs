//! P3-02 acceptance tests against the protocol-faithful mock (P0-02, tag 4.7.9).
use crate::mock::{MockMiaoSpeed, MockResult};
use crate::types::{CheckJobId, CheckerError, SubmitRequest, VendorNode};
use crate::{Checker, MiaoSpeedAdapter};
use serde_json::json;
use uuid::Uuid;

fn submit_req(vendor_id: &str, n: usize) -> SubmitRequest {
    SubmitRequest {
        run_id: Uuid::new_v4(),
        checker_tag: "default".into(),
        vendor: "Local".into(),
        presets: vec!["TEST_PING_RTT".into(), "TEST_PING_CONN".into()],
        nodes: (0..n)
            .map(|i| VendorNode {
                internal_node_id: Uuid::new_v4(),
                vendor_id: format!("{vendor_id}-{i}"),
                payload: json!(""),
            })
            .collect(),
    }
}

/// Acceptance #1: handshake + auth; a wrong token must surface `CheckerError::Auth`.
#[tokio::test]
async fn handshake_and_auth() {
    let token = "TESTTOKEN";
    let mock = MockMiaoSpeed::start(token, vec![MockResult { vendor_id: "n-0".into(), latency_ms: 42 }]).await;
    let ok = MiaoSpeedAdapter::new(&mock.addr, token);
    assert!(ok.status().await.unwrap().online);

    let bad = MiaoSpeedAdapter::new(&mock.addr, "WRONG");
    let err = bad.status().await.unwrap_err();
    assert!(
        matches!(err, CheckerError::Auth(ref m) if m.contains("cannot verify")),
        "wrong token must be an auth error, got {err:?}"
    );
}

/// Acceptance #2: single and batch submit/collect return matching counts.
#[tokio::test]
async fn submit_single_and_batch() {
    let token = "TESTTOKEN";
    let results1 = vec![MockResult { vendor_id: "one-0".into(), latency_ms: 30 }];
    let mock = MockMiaoSpeed::start(token, results1).await;
    let adapter = MiaoSpeedAdapter::new(&mock.addr, token);

    let id = adapter.submit(submit_req("one", 1)).await.unwrap();
    let out = adapter.collect(&CheckJobId(id.0.clone())).await.unwrap();
    assert_eq!(out.len(), 1, "single node must yield one result");
    assert_eq!(out[0].initial.latency_ms, 30);

    let results5: Vec<MockResult> = (0..5)
        .map(|i| MockResult { vendor_id: format!("many-{i}"), latency_ms: 100 + i })
        .collect();
    let mock5 = MockMiaoSpeed::start(token, results5).await;
    let adapter5 = MiaoSpeedAdapter::new(&mock5.addr, token);
    let id5 = adapter5.submit(submit_req("many", 5)).await.unwrap();
    let out5 = adapter5.collect(&CheckJobId(id5.0.clone())).await.unwrap();
    assert_eq!(out5.len(), 5, "batch must yield five results");
}

/// Acceptance #3: results keyed by vendor_id map back to the right internal node ids.
#[tokio::test]
async fn vendor_id_maps_back_to_node_id() {
    let token = "TESTTOKEN";
    let req = submit_req("map", 3);
    let expected: Vec<Uuid> = req.nodes.iter().map(|n| n.internal_node_id).collect();
    let results: Vec<MockResult> = req
        .nodes
        .iter()
        .enumerate()
        .map(|(i, n)| MockResult { vendor_id: n.vendor_id.clone(), latency_ms: 1000 + i as i64 })
        .collect();

    let mock = MockMiaoSpeed::start(token, results).await;
    let adapter = MiaoSpeedAdapter::new(&mock.addr, token);
    let id = adapter.submit(req).await.unwrap();
    let out = adapter.collect(&CheckJobId(id.0)).await.unwrap();

    let got: Vec<Uuid> = out.iter().map(|r| r.internal_node_id).collect();
    let mut e = expected.clone();
    let mut g = got.clone();
    e.sort();
    g.sort();
    assert_eq!(e, g, "every result must map back to its internal node id");
}

/// Acceptance #4: status returns (online) without error.
#[tokio::test]
async fn status_returns() {
    let token = "TESTTOKEN";
    let mock = MockMiaoSpeed::start(token, vec![]).await;
    let adapter = MiaoSpeedAdapter::new(&mock.addr, token);
    let st = adapter.status().await.unwrap();
    assert!(st.online);
}
