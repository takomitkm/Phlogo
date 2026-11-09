## 3-02 MiaoSpeed WebSocket Adapter · MiaoSpeed WebSocket Adapter

### 目标 / Goal

在 `crates/checker-miaospeed` 实现方案 §8 的 `Checker` trait，通过 **WebSocket** 与独立 MiaoSpeed 二进制通信，
把 Phlogo 当前的「HTTP 拉取/回调」测活模型（`internal/checker/handler.go` `GetNodes` + `PostResults`）重写为**主动提交/收集**模型。

```rust
// 方案 §8 的 Core 抽象接口（不是 MiaoSpeed 原生 API；Adapter 负责映射）
pub trait Checker {
    async fn submit(&self, job: CheckJob) -> Result<CheckJobId>;
    async fn collect(&self, job_id: &CheckJobId) -> Result<Vec<CheckResult>>;
    async fn status(&self) -> Result<CheckerStatus>;
}
```

要点：

1. **握手与认证**：建立 WebSocket 连接，完成 auth 签名 / challenge 握手。
2. **vendor + node payload**：把内部 `NormalizedNode`（`node_payloads`，见 `CONVENTIONS.md` §2.3 / §7）转换为 MiaoSpeed vendor 节点负载，并保留「vendor id ↔ 内部 `node_id`」映射。
3. **单节点与批量提交**：`submit` 既可单节点也可批量；`collect` 取回结果。
4. **可测**：提供 **MiaoSpeed stub（mock WS server）**，Adapter 在其上做单元测试。

### 范围 / Scope

- 新建：`crates/checker-miaospeed`（Adapter、trait、stub、vendor 类型转换）。
- 不在此步：结果解析/单位规范化/Node ID 映射（p3-03）、持久化（p3-04）、评分与缓存重建（P4/P5）。
- **不做**：链接 MiaoSpeed Go 代码、复刻其代理协议栈；进程边界只走 WebSocket（方案 §1 原则 3、§3.1）。

### 前置 / Prerequisites

- 已完成 **P0-02 spike**：固定版本 MiaoSpeed 已启动，Rust WebSocket 握手、auth、提交节点、读取结果跑通（方案 §14 P0 #2、§8.2）。
  > ⚠️ **本页所有具体帧格式（握手帧、auth 挑战、vendor 节点结构、结果帧字段、Matrix/测试名）均为占位/预期形态，
  > 真实字段以 P0-02 固定版本协议回填。方案 §8.1 明确要求：**Core 不应硬编码未经验证的 Matrix 名称**——preset 名只作为配置项，不在代码中写死未验证的测试名。
- p3-01 已完成：`CheckJob`（`run_id` / `checker_tag` / `node_ids` / `presets`）已定义。
- `node_payloads` / `NormalizedNode` 在 P2（`03-p2-fetch-parse/`）可用，提供节点的完整协议配置。

### 步骤 / Steps

#### Step 1 — 依赖

```toml
# crates/checker-miaospeed/Cargo.toml
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"      # 版本以 P0-02 验证为准
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["serde"] }
async-trait = "0.1"
thiserror = "1"
# 测试用 stub
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }  # 仅测试侧起 mock server
```

#### Step 2 — 定义 `CheckJob` / `CheckJobId` / `CheckResult`（Core 侧，非 MiaoSpeed 原生）

```rust
// crates/checker-miaospeed/src/types.rs
use uuid::Uuid;

/// Adapter 内部任务句柄；submit 返回，collect 使用
#[derive(Clone, Debug)]
pub struct CheckJobId(pub String); // MiaoSpeed 侧任务标识（具体类型以 P0-02 为准）

/// 一次提交任务：来自 p3-01 的 CheckJob，但拆出纯数据
pub struct SubmitRequest {
    pub run_id: Uuid,            // check_runs.id，幂等键
    pub checker_tag: String,
    pub presets: Vec<String>,    // 仅作为配置透传，不硬编码未验证测试名
    /// vendor 节点负载 + 内部 node_id 的配对，保证结果可映射回 node_id（见 p3-03）
    pub nodes: Vec<VendorNode>,
}

/// 转换自 NormalizedNode + node_payloads；vendor_id 由 Adapter 生成并维护映射
pub struct VendorNode {
    pub internal_node_id: Uuid,
    pub vendor_id: String,       // 提交给 MiaoSpeed 的节点标识
    pub payload: serde_json::Value, // vendor 节点负载（P0-02 验证字段后填充）
}

/// 收集到的结果；字段对齐 Phlogo CheckResult（见 internal/checker/handler.go:104）
pub struct CheckResult {
    pub internal_node_id: Uuid,  // 由 vendor_id 映射回（p3-03）
    pub initial: InitialResult,  // latency_ms；-1 = dead（沿用 Phlogo 约定）
    pub deep: Option<DeepResult>,
}

pub struct InitialResult { pub latency_ms: i32 } // -1 = dead

pub struct DeepResult {
    pub avg_latency_ms: i32,
    pub jitter_ms: i32,
    pub speed_kbps: i32,  // 0 = 未测（Phlogo NodeDeepCheck.SpeedKbps 约定）
    pub region: Option<String>,
    pub streaming: serde_json::Value, // {"netflix":true,...}
    pub metadata: serde_json::Value,
}

pub struct CheckerStatus { /* MiaoSpeed 在线/队列深度等，字段以 P0-02 为准 */ }
```

