## 1-04 Vben Admin 5 固定版本构建与 OpenAPI Client 生成验证 · Verify Vben Admin 5 Pinned Build & OpenAPI Client Generation

> 对应方案 §3（Admin UI: Vben Admin 5 `web-ele`）、§12（前端设计）、CONVENTIONS §2.1 / §4。
> 本页是 **P0 spike**：固定 Vben `web-ele` tag，确认其能构建，并确认
> 「OpenAPI（`api/openapi/spec.yaml`）→ TypeScript client」生成流程可用。
> 生成的 TS client 是 Vben 管理页与 Rust Axum API 的契约桥梁（方案 §11、§12）。

### 目标 / Goal

- 固定 **Vben Admin 5 `web-ele`** 版本（tag/commit，写入 `docs/THIRD_PARTY.md` 的 `<PIN Vben web-ele tag>`）。
- 确认一个最小 Vben app 能 **`pnpm build`** 成功（验证工具链与构建链路）。
- 确认 **OpenAPI → TypeScript client** 生成命令可用，并打通 `api/openapi/spec.yaml` 这条契约源。
- 为 P6（Admin UI 完整页面）与 P1（Axum API 产出 `spec.yaml`）打好前端契约基础。

### 范围 / Scope

- **新建**：scratch 前端工程（如 `web/vben-spike`），仅验证，不进生产 `web/vben-admin`。
- **验证内容**：`pnpm install` + `pnpm build` 成功；OpenAPI client 生成命令跑通并产出 TS 文件。
- **不做**：不实现 Sources/Nodes/Checks/Scoring/Subscriptions 业务页面（P6）；不改动 `api/openapi/spec.yaml`（它由 P1 用 utoipa 产出，本页只是消费方）。
- **契约源**：本页消费的 OpenAPI 文档是 `api/openapi/spec.yaml`（Phlogo 现有 27 paths，见 CONVENTIONS §2.10），未来由 Rust 侧 utoipa 生成。

### 前置 / Prerequisites

- Node.js + pnpm：**校机 `node v24.22.2` / `pnpm 10.33.4`**（满足 Vben v5.7.0 `engines`：node `^22.18.0||^24.0.0`、pnpm `>=10.0.0`）。
- Vben Admin 仓库：<https://github.com/vbenjs/vue-vben-admin>（方案 §20），固定 **tag `v5.7.0`**（commit `63a38dce49ba109f61607994e21ba921d8e970e9`，2026-05-21）。
  `web-ele` = monorepo 内 `apps/web-ele`（包名 `@vben/web-ele`）。npm 源：仓库自带 `.npmrc` 已指 `https://registry.npmmirror.com`（校机可达）。
- 已有 `api/openapi/spec.yaml`（确认存在：`S:\apps\llm\agentwork\repos\Phlogo\api\openapi\spec.yaml`）。
- 已读方案 §12（页面清单）与 §11（API 初始清单）。
- OpenAPI→TS 工具：Vben 生态常用 `openapi-ts`（或 Vben 自带 `vben-api-client` 生成脚本）；具体以固定版本为准。

### 步骤 / Steps

**1. 固定 Vben `web-ele` 版本并取最小工程**

```bash
# 固定版本（写入 docs/THIRD_PARTY.md 的 <PIN Vben web-ele tag>）
git clone --depth 1 --branch <PIN Vben web-ele tag> https://github.com/vbenjs/vue-vben-admin
cd vue-vben-admin
git rev-parse HEAD   # 记录完整 commit SHA
cat package.json | grep -A3 engines   # 记录 Node/pnpm 版本要求
```

> 若 `web-ele` 是独立模板仓库/分支，以方案 §3 注明的 `web-ele` 为准；记录真实仓库路径与 tag。

**2. 安装依赖并构建最小 app**

```bash
pnpm install --frozen-lockfile   # 若有 lockfile，固定依赖
pnpm build
echo "exit=$?"   # 期望 0，产物在 dist/（或固定版本配置的 outDir）
ls -la dist/                     # 确认产出存在
```

