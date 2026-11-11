# P5 筛选与订阅发布 · Filter & Subscription

> 阶段目录：`docs/rebuild/06-p5-filter-subscription/`
> 上游事实来源：[`CONVENTIONS.md`](../CONVENTIONS.md)（§2.3、§2.6、§2.8）、方案 [`paily-core-rust-rebuild-plan.md`](../../../../Downloads/paily-core-rust-rebuild-plan.md)（§9.2、§10、§5.1、P5、§15）
> Phlogo 现状代码：`internal/filter/`、`internal/distribution/`、`internal/templates/`、`internal/naming/`、`docs/distribution.md`

---

## 阶段目标 / Phase Goal

把 Phlogo 当前的 **expr-lang 动态表达式筛选 + 内存缓存分发** 重写为 Rust 侧的 **Typed Filter AST + 地区配额/节点数上限选择 + 订阅输出（Mihomo/Clash YAML、Sing-box JSON、Base64）**，并补齐 **Token 认证 + 缓存失效**。

交付要点：

1. **Typed Filter AST**（替代 `expr-lang`）：白名单字段 + 运算符，参数化 SQL，限制 AST 深度 / 条件数量 / 结果量；保留 `node_distribute_filter` 与 `sponsor`.`filter_expr` 两种用例的可表达性。
2. **地区配额 + 节点数上限**：忠实移植 `internal/distribution/selector.go` 的选区算法（common/all/other preset，按地区比例配额，Efraimidis-Spirakis A-Res 加权抽样），并写等价性测试。
3. **订阅输出**：Mihomo/Clash YAML + Sing-box JSON（独立转换器，非 Mihomo 反向）+ Base64；`subscription_profiles`（筛选 AST / 地区 / 健康 / 最低分 / 上限 / 排序 / 命名）与 `subscription_tokens`（仅存哈希、可撤销、缓存失效）。

---

## 子步骤清单 / Sub-step List

| # | 文件 | 标题 |
|---|---|---|
| 1 | [p5-01-filter-ast.md](p5-01-filter-ast.md) | 类型化筛选 AST · Typed Filter AST |
| 2 | [p5-02-quota-limit.md](p5-02-quota-limit.md) | 地区配额与节点数上限 · Region Quota & Node Limit |
| 3 | [p5-03-subscription-output.md](p5-03-subscription-output.md) | 订阅输出与 Token 认证 · Subscription Output & Token Auth |

依赖顺序：**p5-01 → p5-02 → p5-03**。p5-01 的 AST 是 p5-03 `subscription_profiles`.`filter_ast` 的存储与求值基础；p5-02 的选区算法是 p5-03 生成器内部调用的候选集来源；p5-03 把筛选 + 选区 + 命名 + 格式输出 + Token 串成完整 `/sub/{token}` 链路。

---

## 阶段级验收 / Phase Acceptance

1. **结果符合筛选条件**：对任意 Filter AST，命中节点的字段 / 关系都满足 AST（验收脚本逐条断言，见 p5-01）。
2. **生成的 Mihomo YAML 通过目标版本校验**：以目标 Mihomo 二进制的 config-check 模式（如 `mihomo check -d <dir>`）校验 p5-03 生成的 YAML，退出码为 `0`（版本相关命令见 p5-03 §验收）。
3. **已撤销 Token 无法读取订阅**：撤销 `subscription_tokens` 中的 token 后，携带该 token 访问 `/sub/{token}.yaml`（及 `.base64`/`.json`）返回 `401` 或空体（验收脚本见 p5-03）。

---

## 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo（Go） | Rust crate / 模块 | 说明 |
|---|---|---|
| `internal/filter/filter.go` + `env.go` | `crates/filter/` | `expr-lang` 动态引擎 → 结构化 Filter AST + 参数化 SQL（p5-01） |
| `internal/distribution/selector.go` | `crates/distribution/src/selector.rs` | 地区配额 + A-Res 加权抽样，逐函数移植（p5-02） |
| `internal/distribution/handler.go` | `crates/distribution/src/handler.rs` + `apps/api` 路由 | `GET /{format}` 公开订阅 → `GET /sub/{token}` 带 Token（p5-03） |
| `internal/templates/*` + `internal/naming/naming.go` | `crates/distribution/src/output/`（mihomo/singbox/base64）+ naming | 模板内嵌 + expr 命名（p5-03） |
| `configs`: `node_distribute_filter`、`filename_*`、`node_name_template` | `subscription_profiles` + config crate | 全局分发筛选 / 文件名 / 命名模板（p5-01、p5-03） |
| `sponsors`.`filter_expr` | `sponsors`.`filter_ast`（或 AST 求值） | sponsor 匹配改为 AST（p5-01） |
| `format_configs` 表 | `subscription_profiles` + `subscription_tokens` | 筛选 / 授权拆出（p5-03，方案 §5.1） |

---

## CI 提示 / CI notes（仅描述，不落地 YAML）

- 构建：`cargo build --workspace --all-targets`
- 单测：`cargo test -p filter -p distribution`（重点跑 p5-01 的 AST 解析/拒绝/SQL 单测、p5-02 的选区等价性测试、p5-03 的 token 撤销 / 缓存失效测试）
- 集成：`cargo test --test integration`（需 PostgreSQL；用 Mihomo 二进制做 p5-03 的 YAML config-check）
- 格式 / Lint：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`
- 安全：`cargo audit`；许可证 `cargo deny check licenses`

---

## 风险 / Risks

- **expr-lang 语义差异**：`node_distribute_filter`、`sponsor`.`filter_expr`、命名模板都依赖 expr-lang；p5-01 只替换「布尔/筛选」分支，命名模板（字符串拼接）的迁移单列在 p5-03，p5-01 之前命名仍走 expr 引擎，需确认 `internal/naming/naming.go` 的 `filter.EvalString` 调用不被误删。
- **A-Res 跨语言浮点确定性**：Go `math/rand` 与 Rust `rand` 的浮点/顺序不完全一致；p5-02 的等价性测试对随机分支以「可种子化 + 统计性质」为主，确定分支（pool≤quota、≤1.5×quota top-N、UN 补足、配额分配）做精确相等（见 p5-02 §验收）。
- **Mihomo 版本耦合**：config-check 子命令与 schema 随 Mihomo 版本变化；P0 必须锁定目标版本并固化校验命令（见 p5-03 §风险）。
- **Token 缓存与失效边界**：profile 更新 / 节点健康变化触发失效的时机错误会导致「撤销后仍能读到旧缓存」；p5-03 必须显式测试失效路径。
