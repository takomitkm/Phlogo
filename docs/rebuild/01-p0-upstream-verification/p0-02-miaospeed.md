## 1-02 MiaoSpeed 固定版本 WebSocket 接入验证 · Verify Fixed-Version MiaoSpeed WebSocket Integration

> 对应方案 §3.1（MiaoSpeed 使用边界）、§8（接口边界）、CONVENTIONS §2.7 / §4 差异项 2。
> 本页是 **P0 spike**：用一个最小 Rust WebSocket 客户端驱动固定版本 MiaoSpeed 二进制，
> 验证协议、认证、提交与结果读取；详细协议细节（消息 schema、Matrix 名、错误码）最终要沉淀到 `docs/MIAOSPEED_PROTOCOL.md`（方案 §18）。

### 目标 / Goal

- 启动一个**固定版本**的 MiaoSpeed Go 二进制（独立进程，不被 Core 链接）。
- 用最小 Rust WebSocket 客户端完成：握手 → 认证（challenge/signature）→ 提交节点 → 读取结果。
- 记录协议的认证签名/challenge、Vendor 与 Node Payload 格式、Matrix 名称、错误码，以及版本兼容策略。
- 明确：**Core 不得硬编码未经验证的 Matrix 名称**；所有协议细节以固定版本源码为准（方案 §8.1 末句）。

### 范围 / Scope

- **新建**：scratch crate `spikes/miaospeed-ws-client`（仅验证，不进生产 workspace）。
- **验证内容**：WebSocket 连接、auth 流程、单节点与批量节点提交、结果收集、断连/重连基本行为。
- **不做**：不实现完整 `checker-miaospeed` Adapter trait（方案 §8 的 `submit`/`collect`/`status` 映射到 P3）；
  不链接 MiaoSpeed Go 代码；不实现 JS 脚本测试沙箱。
- **协议细节**：消息 schema、字段名、Matrix 名、错误码均标 `TODO(verify-against-pinned-MiaoSpeed)`，回填固定版本真实值。

### 前置 / Prerequisites

- Go 工具链（构建/运行固定版本 MiaoSpeed）；具体版本待定（CI 需固定）。
- MiaoSpeed 仓库：<https://github.com/AirportR/miaospeed>（方案 §20），固定为 `<PIN MiaoSpeed version>`。
- Rust 工具链 + WebSocket 客户端 crate（如 `tokio-tungstenite`）；`serde` / `serde_json`。
- 已读 `internal/checker/handler.go`：理解现有 HTTP 测活契约（`POST /api/v1/check/nodes`、`POST /api/v1/check/results`、
  `CheckResult{ node_id, initial:{latency_ms}, deep?:{avg_latency_ms,jitter_ms,speed_kbps,region,streaming,metadata} }`，
  以及 per-tag 串行化、四阶段处理）。Rust 侧将用 WebSocket Adapter 替代这条 HTTP 回调链。
- 已读方案 §8（测试预设 `quick`/`quality`/`advanced`/`speed` 的语义）。

### 步骤 / Steps

**1. 固定并构建 MiaoSpeed 二进制**

```bash
# 固定版本（写入 docs/THIRD_PARTY.md 的 <PIN MiaoSpeed version>）
git clone https://github.com/AirportR/miaospeed
cd miaospeed
git checkout <PIN MiaoSpeed version>
go build -o miaospeed .
./miaospeed --version   # 记录实际版本号
sha256sum miaospeed     # 记录二进制哈希（供应链基线，见 p0-05）
```

**2. 启动 MiaoSpeed（独立进程）**

```bash
# 端口/认证参数以固定版本实际配置为准；以下为示意
./miaospeed --addr 127.0.0.1:8080 --secret "<TEST_SECRET>" &
# 记录其 WebSocket 端点，例如 ws://127.0.0.1:8080/ws （真实路径以固定版本为准）
```

**3. 建立 scratch Rust WS 客户端**

```toml
# spikes/miaospeed-ws-client/Cargo.toml
[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = "0.24"
futures-util = "0.3"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
anyhow = "1"
```

**4. 最小 spike：握手 + 认证 + 提交 + 收集（骨架）**

```rust
// spikes/miaospeed-ws-client/src/main.rs
// 全部协议字段（auth challenge/signature、vendor、node payload、matrix、error code）
// 以 <PIN MiaoSpeed version> 源码为准，下方仅给出流程骨架与 TODO 标注。
use futures_util::{SinkExt, StreamExt};
use serde_json::json;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let url = "ws://127.0.0.1:8080/ws"; // TODO: 真实 WS 端点（固定版本）
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await?;

    // (1) Handshake + Auth challenge
    // TODO(verify-against-pinned-MiaoSpeed): 读取 server challenge，用 secret 计算签名并回传。
    //    参考 internal/checker/handler.go 的 CheckerTagMiddleware：Core 侧未来用 checker secret 校验；
    //    此处 MiaoSpeed 侧是「Core 作为客户端」反向认证，方向相反，细节以固定版本为准。
    let auth = json!({ "type": "auth", "token": "<TEST_SECRET>" }); // 占位，回填真实签名
    ws.send(tokio_tungstenite::tungstenite::Message::Text(auth.to_string())).await?;

    // (2) 读取 auth 结果
    if let Some(Ok(msg)) = ws.next().await {
        println!("auth resp: {:?}", msg); // 期望收到已认证确认
    }

    // (3) 提交节点（单节点 + 批量均要验证）
    // TODO: 真实 vendor + node payload 结构（固定版本）。下为示意字段。
    let submit = json!({
        "type": "test",
        "matrix": "quick",                  // 见方案 §8.1；真实 Matrix 名以固定版本为准，禁止硬编码未验证名
        "nodes": [ { "id": "node-1", "type": "vmess", "server": "1.2.3.4", "port": 443, "uuid": "..." } ]
    });
    ws.send(tokio_tungstenite::tungstenite::Message::Text(submit.to_string())).await?;

    // (4) 读取结果，并映射回内部 node_id
    while let Some(Ok(msg)) = ws.next().await {
        println!("result: {:?}", msg);
        // TODO: 解析结果字段（latency/speed/jitter/region/streaming/error code），
        //       按 checker_tag 与 node_id 映射回 Core 内部 id（方案 §8.2）。
        break; // spike 仅取首条验证链路
    }
    Ok(())
}
```

