//! JSON -> AST with whitelist validation + resource limits.
use serde_json;

use crate::ast::{CompareOp, FilterNode};

pub const FIELD_WHITELIST: &[&str] = &[
    "health",    // node_status.health: alive | degraded | dead
    "region",    // nodes.region (ISO code)
    "score",     // node_scores.score (float 0..1)
    "protocol",  // nodes.protocol
    "server",    // nodes.server
    "source_id", // membership: EXISTS in node_sources
    "unlock",    // streaming unlock contains (case-insensitive)
];

pub const MAX_AST_DEPTH: usize = 8;
pub const MAX_CONDITION_COUNT: usize = 32;
pub const MAX_RESULT_SIZE: usize = 1000;

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum FilterError {
    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("unknown operator: {0}")]
    UnknownOperator(String),
    #[error("ast too deep (max {MAX_AST_DEPTH})")]
    TooDeep,
    #[error("too many conditions (max {MAX_CONDITION_COUNT})")]
    TooManyConditions,
    #[error("json: {0}")]
    Json(String),
    #[error("empty expression (no filtering)")]
    Empty,
}

fn is_valid_operator(op: &str) -> bool {
    CompareOp::try_from_name(op).is_some()
}

fn validate(node: &FilterNode, depth: usize, leaves: &mut usize) -> Result<(), FilterError> {
    if depth > MAX_AST_DEPTH {
        return Err(FilterError::TooDeep);
    }
    match node {
        FilterNode::And { conditions } | FilterNode::Or { conditions } => {
            for c in conditions {
                validate(c, depth + 1, leaves)?;
            }
        }
        FilterNode::Not { condition } => validate(condition, depth + 1, leaves)?,
        FilterNode::Cond(c) => {
            *leaves += 1;
            if *leaves > MAX_CONDITION_COUNT {
                return Err(FilterError::TooManyConditions);
            }
            if !FIELD_WHITELIST.contains(&c.field.as_str()) {
                return Err(FilterError::UnknownField(c.field.clone()));
            }
            if !is_valid_operator(c.operator_name()) {
                return Err(FilterError::UnknownOperator(c.operator_name().into()));
            }
        }
    }
    Ok(())
}

/// Parse an AST JSON string. Empty/whitespace input is reported as `FilterError::Empty`
/// so callers can treat it as "no filtering".
pub fn parse(json: &str) -> Result<FilterNode, FilterError> {
    if json.trim().is_empty() {
        return Err(FilterError::Empty);
    }
    let node: FilterNode =
        serde_json::from_str(json).map_err(|e| FilterError::Json(e.to_string()))?;
    let mut leaves = 0;
    validate(&node, 1, &mut leaves)?;
    Ok(node)
}
