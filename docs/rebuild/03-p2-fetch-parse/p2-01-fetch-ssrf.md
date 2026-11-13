## 3-01 抓取与 SSRF 防护 · Fetch & SSRF Guard

### 目标 / Goal

把订阅抓取收编进 `gnosis-worker` 的 **Fetch Worker**，使用 **Reqwest** 直接拉取 `sources.content` 里的订阅 URL，并新增一层 **SSRF 防护**：

- 拦截环回（loopback）、私网（private）、链路本地（link-local）、唯一本地（unique-local）、未指定（unspecified）地址，以及**云元数据地址**。
- 对每次**重定向目标**重新执行同一套校验（不信任自动跟随）。
- 限制**响应大小**与**超时**，防止慢连接/巨响应耗尽 worker。

### 范围 / Scope

- **新建** crate：`crates/fetch`（Fetch Worker 的抓取与防护内核）。
- **net-new**：Phlogo 当前**完全没有** SSRF 防护——抓取由外部 `paily-fetch` 服务完成，`internal/fetcher/handler.go` 只是接收结果的 HTTP 回调端点。本步骤把抓取收进 gnosis-worker，因此防护必须在此补齐。
- **不做**：VProxy 解析（见 `p2-02`）、指纹计算（见 `p2-03`）、对账入库（见 `p2-04`）。本步骤只产出 `Bytes`（原始订阅内容）与结构化错误。

### 前置 / Prerequisites

- P1 已建立 Cargo workspace 与 `gnosis-worker` 骨架（[`CONVENTIONS.md`](CONVENTIONS.md) §2.3）。
- `crates/domain` 已定义 `Source`（至少含 `content` 字段，即订阅 URL 或原始文本）。
- 工具链：Rust + Cargo、`reqwest`、`tokio`、`ipnetwork`（或等价 CIDR 匹配）、`percent-encoding`（仅防护本身不需要，留给 `p2-03`）、`wiremock` 或 `hyper` 用于本地测试服务器。
- 协议依据：`paily-core-rust-rebuild-plan.md` §4（HTTP Client = Reqwest）、§16.9（Fetch 防 SSRF 条目）。

### 步骤 / Steps

**Step 1 — 新增依赖（`crates/fetch/Cargo.toml`）**

```toml
[dependencies]
reqwest = { version = "0.12", default-features = false, features = ["rustls-tls", "stream"] }
tokio = { version = "1", features = ["full"] }
ipnetwork = "0.20"
thiserror = "1"
tracing = "0.1"
bytes = "1"
serde = { version = "1", features = ["derive"] }

[dev-dependencies]
wiremock = "0.6"   # 或 hyper 自建 test server
tokio = { version = "1", features = ["full", "macros", "rt-multi-thread"] }
```

**Step 2 — 定义被封锁的地址范围（`crates/fetch/src/ssrf.rs`）**

依据方案 §16.9「限制环回、内网、链路本地、云元数据地址」，下列网段**一律拒绝**（IPv4 + IPv6）：

| 类别 | 网段 | 说明 |
|---|---|---|
| 未指定 | `0.0.0.0/8`、`::/128` | 请求本身无意义 |
| 环回 | `127.0.0.0/8`、`::1/128` | loopback |
| 私网 | `10.0.0.0/8`、`172.16.0.0/12`、`192.168.0.0/16`、`100.64.0.0/10`（CGNAT） | RFC 1918 + 6598 |
| 链路本地 | `169.254.0.0/16`、`fe80::/10` | 含云元数据 `169.254.169.254` |
| 唯一本地 | `fc00::/7` | IPv6 ULA |
| IPv4 映射 | `::ffff:0:0/96` | 映射到内网 IPv4 |
| 云元数据 | `169.254.169.254`（AWS/GCP/Azure IMDS）、`192.0.0.170`（Azure IMDS 备用） | 方案 §16.9「云元数据地址」 |

```rust
// crates/fetch/src/ssrf.rs (骨架/目标实现)
use ipnetwork::IpNetwork;
use std::net::IpAddr;
use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum SsrfError {
    #[error("blocked address: {0}")]
    BlockedAddress(IpAddr),
    #[error("invalid url: {0}")]
    InvalidUrl(String),
    #[error("dns resolution failed: {0}")]
    DnsResolution(String),
}

// 必须在编译期或启动时构造一次，复用。
fn blocked_networks() -> Vec<IpNetwork> {
    [
        "0.0.0.0/8", "127.0.0.0/8", "10.0.0.0/8", "172.16.0.0/12",
        "192.168.0.0/16", "100.64.0.0/10", "169.254.0.0/16",
        "::/128", "::1/128", "fe80::/10", "fc00::/7", "::ffff:0:0/96",
    ]
    .iter()
    .map(|s| s.parse::<IpNetwork>().unwrap())
    .collect()
}

/// 校验一个已解析的 IP 是否允许被抓取。
pub fn check_ip(ip: IpAddr) -> Result<(), SsrfError> {
    for net in blocked_networks() {
        if net.contains(ip) {
            return Err(SsrfError::BlockedAddress(ip));
        }
    }
    Ok(())
}
```

