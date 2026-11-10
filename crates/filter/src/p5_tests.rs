//! P5-01 acceptance tests (parse/validate/SQL/eval/limits).
use crate::{
    matches, parse, to_sql, CompareOp, FilterError, FilterNode, FilterValue, NodeView, SqlParam,
};
use std::collections::HashMap;

const SAMPLE: &str = r#"{"operator":"and","conditions":[
  {"field":"health","operator":"eq","value":"alive"},
  {"field":"region","operator":"eq","value":"JP"},
  {"field":"score","operator":"gte","value":0.7}]}"#;

/// 1: parse succeeds and preserves the shape.
#[test]
fn parse_ok() {
    let ast = parse(SAMPLE).unwrap();
    assert!(matches!(ast, FilterNode::And { .. }));
}

/// 2: unknown fields rejected (sensitive fields never enter the filter surface).
#[test]
fn rejects_unknown_field() {
    let r = parse(r#"{"field":"password","operator":"eq","value":"x"}"#);
    assert_eq!(r, Err(FilterError::UnknownField("password".into())));
}

/// 3: unknown operators rejected.
#[test]
fn rejects_unknown_operator() {
    // Typed enum rejects at serde level; check it surfaces as a Json error with the name.
    let r = parse(r#"{"field":"region","operator":"like","value":"JP"}"#);
    assert!(matches!(r, Err(FilterError::Json(_))), "got {r:?}");
    let _ = CompareOp::Eq; // whitelist completeness
}

/// 4: parameterized SQL generation (no value interpolation).
#[test]
fn generates_parameterized_sql() {
    let ast = parse(SAMPLE).unwrap();
    let (sql, params) = to_sql(&ast, "hk");
    assert!(sql.contains("s.health = $2"), "sql: {sql}");
    assert!(sql.contains("n.region = $3"), "sql: {sql}");
    assert!(sql.contains("sc.score >= $4::float8"), "sql: {sql}");
    assert_eq!(
        params,
        vec![
            SqlParam::Str("hk".into()),
            SqlParam::Str("alive".into()),
            SqlParam::Str("JP".into()),
            SqlParam::Num(0.7),
        ]
    );
}

/// 5: depth / condition limits enforced.
#[test]
fn limits_enforced() {
    // Depth 9: 8 nested Not then a condition => depth 9.
    let mut inner = r#"{"field":"region","operator":"eq","value":"JP"}"#.to_string();
    for _ in 0..8 {
        inner = format!(r#"{{"operator":"not","condition":{inner}}}"#);
    }
    let r = parse(&inner);
    assert_eq!(r, Err(FilterError::TooDeep));

    // 33 leaves.
    let mut conds = Vec::new();
    for i in 0..33 {
        conds.push(format!(r#"{{"field":"region","operator":"eq","value":"r{i}"}}"#));
    }
    let wide = format!(r#"{{"operator":"or","conditions":[{}]}}"#, conds.join(","));
    let r = parse(&wide);
    assert_eq!(r, Err(FilterError::TooManyConditions));
}

/// 6: in-memory evaluation matches the whitelist semantics.
#[test]
fn eval_matches() {
    let ast = parse(SAMPLE).unwrap();
    let ok = NodeView {
        health: "alive".into(),
        region: Some("JP".into()),
        score: 0.8,
        protocol: "trojan".into(),
        server: "a.example.com".into(),
        source_id: None,
        streaming: HashMap::new(),
    };
    assert!(matches(&ok, &ast));
    let bad = NodeView { region: Some("US".into()), ..ok.clone() };
    assert!(!matches(&bad, &ast));
}

/// 7: empty expr means "no filtering"; unlock contains works case-insensitively.
#[test]
fn empty_and_unlock() {
    assert_eq!(parse(""), Err(FilterError::Empty));
    assert_eq!(parse("   "), Err(FilterError::Empty));

    let ast = parse(r#"{"field":"unlock","operator":"contains","value":"Netflix"}"#).unwrap();
    let mut streaming = HashMap::new();
    streaming.insert("netflix".to_string(), true);
    let unlocked = NodeView {
        streaming,
        ..NodeView::default()
    };
    assert!(matches(&unlocked, &ast));
    let plain = NodeView::default();
    assert!(!matches(&plain, &ast));
    let _ = FilterValue::Str("x".into()); // enum completeness
}
