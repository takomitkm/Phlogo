//! AST data model (plan §9.2 JSON shape). Leaf conditions are objects with a
//! `field`; logical nodes carry `conditions`; deserialization is hand-written so
//! both shapes work without a wrapper tag.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "operator", rename_all = "lowercase")]
pub enum FilterNode {
    And { conditions: Vec<FilterNode> },
    Or { conditions: Vec<FilterNode> },
    Not { condition: Box<FilterNode> },
    Cond(Condition),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Condition {
    pub field: String,
    pub operator: CompareOp,
    pub value: FilterValue,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum CompareOp {
    Eq,
    Ne,
    Gt,
    Gte,
    Lt,
    Lte,
    In,
    NotIn,
    Contains,
    IsNull,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum FilterValue {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<FilterValue>),
}

impl FilterValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            FilterValue::Str(s) => Some(s),
            _ => None,
        }
    }
}

impl std::fmt::Display for CompareOp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

impl CompareOp {
    pub fn name(self) -> &'static str {
        match self {
            CompareOp::Eq => "eq",
            CompareOp::Ne => "ne",
            CompareOp::Gt => "gt",
            CompareOp::Gte => "gte",
            CompareOp::Lt => "lt",
            CompareOp::Lte => "lte",
            CompareOp::In => "in",
            CompareOp::NotIn => "not_in",
            CompareOp::Contains => "contains",
            CompareOp::IsNull => "is_null",
        }
    }
}

impl Condition {
    /// Wire name of the operator (the enum itself is the whitelist).
    pub fn operator_name(&self) -> &'static str {
        self.operator.name()
    }
}

impl<'de> Deserialize<'de> for FilterNode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = Value::deserialize(deserializer)?;
        node_from_value(v).map_err(serde::de::Error::custom)
    }
}

fn node_from_value(v: Value) -> Result<FilterNode, String> {
    let obj = v
        .as_object()
        .ok_or_else(|| "filter node must be an object".to_string())?;
    let op = obj
        .get("operator")
        .and_then(Value::as_str)
        .ok_or_else(|| "missing operator".to_string())?;
    match op {
        "and" | "or" => {
            let conds = obj
                .get("conditions")
                .and_then(Value::as_array)
                .ok_or_else(|| format!("`{op}` requires conditions array"))?;
            let conditions = conds
                .iter()
                .cloned()
                .map(node_from_value)
                .collect::<Result<Vec<_>, _>>()?;
            Ok(if op == "and" {
                FilterNode::And { conditions }
            } else {
                FilterNode::Or { conditions }
            })
        }
        "not" => {
            let inner = obj
                .get("condition")
                .cloned()
                .ok_or_else(|| "`not` requires condition".to_string())?;
            Ok(FilterNode::Not {
                condition: Box::new(node_from_value(inner)?),
            })
        }
        "eq" | "ne" | "gt" | "gte" | "lt" | "lte" | "in" | "not_in" | "contains" | "is_null" => {
            let field = obj
                .get("field")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("`{op}` requires field"))?
                .to_string();
            let value = match obj.get("value") {
                Some(v) => serde_json::from_value(v.clone())
                    .map_err(|e| format!("bad value: {e}"))?,
                None => FilterValue::Bool(true), // is_null sentinel
            };
            Ok(FilterNode::Cond(Condition {
                field,
                operator: CompareOp::try_from_name(op)
                    .ok_or_else(|| format!("unknown operator: {op}"))?,
                value,
            }))
        }
        other => Err(format!("unknown operator: {other}")),
    }
}

impl CompareOp {
    pub fn try_from_name(name: &str) -> Option<Self> {
        Some(match name {
            "eq" => Self::Eq,
            "ne" => Self::Ne,
            "gt" => Self::Gt,
            "gte" => Self::Gte,
            "lt" => Self::Lt,
            "lte" => Self::Lte,
            "in" => Self::In,
            "not_in" => Self::NotIn,
            "contains" => Self::Contains,
            "is_null" => Self::IsNull,
            _ => return None,
        })
    }
}