**3. 打通 OpenAPI → TypeScript client 生成**

```bash
# 以固定版本 Vben 提供的 OpenAPI 生成命令为准（示意，TOTO 标注真实脚本）。
# 常见形式（openapi-ts / vben 内置）：
#   pnpm openapi-ts --input ../api/openapi/spec.yaml --output src/api/core
# 或 Vben 的 gen 脚本读取 spec 生成 typed client。
#
# 关键：输入必须指向 api/openapi/spec.yaml（Phlogo 现有契约），
#       未来由 Rust 侧 utoipa 产出同一份契约以供前端重新生成。
echo "generate TS client from api/openapi/spec.yaml (TODO: 固定版本真实命令)"
```

**4. 验证生成的 TS client 可被最小页面 import**

```bash
# 在最小 Vben 页面里 import 生成的 client 类型，确认类型与 spec.yaml 的 path 对应。
# 例如生成产物中应包含 /api/v1/sources、/api/v1/nodes、/api/v1/checks 等路径的类型。
grep -rl "sources" src/api/   # 示意：确认生成的 client 覆盖关键 path
```

**5. 记录构建与生成产物信息**

- 记录：Node/pnpm 版本、构建命令、构建耗时（用于 CI 缓存策略）、生成 client 的目录与命令。
- 记录：`api/openapi/spec.yaml` 被成功消费，生成的 TS 类型覆盖预期 path。

### 验证结果 / Verified Results（已实测，2026-11-10）

> 环境：校机 `node v24.22.2` / `pnpm 10.33.4`；Vben tag `v5.7.0`（commit `63a38dce…70e9`），npmmirror 源。

**A. `pnpm install` + `build:ele` 成功（真实跑通）**：

```powershell
# 解压 v5.7.0 源码到 p0-04\vue-vben-admin-5.7.0；PATH 含 node24 与 pnpm
pnpm install            # → INSTALL_EXIT=0，2m20s，pnpm v10.33.4（含 postinstall stub/prepare）
pnpm run build:ele      # = turbo build --filter=@vben/web-ele → vite build --mode production
```

实测输出（节选）：`Tasks: 11 successful, 11 total`，`vite build ... built in 4.34s`，`BUILD_ELE_EXIT=0`，
产物 `apps/web-ele/dist/`（`css/ js/ jse/ favicon.ico index.html _app.config.js`，并生成 `dist.zip`）。→ **前端构建链路 PASS**。

**B. OpenAPI → TypeScript 生成（真实跑通）**：

```powershell
npx -y openapi-typescript@7 spec.yaml -o schema.d.ts
# → openapi-typescript 7.13.0，spec.yaml(1256 行) → schema.d.ts(2125 行)，72.1ms，OAPI_EXIT=0
```
用 **Phlogo 现网 `api/openapi/spec.yaml`** 作为输入，产物 `schema.d.ts` 生成成功，含 `/api/v1/sources`、`/api/v1/nodes`、
`sources/nodes/checks/sponsors` 等类型。

> 工具选择：本 spike 用 `openapi-typescript` 证明 **spec.yaml 可被消费成 TS 类型**；Vben 生态若需要「带 fetch 封装的 client」
> 可换 `@hey-api/openapi-ts`（P6 定稿生成器与封装）。两者都以 `spec.yaml` 为唯一契约源，语义一致。

**C. 契约缺口（实测发现，需 P1/P6 补齐）**：当前 `api/openapi/spec.yaml` **不含** 顶层 `/api/v1/checks` 与 `/api/v1/subscriptions` 路径
（`schema.d.ts` 中无这两个 literal path；`checks`/`subscriptions` 仅以零散字段出现）。与 P6 子代理早前标注的 API 契约缺口一致——
Rust 侧 utoipa 重写 `spec.yaml` 时须补齐这些 path，前端 client 才能覆盖 Checks/Subscriptions 页面。

**D. 结论**：Vben v5.7.0 `web-ele` 在固定版本下**可构建**（dist 产出）；`spec.yaml → TS` 契约生成链路**可用**；契约缺口已知并在 P1/P6 收口。