#### Step 3 — Adapter 骨架（trait 实现）

```rust
// crates/checker-miaospeed/src/lib.rs
use async_trait::async_trait;

pub struct MiaoSpeedAdapter {
    ws_url: String,
    secret: String, // MiaoSpeed auth secret；来自配置，不落日志（CONVENTIONS §2.11 #8）
    // vendor_id -> internal_node_id 的会话内映射，配合 submit/collect 做结果回映射
    id_map: std::sync::Mutex<std::collections::HashMap<String, uuid::Uuid>>,
}

#[async_trait]
impl Checker for MiaoSpeedAdapter {
    async fn submit(&self, job: SubmitRequest) -> Result<CheckJobId, CheckerError> {
        let mut ws = self.connect_and_auth().await?; // Step 4
        // 把每个内部节点映射为 vendor 节点并登记 id_map（见 p3-03 映射）
        let (frames, map) = self.build_submit_frames(&job)?;
        for f in frames { ws.send(f).await?; }
        let job_id = self.read_submit_ack(&mut ws).await?;
        *self.id_map.lock().unwrap() = map;
        Ok(job_id)
    }

    async fn collect(&self, job_id: &CheckJobId) -> Result<Vec<CheckResult>, CheckerError> {
        let mut ws = self.connect_and_auth().await?;
        ws.send(self.build_collect_frame(job_id)).await?;
        let raw = self.read_results(&mut ws).await?;
        // 此处只做帧解析；单位规范化与 node_id 映射在 p3-03
        Ok(self.parse_results(raw, &self.id_map.lock().unwrap()))
    }

    async fn status(&self) -> Result<CheckerStatus, CheckerError> {
        let mut ws = self.connect_and_auth().await?;
        ws.send(self.build_status_frame()).await?;
        self.read_status(&mut ws).await
    }
}
```

#### Step 4 — 握手与 auth 签名 / challenge

> 下列帧名/算法为**预期骨架**，切实体以 P0-02 固定版本协议回填。Core 侧须保证：
> - 握手中完成 auth（如 HMAC 签名 / nonce challenge）；
> - 失败必须返回 `CheckerError::Auth`，不静默继续；
> - `secret` 仅用于签名，**不输出到日志/错误回显**（CONVENTIONS §2.11 #8）。

```rust
impl MiaoSpeedAdapter {
    async fn connect_and_auth(&self) -> Result<WsStream, CheckerError> {
        let (mut ws, _) = tokio_tungstenite::connect_async(&self.ws_url).await
            .map_err(|e| CheckerError::Connect(e.to_string()))?;
        // 1) 接收服务端 challenge（nonce）
        let nonce = self.read_challenge(&mut ws).await?;
        // 2) 用 secret 对 nonce 签名并回传
        let sig = hmac_sign(&self.secret, &nonce); // 算法以 P0-02 为准
        ws.send(Frame::auth_response(sig)).await?;
        // 3) 校验服务端 OK
        self.expect_auth_ok(&mut ws).await?;
        Ok(ws)
    }
}
```

#### Step 5 — vendor + node payload（批量提交）

```rust
impl MiaoSpeedAdapter {
    /// 把 SubmitRequest.nodes 转为 MiaoSpeed 提交帧，并建立 vendor_id -> node_id 映射
    fn build_submit_frames(&self, job: &SubmitRequest)
        -> Result<(Vec<Frame>, HashMap<String, Uuid>), CheckerError>
    {
        let mut map = HashMap::new();
        // 单节点与批量都走同一帧序列；批量即在一次 submit 内包含多个 vendor 节点
        let mut frames = Vec::new();
        for n in &job.nodes {
            // vendor_id 由 Adapter 生成（如内部 node_id 的 hex 或短随机串），不依赖未验证外部命名
            let vendor_id = n.vendor_id.clone();
            map.insert(vendor_id.clone(), n.internal_node_id);
            frames.push(Frame::submit_node(&vendor_id, &n.payload, &job.presets));
        }
        frames.push(Frame::submit_end(&job.run_id.to_string()));
        Ok((frames, map))
    }
}
```

#### Step 6 — MiaoSpeed stub（mock WS server，供单元测试）

测试用 mock server 监听 `ws://127.0.0.1:<port>`，按固定协议 echo/返回 canned 结果。
**stub 必须显式声明其为「按 P0-02 验证后的协议的最小实现」**，协议细节变更时同步更新 stub 与适配器断言。

