# 09 自补记录 · Self-Completed Gaps Log

> 目的：记录重构收口后**自查发现并补齐的缺口**，供后续评估「要不要换构架」时参考。
> 每项含：缺口是什么、为什么此前没做、补了什么、验证方式、对架构判断的影响。
> 状态标记：✅ 已完成并验证 / ⏸ 明确推迟（附原因）。

---

## A. 已补齐（本轮）

### A1. ✅ Fetch 闭环从未被调用（最关键）

- **缺口**：P2 的四段实现（`crates/fetch` SSRF 抓取 → `vproxy-adapter` 解析 → `domain` 指纹 → `registry` 对账）各自测试全绿，但 **worker 里没有任何代码把它们串起来**——抓取闭环从未真正运行过。这是「分段测试通过 ≠ 系统可用」的典型案例。
- **补**：`apps/worker/src/fetch_job.rs`——`FetchJob`（Apalis 任务）+ `run_fetch()`（拉 source.content → fetch → parse → `domain::stamp` 指纹 → `reconcile_fetch_result` 落库；失败路径记录 failed run 保留旧关联）+ `enqueue_for_active_sources()`（批量入队）。
- **验证**：`apps/worker/tests/fetch_loop.rs` 端到端——本地 HTTP 订阅服务器 → SSRF 守卫（`PAILY_FETCH_ALLOW_LOOPBACK=1`，新增的显式逃生口，生产不设）→ 抓取 → 解析 2 节点 → 指纹 32-hex → 映射落库 → run=finished → **二次 run 幂等不重复**。校机实测 **1 passed**。
- **架构判断参考**：闭环成立，四段分 crate 的边界（fetch/adapter/domain/registry）在实践中没有互相渗透，抽象成本是值得的。

### A2. ✅ Worker 双循环 + 周期调度缺失

- **缺口**：worker 只有 CheckJob 消费者，没有 fetch 消费者；`lifecycle::run_cleanup_cycle`（P4-02）从未被定时触发——孤儿永远停在 Orphaned，不会被退役/删除。
- **补**：`apps/worker/src/main.rs` 重写——**两个 Apalis worker**（fetch + check）+ 调度协程（`tokio::select!` 双 interval：fetch 批量入队 300s / cleanup tick 600s）。
- **验证**：build/clippy 0；调度逻辑复用已测函数（`enqueue_for_active_sources`、`run_cleanup_cycle`）。
- **架构判断参考**：Apalis PG backend 承载双队列无冲突（同一 `paily_jobs` 库两个 Storage 实例）；周期任务没有引入 cron 框架，`tokio::interval` 够用——若未来需要分布式锁再换。

### A3. ✅ 无登录端点（Vben 登录必挂）

- **缺口**：认证中间件 P1-02 就有（admin JWT / service bearer），但**没有签发 JWT 的端点**——Vben 登录页无从获取 token。
- **补**：`POST /api/v1/auth/login`（`handlers/auth.rs`）——校验 `PAILY_AUTH_ADMIN_PASSWORD` → 签发 HS256 access token（24h，`token_type=access`，与 P1-02 `verify_jwt` 闭环）。
- **验证**：build/clippy 0；`verify_jwt` 侧已有 P1-02 冒烟。
- **架构判断参考**：单管理员模型（plan §17）够用；若要多用户/刷新令牌，现有 claims 结构可扩。

### A4. ✅ 抓取无手动触发入口

- **缺口**：只有 5 分钟周期调度，运营侧无法「立即重抓」。
- **补**：`POST /api/v1/fetch/trigger`（`handlers/fetch_trigger.rs`）——对 active sources 批量入队 FetchJob（复用 `worker::trigger_fetch_batch`）；`AppState` 重构携带 secrets。
- **验证**：build/clippy 0；入队路径与 A1 同函数。
- **架构判断参考**：API 进程直接写 `paily_jobs` 队列表（无 HTTP 转发到 worker）——依赖 `PAILY_DATABASE_JOBS_DSN`，跨进程耦合点**仅此一个环境变量**，可接受。

### A5. ✅ SSRF 守卫缺本地测试逃生口

- **缺口**：P2-01 守卫拦一切 loopback——本地订阅服务器（测试/开发）也被拦，闭环测试无法进行。
- **补**：`FetchPolicy::default()` 读 `PAILY_FETCH_ALLOW_LOOPBACK=1` 显式放行 127.0.0.0/8；**默认不变**（生产不设置该变量则行为与之前完全一致）。
- **验证**：A1 的端到端测试就是靠它跑通的；原有 6 个 SSRF 测试不变仍绿。
- **架构判断参考**：环境变量开关 vs 配置文件——选环境变量因为它是「测试基建」而非「运行时策略」，不应出现在 config.yaml。

### A6. ✅ Node Detail / Subscriptions 页面（同轮完成，见 ACTION-LOG S36）

- **缺口**：P6-02 四页无数据源（端点缺失）。
- **补**：端点（S35）+ 四个 Vue 页面（dashboard/checks/node-detail/subscriptions，token 明文一次性弹窗）。
- **验证**：端到端（stats/checkers/logs 200、profile→token→/sub 200 链）+ `build:ele` 11/11。

---

## B. 明确推迟（附原因与重启条件）

