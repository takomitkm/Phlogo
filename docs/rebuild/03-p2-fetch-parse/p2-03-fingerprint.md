## 3-03 节点指纹 · Node Fingerprint (identity_hash + fingerprint_version)

### 目标 / Goal

为 `domain::NormalizedNode` 计算确定性的 **`identity_hash`** 与 **`fingerprint_version`，并覆盖方案 §7.3 的全部指纹测试：

- **忠实移植** Phlogo `internal/node/upsert.go` 的 `nodeHash` / `nodeAuth` / `normalizeCredential` 算法。
- **必须保留** `normalizeCredential` 的「反复 percent-decode 直到稳定」规范化（防止多层 `%25` 转义产生不同 hash）。
- 明确区分「身份字段」与「展示 / 来源字段」，并补一个**算法升级的迁移/重算方案**（`fingerprint_version` 的用途）。

### 范围 / Scope

- **扩展** `crates/domain`（指纹函数 + `identity_hash`/`fingerprint_version` 字段，字段由 `p2-02` 的 `NormalizedNode` 承载）。
- **不做**：抓取（`p2-01`）、解析（`p2-02`）、对账入库（`p2-04`）。本步骤只产出哈希值，不涉及 DB 写入。
- 依赖 `p2-02` 的 `NormalizedNode` 结构。

### 前置 / Prerequisites

- `p2-02` 已定义 `NormalizedNode` 及其身份字段（`protocol`/`server`/`port`/`credentials`/`transport`/`tls`/`network_options`）。
- 工具链：`sha2`（SHA-256）、`hex`、`percent-encoding`（percent-decode）、`uuid`。
- 依据：[`CONVENTIONS.md`](CONVENTIONS.md) §2.4、`internal/node/upsert.go`（`nodeHash`/`nodeAuth`/`normalizeCredential`）、方案 §7.3。

### 步骤 / Steps

**Step 1 — 移植 `nodeHash` 算法（`crates/domain/src/fingerprint.rs`）**

Phlogo 算法（[`CONVENTIONS.md`](CONVENTIONS.md) §2.4 + `upsert.go`）：

```text
canonical = "type|server|port_int|auth_credential"
digest    = hex(sha256(canonical)[:16])   // 32-char hex
```

```rust
// crates/domain/src/fingerprint.rs (目标实现，算法对齐 Go 版)
use sha2::{Sha256, Digest};

pub const CURRENT_FINGERPRINT_VERSION: u8 = 1;

/// 对齐 Go nodeHash：canonical = "type|server|port_int|auth_credential"
/// 空 type / 空 server / port==0 时返回 None（无身份，与 Go 返回 "" 一致）。
pub fn compute_identity(nn: &NormalizedNode) -> Option<String> {
    if nn.protocol.is_empty() || nn.server.is_empty() || nn.port == 0 {
        return None;
    }
    let auth = normalize_credential(&nn.credentials.primary());
    let canonical = format!(
        "{}|{}|{}|{}",
        nn.protocol, nn.server, nn.port, auth
    );
    let mut h = Sha256::new();
    h.update(canonical.as_bytes());
    let sum = h.finalize();
    Some(hex::encode(&sum[..16])) // 前 16 字节 → 32-char hex
}
```

**Step 2 — 移植 `nodeAuth`（按协议取主凭据）**

直接对齐 `upsert.go` 的 switch：

| 协议 | 主凭据 |
|---|---|
| `vmess`, `vless` | `uuid` |
| `trojan`, `anytls`, `hysteria`, `hysteria2` | `password` |
| `tuic` | V5（`uuid` 非空）→ `uuid:password`；否则 `password` |
| `ss`, `ssr` | `password` |
| `wireguard` | `private-key` |
| `socks5`, `http`, `mieru` | `username:password` |
| `snell` | `psk` |
| `sudoku` | `key` |
| 其它 | 空（与 Go `default: return ""` 一致） |

