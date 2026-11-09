## 1-01 VProxy 解析/指纹/导出 独立编译验证 · Verify VProxy Parser/Fingerprint/Export Compiles Standalone

> 对应方案 §3.1（VProxy 使用边界）、§7（节点指纹与标准化模型）、CONVENTIONS §2.4 / §4 差异项 1。
> 本页是 **P0 spike**，目标是「能否轻依赖独立使用 VProxy」，不是写生产 adapter。

### 目标 / Goal

- 确认 VProxy 的 **Parser / Fingerprint / Export** 模块能脱离 GUI、SQLite 数据层、桌面 Collector 单独编译。
- 用一份示例 Clash YAML 跑通 `解析 → 节点模型 → 计算指纹` 的最小链路，作为后续 `vproxy-adapter` crate 的基线。
- 记录 Cargo 传递依赖与许可证，确认其依赖面可控。
- 明确：**禁止** 让 VProxy 的 `Node` 类型直接成为 Core 的 DB 类型；必须转换为自有 `NormalizedNode`（方案 §1.6 第 4 条、§3.1、§7.2）。

### 范围 / Scope

- **新建**：一个 scratch crate（如 `spikes/vproxy-standalone`），仅用于验证，不进入 workspace 的生产 crate。
- **验证内容**：`parser`、`fingerprint`、`export` 三个能力；`cargo tree` 依赖面；许可证扫描。
- **不做**：GUI、SQLite Database、完整 Collector 数据层、任何业务层代码；不实现完整 `vproxy-adapter`（那是 P2 的事，见 `02-p1-*` / `03-p2-*`）。
- **不做**：不链接、不移植 Mihomo/Sing-box（那属于 MiaoSpeed / 导出层，且 Rust 侧不链接代理协议栈）。

### 前置 / Prerequisites

- Rust 工具链（`cargo --version`，建议 stable 最新；具体 MSRV 待固定 VProxy 后确认）。
- `git` 可访问 VProxy 仓库：<https://github.com/mateo-ki/vproxy>（方案 §20）。
- 网络可达以做 `cargo add` / git 依赖拉取（CI 需配置 vendoring 或 cache）。
- 已读 `internal/node/upsert.go`（`nodeHash` 基线）与 `internal/naming/naming.go`（命名基线）。
- `cargo-deny`（或 `cargo license`）用于许可证扫描；本页只产出扫描输入，`cargo-deny` 脚手架见 `p0-05`。

### 步骤 / Steps

**1. 建立 scratch crate（与未来 workspace 隔离）**

```bash
# 不在主 workspace 内，避免污染生产 Cargo.lock
mkdir -p spikes/vproxy-standalone/src
cd spikes/vproxy-standalone
cargo init --name vproxy-standalone --bin
```

**2. 固定 VProxy 版本并以 git 依赖引入**

> **⚠️ 2026-11-10 P0 实查结论（见 ACTION-LOG S3）：VProxy 是单 crate 应用，无 workspace 子 crate。**
> 其 `[lib]` 虽存在（`crate-type=["rlib","cdylib","staticlib"]`），但 `default=["gui"]` 且
> **基础依赖硬带 `rhai`（脚本引擎）、`sqlx(sqlite)`、`axum`、`ring`、`tokio[full]`**——
> `default-features=false` 也关不掉这些，只会关 GUI。
>
> **结论：不把 vproxy 作为 lib 依赖**（违反方案 §3.1「不让上游类型/依赖渗透」，并引入
> rhai+sqlite 等不需要的攻击面/编译面）。本页 spike 的落地形式改为
> **「clone 后定位 parser/fingerprint/export 源文件，评估最小化移植面」**。
> 以下 A/B 两种原方案保留为历史备选，仅在 VProxy 未来拆出独立 parser crate 时复活。
>
> 版本锁定：`docs/THIRD_PARTY.md` 仍需记录本次调查所用 commit（`main` @ 2026-11-09）与 release tag（`1.16.9`）。

```toml
# 【方案 A｜已否决】git 依赖 —— 保留仅作记录
# vproxy = { git = "https://github.com/mateo-ki/vproxy", rev = "<PIN VProxy commit/tag>", default-features = false }
# 【方案 B｜复活的触发条件】VProxy 若拆出独立 parser crate 并发到 crates.io，则用版本号
# vproxy-parser = "x.y.z"
```

