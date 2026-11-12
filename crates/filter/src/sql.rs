//! AST -> parameterized SQL (plan §9.2). Column names are fixed by the whitelist
//! mapping; user values always bind through `$n` placeholders — never interpolated.
use crate::ast::{CompareOp, FilterNode, FilterValue};

#[derive(Debug, Clone, PartialEq)]
pub enum SqlParam {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl SqlParam {
    fn from_value(v: &FilterValue) -> Self {
        match v {
            FilterValue::Str(s) => SqlParam::Str(s.clone()),
            FilterValue::Num(n) => SqlParam::Num(*n),
            FilterValue::Bool(b) => SqlParam::Bool(*b),
            FilterValue::List(_) => SqlParam::Str(String::new()), // expanded per item
        }
    }
}

/// Fixed whitelist -> column mapping (no arbitrary identifiers can enter SQL).
fn column_of(field: &str) -> &'static str {
    match field {
        "health" => "s.health",
        "score" => "sc.score",
        "region" => "n.region",
        "protocol" => "n.protocol",
        "server" => "n.server",
        _ => "n.id", // unreachable after validation
    }
}

/// Fixed whitelist -> search-view column mapping (plain aggregate columns).
fn view_column_of(field: &str) -> &'static str {
    match field {
        "health" => "v.health",
        "score" => "v.score",
        "region" => "v.region",
        "protocol" => "v.protocol",
        "server" => "v.server",
        _ => "v.id", // unreachable after validation
    }
}

fn op_sql(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Eq => "=",
        CompareOp::Ne => "<>",
        CompareOp::Gt => ">",
        CompareOp::Gte => ">=",
        CompareOp::Lt => "<",
        CompareOp::Lte => "<=",
        _ => "",
    }
}

struct Compiler {
    params: Vec<SqlParam>,
}

impl Compiler {
    fn push(&mut self, p: SqlParam) -> usize {
        self.params.push(p);
        self.params.len() // $n is 1-based; $1 is the checker tag pushed by the caller
    }

    fn leaf(&mut self, field: &str, op: CompareOp, value: &FilterValue) -> String {
        // Special sources (membership / streaming) become EXISTS subqueries.
        match (field, op) {
            ("source_id", CompareOp::Eq) => {
                let n = self.push(SqlParam::from_value(value));
                format!("EXISTS (SELECT 1 FROM node_sources ns WHERE ns.node_id=n.id AND ns.source_id=${n}::uuid)")
            }
            ("source_id", CompareOp::Ne) => {
                let n = self.push(SqlParam::from_value(value));
                format!("NOT EXISTS (SELECT 1 FROM node_sources ns WHERE ns.node_id=n.id AND ns.source_id=${n}::uuid)")
            }
            ("unlock", CompareOp::Contains) | ("unlock", CompareOp::Eq) => {
                let n = self.push(SqlParam::from_value(value));
                format!("EXISTS (SELECT 1 FROM node_streaming ns2 WHERE ns2.node_id=n.id AND lower(ns2.service)=lower(${n}) AND ns2.ok=true)")
            }
            ("unlock", CompareOp::Ne) => {
                let n = self.push(SqlParam::from_value(value));
                format!("NOT EXISTS (SELECT 1 FROM node_streaming ns2 WHERE ns2.node_id=n.id AND lower(ns2.service)=lower(${n}) AND ns2.ok=true)")
            }
            (f, CompareOp::IsNull) => {
                let col = column_of(f);
                if matches!(value, FilterValue::Bool(true)) {
                    format!("{col} IS NULL")
                } else {
                    format!("{col} IS NOT NULL")
                }
            }
            (f, CompareOp::In) | (f, CompareOp::NotIn) => {
                let col = column_of(f);
                let list = match value {
                    FilterValue::List(items) => items.clone(),
                    other => vec![other.clone()],
                };
                let marks: Vec<String> = list
                    .iter()
                    .map(|item| {
                        let n = self.push(SqlParam::from_value(item));
                        format!("${n}")
                    })
                    .collect();
                let joined = marks.join(",");
                if op == CompareOp::In {
                    format!("{col} IN ({joined})")
                } else {
                    format!("{col} NOT IN ({joined})")
                }
            }
            (f, op) => {
                let col = column_of(f);
                let op_str = op_sql(op);
                let n = self.push(SqlParam::from_value(value));
                if f == "score" {
                    format!("{col} {op_str} ${n}::float8")
                } else {
                    format!("{col} {op_str} ${n}")
                }
            }
        }
    }

