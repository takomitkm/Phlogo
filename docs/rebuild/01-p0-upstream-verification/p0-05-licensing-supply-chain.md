## 1-05 许可证与供应链基线 · License & Supply-Chain Baseline

> 对应方案 §16（安全、许可证与供应链）、CONVENTIONS §2.11（许可证与依赖）、§18（`THIRD_PARTY.md` 冻结规格）。
> 本页是 **P0 的合规基线**：在锁定 Cargo / pnpm 依赖之前，建立许可证审查、SBOM 与二进制哈希记录机制。
> **核心原则：进程隔离不是许可证豁免**（方案 §16 第 1 条、CONVENTIONS §2.11）。

### 目标 / Goal

- 建立 **`cargo-deny` 配置脚手架**（licenses / advisories / bans），作为 Rust 依赖的门禁基础。
- 规划 **SBOM** 产出方式（Rust crate + 第三方二进制）。
- 记录四大上游的**许可证与固定版本**：MiaoSpeed（AGPL）、Mihomo（GPL）、VProxy（MIT，保留版权）、phlogo（AGPL，仅架构参考）。
- 规划 **二进制 SHA-256 与构建信息** 的捕获方式（MiaoSpeed、Vben 构建产物、可选 Mihomo）。
- 产出 `docs/THIRD_PARTY.md` 草案，承接 P0-01~P0-04 记录的版本/commit/许可证/修改记录/二进制哈希。

### 范围 / Scope

- **新建/规划**：`deny.toml` 脚手架（放仓库根或 `ci/`）、SBOM 生成脚本规划、`docs/THIRD_PARTY.md` 草案。
- **审查对象**：Rust 依赖（P0-01 VProxy、P0-03 Apalis 的传递依赖）、Go 二进制（P0-02 MiaoSpeed + 其 Mihomo 依赖）、前端（P0-04 Vben + 其 npm 依赖）。
- **不做**：不修改任何 Phlogo 源码（本任务全程 docs only）；不落地 CI YAML（仅描述未来命令）。
- **明确排除**：本页不把「进程隔离」当作许可证义务的解除理由（方案 §16、CONVENTIONS §2.11）。

### 前置 / Prerequisites

- Rust 工具链 + `cargo-deny`（`cargo install cargo-deny`）；可选 `cargo-license`、`cargo-audit`。
- 已读方案 §16（10 条）与 CONVENTIONS §2.11；已读 P0-01~P0-04 的本阶段产出（版本/commit/依赖扫描）。
- 已记录的上游仓库（方案 §20）：VProxy（MIT）、MiaoSpeed（AirportR，AGPL）、Mihomo（MetaCubeX，GPL）、Apalis（MIT）、Vben（Vbenjs）、paily-core/paily-fetch（AGPL，仅架构参考）。
- 可选：ScanCode（方案 §16 第 7 条，用于源码与第三方文件扫描）。

### 步骤 / Steps

**1. 放置 `cargo-deny` 配置脚手架**

```toml
# deny.toml  （仓库根或 ci/ 下；未来 CI 用 cargo deny check 读取）
# 许可证策略：基于方案 §16 / CONVENTIONS §2.11 的已知许可义务。
[advisories]
version = 2
yanked = "warn"

[licenses]
version = 2
# 允许列表：以 P0-01~P0-04 实际扫描结果为准，先用宽松占位，
# 逐步收紧。AGPL/GPL 在「二进制分发」场景下需单独评估（见风险）。
allow = [
    "MIT",
    "Apache-2.0",
    "Apache-2.0 WITH LLVM-exception",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "ISC",
    "MPL-2.0",
    "Unicode-DFS-2016",
    "Unicode-3.0",
    "Zlib",
    "CC0-1.0",
    "OpenSSL",            # 需谨慎：与 GPL 家族同分发时的兼容性
]
# 遇到 copyleft（GPL/AGPL）要先评估「是否进入分发产物」，再决定是否加白。
confidence-threshold = 0.8

[bans]
multiple-versions = "warn"
wildcards = "warn"

[sources]
unknown-registry = "warn"
unknown-git = "warn"
allow-registry = ["https://github.com/rust-lang/crates.io-index"]
```

**2. 运行许可证与漏洞扫描**

```bash
cargo deny check licenses   # 对照 deny.toml 的 allow 列表
cargo deny check advisories # 安全公告
cargo audit                 # 备选/补充漏洞审计
cargo license > /tmp/rust-licenses.txt   # 全量许可证清单，归档
```

**3. 规划 SBOM 产出**

```bash
# Rust 侧 SBOM（CycloneDX / SPDX）
cargo cyclonedx            # 若装了 cargo-cyclonedx，生成 bom.xml
# 或：cargo sbom（视可用工具）
# 第三方二进制（MiaoSpeed、Vben dist、可选 Mihomo）单独登记到 THIRD_PARTY.md + SBOM 附件。
echo "SBOM plan: rust crate bom + binary inventory"
```

