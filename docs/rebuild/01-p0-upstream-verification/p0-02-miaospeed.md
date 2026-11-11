## 1-02 MiaoSpeed 固定版本 WebSocket 接入验证 · Verify Fixed-Version MiaoSpeed WebSocket Integration

> 对应方案 §3.1（MiaoSpeed 使用边界）、§8（接口边界）、CONVENTIONS §2.7 / §4 差异项 2。
> 本页是 **P0 spike**：用一个最小 Rust WebSocket 客户端驱动**固定版本** MiaoSpeed 二进制，
> 已端到端验证「握手 → 认证 → 提交节点 → 读取结果」全链路，并**冻结协议规格**，供 P3 的
> `checker-miaospeed` Adapter 直接照抄。协议细节最终沉淀到 `docs/MIAOSPEED_PROTOCOL.md`（方案 §18）与
> `docs/THIRD_PARTY.md`（固定版本 + 二进制哈希）。

### 目标 / Goal

- 固定并**可复现地构建** MiaoSpeed Go 二进制（独立进程，不被 Core 链接）。
- 用最小 Rust WebSocket 客户端完成：连接 → 认证（`Challenge` 签名）→ 提交节点 → 读取结果。
- **冻结**协议：认证算法、消息 schema、Vendor/Node Payload、Matrix 名称、结果帧与单位、错误行为、版本兼容策略。
- 明确：**Core 不得硬编码未经验证的 Matrix 名称**；所有协议细节以固定版本源码为准（方案 §8.1 末句）。

### 范围 / Scope

- **不新建生产代码**：spike 是 scratch crate（`spikes/miaospeed-ws-client`，仅验证，不进生产 workspace）。
  本页把可复用的协议实现要点（签名算法 + 序列化要求）写成**可直接移植**的规格。
- **验证内容**：WebSocket 连接、auth（Challenge 签名）流程、单节点提交、结果帧收集、**Vendor 签名 quirk**。
- **不做**：不实现完整 `checker-miaospeed` Adapter trait（`submit`/`collect`/`status` 映射到 P3）；
  不链接 MiaoSpeed Go 代码；不实现 JS 脚本测试沙箱；不在本页做混涌节点/速度/地理等全矩阵校准（P3）。
- **已落地事实**（下方步骤回填）：固定版本 = `4.7.9`（tag）；构建=自建；端到端已跑通；协议字段全部来自 4.7.9 源码 + 实测。

### 前置 / Prerequisites

- **MiaoSpeed 仓库**：<https://github.com/AirportR/miaospeed>，固定 **tag `4.7.9`**（方案 §20）。
- **Go 工具链**（自建二进制）：校机 `D:\yzd\magpie-go-toolchain-20261107\go\bin\go.exe`（`go1.26.8 windows/amd64`）。
  `go.mod` 声明 `go 1.25`；`CGO_ENABLED=0` 构建。
- **Go module 镜像**：`golang.org/x/*`、`google.golang.org/protobuf`、`go.yaml.in/yaml` 等**不在** cargo 用的 `rsproxy` 上，
  必须用能代理它们的 Go 代理：`GOPROXY=https://goproxy.cn,https://goproxy.io,https://rsproxy.cn,direct`（实测可用）。
- **Rust + WS 客户端**：校机 `cargo 1.85.1`（host `x86_64-pc-windows-gnu`）；spike 用 `tungstenite 0.21`（同步，`default-features=false, features=["handshake"]`）、
  `serde`/`serde_json`、`sha2`、`base64`。**注意**：`tungstenite` 传递依赖 `url` 会在新版拉入 `icu_*`（需 rustc 1.88），
  必须钉 `url = "=2.5.0"`（其 `idna 0.5` 不依赖 icu）才能在 1.85.1 上编译。
- **已读** `internal/checker/handler.go`：理解现有 HTTP 测活契约（`POST /api/v1/check/nodes`、`POST /api/v1/check/results`、
  `CheckResult{node_id, initial:{latency_ms}, deep?:{avg_latency_ms,jitter_ms,speed_kbps,region,streaming,metadata}}`，per-tag 串行化、四阶段）。
- **已读** 方案 §8（测试预设 `quick`/`quality`/`advanced`/`speed` 语义）。

### 步骤 / Steps

#### A. 固定版本与可复现构建（**结论：自建，不用 GitHub 预编译资产**）