| # | 项 | 推迟原因 | 重启条件 |
|---|---|---|---|
| B1 | **CheckJob 未接 MiaoSpeed 真测** | Adapter（P3-02）已验证可连真实 MiaoSpeed，但校机跑 MiaoSpeed 常驻 + 网络出站受限； handler 目前仅记账（幂等 run 记录），真实测活调用点已预留（`handle_check` 内注释标注） | 有可出站访问 MiaoSpeed WS 的环境；接口形状（`submit/collect`）已冻结，接入是纯装配 |
| B2 | **订阅 proxy 载荷用占位 server** | 真实载荷需 `node_payloads` 凭据加密层（方案 §7；P2 文档预留）——明文凭据入库违反 §16.8 | 加密层设计定稿（建议 AGE/XChaCha20 + `PAILY_PAYLOAD_KEY`）；转换函数本身已写好（P5-03 `clash_proxy_from`） |
| B3 | ~~**Go 源码清除**~~ ✅ **已完成（2026-11-11）** | —（用户裁定执行） | — 已完成：`internal/`、`cmd/`、`go.mod/go.sum/Dockerfile/oapi-codegen.yaml/config.example.yaml` 从工作树移除；35 处文档对照标题改为「与 fork 源对照」+ 基准注释（fork 根提交 `dda4e6f2`，`git show dda4e6f2:<path>` 查原文）；CONVENTIONS §1 模板同步更新 |
| B4 | ~~**Docker/CI 运行时演练**~~ ✅ **部分完成（2026-11-11）**：**audit 已在校机跑通**——`cargo-audit 0.23` 装上了但其 advisory-db 硬编码从 github 拉（校机被墙，`--db/--stale/改origin` 均绕不开，实测三次失败）；改为自写 **`audit-offline` 匹配器**（独立 scratch crate，`semver` crate 做权威版本匹配，本机 clone advisory-db 传入校机），**校机 Rust 实跑：1426 advisories / 327 包 / 4 命中**——`libsqlite3-sys`（Apalis sqlite feature 传递依赖，本系统只用 postgres **不可达，可 ignore**）、`ring 0.17.14`（no-patch，等上游）、`rsa 0.9.10`（ Marvin 侧信道，本场景低危）、`unsafe-libyaml 0.2.11`（serde_yaml 0.9 deprecated，换 1.0 可消）。**剩余不可行**：compose 全栈/镜像 build——校机实测 **podman/docker 都没有**（本机 podman 有但用户裁定资源留给本机），容器演练仍需有容器运行时的环境 | 有容器运行时的机器；audit 每季度跑一次 `audit-offline`（db 从本机刷新传入） |
| B5 | ~~**utoipa 契约未覆盖新端点**~~ ✅ **已完成（commit `de57dcc`）** | — | — 已完成：`crates/api-contract/src/paths2.rs` 补齐 12 个新端点注解，`spec.json` 从 7 条路径 → **19 条**（8652 B），签入 `api/openapi/openapi.json` |
| B6 | **`/api/v1/nodes/search`、`sources/search` 高级搜索** | 这两项是**上游 27 路径契约**里的端点：expr 字符串过滤（`NodeSearchRequest{expr}`）。p5-01 已把「布尔筛选」换成 Filter AST 且无 expr 依赖——补齐方式二选一：(a) 前端直接产 AST JSON 传给后端（**推荐**，新契约字段 `filter_ast`）；(b) 写 expr→AST 兼容解析器（要重新引入 expr 语义依赖，不建议）。基础能力（AST 解析/参数化 SQL/内存求值）p5-01 全就绪，缺的只是两个路由 + `POST` body 接 AST | 需要「高级搜索」页面功能时做，纯装配 |

---

## C. 对「要不要换构架」的输入（基于本轮补齐的观察）

1. **crate 切分经受了闭环验证**：A1 把四段串起来时**零接口改动**——分 crate 边界（fetch/adapter/domain/registry/database/registry）与 plan §2 架构图一致。若当初单 crate，现在拆分会很痛。
2. **Apalis 0.7.4 双队列无摩擦**：两个 job 类型同库双 Storage 实例即可；1.0 的破坏性升级动力仍不足。
3. **重复出现的摩擦点**（换构架时优先审视）：
   - **PS 引号地狱**：所有 e2e 的 JSON body 都要走文件（本机 PS 5.1 原生命令引号吞字）——与构架无关，与「Windows 驱动开发 + 远程编译」工作流有关。
   - **`paily_jobs` 独立库**：apalis setup 内嵌 migration 与业务迁移共享 `_sqlx_migrations` 是唯一被迫的偏离（plan §3.1 的「同库」假设不成立）；若换构架（如 sidekiq 式进程内队列）此项消失，但失去持久化恢复。
   - **utoipa 宏作用域限制**：schemas 与 paths 必须同文件 glob（编译期解析）——契约即代码的代价，B5 的批量补齐仍要做。
4. **无需换构架的判断依据**：本轮补齐全部是「装配」工作（把已测试的零件接线），没有发现任何需要推翻的底层设计；EWMA/指纹/选区/对账的 golden 测试在重构中一次未变。

---

## 提交记录

| 项 | 提交 |
|---|---|
| A1 fetch_job + A2 双循环 + A5 allowlist | 本轮（见 git log `feat(worker)`） |
| A3 login + A4 trigger + AppState 重构 | 同上（见 git log `feat(api)`） |
| fetch_loop e2e 测试 | 同上 |
| 本记录页 | 收尾提交 |