```rust
pub fn node_auth(ptype: &str, cred: &Credential) -> String {
    match ptype {
        "vmess" | "vless" => cred.uuid.clone().unwrap_or_default(),
        "trojan" | "anytls" | "hysteria" | "hysteria2" => cred.password.clone().unwrap_or_default(),
        "tuic" => match (&cred.uuid, &cred.password) {
            (Some(u), Some(p)) if !u.is_empty() => format!("{}:{}", u, p),
            _ => cred.password.clone().unwrap_or_default(),
        },
        "ss" | "ssr" => cred.password.clone().unwrap_or_default(),
        "wireguard" => cred.private_key.clone().unwrap_or_default(),
        "socks5" | "http" | "mieru" =>
            format!("{}:{}", cred.username.clone().unwrap_or_default(),
                            cred.password.clone().unwrap_or_default()),
        "snell" => cred.psk.clone().unwrap_or_default(),
        "sudoku" => cred.key.clone().unwrap_or_default(),
        _ => String::new(),
    }
}
```

**Step 3 — 移植 `normalizeCredential`（反复 percent-decode，必须保留）**

Go 版用 `url.PathUnescape` 循环直到字符串不再变化。Rust 用 `percent-encoding` 的 `percent_decode_str` 等价实现：

```rust
use percent_encoding::percent_decode_str;

/// 反复 percent-decode 直到稳定：防止 %25 多层转义产生不同 hash。
/// 对齐 Go normalizeCredential。带循环保护（最多 16 轮）防止异常输入死循环。
pub fn normalize_credential(s: &str) -> String {
    let mut cur = s.to_string();
    for _ in 0..16 {
        let decoded = percent_decode_str(&cur).decode_utf8_lossy().into_owned();
        if decoded == cur {
            return cur;
        }
        cur = decoded;
    }
    cur
}
```

> **关键不变量**：Phlogo 的 `nodeHash` 先 `normalizeCredential(nodeAuth(...))` 再拼 canonical。Rust 版必须保持同一顺序——先取主凭据、再做反复 percent-decode、最后拼 `canonical`。不能改成只 decode 一次，否则 `%25`-encoded 凭据会得到不同哈希。

**Step 4 — 身份字段 vs 展示字段（文档化）**

| 类别 | 字段 | 参与 `identity_hash` |
|---|---|---|
| 身份计算 | `protocol`, `server`, `port`, `credentials`（主凭据）, `transport`(Path/SNI/Host), `tls`(sni/alpn/reality), `network_options` | ✅ |
| 展示 / 来源追踪 | `display_name`(`name`), `source_context`, `raw_config_reference` | ❌ |

> 与 [`CONVENTIONS.md`](CONVENTIONS.md) §2.4 一致：「名称**不**参与指纹，凭据参与」。transport/tls 等按方案 §5.3/§7.3「影响连接身份的参数参与指纹」处理。

**Step 5 — 算法升级迁移方案（用 `fingerprint_version`）**

当指纹算法需要变更（如纳入更多传输字段）时：

1. 把 `CURRENT_FINGERPRINT_VERSION` 从 `1` 升到 `2`，并保留旧版函数 `compute_identity_v1`。
2. 提供迁移函数 `recompute_all_identities(pool)`，扫描所有 `nodes` 中 `fingerprint_version < 2` 的行，用新算法重算 `identity_hash` 并写回 `fingerprint_version = 2`，**在单条 SQLx 事务内完成**。
3. 重算后可能出现「不同旧哈希 → 相同新哈希」的合并：用 `ON CONFLICT (identity_hash)` 去重，并迁移其 `node_sources` 关联（`p2-04` 的对账逻辑负责）。
4. 迁移作为 P1 migration 之后的独立 job 或 `cargo sqlx` 脚本运行，CI 里单独验证「重算后无孤儿关联」。

### 验收 / Acceptance

机器可校验项（命令 + 预期）：