1. **版本与资产基线（已核实）**

   | 项 | 值 |
   |---|---|
   | 目标源码版本 | **tag `4.7.9`**（release 页面 2026-11-08） |
   | GitHub 预编译 `miaospeed-windows-amd64-4.7.9.zip` | sha256 `685a8a584aa7724479c46626b28cc8344ff5bfb1b18643254275461535e1d159` |
   | 其内 `miaospeed-windows-amd64.exe` | sha256 `5b0ec1ad39723ae10c6a2cd6296561097335e680b5acc62a18ba0b7682ac9611`，`version 4.7.9`，`commit 0fafd30` |
   | **自建** `miaospeed-built.exe`（tag 4.7.9 源码） | sha256 `2c302b294a2cdfff5cabd4009b2148e446eadff0ebc3413397a011874146ab18`，size `87,692,288` |

   > **决策（重要）**：GitHub 预编译二进制的 `commit 0fafd30` **在公开仓库里查无此 SHA**（`api.github.com` 返回 422），
   > 即预编译资产**不可复现**。自建二进制端到端跑通（见 A.4 / E），因此 **PIN = 自建（源码 tag `4.7.9`）**，
   > 不把预编译资产作供应链基线。自建版本串为 `4.6.X`（源码默认常量）；若要显示 `4.7.9` 可用
   > `-ldflags "-X github.com/airportr/miaospeed/utils.VERSION=4.7.9"` 注入（不影响协议）。

2. **补齐构建所需的 embed 占位**（tag 源码**缺失**这些文件，只有 `.gitkeep`；不补齐 `go build` 直接失败）：

   ```
   # 1) 编译 TOKEN（签名盐，见下 D；二进制内嵌默认值即此）
   utils/embeded/BUILDTOKEN.key = MIAOKO4|580JxAo049R|GEnERAl|1X571R930|T0kEN   （含末尾换行，加载时 TrimSpace）
   # 2) 服务端 TLS 证书对（-mtls 时才用；CGO_ENABLED=0 也要嵌入，至少一对）
   preconfigs/embeded/tls/miaoko.crt + miaoko.key   （自签名；openssl 或 Go crypto/x509 生成）
   # 3) 已存在：preconfigs/embeded/ca-certificates.crt、engine/embeded/{predefined,default_geoip,default_ip}.js
   ```

3. **构建命令（校机）**：

   ```powershell
   $env:PATH = "D:\yzd\magpie-go-toolchain-20261107\go\bin;" + $env:PATH
   $env:GOPROXY="https://goproxy.cn,https://goproxy.io,https://rsproxy.cn,direct"; $env:GOSUMDB="off"
   $env:GOTOOLCHAIN="local"; $env:CGO_ENABLED="0"; $env:GOFLAGS="-mod=mod"
   Set-Location <src>\miaospeed-4.7.9
   go build -trimpath -o miaospeed-built.exe .
   # 供应链基线：
   # sha256 = 2c302b294a2cdfff5cabd4009b2148e446eadff0ebc3413397a011874146ab18
   ```

   > 另：`go.mod` 含 `replace github.com/metacubex/mihomo => github.com/AirportR/mihomo v1.19.33-x365-miu.2`（mihomo 的 fork），
   > 首次构建会下载大量代理协议依赖（`golang.org/x/*`、`google.golang.org/protobuf`、`metacubex/*` 等），需上述 Go 代理可达。

4. **已实测**：自建二进制在 `127.0.0.1:18082` 起 server，最小 Rust spike 完成 connect→auth→submit→读结果（见 E）。

#### B. 启动 MiaoSpeed（两种模式）

```bash
# 入站模式（推荐：worker 主动连 miaospeed；compose 内网可达即可）
miaospeed-built.exe server -bind 0.0.0.0:65432 -token "<CHECKER_SECRET>" -path / -verbose

# 反向模式（miaospeed 在 NAT 后、无公网 IP 时：它主动拨向 panel）
miaospeed-built.exe server -connect wss://panel.example.com/reverse -token "<CHECKER_SECRET>"
```

关键 flag（`server -help`，4.7.9 实测）：

