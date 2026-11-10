## 6-01 类型化筛选 AST · Typed Filter AST

### 目标 / Goal

用**结构化、白名单字段 + 运算符**的 Filter AST 替换 Phlogo 当前基于 `expr-lang` 的动态表达式引擎（`internal/filter/filter.go`、`internal/filter/env.go`），满足方案 §9.2：

- 只允许白名单字段（如 `region`、`score`、`protocol`、`health`、`unlock`、`source_id`）与白名单运算符（`eq`/`ne`/`gt`/`gte`/`lt`/`lte`/`in`/`not_in`/`contains`/`is_null` 及逻辑 `and`/`or`/`not`）。
- AST 可序列化为 JSON（plan §9.2 的形状），可反序列化为参数化 SQL（`WHERE ...` + 绑定参数，绝不字符串拼接用户值）。
- 对 AST 施加**深度限制、条件数量限制、单结果集大小限制**。
- 保留两类既有用例的可表达性：`configs`.`node_distribute_filter`（全局分发筛选）与 `sponsors`.`filter_expr`（sponsor 匹配）。

### 范围 / Scope

- **新建** crate：`crates/filter/`，含：
  - `ast.rs`：AST 类型（`FilterNode`、`Condition`、`LogicalOp`、`CompareOp`、`Value`）。
  - `parse.rs`：JSON → AST 解析，含白名单校验（未知字段 / 运算符 → 错误）。
  - `sql.rs`：AST → 参数化 SQL（`sqlx::query` 占位符 `$1..$n`）。
  - `eval.rs`：在内存节点集合上做纯函数布尔求值（供 `sponsor` 匹配与预览使用）。
  - `limits.rs`：深度 / 条件数 / 结果集上限常量与校验。
- **修改**：`sponsors` 表增加 `filter_ast`（JSON）；`configs` 的 `node_distribute_filter` 改为存 AST JSON（不再存 expr 字符串）。
- **不修改**：Phlogo Go 源码（本任务只写文档）。`internal/naming/naming.go` 的命名模板仍用 expr 引擎，迁移单列 p5-03。

### 前置 / Prerequisites

- p5 之前的阶段已完成：`nodes` / `node_status` / `node_scores` / `node_sources` 表（方案 §5.1、CONVENTIONS §2.3）。
- `crates/domain` 已定义 `NormalizedNode` 与 `NodeStatus` / `NodeScore` 快照类型。
- `configs` 表已 seed 默认键（CONVENTIONS §2.6）：`max_distribute=200`、`node_distribute_filter=""`、`grade_*_min`。
- 工具链：Rust + Cargo，`sqlx`（PostgreSQL），`serde_json`。

### 步骤 / Steps

#### 1. 定义 AST 数据模型（对应 plan §9.2 JSON 形状）

`crates/filter/src/ast.rs`：

```rust
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operator", rename_all = "lowercase")]
pub enum FilterNode {
    // 逻辑节点：conditions 递归
    And { conditions: Vec<FilterNode> },
    Or  { conditions: Vec<FilterNode> },
    Not { condition: Box<FilterNode> },
    // 叶子节点
    Cond(Condition),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Condition {
    pub field: String,        // 必须在 FIELD_WHITELIST
    pub operator: CompareOp,  // 必须在 OP_WHITELIST
    pub value: Value,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompareOp { Eq, Ne, Gt, Gte, Lt, Lte, In, NotIn, Contains, IsNull }

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Str(String),
    Num(f64),
    Bool(bool),
    List(Vec<Value>),   // 用于 in / not_in
}
```

plan §9.2 给出的示例（验收与单测以此为准）：

```json
{
  "operator": "and",
  "conditions": [
    { "field": "health", "operator": "eq", "value": "alive" },
    { "field": "region", "operator": "eq", "value": "JP" },
    { "field": "score",  "operator": "gte", "value": 0.7 }
  ]
}
```

#### 2. 白名单与解析校验

`crates/filter/src/parse.rs` —— 解析时即拒绝未知字段 / 运算符，并施加限制（CONVENTIONS §2.8 与 plan §9.2 要求）：