1. **Golden 测试全过** —— `cargo test -p domain fingerprint`：

   | 用例 | 输入变化 | 预期 `identity_hash` |
   |---|---|---|
   | name_change | 仅改 `display_name` | **相同**（改名同身份） |
   | uuid_change | vmess `uuid` 不同 | **不同** |
   | password_change | trojan `password` 不同 | **不同** |
   | transport_change | `transport.path` 不同 | **不同** |
   | tls_change | `tls.sni` 不同 | **不同** |
   | server_change | `server` 不同 | **不同** |
   | port_change | `port` 不同 | **不同** |
   | field_order | JSON 字段顺序不同 | **相同** |
   | null_default | 缺省/null/空字符串按协议规范统一 | **相同** |
   | percent_decode | 凭据 `%25` 多层 vs 明文 | **相同** |

   退出码 `0`，全部用例通过。

2. **percent-decode 不变量** —— 用例 `percent_decode`：`credentials.primary = "abc%25def"`（即字面 `%25`）、`"abc%def"`、`"abc def"` 三种应得到**同一个**规范化结果（反复 decode 后均为 `abc%def` / `abc def`）。断言 `normalize_credential` 对三者输出一致。

3. **空值无身份** —— `cargo test -p domain fingerprint::empty`：空 `protocol` 或空 `server` 或 `port==0` 时 `compute_identity` 返回 `None`（对齐 Go 返回 `""`）。

4. **版本字段** —— 断言 `compute_identity` 产出的节点 `fingerprint_version == CURRENT_FINGERPRINT_VERSION (1)`。

5. **Lint** —— `cargo clippy -p domain --all-targets -- -D warnings` 退出码 `0`。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

- **`internal/node/upsert.go`**：
  - `nodeHash(clash)`：`canonical = "type|server|port_int|auth_credential"`，`digest = hex(sha256(canonical)[:16])`，空字段返回 `""`。→ Rust `compute_identity`。
  - `nodeAuth(ptype, clash)`：按协议 switch 取主凭据。→ Rust `node_auth`。
  - `normalizeCredential(s)`：`url.PathUnescape` 循环直到稳定。→ Rust `normalize_credential`（**必须保留反复 decode**）。
  - `clashPort`：兼容 `float64/int/int64`；Rust 侧 `NormalizedNode.port` 已是 `u16`，无需兼容。
- [`CONVENTIONS.md`](CONVENTIONS.md) §2.4：「重构要点：保留算法可解释性，但改为带 `fingerprint_version` 的 `NormalizedNode` 身份计算」——本步骤即该要点的落地，`fingerprint_version` 是 Phlogo 没有的新增。

### CI 提示 / CI notes（仅描述，不落地 YAML）

- `cargo test -p domain fingerprint` 作为 `cargo test --workspace` 的一部分；
- 增加一个 **golden fixture 变更保护**：`tests/fixtures/fingerprint/*.json` 改变时要求显式 bump 或更新 snapshot，避免指纹静默变化导致全库节点被当成「新节点」重插。
- 算法升级的 `recompute_all_identities` 迁移脚本在 CI 里用一个「旧版本 fixture → 重算 → 断言无孤儿关联」的集成测试覆盖。
- 参见 [`CONVENTIONS.md`](CONVENTIONS.md) §5 的 `sqlx migrate run --check` / `cargo sqlx prepare` 种子（迁移脚本需离线查询校验）。

### 风险 / Risks

- **percent-decode 死循环**：异常输入（如破坏的 `%` 序列）理论上可能不收敛；加「最多 16 轮」保护，对齐 Go 的 `for {}`（Go 版实际靠 `decoded == s` 终止，正常凭据必然收敛）。
- **TUIC V5 vs V4**：`uuid:password` 与仅 `password` 的区分必须和 Go 一致，否则同一节点在不同版本得到不同身份。
- **未知协议**：Go 默认返回 `""`（空凭据），Rust 也返回空串——但空凭据 + 相同 server/port 会让不同未知协议节点**撞身份**，需在后续协议扩展时补表。
- **transport/tls 哪些字段进身份**：方案 §7.3 要求「按实际连接语义判断」，不能一刀切；本步骤先把 Path/SNI/Host/Reality 等纳入，具体字段集要与 `p2-02` 的 `NormalizedNode` 字段定义保持一致并在 fixture 中钉死。
