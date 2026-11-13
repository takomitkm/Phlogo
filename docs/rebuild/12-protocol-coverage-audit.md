# 最新 mihomo / sing-box 协议支持度审计与修复（2026-11-13）

> 触发：校机上 mihomo 正在运行（v1.19.32），需要核对本实现对**最新 mihomo 与 sing-box**
> 的协议支持情况。审计对象：真实订阅样本 + 上游协议清单 + 上游 Go 实现的凭据口径。

## 1. 环境事实

| 项 | 值 |
| --- | --- |
| 校机 mihomo | **v1.19.32**（`D:\yzd\proxy\run2\mihomo-windows-amd64-v3.exe`，go1.26.8；另有 run1 的 go1.25.14 构建） |
| 监听 | mixed `127.0.0.1:7890`、controller `127.0.0.1:9090` |
| 真实订阅样本 | `D:\yzd\proxy\run2\config.yaml`、`run2\sub-new.yaml`、`agy\config.yaml`、`RVQTAZTiGS3p.yaml`（共 4 份，内容为同一批 52 节点） |
| 样本协议分布 | **vless 36 + hysteria2 16**（含 reality / vision-flow / ws 三种 vless 变体） |
| 校机 sing-box | **未安装**（协议清单取自 sing-box v1.14.0 官方文档） |

## 2. 审计结果：解析层漏了一半协议

### 2.1 本实现的凭据提取（修复前）

`crates/vproxy-adapter/src/lib.rs::cred_for` 只认 5 类：

| 协议 | 修复前 | 说明 |
| --- | --- | --- |
| vmess / vless | ✅ `uuid` | |
| trojan | ✅ `password` | |
| ss / shadowsocks | ✅ `password` **或 `cipher`** | `cipher` 是加密算法，**不该当凭据**（会污染身份） |
| hysteria2 / anytls / hysteria / tuic / snell / wireguard / socks5 / http / mieru / sudoku / ssr | ❌ **全部丢弃** | 解析器直接返回 `None`，节点**静默消失** |

### 2.2 真实样本影响（修复前）

```
156 个真实节点 → kept 108 / dropped 48  →  keep_rate 69.2%
（丢掉的 48 个全是 hysteria2）
```

### 2.3 上游 Go 版的凭据口径（`internal/node/upsert.go::nodeAuth`）

```go
case "vmess", "vless":                          → uuid
case "trojan", "anytls", "hysteria", "hysteria2": → password
case "tuic":                                     → uuid + ":" + password（v5）；仅 password（v4）
case "ss", "ssr":                                → password
case "wireguard":                                → private-key
case "socks5", "http":                           → username + ":" + password
case "mieru":                                    → username + ":" + password
case "sudoku":                                   → key
case "snell":                                    → psk
```

**关键发现**：本实现的 **身份层（`domain::fingerprint::node_auth`）本来就完整覆盖上游这 11 类**——
`Credential` 枚举里已有 `Tuic{uuid,password}` / `PrivateKey` / `UsernamePassword` / `Psk` / `Key`，
fingerprint 的 `node_auth` 也逐协议对齐了上游。**缺的只是解析层的接线**（adapter 没把字段填进这些变体）。

### 2.4 与最新 mihomo / sing-box 的差距

| 面 | mihomo 1.19 可解析 | sing-box 1.14 outbound | 本实现（修复前） |
| --- | --- | --- | --- |
| 代理协议 | http/https/socks4/socks5/shadowsocks/vmess/vless/trojan/snell/tuic/hysteria/hysteria2/anytls/ssh/mieru | shadowsocks/vmess/vless/trojan/wireguard/hysteria/hysteria2/tuic/anytls/shadowtls/snell/ssh/socks/http/tor/naive… | 仅 vmess/vless/trojan/ss |
| 缺口 | — | — | **11 类协议** |

## 3. 修复

### 3.1 解析层（`crates/vproxy-adapter/src/lib.rs`）

`cred_for` 按上游 `nodeAuth` **逐协议**补齐（Clash YAML 与 sing-box JSON 字段名对每个协议都一致，
故同一函数通用）：

