//! In-memory boolean evaluation (sponsor matching / preview / tests).
use std::collections::HashMap;

use crate::ast::{CompareOp, FilterNode, FilterValue};

/// Whitelist-shaped view of a node for evaluation.
#[derive(Debug, Clone, Default)]
pub struct NodeView {
    pub health: String,
    pub region: Option<String>,
    pub score: f64,
    pub protocol: String,
    pub server: String,
    pub source_id: Option<uuid::Uuid>,
    pub streaming: HashMap<String, bool>,
}

fn num(v: &FilterValue) -> Option<f64> {
    match v {
        FilterValue::Num(n) => Some(*n),
        _ => None,
    }
}

fn text(v: &FilterValue) -> Option<String> {
    match v {
        FilterValue::Str(s) => Some(s.clone()),
        FilterValue::Bool(b) => Some(b.to_string()),
        FilterValue::Num(n) => Some(n.to_string()),
        FilterValue::List(_) => None,
    }
}

fn compare(op: CompareOp, a: &str, b: &str) -> bool {
    match op {
        CompareOp::Eq => a == b,
        CompareOp::Ne => a != b,
        CompareOp::Gt => a > b,
        CompareOp::Gte => a >= b,
        CompareOp::Lt => a < b,
        CompareOp::Lte => a <= b,
        _ => false,
    }
}

/// Evaluate the AST against a node view.
pub fn matches(node: &NodeView, ast: &FilterNode) -> bool {
    match ast {
        FilterNode::And { conditions } => conditions.iter().all(|c| matches(node, c)),
        FilterNode::Or { conditions } => conditions.iter().any(|c| matches(node, c)),
        FilterNode::Not { condition } => !matches(node, condition),
        FilterNode::Cond(c) => {
            let op = c.operator;
            match c.field.as_str() {
                "health" => text(&c.value).map(|v| compare(op, &node.health, &v)).unwrap_or(false),
                "region" => {
                    let Some(nr) = &node.region else {
                        return op == CompareOp::IsNull;
                    };
                    text(&c.value).map(|v| compare(op, nr, &v)).unwrap_or(false)
                }
                "score" => num(&c.value).map(|v| match op {
                    CompareOp::Eq => node.score == v,
                    CompareOp::Ne => node.score != v,
                    CompareOp::Gt => node.score > v,
                    CompareOp::Gte => node.score >= v,
                    CompareOp::Lt => node.score < v,
                    CompareOp::Lte => node.score <= v,
                    _ => false,
                }).unwrap_or(false),
                "protocol" => text(&c.value).map(|v| compare(op, &node.protocol, &v)).unwrap_or(false),
                "server" => text(&c.value).map(|v| compare(op, &node.server, &v)).unwrap_or(false),
                "source_id" => match (&node.source_id, text(&c.value)) {
                    (Some(id), Some(v)) => match op {
                        CompareOp::Eq => id.to_string() == v,
                        CompareOp::Ne => id.to_string() != v,
                        _ => false,
                    },
                    (None, _) => op == CompareOp::IsNull,
                    _ => false,
                },
                "unlock" => {
                    let Some(want) = text(&c.value) else { return false; };
                    let hit = node
                        .streaming
                        .iter()
                        .any(|(svc, ok)| *ok && svc.eq_ignore_ascii_case(&want));
                    match op {
                        CompareOp::Contains => hit,
                        CompareOp::Eq => hit,
                        CompareOp::Ne => !hit,
                        _ => false,
                    }
                }
                _ => false,
            }
        }
    }
}

/// Whitelist-shaped view of a source (upstream `SourceFilterEnv`).
#[derive(Debug, Clone, Default)]
pub struct SourceView {
    pub id: String,
    pub identifier: String,
    pub source_type: String,
    pub status: String,
    pub info: String,
}

/// Evaluate the AST against a source view (`id|identifier|type|status|info`).
pub fn matches_source(source: &SourceView, ast: &FilterNode) -> bool {
    match ast {
        FilterNode::And { conditions } => conditions.iter().all(|c| matches_source(source, c)),
        FilterNode::Or { conditions } => conditions.iter().any(|c| matches_source(source, c)),
        FilterNode::Not { condition } => !matches_source(source, condition),
        FilterNode::Cond(c) => {
            let op = c.operator;
            let field = match c.field.as_str() {
                "id" => &source.id,
                "identifier" => &source.identifier,
                "type" => &source.source_type,
                "status" => &source.status,
                "info" => &source.info,
                _ => return false, // unreachable after whitelist validation
            };
            match op {
                CompareOp::Contains => text(&c.value)
                    .map(|v| field.to_lowercase().contains(&v.to_lowercase()))
                    .unwrap_or(false),
                CompareOp::IsNull => field.is_empty(),
                _ => text(&c.value).map(|v| compare(op, field, &v)).unwrap_or(false),
            }
        }
    }
}
