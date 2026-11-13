# 09 自补记录 · Self-Completed Gaps Log

> 目的：记录重构收口后**自查发现并补齐的缺口**，供后续评估「要不要换构架」时参考。
> 每项含：缺口是什么、为什么此前没做、补了什么、验证方式、对架构判断的影响。
> 状态标记：✅ 已完成并验证 / ⏸ 明确推迟（附原因）。

---

## A. 已补齐（本轮）

### A1. ✅ Fetch 闭环从未被调用（最关键）

- **缺口**：P2 的四段实现（`crates/fetch` SSRF 抓取 → `vproxy-adapter` 解析 → `domain` 指纹 → `registry` 对账）各自测试全绿，但 **worker 里没有任何代码把它们串起来**——抓取闭环从未真正运行过。这是「分段测试通过 ≠ 系统可用」的典型案例。
- **补**：`apps/worker/src/fetch_job.rs`——`FetchJob`（Apalis 任务）+ `run_fetch()`（拉 source.content → fetch → parse → `domain::stamp` 指纹 → `reconcile_fetch_result` 落库；失败路径记录 failed run 保留旧关联）+ `enqueue_for_active_sources()`（批量入队）。
- **验证**：`apps/worker/tests/fetch_loop.rs` 端到端——本地 HTTP 订阅服务器 → SSRF 守卫（`PHLOGO_FETCH_ALLOW_LOOPBACK=1`，新增的显式逃生口，生产不设）→ 抓取 → 解析 2 节点 → 指纹 32-hex → 映射落库 → run=finished → **二次 run 幂等不重复**。校机实测 **1 passed**。
- **架构判断参考**：闭环成立，四段分 crate 的边界（fetch/adapter/domain/registry）在实践中没有互相渗透，抽象成本是值得的。

### A2. ✅ Worker 双循环 + 周期调度缺失

- **缺口**：worker 只有 CheckJob 消费者，没有 fetch 消费者；`lifecycle::run_cleanup_cycle`（P4-02）从未被定时触发——孤儿永远停在 Orphaned，不会被退役/删除。
- **补**：`apps/worker/src/main.rs` 重写——**两个 Apalis worker**（fetch + check）+ 调度协程（`tokio::select!` 双 interval：fetch 批量入队 300s / cleanup tick 600s）。
- **验证**：build/clippy 0；调度逻辑复用已测函数（`enqueue_for_active_sources`、`run_cleanup_cycle`）。
- **架构判断参考**：Apalis PG backend 承载双队列无冲突（同一 `phlogo_jobs` 库两个 Storage 实例）；周期任务没有引入 cron 框架，`tokio::interval` 够用——若未来需要分布式锁再换。

### A3. ✅ 无登录端点（Vben 登录必挂）

- **缺口**：认证中间件 P1-02 就有（admin JWT / service bearer），但**没有签发 JWT 的端点**——Vben 登录页无从获取 token。
- **补**：`POST /api/v1/auth/login`（`handlers/auth.rs`）——校验 `PHLOGO_AUTH_ADMIN_PASSWORD` → 签发 HS256 access token（24h，`token_type=access`，与 P1-02 `verify_jwt` 闭环）。
- **验证**：build/clippy 0；`verify_jwt` 侧已有 P1-02 冒烟。
- **架构判断参考**：单管理员模型（plan §17）够用；若要多用户/刷新令牌，现有 claims 结构可扩。

### A4. ✅ 抓取无手动触发入口

- **缺口**：只有 5 分钟周期调度，运营侧无法「立即重抓」。
- **补**：`POST /api/v1/fetch/trigger`（`handlers/fetch_trigger.rs`）——对 active sources 批量入队 FetchJob（复用 `worker::trigger_fetch_batch`）；`AppState` 重构携带 secrets。
- **验证**：build/clippy 0；入队路径与 A1 同函数。
- **架构判断参考**：API 进程直接写 `phlogo_jobs` 队列表（无 HTTP 转发到 worker）——依赖 `PHLOGO_DATABASE_JOBS_DSN`，跨进程耦合点**仅此一个环境变量**，可接受。

### A5. ✅ SSRF 守卫缺本地测试逃生口