> 注意：URL 可能使用**主机名**而非字面 IP。防护必须在**解析后**对解析出的每个 IP 调用 `check_ip`，并且为了缓解 DNS rebinding，应在「建立连接那一刻」重新解析并校验（或在连接前一次性解析所有 A/AAAA 记录并全部放行才继续）。主机名本身（如 `metadata.google.internal`）也应解析后校验，不能仅靠字面匹配。

**Step 3 — 受控 fetch（`crates/fetch/src/lib.rs`）**

```rust
// 骨架/目标实现：关闭自动重定向，手动逐跳校验；限制大小与超时。
pub struct FetchPolicy {
    pub connect_timeout: std::time::Duration, // 如 10s
    pub total_timeout: std::time::Duration,   // 如 60s
    pub max_response_bytes: usize,            // 如 16 MiB
}

pub async fn fetch(url: &str, policy: &FetchPolicy) -> Result<bytes::Bytes, FetchError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none()) // 手动处理，逐跳校验
        .connect_timeout(policy.connect_timeout)
        .timeout(policy.total_timeout)
        .build()?;

    let mut next = url.to_string();
    let mut hops = 0;
    loop {
        // 1) 解析 + 校验（参见 ssrf::check_ip，含 DNS 重解析）
        ssrf::guard(&next).await?;
        // 2) 发请求
        let resp = client.get(&next).send().await?;
        // 3) 重定向：手动取出 Location，重新校验后再继续
        if resp.status().is_redirection() {
            if hops >= 5 { return Err(FetchError::TooManyRedirects); }
            let loc = resp.headers().get(reqwest::header::LOCATION)
                .ok_or(FetchError::RedirectWithoutLocation)?.to_str()?.to_string();
            next = resolve_relative(url, &loc);
            hops += 1;
            continue;
        }
        // 4) 限大小读取
        let body = resp.bytes().await?; // 实际实现应流式累加并截断到 max_response_bytes
        if body.len() > policy.max_response_bytes {
            return Err(FetchError::ResponseTooLarge(body.len()));
        }
        return Ok(body);
    }
}
```

### 验证结果 / Verified Results（已实测，2026-11-10）

> 环境：校机 `rustc 1.99.1`。

**落地**：新 crate `crates/fetch`（`ssrf.rs` + `lib.rs`），`reqwest 0.12.28`（`rustls-tls`，**在 GNU/Windows 上编译成功**，走 `hyper-rustls`/`tokio-rustls`，未触发 aws-lc C 构建问题），`ipnetwork 0.20`。
- `ssrf::check_ip`：黑名单含 `0.0.0.0/8,127/8,10/8,172.16/12,192.168/16,100.64/10,169.254/16,192.0.0.170/32,::/128,::1/128,fe80::/10,fc00::/7,::ffff:0:0/96`。
- `ssrf::guard`：字面 IP 直接校验；主机名经 `tokio::net::lookup_host` 解析，**所有**解析结果都须通过（否则拒绝）；`FetchPolicy.allowlist` 可放行特定网段（测试用 loopback）。
- `fetch()`：`redirect(Policy::none())` 手动逐跳、每跳先 `guard`；流式累加并 `max_response_bytes` 截断；`total_timeout`→`FetchError::Timeout`。

**实测（校机）**：
| 项 | 结果 |
|---|---|
| `cargo build -p fetch --all-targets` | **0**（29.27s） |
| `cargo test -p fetch` | **0**，`6 passed`：`blocks_dangerous_and_allows_public`、`benign_fetch_ok`、`metadata_literal_blocked`、`redirect_to_metadata_blocked`、`response_too_large`、`timeout_hit` |
| `cargo fmt -p fetch -- --check` | **0** |
| `cargo clippy -p fetch --all-targets -- -D warnings` | **0** |

→ 覆盖验收 1–6：IP 黑名单表、良性 fetch、云元数据拦截（**不发出真实请求**）、302→元数据被复核拦截、超大/超时、lint。

**残留风险（诚实标注）**：`guard` 在“发请求前解析并校验全部 IP”，但 `reqwest` 实际连接时会**再次解析**，仍存在极小的 DNS-rebinding 时间窗。彻底消除需「连接固定 IP + SNI/Host 头」自定义 dialer（P7 加固项）。

