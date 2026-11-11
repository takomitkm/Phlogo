## 6-01 Vben 脚手架与 OpenAPI 客户端生成 · Vben Scaffold & OpenAPI TypeScript Client

### 目标 / Goal

把 Vben Admin 5（Vue 3 + TypeScript + Element Plus，`web-ele` 变体）落到仓库的 `web/vben-admin/`，并把它与
Rust `core-api` 通过**由 OpenAPI 契约生成的 TypeScript 客户端**接起来。做到：

- 在**固定 tag/commit** 上搭建可构建的最小 Vben 应用（版本冻结由 P0-04 决定）。
- 从 `api/openapi/spec.yaml` 生成强类型 TS client，所有页面复用，不手写 fetch 拼装。
- 本地 dev 通过 Vite proxy 把 `/api` 反代到 `core-api`（默认 `http://localhost:8080`）。
- 确立「前端只调 API，绝不直连 PostgreSQL / MiaoSpeed」的硬约束。

### 范围 / Scope

**新建**：
- `web/vben-admin/`（Vben `web-ele` checkout，固定 tag/commit）。
- `web/vben-admin/openapi-ts.config.ts`（OpenAPI→TS 生成配置）。
- `web/vben-admin/src/api/generated/`（生成产物）。
- `web/vben-admin/src/api/request.ts`（base client：base URL + JWT bearer 注入 + 401 刷新）。
- `web/vben-admin/vite.config.ts`（dev proxy 段）。

**不做（本步骤）**：
- 具体业务页面（Dashboard / Sources / …）→ 见 [p6-02-pages.md](p6-02-pages.md)。
- 任何 CI / workflow YAML（只在末尾「CI 提示」描述，不落地）。
- 修改 Phlogo Go 源码或 Rust 源码（本任务为 docs-only）。

### 前置 / Prerequisites

1. 已完成 `01-p0-upstream-verification/p0-04-vben.md`：Vben `web-ele` 的**固定 tag/commit** 与「能构建的最小示例」
   已验证。**本步骤所有 `<VBN_TAG_OR_COMMIT>` 占位符都来自该结论，不得自行编造版本。**
2. 本地工具链（取固定 checkout 中 `package.json#engines` 的精确值，由 P0-04 冻结）：
   - Node.js LTS（Vben 5 要求 `node >= 18.12` 起，以冻结值准）。
   - pnpm（`pnpm >= 8`，以冻结值准）。
3. Rust `core-api` 至少可达（P1 之后；本步骤冒烟只需一个返回 OpenAPI 文档或 401 的端点，不要求全量业务就绪）。
4. 仓库布局遵循方案 §13：`web/vben-admin/` 与 `api/openapi/spec.yaml` 同仓，生成命令用相对路径 `../../api/openapi/spec.yaml`。

### 步骤 / Steps

#### Step 1 — 在固定 tag/commit 上落 Vben `web-ele`

```powershell
# 在仓库根 S:\apps\llm\agentwork\repos\Phlogo
New-Item -ItemType Directory -Force -Path web | Out-Null
Set-Location web
# <VBN_TAG_OR_COMMIT> 来自 p0-04-vben.md（请勿替换成未经验证的版本）
git clone --depth 1 --branch <VBN_TAG_OR_COMMIT> https://github.com/vbenjs/vue-vben-admin.git vben-admin
Set-Location vben-admin
git fetch --depth 1 origin <VBN_TAG_OR_COMMIT>
git checkout <VBN_TAG_OR_COMMIT>
```

> 若 `web-ele` 是分支/标签名，用 `--branch`；若是 commit SHA，克隆后 `git checkout <sha>`。
> 克隆后**锁定**：把实际使用的 tag/commit 写回 `p0-04-vben.md` 与 `THIRD_PARTY.md`（方案 §18）。

#### Step 2 — 安装依赖（冻结 lockfile）