**4. 规划二进制 SHA-256 与构建信息捕获**

```bash
# 每个固定版本二进制构建后，记录哈希与来源：
sha256sum miaospeed        > build-artifacts/miaospeed-<PIN>.sha256
echo "MiaoSpeed <PIN MiaoSpeed version> built from <commit>" >> build-artifacts/README.md
# Vben dist、可选 Mihomo 同理。
# CI 中应由可复现构建步骤产出并签名/登记。
```

**5. 编写 `docs/THIRD_PARTY.md` 草案**

至少包含以下章节（承接 P0-01~P0-04）：

```markdown
# THIRD_PARTY.md

## 上游版本与 commit
| 上游 | 版本/tag | commit | 用途 | 许可证 |
|---|---|---|---|---|
| VProxy | <PIN VProxy commit/tag> | <sha> | 解析/指纹/导出 (P0-01) | MIT（保留版权） |
| MiaoSpeed | <PIN MiaoSpeed version> | <sha> | 独立测活服务 (P0-02) | AGPL |
| Mihomo（MiaoSpeed 依赖） | <version> | <sha> | MiaoSpeed 内部代理实现 | GPL |
| Apalis | <PIN Apalis version> | <sha> | 后台任务队列 (P0-03) | MIT |
| Vben Admin 5 web-ele | <PIN Vben web-ele tag> | <sha> | 管理前端 (P0-04) | MIT/Apache（以仓库 LICENSE 为准） |
| paily-core / paily-fetch | fork: takomitkm/Phlogo | <sha> | 架构参考，不移植代码 | AGPL（架构参考 only） |

## 修改记录
- VProxy：若仅移植算法，记录移植范围与保留的 MIT 版权文件。
- MiaoSpeed：以独立二进制方式集成，不链接其 Go 代码。

## 二进制哈希与构建信息
| 二进制 | SHA-256 | 构建来源 |
|---|---|---|
| miaospeed | <sha256> | <PIN MiaoSpeed version> @ <commit> |

## 已知限制
- MiaoSpeed Matrix 名、错误码以固定版本协议为准（见 MIAOSPEED_PROTOCOL.md）。
- VProxy 内部 API 以固定版本为准（见 P0-01）。
```

### 固定基线（已落地）/ Pinned Baseline (finalized, 2026-11-10)

> `docs/THIRD_PARTY.md` 已按下列真实值填写（本页只汇总；以 THIRD_PARTY.md 为准）。

| 组件 | 固定版本 | commit/tag | 许可证 | 集成方式 | 基线产物 |
|---|---|---|---|---|---|
| VProxy | v1.16.9 | `19b9c147631ca0ddfd9e1d214fc2ed9244cd5340` | MIT（保留版权） | **仅移植算法**（非依赖，P0-01） | 保留 MIT 版权声明 |
| MiaoSpeed | tag `4.7.9`（**自建**） | tag `4.7.9`（GitHub 预编译 commit 不可复现） | AGPL-3.0 | 独立二进制，**WebSocket 进程边界** | 自建 exe SHA-256 `2c302b29…46ab18`（87,692,288 B） |
| Mihomo | `AirportR/mihomo v1.19.33-x365-miu.2` | 随 MiaoSpeed `go.mod replace` | GPL-3.0 | MiaoSpeed 内部依赖（不链接进 Gnosis） | 见 MiaoSpeed SBOM |
| Apalis | `0.7.4`（+`apalis-sql 0.7.4`；`1.0.0` 为升级候选） | crates.io | MIT | Rust 依赖 | 编译+SQLite 队列 PASS |
| Vben Admin web-ele | tag `v5.7.0` | `63a38dce49ba109f61607994e21ba921d8e970e9` | MIT | 独立前端 | `dist.zip`，OpenAPI→TS 生成 PASS |
| paily-core / paily-fetch | 参考 `openpaily/*`（归档） | Phlogo fork `takomitkm/Phlogo` | AGPL-3.0 | **仅架构参考，不复制代码** | — |

**`deny.toml` 脚手架**：已在仓库根 `deny.toml` 落地（licenses/advisories/bans/sources），
**待 P1 建起 `gnosis-rs/Cargo.toml`（Rust workspace）后正式生效**（当前无 Rust manifest，`cargo deny` 无对象可查）。

**SBOM 计划**：Rust 侧 `cargo cyclonedx`（CycloneDX）产出 `bom.xml`；第三方二进制（MiaoSpeed 自建 exe、Vben `dist.zip`、Mihomo 随 MiaoSpeed）登记到 `THIRD_PARTY.md` + SBOM 附件。CI 构建后 `sha256sum` 并与 `THIRD_PARTY.md` 登记值比对（防篡改）。

