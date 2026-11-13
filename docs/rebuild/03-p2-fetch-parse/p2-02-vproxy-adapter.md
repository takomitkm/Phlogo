## 3-02 VProxy 解析适配器 · VProxy Parser Adapter

### 目标 / Goal

把外部 **VProxy** 的 Parser 能力封装成 `crates/vproxy-adapter`，将任意订阅格式（Clash / Sing-box / Base64）解析为**自有的 `NormalizedNode`**：

- 严格区分「**用于身份计算的字段**」与「**仅用于展示 / 来源追踪的字段**」（方案 §7.2）。
- **绝不允许 VProxy 的 Node 类型渗透到数据库层**：VProxy 模型只作输入适配对象，Adapter 的输出与 `crates/database` 之间只经过 `crates/domain`（`domain::NormalizedNode`）。

### 范围 / Scope

- **新建** crate：`crates/vproxy-adapter`（解析 + 映射到 `domain::NormalizedNode`）。
- **依赖** `crates/domain`（定义 `NormalizedNode`）与 P0 验证过的 VProxy Parser（方案 §3.1，固定 commit）。
- **不做**：指纹计算（见 `p2-03`）、抓取（见 `p2-01`）、对账入库（见 `p2-04`）。Adapter 不接触 DB。
- 注意：Phlogo 现状里 `internal/node/upsert.go` 对节点是**朴素 clash JSON 解析**（`json.Unmarshal` 成 `map[string]interface{}` 再抽 `type`/`server`/`auth`/`raw`），本步骤用 VProxy 强类型解析替换它。

### 前置 / Prerequisites

- P0 已验证 VProxy Parser/Fingerprint/Export 可**无 GUI、无 SQLite 业务层依赖**地独立编译，并已固定 commit/tag（见 [`CONVENTIONS.md`](CONVENTIONS.md) §2.11 / 方案 §3.1）。
- `crates/domain` 已定义 `NormalizedNode`（字段见下）。本步骤与 `p2-03` 共享该结构。
- 协议依据：[`CONVENTIONS.md`](CONVENTIONS.md) §2.4（指纹字段口径）、方案 §7（标准化模型）。

### 步骤 / Steps

**Step 1 — 定义 `NormalizedNode`（`crates/domain/src/node.rs`）**

按方案 §7.2，至少包含：

```rust
// crates/domain/src/node.rs (骨架/目标实现)
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NormalizedNode {
    // ---- 身份计算字段（参与 identity_hash，见 p2-03）----
    pub protocol: String,          // vmess/vless/trojan/ss/...（小写）
    pub server: String,
    pub port: u16,
    pub credentials: Credential,   // 按协议的主凭据（见 p2-03 nodeAuth）
    pub transport: Option<Transport>,   // ws/grpc/http 等及 Path/SNI/Host
    pub tls: Option<TlsSettings>,       // tls/sni/alpn/reality 等
    pub network_options: Option<serde_json::Value>, // 其它影响连接的参数

    // ---- 展示 / 来源追踪字段（不参与身份）----
    pub display_name: Option<String>,     // clash 的 name
    pub source_context: SourceContext,    // 来自哪个 source / 哪次 run
    pub raw_config_reference: String,     // 原始配置（加密存储于 node_payloads，见 §5.1）

    // ---- 由 p2-03 填充 ----
    pub identity_hash: Option<String>,
    pub fingerprint_version: Option<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceContext {
    pub source_id: uuid::Uuid,
    pub fetch_run_id: uuid::Uuid,
}
```

**Step 2 — 实现 Adapter（`crates/vproxy-adapter/src/lib.rs`）**

```rust
// 骨架/目标实现（VProxy 具体 API 以 P0 验证结果为准）
use domain::NormalizedNode;

pub struct VproxyAdapter;

impl VproxyAdapter {
    /// 输入原始订阅 bytes（clash / singbox / base64），
    /// 调用 VProxy Parser，返回自有 NormalizedNode 列表。
    /// VProxy 的 Node 类型只在这里局部存在，绝不外泄到返回值之外。
    pub fn parse_subscription(&self, raw: &[u8]) -> Result<Vec<NormalizedNode>, AdapterError> {
        // 1) 格式识别（按 magic / 结构）
        // 2) 调 VProxy Parser 得到 Vec<VproxyNode>
        // 3) 逐条 map：VproxyNode -> NormalizedNode
        //    - 身份字段：protocol/server/port/credentials/transport/tls/network_options
        //    - 展示/来源字段：display_name = vproxy.name; raw_config_reference = 原始片段
        todo!("per P0-verified VProxy API")
    }
}
```