| flag | 含义 |
|---|---|
| `-bind HOST:PORT` 或 unix socket | 监听地址；与 `-connect` 至少给一个 |
| `-token X` | 签名密钥（见 D 的 `hashMiaoSpeed`）；`-connect` 时必填 |
| `-path /` | WebSocket 路径；请求 `r.URL.Path` 必须**完全等于**此值，否则回 `invalid websocket path` 并断开 |
| `-mtls` | 启用内嵌自签名 TLS（此时对外是 `wss://`）；默认关闭 → 明文 `ws://` |
| `-allowip` | IP 白名单（默认 `0.0.0.0/0,::/0`） |
| `-whitelist` | bot id 白名单；空=允许所有；对应请求 `Basics.Invoker` |
| `-nospeed` / `-upload` | 拒绝全部下行测速 / 允许上行测速 |
| `-connect wss://...` | 反向拨号（可重复/逗号分隔多端点），走 `msr-v1` 握手（见 D.7） |

> 端点：`ws://HOST:PORT{path}`（默认 `ws://HOST:PORT/`）。`-mtls` 时 `wss://`，证书为内嵌自签（客户端需放宽校验或注入 CA）。

#### C. 最小 Rust WS 客户端（spike 已跑通）

```toml
# spikes/miaospeed-ws-client/Cargo.toml
[dependencies]
tungstenite = { version = "0.21", default-features = false, features = ["handshake"] } # 同步 WS，免 tokio
serde = { version = "1", features = ["derive"] }
serde_json = "1"
sha2 = "0.10"
base64 = "0.22"
url = "=2.5.0"   # 关键：钉旧版，避免 icu_* 需要 rustc 1.88
```

客户端核心（**可移植进 P3**）：

```rust
// (1) 构造 SlaveRequest（Go 字段名/声明顺序/内嵌展开，见 D.3）
// (2) 计算 Challenge：注意 D.5 的 Vendor bug —— 用 Vendor="" 的副本算签名
let mut for_hash = req.clone();
for_hash.Challenge = String::new();
for_hash.Vendor    = String::new();               // Clone() 丢 Vendor -> 签名恒为空
let unsigned = serde_json::to_string(&for_hash)?; // 必须与 Go jsoniter 逐字节一致
let sig = hash_miaospeed(token, &unsigned);       // D.4 算法
req.Challenge = sig;                              // req.Vendor 仍保留 "Local"/"Clash" 实际发送
// (3) ws 连接，send(Message::Text(serde_json::to_string(&req)?))
// (4) 循环 socket.read()：Progress 帧逐节点；Result 帧全量结果；按 ProxyInfo.Name 映射回 node_id
```

#### D. 协议冻结规格（**全部来自 4.7.9 源码 + 实测；P3 直接照此实现**）

**D.1 传输**：单 HTTP 端点，WebSocket 升级；无独立「握手/登录」消息——认证是**逐请求内联**的。

**D.2 认证**：客户端构造 `SlaveRequest`，算 `Challenge = hashMiaoSpeed(token, jsoniter.MarshalToString(req[Challenge=""]))` 填入后发送。
服务端解码 → `Challenge=""` → 重算比对，不等则回
`{"Error":"cannot verify the request, please check your token"}` 并断开。
> **关键**：服务端是对**解码后的结构体重新序列化**再比对（`VerifyRequest: req.Challenge == SignRequest(req)`），
> 不是用你发送的原始字节。所以客户端算 Challenge 时用的 JSON 必须等于「Go jsoniter 解码后再序列化的字节」。

**D.3 请求结构 `SlaveRequest`（ApiVersion=3，JSON 键=Go 字段名、声明顺序、内嵌展开、零值一律输出）**：

```json
{"Basics":{"ID":"spike-0001","Slave":"","SlaveName":"phlogo-spike","Invoker":"phlogo","Version":"0.1.0"},
 "Options":{"Filter":"","Matrices":[{"Type":"TEST_PING_RTT","Params":""},{"Type":"TEST_PING_CONN","Params":""}]},
 "Configs":{"STUNURL":"","DownloadURL":"","DownloadDuration":3,"DownloadThreading":1,"PingAverageOver":1,
   "PingAddress":"http://gstatic.com/generate_204","TaskRetry":3,"DNSServers":[],"TaskTimeout":5000,"Scripts":[],
   "ApiVersion":3,"UploadURL":"","UploadDuration":0,"UploadThreading":0},
 "Vendor":"Local","Nodes":[{"Name":"direct-gstatic","Payload":""}],"RandomSequence":"","Challenge":""}
```