    fn compile(&mut self, node: &FilterNode) -> String {
        match node {
            FilterNode::And { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| self.compile(c)).collect();
                format!("({})", parts.join(" AND "))
            }
            FilterNode::Or { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| self.compile(c)).collect();
                format!("({})", parts.join(" OR "))
            }
            FilterNode::Not { condition } => format!("(NOT {})", self.compile(condition)),
            FilterNode::Cond(c) => self.leaf(&c.field, c.operator, &c.value),
        }
    }

    /// Same shape as [`Self::compile`] but against the search view: plain columns,
    /// EXISTS aliases renamed (`nsv`) so they cannot collide with the view's `ns`.
    fn compile_view(&mut self, node: &FilterNode) -> String {
        match node {
            FilterNode::And { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| self.compile_view(c)).collect();
                format!("({})", parts.join(" AND "))
            }
            FilterNode::Or { conditions } => {
                let parts: Vec<String> = conditions.iter().map(|c| self.compile_view(c)).collect();
                format!("({})", parts.join(" OR "))
            }
            FilterNode::Not { condition } => format!("(NOT {})", self.compile_view(condition)),
            FilterNode::Cond(c) => {
                let field = c.field.as_str();
                let op = c.operator;
                let value = &c.value;
                match (field, op) {
                    ("source_id", CompareOp::Eq) => {
                        let n = self.push(SqlParam::from_value(value));
                        format!("EXISTS (SELECT 1 FROM node_sources nsv WHERE nsv.node_id=v.id AND nsv.source_id=${n}::uuid)")
                    }
                    ("source_id", CompareOp::Ne) => {
                        let n = self.push(SqlParam::from_value(value));
                        format!("NOT EXISTS (SELECT 1 FROM node_sources nsv WHERE nsv.node_id=v.id AND nsv.source_id=${n}::uuid)")
                    }
                    // unlock/streaming: no table yet — keep it expressible but always
                    // false in SQL; the caller re-checks these in memory.
                    ("unlock", _) => {
                        let _n = self.push(SqlParam::from_value(value));
                        "FALSE".to_string()
                    }
                    (f, CompareOp::IsNull) => {
                        let col = view_column_of(f);
                        if matches!(value, FilterValue::Bool(true)) {
                            format!("{col} IS NULL")
                        } else {
                            format!("{col} IS NOT NULL")
                        }
                    }
                    (f, CompareOp::In) | (f, CompareOp::NotIn) => {
                        let col = view_column_of(f);
                        let list = match value {
                            FilterValue::List(items) => items.clone(),
                            other => vec![other.clone()],
                        };
                        let marks: Vec<String> = list
                            .iter()
                            .map(|item| {
                                let n = self.push(SqlParam::from_value(item));
                                format!("${n}")
                            })
                            .collect();
                        let joined = marks.join(", ");
                        if op == CompareOp::In {
                            format!("{col} IN ({joined})")
                        } else {
                            format!("{col} NOT IN ({joined})")
                        }
                    }
                    (f, CompareOp::Contains) => {
                        // Case-insensitive substring match over the view column.
                        let col = view_column_of(f);
                        let n = self.push(SqlParam::from_value(value));
                        format!("({col} ILIKE '%' || ${n} || '%' OR {col} ILIKE ${n})")
                    }
                    (f, op) => {
                        let col = view_column_of(f);
                        let op_str = op_sql(op);
                        let n = self.push(SqlParam::from_value(value));
                        if f == "score" {
                            format!("{col} {op_str} ${n}::float8")
                        } else {
                            format!("{col} {op_str} ${n}")
                        }
                    }
                }
            }
        }
    }
}

/// Compile the AST into `SELECT n.id ... WHERE <expr>` with bound parameters.
/// `$1` is always the checker tag; expression params start at `$2`.
pub fn to_sql(node: &FilterNode, checker_tag: &str) -> (String, Vec<SqlParam>) {
    let mut c = Compiler {
        params: vec![SqlParam::Str(checker_tag.to_string())],
    };
    let where_clause = c.compile(node);
    let sql = format!(
        "SELECT n.id FROM nodes n \
         JOIN node_status s ON s.node_id = n.id AND s.checker_tag = $1 \
         JOIN node_scores sc ON sc.node_id = n.id AND sc.checker_tag = $1 \
         WHERE {where_clause}"
    );
    (sql, c.params)
}

/// Compile the AST for the **search view** (`handlers/search.rs` NODE_VIEW_SQL):
/// a per-node aggregate row with plain columns `health`/`score`/`region` (latest
/// across tags, max score) — unlike [`to_sql`], no checker tag is pinned and the
/// params start at `$1`.
///
/// The unlock/streaming branches are preserved for whitelist compatibility; there is
/// no `node_streaming` table yet, so callers keep memory evaluation for those leaves.
pub fn to_search_sql(node: &FilterNode) -> (String, Vec<SqlParam>) {
    let mut c = Compiler {
        params: Vec::new(),
    };
    let where_clause = c.compile_view(node);
    (where_clause, c.params)
}