```powershell
Set-Location web/vben-admin
pnpm install --frozen-lockfile
```

若固定 checkout 尚未提供 lockfile，先 `pnpm install` 生成并提交 `pnpm-lock.yaml`，再据此冻结。

#### Step 3 — 配置 OpenAPI→TypeScript 客户端生成

采用 [`@hey-api/openapi-ts`](https://heyapi.dev/) 作为主方案：它既能生成类型，也能生成强类型 fetch client，
且以 OpenAPI 文档为唯一输入。备选方案为 `openapi-typescript` + `openapi-fetch`（仅生成 `.d.ts` 类型）。

在 `web/vben-admin/openapi-ts.config.ts`：

```ts
import { defineConfig } from '@hey-api/openapi-ts';

export default defineConfig({
  // 契约单一来源：与 Rust 侧 utoipa 产物同源（CONVENTIONS §1 / §2.10）
  input: '../../api/openapi/spec.yaml',
  output: 'src/api/generated',
  plugins: [
    '@hey-api/client-fetch', // 生成 typed fetch client（create<Op> / <op> 函数）
  ],
});
```

在 `package.json` 增加脚本（不写死版本，交给 P0 冻结的 `@hey-api/openapi-ts`）：

```jsonc
{
  "scripts": {
    "openapi:gen": "openapi-ts",
    "openapi:gen:check": "openapi-ts --dry-run" // --dry-run 仅比对不写盘（用于 CI 漂移检测）
  }
}
```

生成命令与产物：

```powershell
pnpm openapi:gen
# 预期产物：
#   src/api/generated/types.gen.ts   —— 所有 schema（LoginRequest/SourceSummary/NodeDetail/...）
#   src/api/generated/sdk.gen.ts     —— 按 operationId 命名的 typed 函数（AuthLogin/SourceList/NodeGet/...）
#   src/api/generated/index.ts       —— 再导出
```

> 备选（`openapi-typescript`）：`npx openapi-typescript ../../api/openapi/spec.yaml -o src/api/generated/schema.d.ts`，
> 再用 `openapi-fetch` 的 `createClient<paths>()` 调 `client.GET('/api/v1/nodes', ...)`。**无论哪种，入参必须是
> 同一份 `api/openapi/spec.yaml`。**

#### Step 4 — 包装 base client（base URL + JWT bearer + 401 刷新）

`web/vben-admin/src/api/request.ts`（示意，基于 hey-api 生成的 client）：

```ts
import { createClient, type Client } from './generated/client.gen';

const BASE_URL = import.meta.env.VITE_API_BASE_URL ?? '/api'; // 生产由 Caddy/Nginx 反代 /api
let accessToken: string | null = null;

export function setAuthToken(t: string | null) { accessToken = t; }

export const api: Client = createClient({ baseUrl: BASE_URL });

api.interceptors.request.use((ctx) => {
  if (accessToken) ctx.request.headers.set('Authorization', `Bearer ${accessToken}`);
  return ctx;
});

// 401 → 用 refresh_token 换 access（refresh_token 存 localStorage），失败则跳登录
api.interceptors.response.use(async (ctx) => {
  if (ctx.response.status === 401 && !ctx.request.url.endsWith('/api/v1/auth/refresh')) {
    const next = await tryRefresh();
    if (next) return api.raw(ctx.request.clone());
  }
  return ctx;
});
```

> 认证模型来自 `spec.yaml` 的 securitySchemes：`AdminJWT`（UI 用）与 `ServiceBearer`（fetcher/checker 用）。
> 前端只走 `AdminJWT`：`POST /api/v1/auth/login` 拿 `token` + `refresh_token`，`POST /api/v1/auth/refresh` 续期。

#### Step 5 — 配置 Vite dev proxy 到 `core-api`

在 `web/vben-admin/vite.config.ts` 增加（默认 `core-api` 端口见 `docs/configuration-and-api.md` 的
`server.port` → `PAILY_SERVER_PORT`，内置默认 `8080`）：

```ts
server: {
  proxy: {
    '/api': {
      target: process.env.CORE_API_URL ?? 'http://localhost:8080',
      changeOrigin: true,
      // 不重写路径：/api/v1/... 原样转发到 core-api
    },
  },
},
```

> 说明：`core-api` 的 `server.cors_origins` 默认忽略 `*`（`docs/configuration-and-api.md`），所以本地**必须**靠
> 反代而非开 CORS；生产同理靠 Caddy/Nginx 反代，不要把 `localhost:8080` 写死进生产构建。

#### Step 6 — 最小冒烟构建

保留 Vben 默认布局，只确认能生成 client 且能 `build`：

```powershell
pnpm openapi:gen      # 生成 client
pnpm build            # 产物到 dist/；预期退出码 0
```

### 验证结果 / Verified Results（已实测，2026-11-11）

**落地策略（与原计划的偏差，已定稿）**：Vben 5.7.0 是 turbo monorepo（数千文件），**不整仓 vendor 进本仓库**。
`web/vben-admin/` 存放**版本 pin 说明 + 自研层文件 + 生成产物约定**；实际构建使用校机固定的
`C:\Users\YiZhidi\p0-04\vue-vben-admin-5.7.0`（tag `v5.7.0`，commit `63a38dce49ba109f61607994e21ba921d8e970e9`，
与 P0-04 一致），CI 按同 tag 拉 zip 构建（P7 落 CI 种子时执行）。

**本步实测完成的三件事**：

1. **Rust 侧契约补全**（`crates/api-contract` 重写）：utoipa 5 此前只输出 247 字节空文档（paths 未注册）——
   现在 `#[utoipa::path]` 声明 **7 条 path**（sources 全 CRUD + ignore-dead + nodes、nodes 3 条、health）
   与 5 个 schema；`utoipa` 加 `uuid` feature（`Uuid: ToSchema` 需要）。`GET /api-docs/openapi.json` 实测 **200**，
   导出 **4580 字节**完整 OpenAPI 3.1 文档，落 **`api/openapi/spec.json`**（canonical，README 说明再生成方式）。
2. **前端自研层**：`web/vben-admin/openapi-ts.config.ts`（输入 `../../api/openapi/spec.json`、输出 `src/api/generated`）、
   `web/vben-admin/src/api/request.ts`（base URL=`VITE_API_BASE_URL ?? '/api'`、Bearer 注入、401 统一处理、
   `openapi-fetch` typed client 封装 `apiFetch`）。
3. **生成 + 构建链路验证（校机，node 24.22.2 / pnpm 10.33.2）**：
   - `npx openapi-typescript@7 spec.json -o apps/web-ele/src/api/generated/schema.d.ts` → **GEN_EXIT=0**（39.3ms，12045 字节，含全部 `/api/v1/*` paths）。
   - `pnpm run build:ele` → **BUILD_ELE_EXIT=0**（11/11 tasks，1m23.9s，dist.zip 1087549 bytes）——生成的 schema 在 app 内不破坏构建。

**契约现状**：spec.json 覆盖 P1 已落端点（sources/nodes/health）。P3-P5 的 check/subscription 路由尚未
经 Axum 装配暴露（分发管线目前是纯函数层），其 `#[utoipa::path]` 与路由随 P6-02/P7 装配时补进契约——
这与 P0-04 发现的「spec 缺 checks/subscriptions path」缺口同因，收口点在路由装配而非文档。

**遗留（诚实标注）**：`request.ts` 引用的 `openapi-fetch` 依赖需在 vben checkout 的 `apps/web-ele/package.json`
补一条（P6-02 做页面时随 pnpm install 落地）；`pnpm openapi:gen` 脚本同批加入。

### 验收 / Acceptance

| # | 命令 | 预期 | 说明 |
|---|---|---|---|
| A1 | `pnpm build` | 退出码 `0` | 最小 Vben 应用绿 |
| A2 | `Test-Path src/api/generated/types.gen.ts` 且 `Test-Path src/api/generated/sdk.gen.ts` | `True` | client 已生成且强类型 |
| A3 | `grep -rE "AuthLogin\|SourceList\|NodeGet\|StatsGet" src/api/generated/` | 命中 operationId | 生成覆盖 27 路径的 schema/client |
| A4 | `grep -n "/api" vite.config.ts` | 命中 `/api` proxy（target 指向 `core-api`） | 代理配置存在 |
| A5 | `grep -rEi "postgres\|postgresql\|sqlx\|miaospeed\|DSN\|database/sql" src/` | 仅 `NO_DIRECT_DB_REF` | 前端不直连 DB/测活 |
| A6 | `pnpm typecheck`（即 `vue-tsc --noEmit`） | 退出码 `0` | 生成类型被引用无类型错误 |

> 端到端冒烟（需 P1+ 的 `core-api` 真正就绪）：启动 `core-api` 后
> `pnpm dev`，浏览器登录 → `POST /api/v1/auth/login` 返回 `token`；
> 用 `curl` 自测：`curl -X POST localhost:8080/api/v1/auth/login -H 'content-type: application/json' -d '{"password":"<ADMIN_PASSWORD>"}'`
> 预期 `200` + `LoginResponse{token,refresh_token}`。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

- 当前 `Phlogo` 仓库**没有前端**（`cmd/server/main.go` 是 Go 单体，无 `web/`、无 `.vue`）。本步骤是整个管理前端的起点。
- 取代对象：上游 paily-core 的既有管理端（能力见 `docs/configuration-and-api.md` 与 `docs/*.md`），但实现改为
  Vben + 生成 client，契约统一为 `api/openapi/spec.yaml`。
- 替换原 `oapi-codegen` 链路：Go 侧用 `oapi-codegen` 生成 server；Rust 侧改用 utoipa 产出 OpenAPI，再由本步骤
  生成 TS client（CONVENTIONS §2.1、§4 差异 5）。

### CI 提示 / CI notes

> 仅描述将来 CI 对应哪条命令，**不落地 YAML**。

- **build job**：`pnpm install --frozen-lockfile && pnpm build`（等价 A1）。
- **typecheck job**：`pnpm typecheck`（等价 A6）。
- **openapi 漂移检测 job**：`pnpm openapi:gen:check`（或 `openapi-ts --dry-run`）——若生成产物与已提交的不一致则失败，
  防止 Rust 侧 utoipa / `spec.yaml` 改动后前端未同步。
- **no-direct-backend-ref 检查**：等价 A5 的 `grep`，作为 lint 关卡，守死「前端只调 API」红线。
- 以上命令需 Node + pnpm 矩阵（版本取自 P0-04 冻结的 `engines`），且在 `web/vben-admin` 目录下执行。

### 风险 / Risks

- **`<VBN_TAG_OR_COMMIT>` 未定**：`p0-04-vben.md` 当前尚未产出，无法给出真实固定版本；若提前开工会违反方案 §4
  「P0 未通过不锁定依赖」。务必先完成 P0-04。
- **生成工具版本漂移**：`@hey-api/openapi-ts` / `openapi-typescript` 的大版本会改变输出文件名与 API，需在 P0-04
  一并冻结，并把生成产物或 `--dry-run` 纳入 CI。
- **契约与运行时不一致**：若 Rust utoipa 实际产出与手维护的 `spec.yaml` 有差异，生成的 client 与运行时不符——
  漂移检测（CI 提示）是关键防线。
- **dev/prod base URL 混淆**：`VITE_API_BASE_URL` 默认 `/api` 依赖反代；若误把 `localhost:8080` 编译进生产包，
  部署后即无法访问 API。