- `Configs` 内嵌顺序（jsoniter 展开）：V1(`STUNURL,DownloadURL,DownloadDuration,DownloadThreading,PingAverageOver,PingAddress,TaskRetry,DNSServers,TaskTimeout,Scripts`) → `ApiVersion` → V3(`UploadURL,UploadDuration,UploadThreading`)。
- 空切片输出 `[]`（若 nil 则 `null`，需与解码语义一致）；**jsoniter 默认不转义 HTML**（仅 ASCII payload 安全；非 ASCII 节点名需对齐转义）。
- 用 **ApiVersion=3**：ApiV0/V1 有兼容 hack（`RandomSequence==""` 时签名把 `Nodes`/`Scripts` 清空），务必避开。

**D.4 签名算法 `hashMiaoSpeed(token, request)`（aws-v4 风格链式 SHA512）**：

```
buildTokens = [token] ++ split(TrimSpace(BUILDTOKEN_KEY), "|")
h = SHA512();  h.Write(request_bytes)
for t in buildTokens:
    if t == "": t = "SOME_TOKEN"          // 空段替换
    h.Write( t_bytes ++ h.currentDigest() )   // Go: hasher.Write(hasher.Sum([]byte(t)))
return base64URL_with_padding( h.final() )    // Go base64.URLEncoding（- _ 字母表，带 '='）
```
- 内嵌 `BUILDTOKEN_KEY`（从 4.7.9 二进制字符串表抽出，与 miaospeed.sh 默认一致）：`MIAOKO4|580JxAo049R|GEnERAl|1X571R930|T0kEN`。
  自建时二进制里就是此值；如需更换，改 `utils/embeded/BUILDTOKEN.key` 重新构建。

**D.5 致命 quirk：`Clone()` 丢 `Vendor` → 签名恒用 `"Vendor":""`**

- Go `interfaces.SlaveRequest.Clone()`（4.7.9 `interfaces/api_request.go`）**漏拷贝 `Vendor` 字段**，而 `SignRequest()` 对 `Clone()` 的结果签名。
- 因此：**算 Challenge 用 `Vendor=""` 的 JSON；实际发送用真实的 `"Vendor":"Local"/"Clash"`**。
- 实测：用 `Vendor="Local"` 算签名 → 服务端 `verify=false`；改 `Vendor=""` 算签名 → `verify=true`（见 E）。
- **P3 必须复刻此行为**；否则永远认证失败。用 `utils.SignRequest` 对真实结构体取签名可自动包含此 quirk（推荐 P3 的契约测试用 Go 交叉验证一次）。

**D.6 结果帧 `SlaveResponse`（实测）**：

```json
// Progress（逐节点；ApiVersion>=2 还会先发一条 Queuing 计数的初始 Progress）
{"ID":"<task-uuid>","MiaoSpeedVersion":"4.6.X","Error":"","Result":null,
 "Progress":{"Index":0,"Record":{"Grouping":"","ProxyInfo":{"Name":"direct-gstatic","Address":"127.0.0.1","Type":"Invalid"},
   "InvokeDuration":5001,"Matrices":[{"Type":"TEST_PING_RTT","Payload":"{\"Value\":0}"},{"Type":"TEST_PING_CONN","Payload":"{\"Value\":0}"}]},"Queuing":0}}
// Result（最终，一次性给出全部 Results）
{"ID":"<task-uuid>","MiaoSpeedVersion":"4.6.X","Error":"","Result":{"Request":{...原请求...},"Results":[<SlaveEntrySlot>...]},"Progress":null}
```
- `SlaveEntrySlot{Grouping, ProxyInfo{Name,Address,Type}, InvokeDuration(int64, 毫秒, 出错时 -1), Matrices:[{Type,Payload(JSON 字符串)}]}`。
- `ID` = MiaoSpeed 内部任务 uuid（**不是** `Basics.ID`）；`MiaoSpeedVersion` = 服务端版本串。
- **映射回内部 `node_id`**：靠 `ProxyInfo.Name`（= 提交时 `Nodes[].Name`），**不要**靠 `Index` 顺序猜测（并发/重试会乱序）。

**D.7 Matrix 名称（4.7.9 `interfaces/matrix.go`，**白名单**，禁硬编码未登记名）**：