- **缺口**：P2-01 守卫拦一切 loopback——本地订阅服务器（测试/开发）也被拦，闭环测试无法进行。
- **补**：`FetchPolicy::default()` 读 `PHLOGO_FETCH_ALLOW_LOOPBACK=1` 显式放行 127.0.0.0/8；**默认不变**（生产不设置该变量则行为与之前完全一致）。
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
| B2 | **订阅 proxy 载荷用占位 server** | 真实载荷需 `node_payloads` 凭据加密层（方案 §7；P2 文档预留）——明文凭据入库违反 §16.8 | 加密层设计定稿（建议 AGE/XChaCha20 + `PHLOGO_PAYLOAD_KEY`）；转换函数本身已写好（P5-03 `clash_proxy_from`） |
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
   - **`phlogo_jobs` 独立库**：apalis setup 内嵌 migration 与业务迁移共享 `_sqlx_migrations` 是唯一被迫的偏离（plan §3.1 的「同库」假设不成立）；若换构架（如 sidekiq 式进程内队列）此项消失，但失去持久化恢复。
   - **utoipa 宏作用域限制**：schemas 与 paths 必须同文件 glob（编译期解析）——契约即代码的代价，B5 的批量补齐仍要做。
4. **无需换构架的判断依据**：本轮补齐全部是「装配」工作（把已测试的零件接线），没有发现任何需要推翻的底层设计；EWMA/指纹/选区/对账的 golden 测试在重构中一次未变。

---

## D. 生产就绪核查（2026-11-13，只读审计）→ **未完成清单（全部必须做）**

> 触发：用户问「上游为什么说不适合生产？我们这版适合了没有？」并给出 5 个必确认项 +
> README 一致性提醒。本节把核查结论**并入本缺口台账**，作为后续排期依据。
> **口径：下列每一项都是「要做」，不是「可选/推迟」。** 本节只记录，不改代码。

### D.0 上游警告的原文与含义

上游 `openpaily/paily-core` README 顶部 `[!WARNING]`（"该代码库非直接生产可用"）给的三条理由：
① 早期开发、部分功能问题较多；② 未知问题可能造成**数据丢失**；③ **近期无继续推进计划**（建议等待可用分支）。

**判定**：①②描述的是 **Go 那一版的代码状态**，不是协议/架构的固有缺陷，不能自动套到 Rust 重写版；
但③意味着**不能等上游修**。我们这版合不合格必须看自己的实际状态——结论见 D.2：**不合格**。

### D.1 五个必确认项：逐条结论（带代码证据）

| # | 问题 | 结论 | 证据 |
|---|---|---|---|
| 1 | 测速后端能否模拟**真实 API 请求**（而非普通网页） | ❌ **不能** | MiaoSpeed 4.7.9 矩阵是固定类型白名单（`01-p0-upstream-verification/p0-02-miaospeed.md` D.7）；唯一可配 HTTP 目标是 `Configs.PingAddress`（**单一全局 URL**）与 `DownloadURL`。我们把它**硬编码**成 `http://gstatic.com/generate_204`、`DownloadURL=""`（`crates/checker-miaospeed/src/adapter.rs:84-88`）。`TEST_SCRIPT`/`Scripts[]` 我们传空数组，且 P0-02 自陈「完整验证放 P3」、JS 沙箱从未实现 |
| 2 | 能否识别**流媒体解锁 vs AI 服务可用性差异** | ❌ **不能**（双层缺失） | ① `map_results` 把 `deep` 直接写 `None`（`adapter.rs:112`）——即使回了 streaming 也丢掉；② `node_streaming` 表不存在，`unlock` 条件在 SQL 里编译成 `FALSE`（`crates/filter/src/sql.rs:170-172`、`apps/curse/src/handlers/search.rs:8-9`）；③ 4.7.9 矩阵白名单里**没有** unlock/AI 测试类型 |
| 3 | 是否支持**按目标服务分别评分**（OpenAI / Google / Anthropic） | ❌ **不支持，且是新设计不是接线** | 评分因子只有 latency/jitter/stability/quality/speed（`crates/scoring/src/engine.rs:89-148`）；`10-review-crosscheck.md` §2.3 已标「流媒体分别评分 ❌」「场景化配置 ❌」。需新增 per-service 探针 → `node_service_scores` → 聚合 → 筛选 → 分发，且受 #1 结构限制（`PingAddress` 一轮一个值） |
| 4 | 下载测速是否**大量消耗节点带宽** | ⚠️ **当前零消耗，但代价是分数封顶** | `DownloadDuration=0`/`DownloadThreading=0`/`DownloadURL=""` → 根本不跑（`adapter.rs:84-88`）。但评分按 Go 基线「速度始终混合」：`speed_kbps=0` → `speed_score=0` → `score = quality×(1−0.55) = 0.45×quality`（`engine.rs:143-148`）；分级阈值 A/B/C/D = 0.75/0.50/0.25（`crates/distribution/src/naming.rs:15`）→ **任何节点都拿不到 A/B，最高 C**。一旦开启：总流量 ≈ `DownloadDuration × DownloadThreading × 节点数`（30s×4×20MB/s×1000 节点 ≈ 2.4 TB/轮），**必须先有配额** |
| 5 | 评分能否**及时反映突然恶化** | ❌ **不能**（算法够灵敏，链路是断的） | 算法：EWMA `alpha=0.7`、权重 `alpha^i`，半衰期约 2 样本（`engine.rs:104`、`params.rs:44`）——够灵敏。但 ① 评分引擎在 `apps/` **零引用**；② 测活从未发生（D.2 M2/M3）；③ **速度只取最新一条、不走 EWMA**（`engine.rs:143` 注 "latest record only"）→ 单次抖动即打到底；④ 增量测活 ❌ 全量轮（`10-review-crosscheck.md` §3.3）、存活窗口 240 分钟（`crates/lifecycle/src/ops.rs:171`）→ 发现延迟 = 跑完一整轮 |