### 验收 / Acceptance

可机器校验：

```bash
cd vue-vben-admin   # 或 web/vben-spike
pnpm install --frozen-lockfile
pnpm build
echo "exit=$?"      # 期望 0
test -d dist && echo "dist exists"   # 期望打印 dist exists

# OpenAPI -> TS client 生成（固定版本真实命令）
# pnpm <openapi-gen-cmd> --input ../api/openapi/spec.yaml ...
# 断言：生成目录非空且包含来自 spec.yaml 的类型
```

- 最小 Vben app **`pnpm build` 成功**（退出码 0，产出 `dist/`）。
- **OpenAPI → TS client 生成命令跑通**，产物可被页面 import，且类型覆盖 `api/openapi/spec.yaml` 的关键 path（如 `/api/v1/sources`、`/api/v1/nodes`、`/api/v1/checks`）。
- 记录：固定 tag/commit、Node/pnpm 版本、构建与生成命令、产物位置。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo 现状（Go） | 重构目标（Rust/Vue） | 本页动作 |
|---|---|---|
| 无独立前端工程；API 由 `oapi-codegen` v2.6.0 从 `api/openapi/spec.yaml` 生成 `internal/api/generated/server.gen.go`（CONVENTIONS §2.1） | Vben Admin 5 `web-ele` + OpenAPI→TS client；Rust 侧 utoipa 生成 `spec.yaml`（方案 §3、§11） | 固定 Vben 版本，验证前端构建与「spec.yaml → TS client」链路 |
| `api/openapi/spec.yaml`（27 paths，CONVENTIONS §2.10） | 同一份 YAML 作为前后端唯一契约（方案 §11 末句「API 以 OpenAPI 为唯一契约」） | 本页消费该 YAML 生成 TS client，未来 P1 由 utoipa 重新产出 |
| 管理面散落在 Gin handler（`internal/api/**`、`internal/checker/handler.go`、`internal/source/**`） | Vben 页面：Sources/Nodes/Checks/Scoring/Subscriptions（方案 §12） | 本页只验证构建与契约生成，业务页面在 P6 |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **前端构建 job**：`pnpm install --frozen-lockfile` + `pnpm build`；用 pnpm cache 加速；固定 Node/pnpm 版本（读取 `docs/THIRD_PARTY.md` 的 `<PIN Vben web-ele tag>` 对应 engines）。
- **契约生成校验**：在 CI 中跑 OpenAPI→TS 生成，并把生成产物纳入 PR diff 检查（防止 spec.yaml 改动后前端 client 漂移未同步）。
- **契约一致性门禁**：未来 P1 用 utoipa 重新生成 `api/openapi/spec.yaml` 后，CI 应重新生成前端 client 并比对；spec 与 client 不一致即失败。
- **版本门禁**：禁止 floating Vben 版本；commit SHA 进入 `THIRD_PARTY.md` 与 SBOM。

### 风险 / Risks

- **`web-ele` 定位**：需确认 `web-ele` 在固定 tag 下的确切仓库/分支与构建方式（Vben 有多个模板分支）；记录真实路径避免 P6 走错基线。
- **OpenAPI 工具版本耦合**：生成命令与 `openapi-ts` / Vben gen 脚本版本强相关；固定版本后回填真实命令，避免凭空假设参数。
- **spec.yaml 漂移**：当前 `api/openapi/spec.yaml` 是 Phlogo 的 27 paths，但 Rust 侧会重写为 utoipa 产出（方案 §11）；前端 client 生成必须绑定到「最终由 Rust 产出的 spec」，否则前后端契约错位。
- **Node/pnpm 版本**：Vben 对 Node/pnpm 版本有要求；CI 镜像需与固定 tag 的 `engines` 一致，否则 `pnpm build` 在 CI 环境与本地不一致。
- **构建产物体积/耗时**：Vben 构建较重，CI 需合理缓存（`node_modules`、构建缓存）以避免超时。
