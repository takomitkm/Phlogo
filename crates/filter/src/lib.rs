//! Typed filter AST (P5-01): whitelist fields/operators, JSON (de)serialization,
//! parameterized SQL generation, and in-memory boolean evaluation.
//! Replaces Phlogo's expr-lang dynamic engine (CONVENTIONS §2.8).
pub mod ast;
pub mod eval;
pub mod parse;
pub mod sql;

pub use ast::{CompareOp, Condition, FilterNode, FilterValue};
pub use eval::{matches, matches_source, NodeView, SourceView};
pub use parse::{
    parse, parse_with, FilterError, FIELD_WHITELIST, MAX_AST_DEPTH, MAX_CONDITION_COUNT,
    SOURCE_FIELD_WHITELIST,
};
pub use sql::{to_search_sql, to_sql, SqlParam};

#[cfg(test)]
mod p5_tests;