### 验收 / Acceptance

机器可校验项（命令 + 预期）：

1. **IP 黑名单单元测试表** —— `cargo test -p fetch ssrf::tests`：

   | 输入 IP | 预期 |
   |---|---|
   | `127.0.0.1` | `Err(BlockedAddress)` |
   | `10.0.0.5` | `Err(BlockedAddress)` |
   | `172.16.3.4` | `Err(BlockedAddress)` |
   | `192.168.1.1` | `Err(BlockedAddress)` |
   | `169.254.169.254` | `Err(BlockedAddress)` |
   | `100.64.0.1` | `Err(BlockedAddress)` |
   | `fe80::1` | `Err(BlockedAddress)` |
   | `8.8.8.8` | `Ok(())` |
   | `1.1.1.1` | `Ok(())` |
   | `2606:4700:4700::1111` | `Ok(())` |

   退出码 `0`，测试全部通过。

2. **良性 URL 成功** —— `cargo test -p fetch benign`：用 `wiremock`（或 `hyper` 自建）起一个本地 HTTP 测试服务器，对 `http://127.0.0.1:<ephemeral>` 之外的良性地址（测试服务器绑定到非环回接口，或把测试服务器 IP 加入白名单）发起 fetch，预期 `Ok(Bytes)` 且内容等于服务器返回体。

   > 实操提示：本地测试服务器若只能绑环回，单测可临时把测试服务器 IP 通过 `FetchPolicy` 的 `allowlist` 放行，或断言「良性但非黑名单地址 → 走到真实网络层（mock 返回 200）」。

3. **云元数据被拦截** —— `cargo test -p fetch metadata`：对 `http://169.254.169.254/latest/meta-data/`（及 `192.0.0.170`）断言返回 `Err(FetchError::Ssrf(...))`，**不得**发出真实请求。

4. **重定向复核** —— `cargo test -p fetch redirect`：测试服务器先 `302` 跳到 `http://169.254.169.254/`，预期整条 fetch 失败（`Err`），证明重定向目标也被复核。

5. **大小/超时限制** —— `cargo test -p fetch limits`：超过 `max_response_bytes` 的响应返回 `Err(ResponseTooLarge)`；超时返回 `Err(Timeout)`。

6. **Lint** —— `cargo clippy -p fetch --all-targets -- -D warnings` 退出码 `0`。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

- **`internal/fetcher/handler.go`**：`PostResults` 处理 `POST /api/v1/fetch/results`，目前是**被动接收**外部 `paily-fetch` 推送的结果（记录 `source_fetch_histories` → `UpsertFromFetchResult` → 评估死源）。**抓取动作本身不在 gnosis 里，因此没有任何 SSRF 逻辑**。Rust 侧把抓取收进 `gnosis-worker`（方案 §2 的 `FW` 节点），防护必须新建。
- 方案 §16.9 明确把「Fetch 防 SSRF」列为安全要求，且 Phlogo 的 `Source.Content` 就是订阅 URL，是 SSRF 攻击面，所以 `crates/fetch` 是这一防护的唯一合理落点。

### CI 提示 / CI notes（仅描述，不落地 YAML）

- 单独 job：`cargo test -p fetch`（纯单元 + 本地 mock 服务器，不需要外网，应快速且稳定）。
- 由于 SSRF 单测涉及「不应真的打外网」，CI 中该 job 应**禁用网络**或仅允许 loopback/mock，避免误发请求；可加 `cargo clippy -p fetch -- -D warnings`。
- 不要把这个 job 和需要 ephemeral Postgres 的集成测试（`p2-04`）混在一起，避免互相拖慢。
- 参见 [`CONVENTIONS.md`](CONVENTIONS.md) §5 的 `cargo test --workspace`、`cargo clippy` 种子。

### 风险 / Risks

- **DNS rebinding**：仅校验初始解析结果不够，攻击者可让首次解析为公网、连接时变内网。需在连接建立时重新解析并校验，或一次性解析全部候选 IP 且全部通过才继续。
- **IPv6 细节**：`::ffff:0:0/96`（IPv4-mapped）、`fe80::/10`、ULA `fc00::/7` 都需覆盖，否则存在绕过。
- **云元数据别名**：GCP 的 `metadata.google.internal`、Azure `192.0.0.170` 等，必须解析后再校验，不能只匹配字面 `169.254.169.254`。
- **巨响应/慢连接 DoS**：必须**流式**截断到 `max_response_bytes` 且设置总超时，否则大响应仍可能占满内存。
- **P0 未过前不锁定依赖版本**：`reqwest` TLS backend（rustls vs native）选择影响二进制分发与许可证，参见 [`CONVENTIONS.md`](CONVENTIONS.md) §2.11。