### D.2 未完成清单（MUST-DO，按依赖排序）

> 每条含：现状证据 → 影响 → 验收标准。**全部要做**；P0 = 上线前必须，P1 = 紧随，P2 = 功能补齐。

| ID | 项 | 现状证据 | 影响 | 验收标准 | 优先级 / 依赖 |
|---|---|---|---|---|---|
| **M1** | `/sub` 输出**真实 proxy 载荷**（`node_payloads` + 凭据加密） | `apps/curse/src/handlers/subscription.rs:120` 仍是 `format!("{}://node/{}")`；即 B2 | **客户端拿到 200 OK 但全是连不上的假节点**——比上游警告更直接 | `curl /sub/{token}.yaml` 产出真实节点 → 经 `mihomo -t` 通过 → 真代理出网 204；凭据不明文入库（AGE/XChaCha20 + `PHLOGO_PAYLOAD_KEY`） | **P0**，即 B2 |
| **M2** | CheckJob **接 MiaoSpeed 真测**（B1 升级） | `apps/worker/src/check_job.rs:48-58` 只 `record`+`finish`，注释自陈未接；`MiaoSpeedAdapter` 仅在自身单测出现 | 一次真实测活都不会发生 → 无延迟/速度/地区/streaming 数据 | 一条真实节点经 WS 提交→收结果→`node_check_results` 落库；golden 签名不回归 | **P0**，即 B1 |
| **M3** | **CheckJob 入队/调度/手动触发** | 全仓 `CheckJob {` 只出现在定义与测试；`main.rs` 调度只有 fetch 300s + cleanup；`router.rs` 无 `/check/trigger` | M2 接了也没人调用 | 周期 tick 入队 + `POST /api/v1/check/trigger` 手动触发 + 幂等（`ON CONFLICT`） | **P0**，依赖 M2 |
| **M4** | adapter **保留 deep** | `adapter.rs:112` `deep: None`；`result.rs` 的 deep 解析未被调用 | #2/#4/#5 全无数据 | `map_results` 填 `avg_latency/jitter/speed/region/streaming`；单测断言 deep 非空 | **P0**，依赖 M2 |
| **M5** | **评分引擎接线** | `apps/` 对 `scoring` 零引用（grep 无命中） | `node_scores` 永不更新，`/sub` 全按 score=0 选 | 一条 check 结果入库后 `node_scores` 更新且 `components` JSONB 可读；EWMA 随历史变化 | **P0**，依赖 M2+M4 |
| **M6** | `/sub` 真正使用 `filter_ast` / `preset` | `subscription.rs:46-49` `let _ = _filter_ast; let _ = preset;`；`:84` 硬编码 `"common"` | 档案的地区/健康/最低分/AST 筛选**全部失效** | 建两份不同 profile → 同一 token 只出符合该档案的节点；preset 取 profile 值 | **P1**，依赖 M1 |
| **M7** | 建 **`node_streaming`** 表并落库 | 无迁移；`filter/sql.rs:82` 的 EXISTS 永远查空表 | `unlock` 筛选永远不匹配（D.1 #2） | 迁移建表；一条 streaming 结果入库后 `unlock contains Netflix` 能筛出该节点 | **P1**，依赖 M4 |
| **M8** | **测速开关 + 带宽配额**（开之前先定） | `Configs` 全 0；开启后 `score` 才可能 >0.45 | 不开→A/B 永不出现；乱开→TB 级流量 | 配置化 `DownloadURL/Duration/Threading`；单节点字节上限 + 候选抽样 + 只测过初筛节点；`score` 可达 A | **P1**，依赖 M4+M5 |
| **M9** | **速度维度平滑** | `engine.rs:143` "latest record only (Go baseline)" | 单次抖动把速度分打到底，或一次侥幸掩盖持续恶化 | 速度参与 EWMA（或单独滑窗）；单测：一次坏样本不至于从 A 掉到 D | **P2**，依赖 M5 |
| **M10** | **增量测活 + 恶化快速降级** | `10-review-crosscheck.md` §3.3「增量测活 ❌ 现为每轮全量」；§2.3「多 Checker 置信度/连续 vs 偶发失败 ❌」；窗口 240min | 突然恶化到被发现 = 跑完整轮；偶发抖动与真死不分 | 只测有变化/高风险节点；连续 N 次失败立即降级；`node_alive_check_window_minutes` 可配且接线 | **P2**，依赖 M2 |
| **M11** | **按目标服务分别评分**（OpenAI/Google/Anthropic） | D.1 #3 | 目前只能给"一个总分"，无法表达"这节点能开 OpenAI 但不能开 Netflix" | `node_service_scores` + per-service 评分聚合 + 筛选维度 + 分发档案 | **P2**，依赖 M12 |
| **M12** | **多目标探针能力**（M11 的前置） | `PingAddress` 单一全局值硬编码；`TEST_SCRIPT`/`Scripts[]` 未启用、无 JS 沙箱 | 结构上无法「同一轮对每个服务各打一个探针」 | 扩展测活后端或自写脚本测试 + 安全沙箱；一轮可对 N 个目标分别出结果 | **P2**（M11 前置） |
| **M13** | Sub-Store sidecar **接 Rust 侧** | `14-substore-sidecar.md` §9 明确「未做」 | 扩展格式目前没有出口 | `crates/format-sidecar` + `format_configs` 行 + 模板包装 + compose healthcheck + CI 门禁 | **P2**，依赖 M1 |
| **M14** | **CI**（用户先前裁定「先不写」） | `10-review-crosscheck.md` §工程化「CI ❌ 用户裁定先不写」 | 上述改动无自动回归 | 校机同款门禁脚本化；D.3 的入口一致性校验进 CI | **P1**（恢复排期） |

