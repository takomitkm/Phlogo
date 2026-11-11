# P2 · 抓取与解析闭环 · Fetch & Parse Loop

> 阶段目录：`docs/rebuild/03-p2-fetch-parse/`
> 上游依据：`paily-core-rust-rebuild-plan.md` §2（架构图里的 `FW` + `PA` + `REG` + `LIFE`）、§3 VProxy 行、§6、§7、§15、§16.9。
> 现状基准：[`CONVENTIONS.md`](CONVENTIONS.md) §2.3（数据模型）、§2.4（指纹）、§2.9（生命周期）、§2.11（许可证）。
> 本目录只写文档，不写 CI（见 [`CONVENTIONS.md`](CONVENTIONS.md) §1/§5）。

---

## 1. 阶段目标 / Phase Goal

把「抓取 → 解析 → 规范化 → 指纹 → 对账入库」这一整条闭环收编进 `core-worker`，替代 Phlogo 当前由**外部 `paily-fetch` 服务**通过 HTTP 回调 `POST /api/v1/fetch/results`（`internal/fetcher/handler.go`）把结果推回来的做法。

具体要达成的 4 点：

1. fetch 由 `core-worker` 内的 **Fetch Worker** 用 **Reqwest** 直接抓取订阅 URL，并新增 **SSRF 防护**（Phlogo 现状完全没有这一层）。
2. 订阅内容经 **VProxy Parser Adapter** 解析为自有的 `NormalizedNode`，**VProxy 的 Node 类型不得渗透到数据库层**。
3. 用 `identity_hash` + `fingerprint_version` 计算节点身份，忠实移植 Phlogo `internal/node/upsert.go` 的 `nodeHash` / `nodeAuth` / `normalizeCredential`（**反复 percent-decode 的规范化必须保留**）。
4. 用 **Fetch Run 历史**（`source_fetch_runs`）+ **Replace / Accumulate 对账** + **事务内孤儿清理** + **死源保护**，把结果写进 `nodes` / `node_sources`，全部关联清理在**同一个 DB 事务**内完成。

---

## 2. 子步骤清单 / Sub-steps

| 编号 | 页面 | 主题 |
|---|---|---|
| 3-01 | [p2-01-fetch-ssrf.md](p2-01-fetch-ssrf.md) | Reqwest 抓取 + SSRF 防护（环回/私网/链路本地/云元数据，重定向复核，限大小与超时） |
| 3-02 | [p2-02-vproxy-adapter.md](p2-02-vproxy-adapter.md) | VProxy Parser Adapter → 自有 `NormalizedNode`；身份字段 vs 展示/来源字段分离 |
| 3-03 | [p2-03-fingerprint.md](p2-03-fingerprint.md) | `identity_hash` + `fingerprint_version`；golden 测试；算法升级迁移方案 |
| 3-04 | [p2-04-reconciliation.md](p2-04-reconciliation.md) | `source_fetch_runs` + Replace/Accumulate 对账 + 事务内孤儿清理 + 死源保护 |

执行顺序建议：`p2-01`（抓取，可独立）→ `p2-02`（解析，产出 `NormalizedNode`）→ `p2-03`（指纹，填 `identity_hash`）→ `p2-04`（对账入库，消费前三者）。

---

## 3. 阶段级验收 / Phase-level Acceptance

下列清单是 CI 验收的核心种子（逐项可由集成测试证明）：

- [x] **Replace 成功删除该来源消失节点的关联**：某来源上一轮关联 node1+node2，本轮只返回 node1，则 node2 对该来源的 `node_sources` 映射被删除（node2 若再无其它来源引用则进入 `Orphaned`）。（p2-04 集成测试 `replace_removes_vanished_and_marks_orphaned` ✓）
- [x] **失败抓取保留旧关联**：`success=false` 的抓取结果不产生任何 `node_sources` 删除，旧关联原样保留。（`failed_fetch_keeps_mappings` ✓）
- [x] **Accumulate 不删除旧关联**：来源上一轮关联 node1，本轮 Accumulate 只返回 node2（node1 未出现），则 node1 与 node2 的映射都保留。（`accumulate_keeps_old` ✓）
- [x] **删除来源不影响其它来源仍引用的节点**：node1 同时被来源 A、B 引用，删除来源 A 后 node1 仍存在且仍与 B 关联。（`delete_source_keeps_shared_nodes` ✓）
- [x] **无来源引用的节点进入 `Orphaned`**：某 node 的最后一条 `node_sources` 被移除后，其 `membership` 标记为 `Orphaned` 并记录时间（物理删除/历史清理推迟到 P4）。（`empty_replace_orphans_all` ✓）
- [x] **SSRF 防护生效**：对 `127.0.0.1`、`169.254.169.254`（云元数据）、以及私网/链路本地地址的抓取返回错误；良性 URL 在本地测试服务器上成功。（p2-01 6 测试 ✓）
- [x] **VProxy 类型不渗透数据库层**：`crates/database` 仅依赖 `crates/domain`，不依赖 `crates/vproxy-adapter`；所有入库结构都是 `domain::NormalizedNode` 或下游表结构。（`cargo tree -p database` 无 vproxy ✓）
- [x] **指纹稳定性**：改名不产生新身份；凭据/端口/服务器变化产生新身份；字段顺序与无意义缺省/空值规范化后身份稳定。（p2-03 14 golden ✓）