```rust
pub const FIELD_WHITELIST: &[&str] = &[
    "health",      // node_status.health: alive | degraded | dead
    "region",      // nodes.region (ISO 代码)
    "score",       // node_scores.score (float 0..1)
    "protocol",    // nodes.protocol
    "server",      // nodes.server
    "source_id",   // 是否在 node_sources 中（membership）
    "unlock",      // streaming 解锁结果中是否含该服务（不区分大小写）
];
pub const MAX_AST_DEPTH: usize = 8;     // 递归嵌套上限
pub const MAX_CONDITION_COUNT: usize = 32; // 叶子条件总数上限
pub const MAX_RESULT_SIZE: usize = 1000;   // 单次结果集上限（与 max_distribute 解耦，取两者较小值）

pub fn parse(json: &str) -> Result<FilterNode, FilterError> {
    let node: FilterNode = serde_json::from_str(json)?;
    validate(&node, 1, &mut 0)?;
    Ok(node)
}
// validate: 递归检查 field∈白名单、operator∈白名单、深度≤MAX_AST_DEPTH、叶子数≤MAX_CONDITION_COUNT
```

> 字段来源依据：`internal/filter/env.go` 的 `NodeFilterEnv`（Server/Protocol/Region/Alive/Score/Streaming 等）与 `node_status`/`node_scores`/`node_sources` 三个快照/关系表（CONVENTIONS §2.3）。`password`/`hash` 等敏感字段**不在白名单**（防止凭据泄露到筛选 API）。

#### 3. 生成参数化 SQL（绝不拼接用户值）

`crates/filter/src/sql.rs` —— 把 AST 编译成 `SELECT ... WHERE <expr>` + 绑定参数：

```rust
pub fn to_sql(node: &FilterNode, checker_tag: &str) -> (String, Vec<SqlParam>) {
    // 基础查询：nodes 关联 node_status(当前 tag) 与 node_scores(当前 tag)
    // WHERE 子句由 AST 生成，列名写死在白名单映射里，值一律走 $n 占位符
}
```

针对上面示例，期望输出（占位符编号随编译顺序递增）：

```sql
SELECT n.id FROM nodes n
JOIN node_status s  ON s.node_id = n.id AND s.checker_tag = $1
JOIN node_scores sc ON sc.node_id = n.id AND sc.checker_tag = $1
WHERE s.health = $2
  AND n.region = $3
  AND sc.score >= $4
```

绑定参数：`[$1="hk", $2="alive", $3="JP", $4=0.7]`（`checker_tag` 来自 `tag` 参数，文档见 `docs/distribution.md` §查询参数）。

字段 → 列映射（`field_to_column`）为固定 `match`，保证无任意列注入：

| field | 目标列（join） |
|---|---|
| `health` | `node_status.health` |
| `region` | `nodes.region` |
| `score` | `node_scores.score` |
| `protocol` | `nodes.protocol` |
| `server` | `nodes.server` |
| `source_id` | 子查询 `EXISTS (SELECT 1 FROM node_sources ns WHERE ns.node_id=n.id AND ns.source_id=$n)` |
| `unlock` | 子查询 `EXISTS (SELECT 1 FROM node_streaming ns2 WHERE ns2.node_id=n.id AND lower(ns2.service)=lower($n) AND ns2.ok=true)` |

`in` / `not_in` 展开为 `$n, $n+1, ...`；`is_null` 忽略 `value` 生成 `IS NULL` / `IS NOT NULL`。

#### 4. 内存布尔求值（用于 sponsor 匹配 / 预览 / 单元测试）

`crates/filter/src/eval.rs` —— 对 `NormalizedNode + NodeStatus + NodeScore + streaming map` 求值布尔：

```rust
pub fn matches(node: &NodeView, ast: &FilterNode) -> bool { /* 递归 */ }
```

`NodeView` 字段与 `FIELD_WHITELIST` 一一对应；`unlock` 在 `streaming: HashMap<String,bool>` 上做大小写无关 `contains` 检查。

#### 5. 保留既有用例

