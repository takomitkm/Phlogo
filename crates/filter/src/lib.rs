//! Typed filter AST (P5-01): whitelist fields/operators, JSON (de)serialization,
//! parameterized SQL generation, and in-memory boolean evaluation.
//! Replaces Phlogo's expr-lang dynamic engine (CONVENTIONS §2.8).
pub mod ast;
pub mod eval;
pub mod parse;
pub mod sql;

pub use ast::{CompareOp, Condition, FilterNode, FilterValue};
pub use eval::{matches, NodeView};
pub use parse::{parse, FilterError, MAX_AST_DEPTH, MAX_CONDITION_COUNT, FIELD_WHITELIST};
pub use sql::{to_sql, SqlParam};

#[cfg(test)]
mod p5_tests;