> **P2 状态：全部完成（2026-11-11，真实 PG 18.6 验证）。**

> 上述「删除来源」「Orphaned」两条依赖 P1 已落地的 `sources` / `nodes` / `node_sources` CRUD 与 `source.fetch_policy` 列（见 [`CONVENTIONS.md`](CONVENTIONS.md) §2.3）。Phlogo 当前**没有** Replace/Accumulate 区分、也**没有** Orphaned/Retired 状态——这是本阶段与 P4 的重点新增（见 [`CONVENTIONS.md`](CONVENTIONS.md) §2.9）。

---

## 4. 与 fork 源对照（摘要）/ Mapping Summary
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| 阶段能力 | Phlogo 现状（Go） | Rust 目标 |
|---|---|---|
| 抓取 | 外部 `paily-fetch` 服务抓取，`internal/fetcher/handler.go` 仅接收结果 | `core-worker` Fetch Worker 用 Reqwest 抓取 |
| SSRF 防护 | **无**（抓取在外部服务） | `crates/fetch` 新增 `SsrfGuard`（net-new） |
| 解析 | `internal/node/upsert.go` 用 `json.Unmarshal` 做朴素的 clash JSON 解析 | `crates/vproxy-adapter` 用 VProxy Parser → `domain::NormalizedNode` |
| 指纹 | `nodeHash`/`nodeAuth`/`normalizeCredential`（sha256 前 16 字节） | `crates/domain` 同算法 + `fingerprint_version` |
| 对账 | `UpsertFromFetchResult` + `removeSourceMappings`（始终 Replace 语义，立即物理删孤儿） | `crates/registry` Replace/Accumulate + 事务内清理 + 标记 `Orphaned` |
| 死源保护 | `UpsertFromFetchResult` 检查 `status=="dead"` 跳过 | 对账入口的死源/挂起守卫 |
| 抓取历史 | `source_fetch_histories`（success/node_count/fetched_at） | `source_fetch_runs`（status/started_at/finished_at/counts/error） |
| 死源清理 | `internal/source/service.go` `MarkDeadAndClean`（条件 UPDATE + 删映射） | `crates/lifecycle` 同语义（条件 UPDATE 防并发重复清理） |

---

## 5. CI 提示 / CI notes（仅描述，不落地 YAML）

- 阶段门禁建议组合：`cargo build --workspace --all-targets` → `cargo test -p fetch -p vproxy-adapter -p domain -p registry -p lifecycle` → `cargo test --test integration`（需临时 Postgres）→ `cargo clippy --workspace --all-targets -- -D warnings`。
- `p2-01` 的 SSRF 测试应作为单独的 `cargo test -p fetch` 门禁，避免被合并进需要外网的集成测试而变慢。
- 集成测试（`p2-04`）需要 ephemeral Postgres（如 `testcontainers` 或 `sqlx` + docker），CI 里单独一个 job，失败即阻断合并。
- 参见 [`CONVENTIONS.md`](CONVENTIONS.md) §5 的推荐 CI 命令种子（fmt / clippy / audit / deny / migrate）。

---

## 6. 风险 / Risks

- **VProxy 依赖边界**（P0 未过前不锁定）：若 VProxy 无法无 GUI/无 SQLite 干净编译，则 `p2-02` 退化为「最小化移植必要算法」，需保留 MIT 版权（见 [`CONVENTIONS.md`](CONVENTIONS.md) §2.11）。
- **孤儿清理时机变化**：Phlogo 现在立即物理删除孤儿节点并清理其测活历史；Rust 方案按 [`CONVENTIONS.md`](CONVENTIONS.md) §2.9 改为先标记 `Orphaned`，物理删除/历史清理推迟到 P4。必须保证 P4 的清理 Job 接管，否则会出现「已无来源但数据残留」。
- **Replace 语义是新增**：Phlogo 的 `UpsertFromFetchResult` 没有 Replace/Accumulate 区分，本质是「始终 Replace」。新代码必须按 `sources.fetch_policy` 分支，且默认值与 Phlogo 行为一致（默认 Replace）以避免回退。
- **死源/挂起守卫的并发**：与 `MarkDeadAndClean` 的竞态需条件 UPDATE（`WHERE status != 'dead'`）防重复清理，沿用 Go 侧写法。
