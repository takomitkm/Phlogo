# P6 管理前端 · Admin UI

> 阶段目录：`docs/rebuild/07-p6-admin-ui/`
> 上游依据：`paily-core-rust-rebuild-plan.md` §12 前端设计、§3 Vben 行、P6 验收；
> `docs/rebuild/CONVENTIONS.md` §2.10 API 面、§3 组件映射；
> `api/openapi/spec.yaml`（27 路径契约来源）；`docs/configuration-and-api.md`（权限与动态配置）。

## 阶段目标 / Phase Goal

为重构后的 Rust `core-api` 提供一个 **Vben Admin 5（`web-ele`）** 管理前端，使运营人员可通过 UI 完成
日常运维，而**绝不**直接访问 PostgreSQL 或 MiaoSpeed。前端通过由 OpenAPI 契约生成的 TypeScript 客户端
消费 `api/openapi/spec.yaml` 描述的 27 个 API 路径。

要点：
- 前端仅以 HTTP（REST / OpenAPI）访问 `core-api`；管理员认证用 JWT（先 `POST /api/v1/auth/login` 取 token）。
- 所有数据读写都经由 OpenAPI 契约；前端不持有任何数据库连接、不实现测活协议（见方案 §2.1：
  `Vben Admin | 管理界面 | 不负责直接访问数据库或 MiaoSpeed`）。
- 与 `core-api` 解耦：契约不变、API 行为不变时，前端可独立迭代、独立部署（静态资源由 Caddy/Nginx 提供并反代 `/api`）。
- 本阶段**不**写后端业务逻辑（P1–P5 已定义），**不**写 CI YAML（见各页「CI 提示」）。

## 子步骤 / Sub-steps

| 子步骤 | 文件 | 内容 |
|---|---|---|
| 6-01 | [p6-01-vben-scaffold.md](p6-01-vben-scaffold.md) | Vben `web-ele` 脚手架（固定 tag/commit）、OpenAPI→TypeScript Client 生成、dev 代理到 `core-api`、构建冒烟 |
| 6-02 | [p6-02-pages.md](p6-02-pages.md) | 业务页面：Dashboard / Sources / Nodes / Node Detail / Checks / Scoring / Subscriptions，逐页映射到 API 路径与 operationId |

## 阶段级验收 / Phase Acceptance

1. **常用管理操作无需直连数据库即可完成**：不打开 `psql`、不执行 SQL，仅通过前端 UI 即可完成
   来源增删改查与 `ignore_dead` 切换、节点列表筛选与删除、测活批次查看、评分权重/阈值查看、订阅筛选预览与输出测试。
   - 机器校验（手动冒烟后补进 CI）：前端源码中不存在对数据库 / 测活后端的直接依赖引用：
     ```powershell
     # 在 web/vben-admin 根目录
     grep -rEi "postgres|postgresql|sqlx|miaospeed|DSN|database/sql" src/ || echo "NO_DIRECT_DB_REF"
     ```
     预期：输出仅含 `NO_DIRECT_DB_REF`（或只出现在注释/文档中）。
2. **页面能展示节点的来源、关联、历史与评分解释**：
   - Node Detail 页展示 `sources`（来源关系，对应 `GET /api/v1/nodes/{id}` 的 `sources` / `SourceSummary`，
     以及 `GET /api/v1/sources/{id}/nodes` 反向追溯）；
   - 展示按 `checker_tag` 区分的初筛时间线（`GET /api/v1/nodes/{id}/checks/initial`）与复筛历史
     （`GET /api/v1/nodes/{id}/checks/deep`）；
   - 展示 `tag_scores`（评分分项，见 `GET /api/v1/nodes/{id}` 的 `NodeDetail`）并附评分解释
     （方案 §9.1：`score` + `components/explanations`）。
   - 机器校验：`pnpm build` 退出码 `0`，且产物包含 7 个路由入口。
3. **前端可独立构建与部署**：`pnpm build` 产出静态 `dist/`，可被 Caddy/Nginx 作为静态资源服务并反代 `/api`。
   - 机器校验：`Test-Path dist/index.html` 为 `True`。

## 与 Phlogo 现状对照 / Mapping from current Phlogo

- 当前 `Phlogo` 仓库（`S:\apps\llm\agentwork\repos\Phlogo`）**没有任何前端代码**：顶层只有
  `api/`、`cmd/`、`internal/`、`server/`、`docs/` 与 Go 源文件，无 `web/` 目录，也无 `.vue` / `.tsx` 文件。
- `docs/configuration-and-api.md`、`docs/source-management.md`、`docs/node-pool.md`、`docs/scoring.md`、
  `docs/distribution.md`、`docs/templates.md` 中描述的「管理界面 / 管理端的 … 页面」是 **上游 paily-core
  既有管理端的能力文档**，并非本 fork 已实现的前端。本阶段是**全新实现**这些能力，目标后端换成 Rust `core-api`，
  但复用的契约一致（`api/openapi/spec.yaml` 的 27 个路径）。
- 组件映射（CONVENTIONS §3）：`web/vben-admin` 为新增项，对应方案 §2.1 中 `Vben Admin | 管理界面` 的职责，
  且明确「不负责直接访问数据库或 MiaoSpeed」。

## 风险 / Risks

- **P0-04 未落地**：Vben 的精确固定 tag/commit 由 `01-p0-upstream-verification/p0-04-vben.md` 定义；
  本阶段所有「固定版本」均依赖该结论。该页当前尚未产出，需先补 P0 验证再开工（见各页 Risks）。
- **OpenAPI 契约漂移**：`spec.yaml` 是契约单一来源；若 Rust 侧 utoipa 产物与 `spec.yaml` 不一致，
  生成的 TS client 会与运行时不符。生成命令必须指向同一份契约（见 p6-01）。
- **契约缺口**（详见 p6-02）：实际 27 路径契约中**没有** `/sources/{id}/runs`、`/checks`（顶层）、
  `/subscriptions`（CRUD）端点，也没有 `fetch_policy` / `scoring_formula_version` 字段。部分 plan §11/§12
  设想的页面能力需依赖 P1/P4/P5 先补齐契约，否则本阶段只能用 `GET /api/v1/logs/fetch`、`GET /api/v1/logs/check`、
  `GET /{format}` 等现有端点做替代展示。