**上线总门禁（一条龙验收）**：
`抓取 → 真测活 → deep 落库 → 评分更新 → /sub 出真实节点 → mihomo -t 通过 → 真代理出网 204`。
这条链目前**断点在第一步之后**，M1–M5 之前任何一项未完成都不算可部署。

### D.3 部署入口与文档一致性（用户提醒的核查结果，**必须修**）

> 结论分两半：**本仓 README 已经是纯 Rust**（`cargo run -p curse` / `-p worker`、`podman compose`），
> 用户看到的 `go build -o paily-core ./cmd/server` 是**上游 README**的启动段，不是本仓遗留。
> 但确实存在三处会影响部署的真实不一致：

| ID | 不一致 | 证据 | 必须修成 |
|---|---|---|---|
| **M15** | quickstart **少起服务** | `README.md:74` 只起 `postgres curse worker caddy`；`deploy/compose.yaml:56-66` 另有 `miaospeed`、`web` | quickstart 补齐 `miaospeed web`（否则即使 M2 接线，测活后端也不存在）；或明确标注"不含测活/前端"并给出完整命令 |
| **M16** | miaospeed 镜像 **floating tag** | `compose.yaml:57` 为 `ghcr.io/miaokobot/miaospeed:latest`，注释"PIN at deploy time" | 换成 P0-02 自建 `4.7.9` + SHA-256（`docs/THIRD_PARTY.md` 已有哈希），落实本仓「禁止 floating」供应链规则 |
| **M17** | rebuild 文档**残留旧入口名** | `08-p7-productionization/p7-01-compose-backup.md:52` 写 `--bin gnosis-api`；`02-p1-core-skeleton-data/p1-02-api-skeleton.md:230` 写 `cargo run -p api`；`00-index.md:12` 写「Phlogo 当前是一个 Go 单体」 | 历史步骤页加「当时命名」标注或更正；**以实际产物为准**：`deploy/Dockerfile.curse:11` = `--bin curse`、`Dockerfile.worker:9` = `--bin worker`；入口 `apps/curse/src/main.rs` / `apps/worker/src/main.rs`，caddy 21380 唯一入口 |

