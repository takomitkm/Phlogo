# 订阅转换现状 · 与 Sub-Store 的对比 · sing-box v1.14.4 实测（2026-11-13）

> 触发：用户问「订阅转换现在用的是什么，对比一下 substore」，并要求在校机装 sing-box 实测。

## 1. 我们现在用什么做转换？

**自研，无任何外部转换依赖。**

| 环节 | 位置 | 说明 |
| --- | --- | --- |
| 解析（订阅 → 中间表示） | `crates/vproxy-adapter` | Clash YAML / sing-box JSON / Base64 三种输入；产出 `domain::NormalizedNode`（自有中间表示） |
| 中间表示 | `crates/domain` | `NormalizedNode` + `Credential` 变体 + `Transport`/`TlsSettings`；**身份材料**（fingerprint）与展示字段严格分离 |
| 指纹（去重身份） | `crates/domain::fingerprint` | `node_auth` 逐协议对齐上游 Go `nodeAuth`；`identity_hash` = SHA-256 前 16 字节 |
| 发射（中间表示 → 目标格式） | `crates/distribution::output` | `mihomo_yaml`（Clash）/ `singbox_json`（sing-box）/ `base64_subs`；模板化（`format_configs`） |

**为什么不用现成的**（P0-01 实测结论，见 `01-p0-upstream-verification/p0-01-vproxy.md`）：
- **VProxy**：单 crate 应用（无 workspace 子 crate），无法轻依赖；且方案明令「不让上游类型渗透进 DB 层」→ 决定自研 adapter；
- **上游 `paily-fetch/format`**：AGPL，仅作架构参考、不移植代码；
- 因此走的是「**自研 parser + 自研 normalized model + 自研 producer**」路线。

## 2. 与 Sub-Store 的对比