**Step 3 — 身份字段 vs 展示/来源字段（强制约束）**

| 类别 | 字段 | 是否参与 `identity_hash` |
|---|---|---|
| 身份计算 | `protocol`, `server`, `port`, `credentials`, `transport`, `tls`, `network_options` | ✅ 是 |
| 展示 / 来源追踪 | `display_name`（`name`）、`source_context`、`raw_config_reference` | ❌ 否 |

> 这直接对应 [`CONVENTIONS.md`](CONVENTIONS.md) §2.4「名称**不**参与指纹，凭据参与」与方案 §5.3/§7.3。注意：方案 §7.3 还要求 transport/Path/SNI/TLS/Reality 等**影响连接身份**的参数参与指纹，但具体哪些字段算「影响连接语义」要按协议逐一定义，不能仅凭 VProxy 默认行为假定全部覆盖——必须为本项目实际支持的协议建立自己的 golden fixture（方案 §7.3 末段）。

**Step 4 — Golden fixtures（`tests/fixtures/parser/`）**

提供样例订阅，断言解析结果：

- `clash.sample.yaml`：含 2 个 vmess + 1 个 trojan 的 `proxies:` 列表。
- `singbox.sample.json`：Sing-box `outbounds` 样例。
- `base64.sample.txt`：Base64 编码的 clash 文本。

```yaml
# tests/fixtures/parser/clash.sample.yaml
proxies:
  - name: "hk-vmess-01"
    type: vmess
    server: hk.example.com
    port: 443
    uuid: 11111111-1111-1111-1111-111111111111
    alterId: 0
    cipher: auto
    tls: true
    network: ws
    ws-opts:
      path: "/ray"
      headers:
        Host: hk.example.com
  - name: "jp-vmess-02"
    type: vmess
    server: jp.example.com
    port: 443
    uuid: 22222222-2222-2222-2222-222222222222
    alterId: 0
    cipher: auto
  - name: "sg-trojan-01"
    type: trojan
    server: sg.example.com
    port: 443
    password: "s3cr3t-password"
    sni: sg.example.com
    alpn: ["h2"]
```

### 验证结果 / Verified Results（已实测，2026-11-10）

> 依据 P0-01 结论：**VProxy 不作依赖，最小化移植**。故 `crates/vproxy-adapter` 自实现解析（Clash YAML / Sing-box JSON / Base64），不引入 VProxy/VProxy 类型。

**落地**：
- `crates/domain/src/node.rs`：`NormalizedNode{protocol,server,port,credentials,transport,tls,network_options | display_name,source_context,raw_config_reference | identity_hash,fingerprint_version}` + `Credential{Enum:Uuid/Password/None}`、`Transport`、`TlsSettings`、`SourceContext`。
  **`NormalizedNode::identity_material()`**：只序列化**身份字段**（`protocol/server/port/credentials/transport/tls/network_options`），供 p2-03 做哈希；显示/来源字段不参与。
- `crates/vproxy-adapter`：`parse_subscription(bytes, ctx) -> Vec<NormalizedNode>`。识别顺序 Clash(YAML)→Sing-box(JSON)→Base64(解一层后重试)；`from_clash`/`from_singbox` 逐协议抽 `credentials`（vmess/vless=uuid，trojan/ss=password）、`transport`(ws path/host、grpc service_name)、`tls`(enabled/sni/alpn/reality)。**解析阶段保留原始凭据值**，percent-decode 留到 p2-03，避免双重解码。
- 不支持的协议（无 `uuid`/`password`）被跳过；仅当无任何可支持节点时返回 `UnrecognizedFormat`。

**实测（校机）**：
| 项 | 结果 |
|---|---|
| `cargo build -p vproxy-adapter -p domain --all-targets` | **0**（8.99s） |
| `cargo test -p vproxy-adapter -p domain` | **0**：`clash`/`singbox`/`base64`/`field_separation` + `identity_ignores_display_name_but_not_transport`（5 passed） |
| `cargo fmt -p vproxy-adapter -p domain -- --check` | **0** |
| `cargo clippy -p vproxy-adapter -p domain --all-targets -- -D warnings` | **0** |
| **类型不渗透** `cargo tree -p database` 含 `vproxy`? | **NO**（DB 层不依赖 adapter） |