**再次确认**：**进程隔离不是许可证豁免**——MiaoSpeed（AGPL）、Mihomo（GPL）即使独立进程运行，分发时仍须提供对应源码/保留版权文本（方案 §16、CONVENTIONS §2.11）。

### 验收 / Acceptance

可机器校验：

```bash
cargo deny check licenses
echo "exit=$?"   # 期望 0（或仅出现 deny.toml 中已审批项）

cargo deny check advisories
echo "exit=$?"   # 期望 0（无未处理高危公告）

test -f deny.toml && echo "deny.toml exists"      # 期望打印
test -f docs/THIRD_PARTY.md && echo "THIRD_PARTY exists"  # 期望打印
```

- **`cargo-deny` 配置脚手架存在**（`deny.toml`），且 `cargo deny check licenses` / `advisories` 通过（仅允许已审批项）。
- **SBOM 计划存在**：明确 Rust crate SBOM + 第三方二进制清单的产出方式与归档位置。
- **许可证与固定版本已记录**到 `docs/THIRD_PARTY.md` 草案：MiaoSpeed（AGPL）、Mihomo（GPL）、VProxy（MIT，保留版权）、phlogo（AGPL，架构参考 only）。
- **二进制 SHA-256 与构建信息捕获计划**已记录（至少 MiaoSpeed；Vben dist、可选 Mihomo 同样规划）。
- 文档中明确写出：**进程隔离 ≠ 许可证豁免**（方案 §16、CONVENTIONS §2.11）。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo 现状（Go） | 重构目标（Rust/多进程） | 本页动作 |
|---|---|---|
| AGPL-3.0 继承 paily-core（CONVENTIONS §2.11） | 同上游义务延续；需单独核 MiaoSpeed/Mihomo/VProxy | 建立许可证基线，记录各上游许可证 |
| `go.mod` 间接依赖含 `metacubex/mihomo`、`enfein/mieru` 等大量代理协议库（CONVENTIONS §2.11） | Rust 侧**不链接**这些；测活交给独立 MiaoSpeed 二进制（进程边界） | 强调进程隔离不是许可证豁免；Mihomo GPL 义务仍需履行 |
| 无 SBOM / 二进制哈希记录 | 固定上游 commit/tag；保存二进制 SHA-256、构建信息、SBOM（方案 §16 第 6 条） | 规划 SBOM + 哈希捕获机制 |
| 无 `cargo-deny`（Go 项目） | `cargo deny check licenses` + `cargo audit`（CONVENTIONS §2.11、§5） | 提供 `deny.toml` 脚手架 |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **许可证门禁**：`cargo deny check licenses` 与 `cargo deny check advisories` 作为必过 job；`cargo audit` 补充。
- **依赖更新审查**：PR 修改 `Cargo.lock` / 新增依赖时触发 deny 检查；copyleft（GPL/AGPL）进入分发产物需人工评审。
- **SBOM 产出**：CI 中生成 Rust crate SBOM（CycloneDX/SPDX）并归档为 artifact；第三方二进制哈希进入同一 SBOM/清单。
- **版本门禁**：CI 读取 `docs/THIRD_PARTY.md` 记录的固定 commit/tag，禁止 floating 版本；二进制构建后 `sha256sum` 并比对登记值（防供应链篡改）。
- **ScanCode（可选）**：对源码与第三方文件跑 ScanCode（方案 §16 第 7 条），补充许可证发现。

### 风险 / Risks

- **进程隔离 ≠ 许可证豁免（最高优先级）**：MiaoSpeed（AGPL）、Mihomo（GPL）即使以独立二进制运行，其许可证义务（如提供对应源码、保留版权）仍需履行；不能因为「没链接」就忽略（方案 §16 第 1、2 条，CONVENTIONS §2.11）。
- **copyleft 进入分发产物**：若最终分发包含 AGPL/GPL 组件，需评估（提供源码、许可证兼容）。`deny.toml` 的 allow 列表对 copyleft 要谨慎加白并留评审记录。
- **VProxy MIT 版权保留**：即使只移植算法，也必须保留 MIT 版权与许可证声明（方案 §3.1、§16 第 3 条）。
- **phlogo 代码误移植**：paily-core/paily-fetch 是 AGPL 且「仅架构参考」；不得直接复制其代码（方案 §16 第 4 条、CONVENTIONS §4 差异项 1/2）。
- **版本漂移**：上游 commit/tag 未固定时，SBOM 与哈希比对会失效；必须等 P0-01~P0-04 锁定后才算基线完成。
- **npm 依赖许可证**：Vben 的 npm 依赖同样需扫描（前端 SBOM），不能只查 Rust 侧。