```
连通/延迟: TEST_PING_RTT · TEST_PING_CONN · TEST_PING_MAX_RTT · TEST_PING_TOTAL_CONN · TEST_PING_TOTAL_RTT
           TEST_PING_SD_RTT · TEST_PING_SD_CONN · TEST_PING_PACKET_LOSS · TEST_HTTP_CODE · TEST_HIJACK_DETECTION
测速:      SPEED_AVERAGE · SPEED_MAX · SPEED_PER_SECOND · USPEED_AVERAGE · USPEED_MAX · USPEED_PER_SECOND
其它:      GEOIP_INBOUND · GEOIP_OUTBOUND · UDP_TYPE · TEST_SCRIPT · DEBUG_SLEEP · INVALID
```
- 方案 §8 的 `quick/quality/advanced/speed` 预设 → P3 映射为上述矩阵的**组合**（例如 quick=`TEST_PING_RTT`+`TEST_PING_CONN`），映射表进 P3。
- 各矩阵 `Payload` 结构（`interfaces/matrix_fields.go`）：`RTTPingDS/HTTPPingDS/MaxRTTDS`=`{"Value":uint16}`；
  `AverageSpeedDS/MaxSpeedDS`=`{"Value":uint64}`；`PerSecondSpeedDS`=`{"Max","Average","Speeds":[]uint64}`；
  `SDRTTDS/SDHTTPDS/PacketLossDS`=`{"Value":float64}`；`TotalRTTDS/TotalHTTPDS`=`{"Values":[]uint16}`；
  `HTTPStatusCodeDS`=`{"Values":[]int}`；`UDPTypeDS`=`{"Value":string}`；`Inbound/OutboundGeoIPDS`=MultiStacks。
  **单位/空值以 P3 真实节点校准**（本次 gstatic 不可达 → RTT/Conn 均 `0`，`InvokeDuration≈TaskTimeout`）。

**D.8 Vendor 与 Node.Payload**：`Vendor ∈ {"Local","Clash","Invalid"}`。`Local`=直连（`Payload` 忽略，`ProxyInfo` 为 `127.0.0.1/Invalid`）；
`Clash`=payload 为 mihomo 代理配置（真实节点测试用；payload 形态在 P3 用真实节点定稿）。

**D.9 反向模式 `msr-v1`（可选，部署在 NAT 后时）**：miaospeed 拨 `-connect` 后发
`ReverseHello{Type:"hello",Protocol:"msr-v1",Version,Nonce,Timestamp,Sign=hashMiaoSpeed(token,"reverse-hello|"+nonce+"|"+ts)}`；
panel 回 `ReverseAck{Type:"ack",Protocol,Sign=hashMiaoSpeed(token,hello.Sign),...}`；ack 后**复用同一 `SlaveRequest/Response` 协议**。
控制连接（`Role:"control"`）用 `ReverseScale/ReverseScaleAck` 调整连接池。

#### E. 端到端验证证据（自建 tag 4.7.9 二进制，校机实测）

- 服务端日志：`Receive Task, name=spike-0001 invoker=phlogo verify=true ... matrices=[{TEST_PING_RTT } {TEST_PING_CONN }] payload=1`（**verify=true**）。
- 客户端依次收到：初始 `Progress(Queuing=1)` → 单节点 `Progress`（`ProxyInfo{direct-gstatic,127.0.0.1,Invalid}`、`InvokeDuration=5001`、
  `TEST_PING_RTT=0`、`TEST_PING_CONN=0`）→ 最终 `Result`（含完整 `Results[]`）。
- 用 `utils.SignRequest("SPIKETOKEN", &req)`（真实包）得到的签名 = `fKdMhefuCZz2RmiaQxar5SLkAMnLd5i7D1_ZUYSHCpQykOT_smWDJ-pnhmTgSyOnJl7c97mR90bw6UIe4hMJ9Q==`，与 Rust spike 计算值**逐字节一致**。
- `Value=0` 系校机直连 `gstatic.com` 超时（`InvokeDuration≈TaskTimeout 5000ms`），属环境网络，不是协议问题。

### 验收 / Acceptance

- [x] 固定版本**自建**成功，记录 `sha256`（`2c302b29…6ab18`）与构建信息（tag `4.7.9`，Go `1.26.8`，`CGO_ENABLED=0`）。
- [x] 最小 Rust WS 客户端：**连接**成功、**认证**（Challenge 签名，含 Vendor=""/Vendor 实发 的 quirk）成功、**提交**节点成功、**接收**到 `Progress`+`Result` 帧。
- [x] 协议字段（认证算法、请求/结果 schema、Matrix 名单、Payload 形态、错误行为、反向握手）均已冻结并标注来源 `tag 4.7.9`。
- [ ] `docs/MIAOSPEED_PROTOCOL.md` 独立成档（从本页 D 抽出具名契约；P3 前完成）。
- [ ] `docs/THIRD_PARTY.md` 登记 MiaoSpeed 固定版本 + 自建二进制哈希（P0-05 统一维护，本页先给出值）。