### D.4 与既有 B1/B2 的关系（口径变更）

- 上表 **M2 = B1**、**M1 = B2**：B 节原本标注「明确推迟（附重启条件）」。**本轮用户裁定：这些都要做**，
  故状态由 `⏸ 推迟` 升级为 **`MUST-DO / P0`**，重启条件视为已满足（不再等"可出站环境"才评估，
  而是列入排期并补 CI 门禁）。
- B6（高级搜索）保持原状；B4 的容器演练仍受"无容器运行时"约束，但 **M15 的 compose 一致性核对不需要容器**。
- **本节只做记录与排期，未改动任何代码**；代码实现按 M1→M2→M3→M4→M5 顺序逐步落地，每步照本仓惯例
  「做一步、写一页、push 一步」。

### D.5 生产级能力 5 项（用户 2026-11-13 追加，**全部要做**）

> 用户给出 5 行能力对照（无表头，中间/右侧两列是两档评价）。**不臆测两列指哪两个对象**，
> 只按「能力维度」核查**我们这版**的实际状态；若需补对比列，用户给出对象后再补。
> 结论：5 项**没有一项达到完整档**，且多数比"有限/部分"更弱（是**未接线/未实现**）。

| # | 能力 | 用户给的档位 | **我们这版的现状（带证据）** | 判定 |
|---|---|---|---|---|
| 1 | **按业务选择出口** | 有限 / 有 | schema 有 `subscription_profiles.{filter_ast,region,health,min_score,preset,output_format}`（`migrations/0009_subscriptions.up.sql:2-16`），Filter-AST 白名单 7 字段（`crates/filter/src/parse.rs:6-14`）；**但 `/sub` 把 `filter_ast`/`preset` 用 `let _ =` 丢掉、preset 硬编码 `"common"`**（`handlers/subscription.rs:46-84`，即 M6），`unlock` 无表（M7），无 per-service 维度（M11/M12）。上游 Go 版另有「按检测视角（checker_tag）分发」（上游 README 主要能力），我们也没接 | **比"有限"更弱：未接线** |
| 2 | **按用户地理位置调度** | 非核心能力 / 有 | PROXY protocol v1 透传真实客户端 IP **只用于登录限流分桶**（`apps/curse/src/rate_limit.rs`、`proxy_protocol.rs`）；全仓无 GeoIP 查用户位置、无按用户地区路由（grep `client_ip\|geoip\|x-forwarded` 零命中）。`region` 是**节点出口地区**（MiaoSpeed `GEOIP_*` 矩阵，`p0-02` D.7）而非用户位置，且 deep 被丢（M4） | **不是"非核心"，是未实现** |
| 3 | **SLA 保障** | 不完整 / 有 | 只有 `/health` + 最小 `/metrics`（`handlers/metrics.rs:1` 自陈 "Minimal Prometheus…"）；**compose 无 `healthcheck` / `restart` / `stop_grace_period`**（grep 零命中）；无 SLO、错误预算、告警规则、降级策略；容器全栈演练因无运行时未做（B4） | 不完整，且比该档更薄 |
| 4 | **故障自动恢复** | 部分 / 完整体系 | 应用层**有**：Apalis PG at-least-once + 租约/超时回收（P0-03 真机验证）、`ON CONFLICT` 幂等（`check_job.rs:22-34`）、`should_overwrite` 迟到不覆盖、fetch 失败保留旧关联。**编排层无**：compose 无 restart/healthcheck、无断路器/退避配置、sidecar 无 supervision、M2 未接线故测活故障无从恢复；B4 容器演练未做 | 部分（**仅应用层**） |
| 5 | **权限、审计、合规** | 不完整 / 通常有 | **权限已有且修过**：fail-closed 权限矩阵（P0-01）、登录限流（P0-02）、token 哈希可吊销、JWT、敏感字段 redaction、SSRF 守卫、不信任 XFF（`auth.rs`、`telemetry.rs`、`p7-03`）。**审计/合规缺**：migrations 无 `audit*` / `actor` / `ip_address` 表；`/logs/check`、`/logs/fetch` 是**测活/抓取业务日志**，不是管理操作审计；无配置变更留痕；SBOM 未产出（`p0-05` 仅规划）、`cargo audit` 校机受限（B4 离线匹配器 4 命中） | 权限 ✅ / 审计合规 ❌ |

