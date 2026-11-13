# 第三方依赖台账 · Third-Party Inventory

> 用途：记录所有外部依赖的**许可证 + 固定版本 + 二进制哈希**，作为供应链基线与 SBOM 来源（方案 §16、§20；CONVENTIONS §2.11）。
> 本文件由 P0 各步骤逐步填充，P0-05 统一收口。**进程隔离不是许可证豁免**——MiaoSpeed(AGPL)/Mihomo(GPL) 义务仍须履行。

## 上游仓库与许可证 · Upstreams & licenses

| 组件 | 仓库 | 许可证 | 用途 | 是否链接进 Gnosis |
|---|---|---|---|---|
| paily-core | `openpaily/paily-core`（归档） | AGPL-3.0 | 重构对象（架构参考） | — |
| paily-fetch | `openpaily/paily-fetch` | AGPL-3.0 | 架构参考（format 插件） | 否（重写） |
| **MiaoSpeed** | `AirportR/miaospeed` | **AGPL-3.0** | 独立测活服务（WS） | **否**（进程边界，经 WebSocket） |
| Mihomo（MiaoSpeed 依赖） | `MetaCubeX/mihomo`（MiaoSpeed 用 `AirportR/mihomo` fork） | **GPL-3.0** | MiaoSpeed 内部代理实现 | 否 |
| VProxy | `mateo-ki/vproxy` | MIT | 仅参考，算法最小移植 | 否（仅移植算法，保留版权） |
| Apalis | `apalis/*` | MIT | Worker 任务队列（P0-03 验证） | 是（Rust 依赖） |
| Vben Admin | `vbenjs/vben-admin` | MIT | 管理前端（P0-04 验证） | 否（独立前端） |
| **sub-store-convert** | [`tbxark/sub-store-convert`](https://github.com/tbxark/sub-store-convert) | 仓库自称 MIT，**但 vendor 并打包 Sub-Store（AGPL-3.0）源码** → 按 **AGPL-3.0** 分发 | 扩展目标格式（Surge/QX/Loon/…）的转换旁挂服务，见 `docs/rebuild/14-substore-sidecar.md` | **否**（独立 Node 进程，HTTP 进程边界） |
| Sub-Store（被上者 vendor） | [`sub-store-org/Sub-Store`](https://github.com/sub-store-org/Sub-Store) | **AGPL-3.0** | parser/producer 逻辑被 bundle 打包 | 否 |

## 固定版本与二进制哈希 · Pinned versions & binary hashes

| 组件 | 固定版本 | 来源 | 二进制 SHA-256 | 状态 |
|---|---|---|---|---|
| **MiaoSpeed** | **tag `4.7.9`（自建）** | 源码 tag `4.7.9`，Go `1.26.8`，`CGO_ENABLED=0` | `miaospeed-built.exe` = `2c302b294a2cdfff5cabd4009b2148e446eadff0ebc3413397a011874146ab18`（size 87,692,288） | ✅ 已验证（P0-02） |
| MiaoSpeed（GitHub 预编译，**不采用**） | `miaospeed-windows-amd64-4.7.9.zip` | GitHub release | zip `685a8a584aa7724479c46626b28cc8344ff5bfb1b18643254275461535e1d159`；exe `5b0ec1ad39723ae10c6a2cd6296561097335e680b5acc62a18ba0b7682ac9611`（commit `0fafd30`，公开仓库查无 → 不可复现） | ⚠️ 仅作参考，不作基线 |
| Mihomo（MiaoSpeed 内嵌） | `AirportR/mihomo v1.19.33-x365-miu.2`（go.mod `replace`） | 随 MiaoSpeed 构建 | 见 MiaoSpeed SBOM | 债务：GPL 义务 |
| Apalis | `apalis 0.7.4` + `apalis-sql 0.7.4`（features `sqlite,postgres`；最新稳定 `1.0.0` 列为升级候选） | crates.io | N/A（Rust 库，无独立二进制） | ✅ 编译通过 + SQLite 运行时队列 PASS（P0-03） |
| Vben Admin (web-ele) | tag `v5.7.0`（commit `63a38dce49ba109f61607994e21ba921d8e970e9`） | GitHub tag | N/A（源码构建，产物 `dist.zip`） | ✅ 构建通过 + OpenAPI→TS 生成通过（P0-04） |
| VProxy | `mateo-ki/vproxy @ 19b9c147631ca0ddfd9e1d214fc2ed9244cd5340`（v1.16.9） | GitHub main | N/A（仅移植算法，不链接/分发其二进制） | ✅ 决策：最小化移植（非依赖，P0-01） |
| **sub-store-convert core bundle** | **commit `cddcab9c536da53c0f867cd6b5738791bc4e8121`**（master HEAD，**非 floating**） | `packages/core/build/index.js`，由 `deploy/substore-sidecar/fetch-bundle.mjs` 按 pin 从 CDN 取回并校验 | `index.mjs` = `ee99f9a7fe2bb3a82321a5b94983ae3e223bfe02801fa55ef7c29478152027ef`（size **503,527**） | ✅ 固定 URL 复核 200 + HASH MATCH；校机真实 52 节点 E2E 11/11 格式通过（S57） |
| sub-store-convert 运行依赖 | `js-base64 3.9.6` / `json5 2.2.3` / `lodash 4.18.1` / `peggy 5.1.1` / `yaml 2.9.1`（全部定版，见 `deploy/substore-sidecar/package.json`） | npm registry（校机用 pnpm 10.33.4 安装） | N/A（源码包） | ✅ `pnpm install` + `IMPORT OK` 实测 |

> 备注：MiaoSpeed 采用**自建**（源码 tag `4.7.9`）作为供应链基线；GitHub 预编译资产的 commit 不在公开仓库，无法复现同哈希构建，故不作基线。
> MiaoSpeed 构建需补齐 `utils/embeded/BUILDTOKEN.key` 与 `preconfigs/embeded/tls/*`（详见 `01-p0-upstream-verification/p0-02-miaospeed.md` 步骤 A）。