> 若 VProxy 不是以库（lib）形式暴露上述模块，或模块依赖 GUI/SQLite feature，则需要：
> 在 `Cargo.toml` 关闭相关 default-features（如 `default-features = false`）再按需开启；
> 若仍无法干净拆包，则**只移植必要算法**并保留 MIT 版权/许可证（方案 §3.1 末句），见「风险」。
> **2026-11-10 实查：正是后一种情况 → 走「只移植必要算法」路线。**

**3. 准备示例 Clash YAML fixture**

```yaml
# spikes/vproxy-standalone/examples/sample-clash.yaml
proxies:
  - name: "example-vmess"
    type: vmess
    server: 1.2.3.4
    port: 443
    uuid: 12345678-1234-1234-1234-1234567890ab
    alterId: 0
    cipher: auto
    network: ws
    ws-opts:
      path: /ray
  - name: "example-trojan"
    type: trojan
    server: 5.6.7.8
    port: 443
    password: example-password-123
    sni: example.com
```

**4. 最小 spike 代码骨架**

```rust
// spikes/vproxy-standalone/src/main.rs
// 说明：下方 VProxy 的具体 module 路径与函数签名以「固定版本源码」为准，
//      执行者需打开 <PIN VProxy commit/tag> 的源码确认后替换 TODO 标注处。
use std::fs;

fn main() -> anyhow::Result<()> {
    let raw = fs::read_to_string("examples/sample-clash.yaml")?;

    // (1) Parse: Clash YAML -> VProxy 内部节点模型
    // TODO(verify-against-pinned-VProxy): 用 parser 模块解析，例如：
    //   let nodes = vproxy::parser::parse_clash(&raw)?;
    //   实际函数名/返回类型以固定版本源码为准。
    let nodes: Vec<serde_yaml::Value> = serde_yaml::from_str(&raw)?; // 占位：替为真实 parse
    println!("parsed {} proxies", nodes.len());

    // (2) Fingerprint: 每个节点 -> 指纹串
    // TODO(verify-against-pinned-VProxy): 用 fingerprint 模块，例如：
    //   let fp = vproxy::fingerprint::node_hash(&node)?;
    //   注意：仅把 VProxy 指纹作为「基线参考」，最终 Core 用自有 NormalizedNode + fingerprint_version。
    let fp = "<fingerprint-from-vproxy>";
    println!("fingerprint = {}", fp);

    // (3) Export: 转换为对外订阅格式（例如 Mihomo/Clash）
    // TODO(verify-against-pinned-VProxy): 用 export 模块，例如：
    //   let out = vproxy::export::to_clash(&nodes)?;

    // 关键约束：VProxy 的 Node 类型 **不得** 直接落入 Core DB。
    // 必须在此处 map 到自有 NormalizedNode（字段见方案 §7.2）。
    // let normalized: NormalizedNode = convert_to_normalized(&node);
    Ok(())
}
```

> 验收用的最简版本可先只用 `serde_yaml` 解析并打印，把 VProxy 三个能力的真实调用
> 以 `TODO(verify-against-pinned-VProxy)` 标出——但 **P0 通过要求**是至少把真·VProxy
> 三个模块都实际驱动一次（见「验收」）。占位代码只是起点，不等于通过。

**5. 编译并查看依赖面 + 许可证扫描**

```bash
cargo build
echo "--- Cargo tree（依赖面）---"
cargo tree -e no-dev
echo "--- 许可证初扫（需先 cargo install cargo-deny 或 cargo-license）---"
cargo deny check licenses   # 脚手架见 p0-05；若未装可用 cargo license 替代
cargo license               # 备选
```

**6. 对照 Phlogo 现有基线记录差异**

- `internal/node/upsert.go` 的 `nodeHash`：`canonical = "type|server|port_int|auth_credential"`，`digest = hex(sha256(canonical)[:16])`。
- 把 VProxy 指纹结果与 `nodeHash` 对同一 fixture 做对比，记录是否一致、差异点在哪。
- `internal/naming/naming.go`（expr 模板 + `flags.json`）只负责展示命名，不影响指纹；记录本次不影响命名。

### 验收 / Acceptance

可机器校验：