**clash 用例断言**：解析出 3 个节点，`display_name` = `hk-vmess-01/jp-vmess-02/sg-trojan-01`；vmess `credentials=Uuid(1111…)`、ws `path=/ray`、`Host=hk.example.com`；trojan `credentials=Password("s3cr3t-password")`、`alpn=["h2"]`。
**field_separation**：改名不改 `identity_material()`；改变 `transport.path` 则改变。

**说明**：fixtures 以测试内联字符串实现（等价于 `tests/fixtures/parser/*`，避免运行期路径依赖）；percent-decode 与身份哈希在 p2-03。

### 验收 / Acceptance

1. **解析样例** —— `cargo test -p vproxy-adapter`：

   ```text
   running 4 tests
   test parser::clash ... ok
   test parser::singbox ... ok
   test parser::base64 ... ok
   test parser::field_separation ... ok
   test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured
   ```

   其中 `clash` 用例断言：从 `clash.sample.yaml` 解析出 **3** 个 `NormalizedNode`；`display_name` 分别等于 `hk-vmess-01` / `jp-vmess-02` / `sg-trojan-01`；`protocol`/`server`/`port`/`credentials` 抽取正确（trojan 的 `credentials` 来自 `password`，vmess 来自 `uuid`）。

2. **类型不渗透** —— 编译期约束：`crates/database` 的 `Cargo.toml` **不**依赖 `crates/vproxy-adapter`；从 `crates/vproxy-adapter` 返回到入库路径的所有节点结构都是 `domain::NormalizedNode`。可用 `cargo tree -p database` 断言无 `vproxy-adapter` 节点；并用 `cargo build -p database` 通过证明 DB 层不需要 VProxy 类型。

3. **字段分离** —— `field_separation` 用例断言：修改 `display_name` 不影响后续 `identity_hash`（与 `p2-03` 的「改名同身份」测试呼应），而 `transport`/`tls` 变化会影响。

4. **Lint** —— `cargo clippy -p vproxy-adapter --all-targets -- -D warnings` 退出码 `0`。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

- **`internal/node/upsert.go` 的 `UpsertFromFetchResult`**：当前用 `json.Unmarshal(raw, &fields)` 把节点解析成 `map[string]interface{}`，再 `nodeAuth(proto, fields)` 抽主凭据，最后把**整个原始 JSON** 存进 `nodes.raw`。这是**朴素 clash JSON 解析**，没有强类型、没有 transport/tls 的结构化抽取。Rust 侧用 VProxy Parser 替换这层，产出强类型 `NormalizedNode`。
- 方案 §3.1 / §7.2 明确「不直接采用 VProxy 的 Node 类型作为 Gnosis 数据库类型；必须转换为自己的 `NormalizedNode`」——这正是本步骤的核心约束。
- 许可证：VProxy 为 MIT（[`CONVENTIONS.md`](CONVENTIONS.md) §2.11），复用需保留版权；若 P0 发现无法干净拆包，则退化为最小化移植必要算法并保留 MIT 要求。

### CI 提示 / CI notes（仅描述，不落地 YAML）

- `cargo test -p vproxy-adapter`：纯解析 + fixture 断言，不需要 Postgres，应作为快速 job。
- 增加一个 **fixture lint / snapshot** 步骤：fixture 文件被改动时（golden 变化）CI 应失败或要求显式更新 snapshot，避免解析行为悄悄回归。
- `cargo tree -p database` 断言无 `vproxy-adapter`，可放进 `cargo clippy`/架构约束检查 job。
- 参见 [`CONVENTIONS.md`](CONVENTIONS.md) §5 的 `cargo test --workspace` 种子。

### 风险 / Risks

- **VProxy API 细节（P0 未过前不确定）**：Parser 函数签名、错误类型、对 singbox/base64 的支持程度都依赖 P0 验证结果；本页代码是**骨架**，落地时按 P0 结论填实。
- **字段命名/语义错位**：clash 的 `ws-opts`、singbox 的 `outbounds[].transport` 与 VProxy 内部字段名可能不一致，需逐协议映射；transport/Path/SNI/TLS 哪些参与身份要在 `p2-03` 一起定稿。
- **percent-decode 时机**：凭据里的 `%` 转义应在**映射为 `credentials` 之前还是之后**处理？Phlogo 在指纹阶段才 `normalizeCredential`（见 `p2-03`），解析阶段应保持原始值，避免双重解码。
- **Base64 嵌套**：部分订阅是 Base64(clash) 再嵌套，需确认 VProxy 是否递归识别，否则要 Adapter 自己先 Base64-decode 一层。