| 协议 | Credential 变体 | 取字段 |
| --- | --- | --- |
| vmess / vless | `Uuid` | `uuid` |
| trojan / anytls / hysteria / hysteria2 | `Password` | `password` |
| ss / shadowsocks / ssr | `Password` | `password`（**不再回退 `cipher`**——算法不是凭据） |
| tuic | `Tuic{uuid,password}` | `uuid` + `password`（v4 只有 password） |
| wireguard | `PrivateKey` | `private-key` / `private_key` |
| snell | `Psk` | `psk` |
| socks5 / socks / http / https / mieru | `UsernamePassword` | `username` + `password` |
| sudoku | `Key` | `key` |

### 3.2 发射层（`crates/distribution/src/output.rs`）

**问题**：sing-box outbound 原先由硬编码的 `singbox_outbound_from(protocol, server, port, cred, sni, ws_path)`
生成——只认 vmess/vless/trojan，**hysteria2 节点会丢掉 password**，且 `ws` 之外的其他传输层细节全丢。

**修复**：新增 `singbox_outbound_from_clash(payload, name)`——从完整 Clash payload 转换，
逐协议映射字段：

| Clash | sing-box |
| --- | --- |
| `server` / `port` | `server` / `server_port` |
| `name` | `tag` |
| `sni` / `alpn` / `skip-cert-verify` / `reality-opts` | `tls{enabled, server_name, alpn, insecure, reality{public_key, short_id}}` |
| `network` + `ws-opts` / `grpc-opts` / `http-opts` | `transport{type: ws\|grpc\|http, path, headers, service_name}` |
| `cipher`（ss） | `method` |
| `private-key`（wireguard） | `private_key` |
| Clash 专有键（`udp`/`flow`/`client-fingerprint`/`alterId`/`fingerprint`/`ports`/`mport`/…） | 丢弃（sing-box 会拒绝） |

`render_subscription` 改为用该函数（原 Clash payload 路径本就无损——`clash_proxy_from` 只改名）。

### 3.3 回归测试

- `crates/vproxy-adapter`：`modern_protocols_are_parsed`（7 种协议全解析）、`credentials_match_upstream_node_auth`
  （断言凭据口径与上游 `nodeAuth` 一致：hysteria2→`hy2-pw`，tuic→`uu-11:tuic-pw`）；
- `crates/distribution`：`singbox_from_clash_keeps_protocol_fields`、`..._maps_tuic_and_shadowsocks`、
  `..._maps_transport_and_reality`。

## 4. 修复后真实样本回归

```
156 个真实节点 → kept 156 / dropped 0  →  keep_rate 100.0%
```

## 5. 仍未覆盖的部分（如实记录）

| 项 | 状态 | 说明 |
| --- | --- | --- |
| `shadowtls` | ❌ 不解析 | sing-box 专有（无独立凭据字段，是 TLS 包装层）；mihomo 无此类型 |
| `snell` / `mieru` / `sudoku` / `socks4` / `https` | ⚠️ 解析已支持，但**发射层**未做 sing-box 字段映射 | sing-box 无 `snell`/`mieru`/`sudoku` outbound；socks/http 的 sing-box 形态为 `socks`/`http`（`username`/`password` 已带） |
| `wireguard` 的 `peers`/`reserved`/`mtu` 等多字段 | ❌ 只保留 `private_key` | sing-box 1.11 起 wireguard outbound **已废弃**（1.13 移除），迁往 endpoint 形态 |
| `hysteria2` 的 `obfs` / `up_mbps` / `down_mbps` / `server_ports` | ❌ 不保留 | 端口跳跃与混淆会影响连通性，但**不在上游 `nodeAuth` 身份口径内**，属「同一节点不同参数」的区分问题（另议） |
| `ech-opts` / `support-x25519mlkem768`（mihomo 1.19 新增） | ❌ 不保留 | 后量子 REALITY 扩展，需 sing-box 侧同步支持才有意义 |
| API 层订阅输出 | ⏸ 占位 | `apps/curse/src/handlers/subscription.rs` 目前输出 `xxx://node/{id}` 占位，真实 payload 转换待 `node_payloads`（P1，已裁定后做） |

## 6. 与 MiaoSpeed 的关系（不受影响）

`Basics.Invoker` / `SlaveName` 是 **MiaoSpeed 协议字段**（对应其 `-whitelist` bot id），
字段名与本轮修复无关；P0-02 实测捕获时上游使用的就是 `phlogo`（原提交 `51e6f50` 的 golden fixture
一直是 `phlogo-spike`/`phlogo`），**保持不变即兼容**。