- **`node_distribute_filter`**：`configs` 中该键由 expr 字符串改为存 AST JSON（plan §9.2 形状）。分发时读取后走 `parse` → `to_sql` / `matches`，行为等价于原 `filter.EvalBool` 的全局筛选。`configs` 默认种子保持 `node_distribute_filter=""`（空 → 不过滤），见 CONVENTIONS §2.6。
- **`sponsor`.`filter_expr`**：`sponsors` 表新增 `filter_ast` 列（JSON），原 `filter_expr`（expr 字符串）+ `priority` 迁移为 `filter_ast`。sponsor 匹配：对每个节点用 `eval::matches` 判定，命中且 `priority` 最高者胜出，逻辑等价于 CONVENTIONS §2.3 中 `sponsors`（`name`/`text`/`filter_expr`/`priority`）的用途。

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地**（新 crate `crates/filter`，四模块）：
- `ast.rs`：`FilterNode{And,Or,Not,Cond}` + `Condition{field,operator,value}` + `CompareOp`（10 白名单运算符）+ `FilterValue{Str,Num,Bool,List}`。**手写 Deserialize**：叶子条件=含 `field` 的对象、逻辑节点=含 `conditions` 的对象，两者由顶层 `operator` 区分——严格贴合 plan §9.2 JSON 形状（serde 的 tag 分派做不到）。
- `parse.rs`：白名单校验（`FIELD_WHITELIST` 7 字段、未知字段→`UnknownField`、未知运算符→`UnknownOperator`/Json 拒绝）+ 深度 8 / 叶子 32 限制 + `MAX_RESULT_SIZE=1000` 常量导出；空串→`FilterError::Empty`（调用方视为「不过滤」）。
- `sql.rs`：AST→参数化 SQL。列名固定映射（`health→s.health`、`score→sc.score`、`region/protocol/server→n.*`）；`source_id`→`EXISTS node_sources` 子查询、`unlock`→`EXISTS node_streaming`（`lower()` 大小写无关）；`in/not_in` 展开 `$n` 列表；`is_null` 忽略 value；`$1` 恒为 checker_tag，值全部走占位符，**零字符串拼接**。
- `eval.rs`：`NodeView`（与白名单一一对应，含 `streaming: HashMap<String,bool>`）+ `matches` 纯函数递归求值。

**实测（校机）**：
| 项 | 结果 |
|---|---|
| `cargo build --workspace --all-targets` | **0**（34.13s） |
| `cargo test --workspace` | **0**——`filter` **7/7** |
| `cargo fmt --all -- --check` | **0** |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |

**7 测试（验收 1–7）**：`parse_ok`（and 形状）✓、`rejects_unknown_field`（password 拒绝）✓、`rejects_unknown_operator`（like 拒绝）✓、`generates_parameterized_sql`（`s.health = $2`/`n.region = $3`/`sc.score >= $4::float8` + 参数序列 `[hk, alive, JP, 0.7]`）✓、`limits_enforced`（深度 9→TooDeep、33 叶→TooManyConditions）✓、`eval_matches`（JP/0.8/alive 真、US 假）✓、`empty_and_unlock`（空串→Empty；netflix 大小写无关 contains）✓。

**说明/偏差**：`node_distribute_filter` 与 `sponsors.filter_ast` 的**存储迁移**（expr 字符串→AST JSON）在本步未动 DB——`configs` 键与 `sponsors` 表由 p5-02/p5-03 消费时一并迁移（本 crate 提供 parse/sql/eval 全部能力，消费端只需替换读取路径）；`node_streaming` 表目前尚不存在（unlock 的 SQL 分支为 P5-03 就绪预留，eval 分支已有测试）。命名模板（`naming.go` EvalString）按规格留 p5-03。

### 验收 / Acceptance

下列命令应在 `crates/filter` 单测下通过（退出码 `0`）：

```bash
cargo test -p filter
```

代表性单测（写在同一 crate 的 `tests/` 或 `#[cfg(test)]`）：

1. **解析成功（形状正确）**：
   ```rust
   let ast = parse(r#"{"operator":"and","conditions":[
     {"field":"health","operator":"eq","value":"alive"},
     {"field":"region","operator":"eq","value":"JP"},
     {"field":"score","operator":"gte","value":0.7}]}"#).unwrap();
   assert!(matches!(ast, FilterNode::And { .. }));
   ```
