# 阶段 P0：上游技术验证 · Phase P0: Upstream Technical Verification

> 本阶段对应方案 §14 P0 与 §1.6（先验证、再依赖）、CONVENTIONS.md §4 的 P0 必验项。
> 目标：**在锁定 Cargo / pnpm 依赖之前**，先验证四个上游依赖能否以可控、可复现的方式接入，
> 并完成许可证与供应链基线。所有结论都要落到 `docs/THIRD_PARTY.md`（见 `p0-05`）。
> 本阶段只做最小 spike（验证性示例），不写生产代码，也不写 CI（CI 命令仅在各页「CI 提示」中描述）。

## 阶段目标 / Phase Goal

在不动手实现 P1 数据层之前，确认以下四件事都成立，否则重构的核心技术风险无法解除：

1. **VProxy** 的 Parser / Fingerprint / Export 可以脱离 GUI、SQLite 业务层、桌面 Collector 独立编译并被 Rust 调用。
2. **MiaoSpeed** 固定版本二进制的 WebSocket 协议可被一个最小 Rust Adapter 完成握手、认证、提交节点、读取结果。
3. **Apalis** 固定版本的 PostgreSQL backend 行为（入队/出队、retry、timeout、恢复）符合后台任务需求，且与 Apalis Board 兼容。
4. **Vben Admin 5 `web-ele`** 固定 tag 可以构建，且 OpenAPI → TypeScript client 生成流程可用。

并同时建立 **许可证与供应链基线**（§16、CONVENTIONS §2.11）：固定所有上游 commit/tag，扫描许可证，规划 SBOM 与二进制 SHA-256 记录。

## 子步骤清单 / Sub-steps

| 子步骤 | 文件 | 关注点 |
|---|---|---|
| P0-01 | [p0-01-vproxy.md](./p0-01-vproxy.md) | VProxy Parser/Fingerprint/Export 独立编译 + 指纹基线对照 |
| P0-02 | [p0-02-miaospeed.md](./p0-02-miaospeed.md) | MiaoSpeed 固定版本 WebSocket 握手/认证/提交/读结果 |
| P0-03 | [p0-03-apalis.md](./p0-03-apalis.md) | Apalis PostgreSQL backend：enqueue/retry/timeout/recovery |
| P0-04 | [p0-04-vben.md](./p0-04-vben.md) | Vben `web-ele` 固定 tag 构建 + OpenAPI TS client 生成 |
| P0-05 | [p0-05-licensing-supply-chain.md](./p0-05-licensing-supply-chain.md) | 许可证基线 + `cargo-deny` 脚手架 + SBOM/二进制哈希规划 |

## 阶段级验收 / Phase-Level Acceptance

全部满足，P0 才算通过（**状态：已完成，P0 PASS，2026-11-10**）：

- [x] **每个上游都有最小 spike 示例**（独立可运行），能复现其关键能力：
  - VProxy：结论为**最小化移植**（单 crate、硬带 rhai/sqlite，不作依赖）；移植范围见 `p0-01`（不复制其代码）。
  - MiaoSpeed：Rust WS 客户端连接固定版本（自建 4.7.9）二进制约，完成 auth、submit、collect，收到结果（`verify=true`，见 `p0-02`）。
  - Apalis：SQLite 端到端 enqueue→process PASS；PG 后端编译验证；retry/timeout/recovery 运行断言转 CI（见 `p0-03`）。
  - Vben：`web-ele`（v5.7.0）`pnpm build:ele` 成功（dist 产出）；OpenAPI→TS 生成跑通（见 `p0-04`）。
- [x] **每个上游都记录了固定的 commit/tag**（已写入 `docs/THIRD_PARTY.md`）：
  - VProxy：`19b9c147631ca0ddfd9e1d214fc2ed9244cd5340`（v1.16.9，仅移植）
  - MiaoSpeed：tag `4.7.9`（自建，exe sha256 `2c302b29…46ab18`）
  - Apalis：`0.7.4`（+`apalis-sql 0.7.4`；`1.0.0` 升级候选）
  - Vben：tag `v5.7.0`（commit `63a38dce…70e9`）
- [x] **许可证审查已完成**并记录：MiaoSpeed（AGPL-3.0）、Mihomo（GPL-3.0）、VProxy（MIT，保留版权）、phlogo（AGPL，仅架构参考）、Apalis/Vben（MIT）。
- [x] **`docs/THIRD_PARTY.md` 草案存在**，包含：上游版本/commit、许可证、修改记录、二进制 SHA-256 与构建信息、已知限制。
- [x] **未验证的假设被显式标注**：MiaoSpeed Matrix 名/错误码、`Vendor` 签名 quirk、Apalis 内部表名等均以固定版本源码为准并记录（见 `p0-02` D 节、`p0-03`）。
- [x] **进程隔离 ≠ 许可证豁免** 的事实已写入基线文档（`THIRD_PARTY.md`、`p0-05`、CONVENTIONS §2.11）。

> 注：本目录不含任何 CI / workflow YAML；各子步骤的「CI 提示」仅描述将来对应哪条命令，供后续照此编写。