```rust
// crates/checker-miaospeed/tests/support/mock_miaospeed.rs
/// 最小 MiaoSpeed 行为模拟：接收 submit 帧 -> 回 ack + 任务 id -> 收到 collect -> 返回预设结果
pub struct MockMiaoSpeed { pub addr: String }
impl MockMiaoSpeed {
    pub async fn start(results: Vec<MockResult>) -> Self {
        // 用 tokio-tungstenite server 监听，按 P0-02 协议解析并提交 canned results
        // 协议帧名/字段以 P0-02 回填；此处仅描述行为契约
        todo!("见 p3-02 Step 6：按 P0-02 固定协议实现 mock 的 submit/ack/collect 流程")
    }
}
```

### 验收 / Acceptance

1. **连接 + 认证**：Adapter 能连上 MiaoSpeed stub 并完成 auth（包含错误签名应返回 `CheckerError::Auth`）。
   ```bash
   cargo test -p checker-miaospeed --lib ws_adapter::tests::handshake_and_auth
   ```
   预期：pass（exit 0）；错误签名用例断言返回 auth 错误且不发送后续帧。

2. **单节点 + 批量提交并取回**：用 `MockMiaoSpeed` 提交 1 个与 N 个节点，断言 `collect` 返回数量与提交一致。
   ```bash
   cargo test -p checker-miaospeed --lib ws_adapter::tests::submit_single_and_batch
   ```
   预期：单节点用例 `results.len() == 1`；批量用例 `results.len() == N`。

3. **vendor_id → node_id 映射闭环**（与 p3-03 衔接）：stub 用 vendor_id 回传结果，Adapter 能还原 `internal_node_id`。
   ```bash
   cargo test -p checker-miaospeed --lib ws_adapter::tests::vendor_id_maps_back_to_node_id
   ```
   预期：每个返回 `CheckResult.internal_node_id` 与提交节点一致。

4. **status 可用**：
   ```bash
   cargo test -p checker-miaospeed --lib ws_adapter::tests::status_returns
   ```
   预期：pass；返回结构非错误。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo | Rust（本步骤） |
|---|---|
| `internal/checker/handler.go` `GetNodes`（`:60`）pull 全量节点 `{hash,id,protocol,raw,server}` 给外部 checker | 反向：Adapter `submit` 主动把 `VendorNode`（来自 `node_payloads`/`NormalizedNode`）推给 MiaoSpeed |
| `internal/checker/handler.go` `PostResults`（`:103`）被动接收 `CheckResultsRequest{results:CheckResult[]}` | `collect` 主动取回 `Vec<CheckResult>` |
| `CheckResult{ node_id, initial:{latency_ms}, deep?:{...} }`（由 `api.CheckResult` 生成） | `CheckResult{ internal_node_id, initial, deep }`（`types.rs`，字段对齐 `internal/checker/handler.go:104`） |
| `middleware.CheckerTagFromCtx` 注入 `checker_tag` | `SubmitRequest.checker_tag` 随任务携带（与 p3-01 `CheckJob` 一致） |
| `internal/checker/port.go` `ScoringPort` / `CacheRebuildPort`（结果处理后触发） | 本步骤不触发；结果处理在 p3-03/p3-04，评分/缓存重建在 P4/P5 |

### CI 提示 / CI notes

> 仅描述未来 CI 命令，**不落地 YAML**。

- **单元测试**（含 mock WS server）：`cargo test -p checker-miaospeed --lib`（stub 在 `tests/support/` 或 `#[cfg(test)]` 模块内，不依赖真实 MiaoSpeed）。
- **可选集成**：CI 可起固定版本 MiaoSpeed 容器跑 `cargo test -p checker-miaospeed --test live_ws`（默认关闭，避免供应链/许可耦合）。
- **lint/build**：`cargo build --workspace --all-targets`、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`（CONVENTIONS §5）。
- **协议漂移告警**：P0-02 冻结的 `MIAOSPEED_PROTOCOL.md`（方案 §18）变更时，mock stub 与适配器测试必须同步更新，CI 应 fail-fast。

### 风险 / Risks

- **协议未冻结（最高风险）**：握手/auth/vendor payload/结果帧字段以 **P0-02 固定版本**为准；本页所有帧名均为占位，未回填前不得当作事实。方案 §8.1 明确禁止硬编码未验证 Matrix 名称——`presets` 只作配置透传。
- **vendor_id 映射丢失**：`id_map` 是会话内状态；若 submit 与 collect 不在同一连接/进程，需把映射持久化或把内部 `node_id` 编码进 vendor payload（p3-03 决定方案）。映射错会直接导致「结果错位到错误 node_id」，由 p3-03 测试兜底。
- **连接生命周期**：长批量任务可能跨重连；disconnect/reconnect 的语义（是否需重 submit 还是仅 re-collect）以 P0-02 为准，p3-03 处理迟到/重复。
- **secret 泄露面**：`secret` 不得进日志/错误（CONVENTIONS §2.11 #8）；mock 不得把真实 secret 提交进仓库。
- **许可证**：MiaoSpeed 为 AGPL，进程隔离非豁免（CONVENTIONS §2.11）；Adapter 不链接其代码即可。