2. **拒绝未知字段**：
   ```rust
   let r = parse(r#"{"operator":"eq","field":"password","value":"x"}"#);
   assert!(matches!(r, Err(FilterError::UnknownField("password"))));
   ```
3. **拒绝未知运算符**：
   ```rust
   let r = parse(r#"{"operator":"like","field":"region","value":"JP"}"#);
   assert!(matches!(r, Err(FilterError::UnknownOperator("like"))));
   ```
4. **生成参数化 SQL（无拼接）**：
   ```rust
   let (sql, params) = to_sql(&ast, "hk");
   assert!(sql.contains("node_status.health = $2"));
   assert_eq!(params, vec![SqlParam::Str("hk"), SqlParam::Str("alive"),
                           SqlParam::Str("JP"), SqlParam::Num(0.7)]);
   ```
5. **深度 / 条件数超限被拒**：构造深度 9 或 33 个叶子的 AST → `Err(FilterError::TooDeep)` / `Err(FilterError::TooManyConditions)`。
6. **`matches` 正确**：构造一个 `region=JP, score=0.8, health=alive` 的节点，`matches` 返回 `true`；改为 `region=US` 返回 `false`。
7. **`node_distribute_filter` 与 sponsor AST 可表达**：空字符串 `parse("")` → 视为「不过滤」；一个含 `unlock` 的 sponsor AST 对「解锁 netflix」的节点 `matches==true`、对未解锁节点 `false`。

预期输出：`test result: ok. N passed; 0 failed`（N ≥ 7）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo 现状 | 重构后 |
|---|---|
| `internal/filter/filter.go`：`compiled()` 缓存 + `EvalBool` / `EvalString`，基于 `expr-lang/expr` 任意代码执行 | `crates/filter/parse.rs`（白名单解析）+ `eval.rs`（布尔求值）+ `sql.rs`（参数化 SQL）。**移除 `expr-lang` 依赖**（CONVENTIONS §2.1 技术栈表明确标注「自研 Typed Filter AST」）。 |
| `internal/filter/env.go`：`NodeFilterEnv` / `SourceFilterEnv` / `SourceInfo`（expr 求值上下文） | `NodeView`（eval 用，字段同白名单）+ `field_to_column` 映射（sql 用）。敏感字段（Password/Hash）不再出现在筛选面。 |
| `configs.node_distribute_filter`（expr 字符串） | `configs.node_distribute_filter`（AST JSON 形状），解析后走 `to_sql`/`matches`。 |
| `sponsors.filter_expr`（expr 字符串） | `sponsors.filter_ast`（AST JSON），`eval::matches` 做匹配。 |

### CI 提示 / CI notes（仅描述，不落地 YAML）

- 单测：`cargo test -p filter` —— 覆盖解析 / 拒绝 / SQL / 求值 / 限制五类（见 §验收 1–7）。
- 额外模糊测试（建议，可后续加）：从合法字段/运算符组合随机生成 AST，断言 `parse` 永不 panic、且 SQL 永远含 `$n` 占位符而非内联值（防注入回归）。
- 迁移检查：在 DB migration 测试里确认 `node_distribute_filter` 默认值 `""` 与 `sponsors.filter_ast` 列存在。

### 风险 / Risks

- **命名模板未在本步迁移**：`internal/naming/naming.go` 仍调用 `filter.EvalString` 做字符串模板（CONVENTIONS §2.8）。p5-01 只替换「布尔筛选」分支；命名模板迁移单列 p5-03，期间 expr 引擎不得删除，否则命名会失效（fallback 名只是 `region seq`）。
- **字段语义漂移**：`health`/`score` 来自 `node_status`/`node_scores` 且按 `checker_tag` 区分（CONVENTIONS §2.3、§2.8）。SQL 生成必须带 `checker_tag` 连接，否则跨 tag 污染。
- **白名单过严导致旧 expr 无法表达**：若某 `sponsor.filter_expr` 用到白名单外字段，迁移时需改写或扩展白名单并补齐 `field_to_column`。应先扫描 Phlogo 现有 `sponsors` 数据与 `node_distribute_filter` 取值（本任务未读取该表内容，需实跑 `SELECT` 核对）。
