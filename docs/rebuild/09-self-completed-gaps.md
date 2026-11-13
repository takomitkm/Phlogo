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

---

## 提交记录

| 项 | 提交 |
|---|---|
| A1 fetch_job + A2 双循环 + A5 allowlist | 本轮（见 git log `feat(worker)`） |
| A3 login + A4 trigger + AppState 重构 | 同上（见 git log `feat(api)`） |
| fetch_loop e2e 测试 | 同上 |
| 本记录页 | 收尾提交 |