```bash
# 1) 编译成功（退出码 0）
cd spikes/vproxy-standalone && cargo build
echo "exit=$?"   # 期望 0

# 2) 最小示例能解析示例 YAML 并算出指纹（退出码 0 且输出含 parsed 与 fingerprint）
cargo run --quiet
echo "exit=$?"   # 期望 0，且 stdout 含 "parsed 2 proxies" 与 "fingerprint ="

# 3) 依赖面与许可证已记录
cargo tree -e no-dev > /tmp/vproxy-cargo-tree.txt && wc -l /tmp/vproxy-cargo-tree.txt
# 期望：文件非空，且能在 docs/THIRD_PARTY.md 中引用

cargo deny check licenses
echo "exit=$?"   # 期望 0（或仅出现已审批的 allow 列表内许可证）
```

- 必须能解析示例 Clash YAML 为节点模型并计算出指纹（`cargo build` 成功 + `cargo run` 输出指纹）。
- `cargo tree` 输出与许可证审查结论已写入 `docs/THIRD_PARTY.md` 的 VProxy 章节。
- 记录结论：**VProxy 的 `Node` 类型未被用作 Core DB 类型**；已规划到 `NormalizedNode` 的转换。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo 现状（Go） | 重构目标（Rust） | 本页动作 |
|---|---|---|
| `internal/node/upsert.go` 的 `nodeHash`（sha256 前 16 字节）、`nodeAuth`、`normalizeCredential`（反复 percent-decode）、`clashPort` | 自有 `NormalizedNode` + `fingerprint_version` 身份计算（方案 §7） | 把 VProxy 指纹作为基线对照；保留算法可解释性，但改为带版本的规范化身份 |
| `internal/naming/naming.go`（expr 模板 + `flags.json`） | `filter` / `distribution` 内的命名（方案 §9.2、§10） | 本页不涉及命名，仅记录「命名不参与指纹」 |
| `go.mod` 依赖 `openpaily/paily-fetch v0.0.0-20260919045243-...` 的 `format`（clash/singbox/base64） | VProxy Parser/Export（差异项 1，CONVENTIONS §4） | 验证 VProxy 能否替代 paily-fetch/format |

> Phlogo 当前指纹字段取舍（`upsert.go`）：vmess/vless→`uuid`；trojan/anytls/hysteria/hysteria2→`password`；
> tuic→`uuid:password`(V5)/`password`(V4)；ss/ssr→`password`；wireguard→`private-key`；
> socks5/http/mieru→`username:password`；snell→`psk`；sudoku→`key`。这些是 VProxy 指纹基线对照的必测协议清单。

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **构建 spike**：`cargo build`（spike 可用独立 job，或在 `p0` 验证阶段手动跑；不进 `cargo build --workspace`）。
- **依赖/许可证门禁**：`cargo tree -e no-dev` 产物归档；`cargo deny check licenses` 作为门禁（完整脚手架见 `p0-05`）。
- **回归基线**：把本页示例 fixture 与指纹对照结论固化为一份 golden test，未来 `vproxy-adapter`（P2）改动时比对，防止指纹语义漂移。
- **版本锁**：CI 必须读 `docs/THIRD_PARTY.md` 中记录的 `<PIN VProxy commit/tag>`，不允许用 floating 版本。

### 风险 / Risks

- **无法干净拆包**：若 VProxy 的 parser/fingerprint/export 强依赖 GUI 或 SQLite feature，则按方案 §3.1 末句**只移植必要算法**，并保留 MIT 版权与传递依赖许可证审查——此时本页验收改为「移植后的算法 crate 编译并复现指纹基线」。
- **内部 API 漂移**：VProxy 具体 module/函数名以固定版本源码为准；本页 `TODO` 标注处必须回填真实签名，不能凭空假设。
- **指纹语义不一致**：VProxy 指纹与 `nodeHash` 可能在某些协议字段（TLS/Reality、transport path、SNI）上有差异；必须建自有 golden fixtures（方案 §7.3），不能仅凭 VProxy 测试结果假定覆盖。
- **传递依赖许可证**：即便 VProxy 是 MIT，其传递依赖可能含 Copyleft；需 `cargo deny` 全量扫描并在 `THIRD_PARTY.md` 记录。
- **类型渗透**：务必守住「VProxy `Node` 不进 Core DB」红线，否则后续 `domain` / `database` crate 会被上游类型污染（方案 §1.6 第 4 条）。