**5. 覆盖方案 §8.2 的必测项并记录**

逐一验证并写入 `docs/MIAOSPEED_PROTOCOL.md` 草稿：

- WebSocket 握手与认证（challenge/signature）。
- Vendor 与 Node Payload 格式（单节点 + 批量）。
- 结果如何映射回内部 `node_id`（避免晚到结果覆盖更新状态快照，CONVENTIONS §2.7 / 方案 §6.4）。
- 部分成功、超时、断连、重连、重复结果的行为。
- Matrix 字段、单位、空值、错误码。
- MiaoSpeed 服务端版本 ↔ Core Adapter 的兼容策略（如版本协商/拒绝）。

### 验收 / Acceptance

可机器校验（在 MiaoSpeed 已启动前提下）：

```bash
cd spikes/miaospeed-ws-client
cargo build
echo "exit=$?"   # 期望 0

cargo run --quiet
echo "exit=$?"   # 期望 0；stdout 依次出现 auth resp 与 result
```

- spike **连接**固定版本 MiaoSpeed、**认证**成功、**提交**节点（单+批量）、**接收**到结果。
- 记录并归档：`auth challenge/signature` 流程、`vendor + node payload` 格式、Matrix 名称
  （`quick`/`quality`/`advanced`/`speed` 与固定版本真实名映射）、错误码、版本兼容策略。
- `docs/MIAOSPEED_PROTOCOL.md` 草稿存在，且所有协议字段标注其来源为 `<PIN MiaoSpeed version>`。
- 明确写明：**Core 不硬编码未经验证的 Matrix 名称**（以配置文件/枚举 + 固定版本校验表驱动）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo 现状（Go） | 重构目标（Rust） | 本页动作 |
|---|---|---|
| `internal/checker/handler.go`：`POST /api/v1/check/nodes`（拉全部节点给 checker）、`POST /api/v1/check/results`（`CheckResultsRequest{results:[]}`） | Core Worker 经 MiaoSpeed **WebSocket Adapter** 测活，业务层只依赖 `Checker` trait（`submit`/`collect`/`status`，方案 §8） | 用 WS 客户端替代 HTTP 回调；结果仍需能映射回 `node_id`（handler.go 的 `node_id` 字段） |
| `checker_runs` + per-tag mutex 串行化 + 四阶段（事务写初筛/复筛 → 批量存活判定 → 异步评分 → 死源评估+缓存重建） | Apalis Check Job（P3）+ WS Adapter 结果映射 | 本页只验证「能拿到结果」，四阶段在 P3 重写 |
| `internal/checker/handler.go` 的 `initial.latency_ms`、`deep.{avg_latency_ms,jitter_ms,speed_kbps,region,streaming,metadata}` | `node_check_results`(metrics) / `node_status` / `node_scores`（方案 §5.1） | 验证结果字段能回填到上述结构（单位/空值需记录） |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **集成 job**：CI 中构建固定版本 MiaoSpeed 二进制（缓存 + 记录 SHA-256），作为 service container / sidecar 启动，再跑 Rust WS 客户端 spike。
- **协议回归**：把本页记录的 `MIAOSPEED_PROTOCOL.md` 作为契约；未来 `checker-miaospeed` 改动后，新增/修改 Matrix 或字段须回写该协议文档并加回归测试。
- **版本门禁**：CI 读取 `docs/THIRD_PARTY.md` 的 `<PIN MiaoSpeed version>`，禁止 floating 版本；二进制 SHA-256 进入 SBOM（见 `p0-05`）。
- **契约稳定检查**：在 CI 中加一步「Matrix 名称白名单校验」——任何未在固定版本协议文档登记的 Matrix 名都会导致构建/测试失败，落实「不硬编码未验证 Matrix 名」。

### 风险 / Risks

- **协议未文档化/漂移**：MiaoSpeed 的 WS 协议细节以固定版本源码为准；过早硬编码会导致升级即崩。所有字段必须回填真实值后再进入 P3。
- **认证方向易混淆**：现有 `handler.go` 是「checker 回调 Core」，而本页是「Core 作为客户端连 MiaoSpeed」，方向相反；勿把 `CheckerTagMiddleware` 的 secret 逻辑直接套用，需对照固定版本 auth 流程。
- **结果迟到/重复**：多节点批量提交下，需验证晚到结果不覆盖更新状态快照（方案 §6.4）；本页要在 spike 中至少观察到该行为并记录。
- **许可证义务**：MiaoSpeed 为 AGPL，其 Go 依赖（含 Mihomo，GPL）义务仍需履行；进程隔离不是许可证豁免（方案 §16、CONVENTIONS §2.11）。
- **JS 脚本测试**：方案 §8.2 要求覆盖 JS 测试输入/输出与安全限制；本页可仅记录「存在该能力」，完整验证放 P3。