### 与 fork 源对照 / Mapping to the fork source
> **对照基准**：Go 源码已于收口后从工作树移除；本节引用的 `internal/**`、`cmd/**` 文件与行号
> 以 **fork 根提交 `dda4e6f2`** 为准（`git show dda4e6f2:<path>` 可查任何文件全文）。

| Phlogo 现状（Go） | 重构目标（Rust） | 本页动作 |
|---|---|---|
| `internal/checker/handler.go`：`POST /api/v1/check/nodes`（拉全量给 checker）、`POST /api/v1/check/results`（`CheckResultsRequest{results:[]}`） | Core Worker 经 MiaoSpeed **WebSocket Adapter** 测活，业务层只依赖 `Checker` trait（`submit`/`collect`/`status`，方案 §8） | 用 WS 客户端替代 HTTP 回调；结果靠 `ProxyInfo.Name` 映射回 `node_id` |
| `checker_runs` + per-tag mutex 串行化 + 四阶段 | Apalis Check Job（P3）+ WS Adapter 结果映射 | 本页只验证「能拿到结果」，四阶段在 P3 重写 |
| `handler.go` `initial.latency_ms`、`deep.{avg_latency_ms,jitter_ms,speed_kbps,region,streaming,metadata}` | `node_check_results`/`node_status`/`node_scores`（方案 §5.1） | 结果 `InvokeDuration`(ms)、`Matrices[].Payload`(见 D.7) 回填；单位/空值 P3 校准 |

### CI 提示 / CI notes

> 仅描述将来 CI 对应命令，**不落地 YAML**。

- **集成 job**：CI **自建**固定版本 MiaoSpeed（tag `4.7.9`），缓存 + 记录 `sha256`，作为 service/sidecar 启动，再跑 Rust WS 客户端契约测试。
  - 构建需：补齐 `utils/embeded/BUILDTOKEN.key`、`preconfigs/embeded/tls/miaoko.{crt,key}`；`GOPROXY` 用 goproxy.cn 一类能代理 `golang.org/x/*` 的源。
- **协议回归**：以 `docs/MIAOSPEED_PROTOCOL.md`（本页 D 抽出）为契约；`checker-miaospeed` 改动后须回写协议并加回归测试。
- **版本门禁**：读 `docs/THIRD_PARTY.md` 的 PIN（tag `4.7.9`），禁止 floating；二进制 `sha256` 进 SBOM（p0-05）。
- **契约稳定**：加「Matrix 名白名单校验」与「签名 quirk 回归」（用 Go `utils.SignRequest` 交叉验证一条固定样例，防 Clone 行为变化时静默漂移）。

### 风险 / Risks

- **协议未文档化/漂移**：字段、Matrix 名、**Vendor 签名 quirk** 都以固定版本源码为准；升级即可能变，必须版本门禁 + 契约回归（尤其 quirk 与 jsoniter 转义）。
- **预编译资产不可复现**：GitHub 预编译 commit 不在公开仓库 → **只能自建**；构建链（Go 版本、代理、embed 占位）须固化进 CI，否则无法重建同哈希二进制。
- **jsoniter 逐字节一致**：Rust 序列化必须与 Go jsoniter 完全一致（字段名/顺序/内嵌展开/零值/`[]` vs `null`/HTML 转义）。P3 建议保留一个 Go 交叉校验器。
- **结果迟到/重复**：批量下 `Progress` 逐节点、`Result` 最终全量；映射靠 `ProxyInfo.Name`。晚到/重复结果不得覆盖更新状态（方案 §6.4）——在 P3/P4 落实。
- **许可证义务**：MiaoSpeed 为 AGPL，其 Go 依赖（含 Mihomo，GPL）义务仍需履行；进程隔离**不是**许可证豁免（方案 §16、CONVENTIONS §2.11）。自建/分发须附对应源码与许可证。
- **JS 脚本测试**：方案 §8.2 要求覆盖 JS 测试安全限制；本页仅记录「存在 `TEST_SCRIPT`/`script` 能力」，完整验证放 P3。
