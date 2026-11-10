# 第三方依赖台账 · Third-Party Inventory

> 用途：记录所有外部依赖的**许可证 + 固定版本 + 二进制哈希**，作为供应链基线与 SBOM 来源（方案 §16、§20；CONVENTIONS §2.11）。
> 本文件由 P0 各步骤逐步填充，P0-05 统一收口。**进程隔离不是许可证豁免**——MiaoSpeed(AGPL)/Mihomo(GPL) 义务仍须履行。

## 上游仓库与许可证 · Upstreams & licenses

| 组件 | 仓库 | 许可证 | 用途 | 是否链接进 Core |
|---|---|---|---|---|
| paily-core | `openpaily/paily-core`（归档） | AGPL-3.0 | 重构对象（架构参考） | — |
| paily-fetch | `openpaily/paily-fetch` | AGPL-3.0 | 架构参考（format 插件） | 否（重写） |
| **MiaoSpeed** | `AirportR/miaospeed` | **AGPL-3.0** | 独立测活服务（WS） | **否**（进程边界，经 WebSocket） |
| Mihomo（MiaoSpeed 依赖） | `MetaCubeX/mihomo`（MiaoSpeed 用 `AirportR/mihomo` fork） | **GPL-3.0** | MiaoSpeed 内部代理实现 | 否 |
| VProxy | `mateo-ki/vproxy` | MIT | 仅参考，算法最小移植 | 否（仅移植算法，保留版权） |
| Apalis | `apalis/*` | MIT | Worker 任务队列（P0-03 验证） | 是（Rust 依赖） |
| Vben Admin | `vbenjs/vben-admin` | MIT | 管理前端（P0-04 验证） | 否（独立前端） |

## 固定版本与二进制哈希 · Pinned versions & binary hashes

| 组件 | 固定版本 | 来源 | 二进制 SHA-256 | 状态 |
|---|---|---|---|---|
| **MiaoSpeed** | **tag `4.7.9`（自建）** | 源码 tag `4.7.9`，Go `1.26.8`，`CGO_ENABLED=0` | `miaospeed-built.exe` = `2c302b294a2cdfff5cabd4009b2148e446eadff0ebc3413397a011874146ab18`（size 87,692,288） | ✅ 已验证（P0-02） |
| MiaoSpeed（GitHub 预编译，**不采用**） | `miaospeed-windows-amd64-4.7.9.zip` | GitHub release | zip `685a8a584aa7724479c46626b28cc8344ff5bfb1b18643254275461535e1d159`；exe `5b0ec1ad39723ae10c6a2cd6296561097335e680b5acc62a18ba0b7682ac9611`（commit `0fafd30`，公开仓库查无 → 不可复现） | ⚠️ 仅作参考，不作基线 |
| Mihomo（MiaoSpeed 内嵌） | `AirportR/mihomo v1.19.33-x365-miu.2`（go.mod `replace`） | 随 MiaoSpeed 构建 | 见 MiaoSpeed SBOM | 债务：GPL 义务 |
| Apalis | `<待 P0-03>` | — | — | ⏳ |
| Vben Admin | `<待 P0-04>` | — | — | ⏳ |
| VProxy | `mateo-ki/vproxy @ <commit 待 P0-01 锁>` | — | — | ⏳ |

> 备注：MiaoSpeed 采用**自建**（源码 tag `4.7.9`）作为供应链基线；GitHub 预编译资产的 commit 不在公开仓库，无法复现同哈希构建，故不作基线。
> MiaoSpeed 构建需补齐 `utils/embeded/BUILDTOKEN.key` 与 `preconfigs/embeded/tls/*`（详见 `01-p0-upstream-verification/p0-02-miaospeed.md` 步骤 A）。