**新增 MUST-DO（M18–M22，全部要做，编号续 D.2）：**

| ID | 项 | 现状证据 | 验收标准 | 优先级 / 依赖 |
|---|---|---|---|---|
| **M18** | **按业务/目标服务选出口** | D.5 #1 | profile 能按 service（OpenAI/Netflix/…）与 checker_tag 选出口，`/sub` 真正生效；不接 M6 的硬编码 | **P1**，依赖 M6+M7+M11+M12+M13 |
| **M19** | **按用户地理位置调度** | D.5 #2 | 引入用户侧 GeoIP（沿用 PROXY 真实 IP，**不信任 XFF**）+ 路由策略；不同地区请求得到不同节点集；有"未知/私有 IP"兜底 | **P1**，依赖 M19 自身新增 + M6 |
| **M20** | **SLA 保障** | D.5 #3 | 定义 SLO（`/sub` 可用率、生成 P95）；Prometheus 告警规则；compose `healthcheck` + `restart`；**sidecar/MiaoSpeed 不可用时原生三格式仍可用**；故障注入下 `/sub` 仍 200 | **P1**，依赖 M2+M13 |
| **M21** | **故障自动恢复（编排层）** | D.5 #4 | compose restart/healthcheck、任务级重试退避、断路器、sidecar supervision；kill 容器后自动拉起且队列任务不丢不重；补做 B4 容器演练 | **P1**，依赖 M2+M13+M20 |
| **M22** | **审计与合规** | D.5 #5 | 建 `admin_audit_log`（actor/time/action/target/before→after）；任一管理写可查到审计行；配置变更留痕；SBOM 产出且与 `THIRD_PARTY.md` 哈希一致；`cargo deny`/`audit` 进 CI | **P1**，依赖 M14 |

**分层门禁（口径：全部要做，但分两道，不要混成"上线前全做完"）**：

- **可部署门禁**（不给用户发假节点）：**M1–M5 + M15** —— 即 D.2 的上线总门禁那条链。
- **生产级门禁**（GA 前）：**M6–M14 + M18–M22** —— 上面 5 项能力 + 筛选/评分/配额/审计/CI。
- 用户裁定：**两道都算「必须做」**，区别只在顺序与验收时点，不是做不做。

> 与 D.4 一致：本节**只做记录与排期，未改代码**；未改动 A/B/C 节历史结论。

### D.6 计划对照：`paily-core-rust-rebuild-plan.md` 逐节核查（2026-11-14，用户裁定全部要做）

> 对照对象：`S:\Downloads\paily-core-rust-rebuild-plan.md`（§14 分阶段验收、§15 核心验收 17 条、
> §16 安全供应链 10 条、§17 MVP 不做、§18 冻结规格 10 份、§19 技术基线）。
> **结论分三层**：① 阶段实现层基本完成（P0–P7 有实现有测试有文档，3 个 ⚠️）；② 计划验收条款**未全达成**
> （§18 规格 9/10 缺、P4 评分历史 ❌、§15 #15/#17、§16 四条 ⚠️）；③ 系统目标（§1.1 能实际提供订阅服务）未达——
> 即 D.2 的 M1–M5。新增 **M23–M27**，与既有 M1–M22 合并为完整 MUST-DO 总账。

#### D.6.1 §14 分阶段验收逐条判定