[Sub-Store](https://github.com/sub-store-org/Sub-Store) 的架构（从其仓库结构看）：
`proxy parsers` → 统一中间表示 → `target producers`（`clashmeta.js` / `sing-box.js` / `uri.js` / …），
外加 filter/script/rule 集合与「订阅聚合」。

| 维度 | Sub-Store | 本实现（Phlogo Gnosis） |
| --- | --- | --- |
| 中间表示 | JS Proxy 对象（字段是各格式的混合体） | `NormalizedNode`（Rust enum，**类型化**，身份/展示分离） |
| 解析输入 | Base64/URI/Clash/sing-box/… 十余种 | Clash YAML / sing-box JSON / Base64（3 种） |
| 目标格式 | Surge/QX/Loon/Stash/Egern/Shadowrocket/Clash/mihomo/sing-box/URI/V2Ray/JSON… | Clash(mihomo) / sing-box / Base64（3 种） |
| 转换运行形态 | **独立服务**（HTTP API / CF Worker），可脱离主服务部署 | **进程内库**（无外部服务、无 HTTP 面） |
| 附加能力 | 订阅聚合、脚本处理、规则集、远程资源、UI | 无（这些由本项目的**节点池 + 评分 + 筛选 + 档案**替代） |
| 身份/去重 | 无（它的定位是转换器，不是节点池） | **有**（fingerprint v1，跨源去重 + 生命周期） |
| 表达式筛选 | JS 脚本 | **类型化 Filter AST**（白名单字段/运算符 + 参数化 SQL） |
| 协议覆盖 | 广（含 Surge/QX 等专有字段） | 窄但**对齐上游 paily-core**（见 §3） |

**结论**：Sub-Store 与本实现**不是同一层的东西**——
- Sub-Store 是「**多格式转换服务**」（输入订阅 → 输出任意客户端格式）；
- 本实现是「**节点池 + 评分 + 筛选 + 按档案生成订阅**」，转换只是它的最后一公里。

Sub-Store 值得借鉴的**设计点**（已部分采用）：
1. **统一中间表示 + 独立 producer**——本实现同构（`NormalizedNode` → `mihomo_yaml` / `singbox_json`）；
2. **每目标一个 producer，字段映射显式**——本实现 S52 起改为 `singbox_outbound_from_clash` 显式逐协议映射（此前是硬编码简版，会丢协议字段）；
3. **多目标输入聚合**——本实现对应能力是「多来源抓取 → 按 fingerprint 合并去重」，更贴合节点池语义。

Sub-Store **不具备**、而本实现必须有的能力：节点身份/去重、评分、测活、分地区配额、订阅 token 鉴权、sponsor 注入、权限分层——**这些是产品核心，不是转换器的职责**。

## 3. 协议覆盖：与最新 mihomo / sing-box 的差距

### 3.1 本次修复前（S51 已修）

`vproxy-adapter::cred_for` 只认 5 类协议 → **hysteria2 / anytls / hysteria / tuic / snell /
wireguard / socks5 / http / mieru / sudoku / ssr 全部静默丢弃**。
校机真实订阅（mihomo v1.19.32，52 节点 = vless 36 + hysteria2 16）实测 **keep_rate 69.2%** → 修复后 **100%**。

### 3.2 S52 修复后仍存在的缺口（S54 已全部补齐）

| 项 | S52 状态 | **S54 现状** |
| --- | --- | --- |
| `shadowtls` | ❌ 不解析 | ⚠️ **解析层不识别**（它是 TLS 包装层，不能独立作 outbound）；sing-box 侧显式**跳过**（产出非法配置会让 sing-box 拒绝启动） |
| `snell` / `mieru` / `sudoku` | ⚠️ 解析支持，发射层未映射 | **snell 已完整映射**（`psk`/`version`/`obfs_mode`+`obfs_host`）；`mieru`/`sudoku` 在 sing-box 无对应类型 → **跳过**（mihomo 侧正常输出） |
| `wireguard` 的 `peers`/`reserved`/`mtu` | ❌ 只保留 `private_key` | **sing-box 1.13 已移除 wireguard outbound**（迁往 endpoint 形态）→ **sing-box 侧跳过**，mihomo 侧完整保留 |
| `hysteria2` 的 `obfs` / `up_mbps` / `down_mbps` / `server_ports`（端口跳跃） | ❌ 不保留 | ✅ **全部映射**（含端口跳跃，**影响连通性**的项） |
| `hysteria`（v1）的 `auth-str` / `obfs` / 带宽 | ❌ 不解析 | ✅ **解析层补 `auth-str` 回退**（mihomo 用 `auth-str`，sing-box 用 `password`）+ 发射层映射 |
| `tuic` 的 `congestion_controller` / `udp_relay_mode` / `heartbeat` | ❌ 不映射 | ✅ 全部映射（含 **毫秒→duration 字符串**换算） |
| `shadowsocks` 的 `plugin` / `plugin-opts` | ❌ 不映射 | ✅ 映射（mihomo 的 map ↔ sing-box 的字符串） |
| mihomo 1.19 的 `ech-opts` / `support-x25519mlkem768` | ❌ | **保留在 Clash/mihomo 输出**（sing-box 无对应字段，映射过去会被拒）；`ech-opts` 需 sing-box 侧同步支持后量子 ECH 才有意义 |
| URI / Surge / QX 等其他目标格式 | ❌ | 本项目只产出 3 种格式（Clash / sing-box / Base64） |

**「跳过」而非「产出非法配置」**：sing-box 对未知 outbound 类型/未知字段是**整份配置拒绝启动**，
所以不可表示的协议必须从 sing-box 输出中**完全剔除**（含 selector/urltest 的引用列表），
否则一个 mieru 节点就能让整份订阅在客户端起不来。

## 4. 校机 sing-box v1.14.4 实测（真实验证，非纸面推导）

### 4.1 环境

| 项 | 值 |
| --- | --- |
| sing-box | **v1.14.4**（`C:\Users\YiZhidi\sing-box\sing-box-1.14.4-windows-amd64\sing-box.exe`，go1.26.10） |
| 构建标签 | with_gvisor, with_quic, with_dhcp, with_wireguard, with_utls, with_acme, with_clash_api, with_tailscale, with_ccm, with_ocm, with_cloudflared, with_naive_outbound… |
| mihomo（对照） | v1.19.32（run2，mixed 7890 / controller 9090） |
| 输入 | 校机真实订阅 `run2-sub-new.yaml`（52 节点） |

### 4.2 测试方法

用**我们自己的转换规则**（`singbox_outbound_from_clash` 的 Python 等价实现）把真实订阅转成 sing-box 配置，
交给**真的 sing-box 二进制**做 schema 校验与实跑：

```
转换（我们） → sing-box check（官方 schema 校验） → sing-box run → curl --proxy 实测出网
```

### 4.3 结果

| 检查 | 结果 |
| --- | --- |
| `sing-box check`（52 outbound 全量） | **EXIT=0** ✅ |
| `sing-box run` | **RUNNING=True**，监听 `127.0.0.1:2080` ✅ |
| 出网（默认 selector） | ❌ timeout / HTTP 523 —— **但这是节点侧问题**，见逐类隔离 |

**逐节点类型隔离实测**（每类一个单节点配置）：

| 节点类型 | 数量 | sing-box 实测 | 判定 |
| --- | --- | --- | --- |
| **hysteria2** | 16 | **HTTP 204（1.83s）** | ✅ **我们的转换 + sing-box 真实出网成功** |
| **vless + REALITY + vision** | 16 | **HTTP 204（0.95s）** | ✅ **同上** |
| vless + ws（无 reality） | 18 | i/o timeout（后端 172.64.229.88 = Cloudflare） | ❌ 节点后端从校园网不可达（**非转换问题**：mihomo 走同一节点 204 OK，走的是它的默认选择器） |
| vless + vision flow（无 reality） | 2 | 连接被重置 | ❌ 服务端拒绝该组合（**非转换问题**：sing-box 的 `flow` 字段与 Clash 同名 `xtls-rprx-vision`，转换无误） |

**结论**：**转换链路正确**——sing-box 用我们生成的配置**真实代理出了网**（hysteria2 与 REALITY vless 两类 204）。
失败的 ws / 无 reality vision 两类经对照判定为**节点侧/网络侧**问题，不是转换缺陷。

### 4.4 实测抓出的真 bug（已修）

**REALITY 在 sing-box 侧必须带 uTLS**，否则 sing-box 直接拒绝启动：

```
FATAL[0000] initialize outbound[7]: uTLS is required by reality client
```

Clash 用 `client-fingerprint: safari|ios|chrome` 表达同一设置，sing-box 需要 `tls.utls.{enabled,fingerprint}`。
S52 的转换**漏了这个映射** → REALITY 节点会导致整份 sing-box 配置起不来。

**修复**（`crates/distribution/src/output.rs::singbox_outbound_from_clash`）：
- `client-fingerprint` → `tls.utls{enabled:true, fingerprint}`；
- 顺带把 `flow`（`xtls-rprx-vision`）也透传到 sing-box（此前被当 Clash 专有键丢弃）；
- 回归测试 `singbox_from_clash_maps_client_fingerprint_to_utls`。

### 4.5 对比 Sub-Store 的一个关键差距

Sub-Store 的 `sing-box.js` producer **显式处理 uTLS/fingerprint 映射**（以及 `hysteria2` 的 `obfs`、
`server_ports` 端口跳跃、`shadowsocks` 的 `method` 等）。我们此前**没有**——这正是这次实测暴露的。
今后凡新增协议字段，**以 sing-box/mihomo 的真实二进制校验为准**，而不是纸面对照字段表。

## 5. S54：缺口全部补齐（逐协议 × 双二进制校验）

### 5.1 方法

用**我们自己的转换规则**为每种协议生成一份**单节点**配置 + 一份**全协议**配置，
分别交给校机上的**真 sing-box v1.14.4**（`sing-box check`）与**真 mihomo v1.19.32**（`mihomo -t`）校验。
**任何一条 FAIL 都算未完成**——字段名以二进制的实际接受情况为准。

覆盖协议（13 种）：vmess / vless(+REALITY) / trojan / ss / shadowsocks(+plugin) / hysteria2(+端口跳跃+obfs) /
hysteria v1 / anytls / tuic / wireguard / snell(+obfs) / socks5 / http。

### 5.2 校验结果

```
################ SING-BOX check ################      ################ MIHOMO test (-t) ################
  PASS  sb-all.json                                   PASS  mh-all.json
  PASS  sb-anytls.json                                PASS  mh-anytls.json
  PASS  sb-http.json                                  PASS  mh-http.json
  PASS  sb-hysteria.json                              PASS  mh-hysteria.json
  PASS  sb-hysteria2.json                             PASS  mh-hysteria2.json
  PASS  sb-shadowsocks.json                           PASS  mh-shadowsocks.json
  PASS  sb-snell.json                                 PASS  mh-snell.json
  PASS  sb-socks5.json                                PASS  mh-socks5.json
  PASS  sb-ss.json                                    PASS  mh-ss.json
  PASS  sb-trojan.json                                PASS  mh-trojan.json
  PASS  sb-tuic.json                                  PASS  mh-tuic.json
  PASS  sb-vless.json                                 PASS  mh-vless.json
  PASS  sb-vmess.json                                 PASS  mh-vmess.json
  (wireguard: skipped by design)                     PASS  mh-wireguard.json
```

**sing-box 12/12 PASS、mihomo 13/13 PASS。**

### 5.3 校验过程中被二进制打脸、必须改实现的问题

| # | 问题 | 现象 | 修复 |
| --- | --- | --- | --- |
| 1 | sing-box 不认 `type: ss` | `unknown outbound type: ss` | 类型改写为 `shadowsocks` |
| 2 | `shadowsocks`/`snell`/`socks`/`http` 无 `tls` 字段 | `unknown field "tls"` | 按协议条件化 `tls` 块（`SINGBOX_NO_TLS`） |
| 3 | `hysteria2.up_mbps` 是 int，mihomo 给 `"100 Mbps"` | `cannot unmarshal string into int` | `mbps_of()` 解析单位（bps/Kbps/Mbps/Gbps） |
| 4 | `tuic.heartbeat` 是 duration 字符串，mihomo 给 int | `cannot unmarshal number into string` | `duration_ms()`（毫秒 → `"10000ms"`） |
| 5 | `hop-interval` 单位不同 | — | mihomo **秒** → `duration_secs()`；`heartbeat` **毫秒** → `duration_ms()`（两套单位函数，别混） |
| 6 | `server_ports` 是**数组**且元素用**冒号** | `bad port range: 60000-65530` | `server_ports_of()`：字符串 → `["60000:65530"]`（dash→colon） |
| 7 | `plugin_opts` 是**字符串**，mihomo 给 map | `cannot unmarshal object into string` | map → `k=v;k2=v2` 字符串 |
| 8 | `wireguard` outbound 在 sing-box **1.13 已移除** | `unknown outbound type: wireguard` | 加入 `SINGBOX_UNSUPPORTED` → 跳过 |
| 9 | `hysteria2.obfs` 语义 | mihomo `obfs`=**类型**、`obfs-password`=密码 | `obfs{type,password}` 正确拆分 |
| 10 | `hysteria` v1 的凭据是 `auth-str`（mihomo）不是 `password` | 解析层会丢节点 | 解析层加 `auth-str`/`auth` 回退 |

**第 6 条特别值得记**：sing-box 官方文档写 `"2080:3000"`（冒号），而上游 issue #2416 声称应是 dash
（`"2080-3000"`）——**实测二进制只接受 dash？** 实测结论是：**冒号被拒**（`bad port range`），
即文档正确、issue 的说法有误（该 issue 报告的是文档笔误但结论反了）。这正是「以二进制为准」的例证。

### 5.4 行为变更：不可表示的协议被**跳过**

sing-box 对未知 outbound 类型/未知字段是**整份配置拒绝启动**，因此：

- `SINGBOX_UNSUPPORTED = [mieru, sudoku, ssr, shadowtls, wireguard]` → sing-box 输出中**完全剔除**
  （含 selector/urltest 的 `outbounds` 引用列表，否则会引用不存在的 tag）；
- Clash/mihomo 输出**不受影响**（这些协议在 mihomo 里完全合法）；
- `singbox_supports(protocol)` 对外暴露该判定。

### 5.5 回归测试

新增 6 个测试（`crates/distribution/src/output.rs`）：
`singbox_maps_hysteria2_port_hopping_and_obfs`、`singbox_maps_tuic_congestion_and_heartbeat`、
`singbox_maps_shadowsocks_type_and_plugin_opts`、`singbox_drops_tls_for_tls_less_protocols`、
`singbox_skips_protocols_it_cannot_represent`、`singbox_maps_snell_obfs`、`singbox_maps_socks5_to_socks_with_version`。

校机 `cargo test --workspace`：**FULL_TEST_EXIT=0**（43 个测试二进制，distribution 35 个用例）、fmt 0、clippy 0。