| 阶段 | 计划验收要点 | 判定 | 证据 / 缺口 |
|---|---|---|---|
| P0 | 独立示例+测试+固定版本+许可证（VProxy/MiaoSpeed/Apalis/Vben） | ✅ PASS | 5 页全过；VProxy 改「最小移植」为 P0-01 判定的分歧路径（已记录） |
| P1 | 两来源一节点、两关联 | ✅ | p1-04 去重/关联集成测试 |
| P2 | Replace 删消失、失败不清空、Accumulate 不删 | ✅ | p2-04 reconcile 6 测试 + `fetch_loop` e2e（§A1） |
| **P3** | 批量测活准确映射、断连重试不重复 | ⚠️ **零件绿、整机桩** | adapter/映射/幂等/持久化全有测试；但 `handle_check` 未接 adapter、无入队点 → **M2/M3**；真机仅 P0-02 一次 spike |
| **P4** | tag 独立、不可达不删关联、评分公式+版本、**评分历史/快照**、orphan | ⚠️ | 状态机/orphan/ewma-v1 golden ✅；**评分历史 ❌ → M23**（`0007_node_scores` 仅当前行 `PK(node_id,checker_tag)`，grep `score_history/snapshot` 零命中）；评分未接线（M5） |
| **P5** | 结果符合条件、**Mihomo 校验**、撤销立即失效 | ⚠️ | AST/配额/token ✅；`/sub` 丢 `filter_ast`/`preset`（M6）；Mihomo 校验仅到转换映射层（S54 真 `mihomo -t` 13 协议全 PASS），**`mihomo_yaml` 完整模板产物未过真二进制 → M27**（p5-03 自陈留 P7） |
| P6 | 不碰库即可管理、可看来源/历史/评分解释 | ✅ | Vben 5.7.0、页面+端点+e2e（§A6） |
| **P7** | compose、**备份/恢复演练**、日志指标、任务恢复、限流认证、SSRF、**SBOM 扫描** | ⚠️ | compose/脚本/日志/限流/SSRF ✅；**SBOM ❌ → M25**、**备份恢复演练 ❌ → M26**（B4 无容器运行时）、cargo-deny/ScanCode ❌（M25）、CI ❌（M14） |

#### D.6.2 §15 核心验收测试 17 条对照

**15 条有直接测试证据**：#1 两来源一节点（p1-04）；#2–4 指纹 golden 全表——`name_change` 相同 /
`uuid·password·transport·tls·server·port_change` 不同 / `field_order`、`null_default` 相同
（`p2-03-fingerprint.md:152`，`cargo test -p domain` 14 passed）；#5–9 fetch/对账/孤儿（p2-04、p4-02）；
#10–14 tag 独立（`0007` PK）、批量映射（p3-03）、迟到不覆盖（`should_overwrite`）、重试幂等（同 run 4 次 1 行）。

**2 条未达成 → M27**：
- **#15 订阅不输出 Retired 节点**：结构上满足（`/sub` SQL 过滤 `membership='Linked'`），**无专门断言测试**；
- **#17 生成的 Mihomo YAML 过目标版本校验**：见 D.6.1 P5 行。

> **关键口径**：这 15 条 ✅ 是**零件级**（mock/单测/集成）。P0-02 只做过一次真机 spike——
> **没有任何一条在「真实 MiaoSpeed 常驻 + 真实订阅」下端到端跑出来**。计划的验收意图是系统级，
> 所以 P3/P4/P5 的系统级验收实际未达成（M2–M5 接线后才算）。

#### D.6.3 §16 安全/许可证/供应链 10 条对照

✅ 6 条：MiaoSpeed AGPL + 自建哈希（THIRD_PARTY）、Mihomo GPL 义务登记、VProxy MIT 保留版权（最小移植）、
AGPL 参考不复制（B3 clean-room）、SSRF（6 测试 + 重定向二次拦截）、端口隔离（PG/MiaoSpeed 零公网，compose）。
Koipy 未用 = 符合 §17。

⚠️ 4 条：**SBOM 未产出**（`p0-05` 仅规划）、**cargo-deny / ScanCode 未跑**（deny.toml 脚手架在；audit 用
B4 离线匹配器部分覆盖）、**凭据加密层未做**（§16.8——当前恰好未存凭据是因为载荷占位，M1）、
**备份恢复演练未做**（M26）。

#### D.6.4 §18 冻结规格文件：**9/10 缺 → M24**

计划要求 P0/P1 完成的 10 份，`docs/` 下只有 **`THIRD_PARTY.md`** ✅。
缺：`SPEC.md`、`ARCHITECTURE.md`、`DATA_MODEL.md`、`NORMALIZED_NODE.md`、`LIFECYCLE.md`、
`MIAOSPEED_PROTOCOL.md`、`SUBSCRIPTION_SPEC.md`、`SECURITY.md`、`ADR/`。

内容**散落在** `CONVENTIONS.md`、rebuild 各页与 `docs/{scoring,node-pool,distribution,templates,source-management,configuration-and-api}.md`，
但计划点名的命名文件不存在。**佐证**：`p0-02` 验收里留有未勾选方框
「`[ ] docs/MIAOSPEED_PROTOCOL.md 独立成档（P3 前完成）`」；且 `10-review-crosscheck.md` 引用了
不存在的 `LIFECYCLE.md`（"Degraded 规则一并冻结于 LIFECYCLE.md"）——文档互相引用悬空。

**M24**：9 份文件落地，内容**单源化**（从既有 rebuild 页抽取/互链，不复制两份导致漂移）；
`MIAOSPEED_PROTOCOL.md` 直接以 `p0-02` D 节为源抽出。验收：9 份存在、交叉引用不再悬空、
CI 可断言文件存在。

#### D.6.5 其余节

- **§17 MVP 不做**：全部遵守 ✅（sing-box 完整转换是计划标注「第一版不做」的项，S52–S54 做了——超出计划，不算违规）。
- **§19 技术基线**：与实现一致 ✅；VProxy 一项为 P0-01 记录在案的决策分歧（计划允许「P0 验证后复用或最小化移植」）。
- **计划外新增（在案）**：S52–S54 协议覆盖 100% + 双真二进制校验、S57 Sub-Store sidecar、S58/S59 审计。

#### D.6.6 新增 MUST-DO（M23–M27，全部要做）

| ID | 项 | 现状证据 | 验收标准 | 优先级 / 依赖 |
|---|---|---|---|---|
| **M23** | **评分历史/快照**（P4 计划要求） | `0007_node_scores` 仅当前行，无历史表 | 历史表 + 评分接线后每轮落一行；能回答「该节点 N 天前分数」；保留天数可配 | **P1**，依赖 M5 |
| **M24** | **§18 冻结规格 9 份落地** | D.6.4 | 9 份存在、内容单源化、交叉引用不悬空、CI 断言文件存在 | **P1**，无依赖（纯文档可先做） |
| **M25** | **SBOM + cargo-deny + ScanCode**（§16.6/16.7） | deny.toml 脚手架在、SBOM 未产出 | 有网环境跑 `cargo deny check licenses/bans` + `cargo cyclonedx`（SBOM）+ ScanCode 抽样；与 THIRD_PARTY 哈希一致；进 CI（M14） | **P1**，依赖 M14 |
| **M26** | **备份/恢复演练**（P7 + B4） | 脚本在、演练未做 | 容器运行时环境：compose 全栈拉起 → `backup.sh` → 清库 → `restore.sh` → 数据/来源关系/任务状态一致 | **P2**，依赖容器运行时（B4） |
| **M27** | **§15 #15 + #17 两条验收** | D.6.2 | #15：集成测试断言 Retired 节点不出现在 `/sub` 输出；#17：校机真 `mihomo -t` 校验 `mihomo_yaml` **完整模板产物** | **P1**，#17 依赖 M6 + 真 mihomo 环境 |

**门禁归属**：M23–M27 全部归**生产级门禁**（M6–M14 + M18–M22 扩为 M6–M27）；可部署门禁仍是 M1–M5 + M15。
**总量：M1–M27，全部要做**（用户裁定）。

---

## 提交记录

| 项 | 提交 |
|---|---|
| A1 fetch_job + A2 双循环 + A5 allowlist | 本轮（见 git log `feat(worker)`） |
| A3 login + A4 trigger + AppState 重构 | 同上（见 git log `feat(api)`） |
| fetch_loop e2e 测试 | 同上 |
| 本记录页 | 收尾提交 |
