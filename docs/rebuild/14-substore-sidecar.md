# 14 · 引入 Sub-Store 转换旁挂服务（决策 A 落地）

> 对应决策：用户在 S56 选定「**A. 旁挂 sub-store-convert**」。
> 本页是「一个大步骤一页」的落地记录，命令可直接照抄进后期 CI。
> 日期：2026-11-13。

## 1. 目标与边界

**目标**：在不重写各客户端格式的前提下，让 Gnosis 能产出 Surge / QX / Loon / Stash /
Shadowrocket / Surfboard / Egern / URI / V2Ray / JSON 等格式。

**本步做了**：真实 spike → 确定输入契约 → 写薄 sidecar → 真机端到端验收 → 固化部署与版本 pin。

**本步没做**（明确留待后续，别误判为完成）：
- ❌ Rust 侧客户端 / `crates/format-sidecar` 接入；
- ❌ `format_configs` 新增格式行与各格式的**模板包装**（sidecar 输出的是**节点行**，不是完整客户端配置）；
- ❌ docker-compose / Dockerfile 服务定义、健康检查接线；
- ❌ CI job。

## 2. 环境约束（实测，都是硬事实）

| 项 | 校机 `WIN11-HV1` | 影响 |
| --- | --- | --- |
| Node.js | **v22.22.2**，在 `C:\Users\YiZhidi\shortcuts-verify\node.exe`（**不在非交互 ssh 的 PATH**） | 脚本要用全路径，或注入 PATH |
| npm | ❌ 无 | 不能用 `npm i` |
| pnpm | ✅ `C:\Users\YiZhidi\AppData\Local\pnpm\.tools\pnpm\10.33.4\bin\pnpm.CMD` | 用它装依赖 |
| Docker | ❌ 无 | **用不了**上游的 `ghcr.io/tbxark/sub-store-convert:latest` |
| Bun | ❌ 无 | 用不了 `pnpm run start:bun` |
| `@sub-store-convert/core` npm 包 | ❌ **404 未发布** | 不能 `npm add` |

> 这三条把「直接跑上游 Docker 镜像」这条路堵死了。但仓库里
> **`packages/core/build/index.js` 是已构建产物**（503,527 B），可以直接用。

## 3. Spike：逐项验证，不靠猜

### 3.1 bundle 能不能直接用

```
IMPORT FAILED: Cannot find package 'peggy' imported from ...\index.mjs
```

**不是全自包含**：另外依赖 `peggy` / `js-base64` / `json5` / `lodash` / `yaml` 五个包
（其余如 `base64-js`、`buffer`、`ieee754` 已 inline）。pnpm 装上后：

```
IMPORT OK  exports = buildProxyServer,convert,loadProducer,loadRemoteData,
                     parseProxyLines,parsers,preprocessors,produce,produceOutput,tryParseProxy
produce targets = qx,QX,QuantumultX,surge,Surge,SurgeMac,Loon,Clash,clash,meta,
                  clashmeta,clash.meta,Clash.Meta,ClashMeta,mihomo,Mihomo,uri,URI,
                  v2,v2ray,V2Ray,json,JSON,stash,Stash,shadowrocket,Shadowrocket,
                  ShadowRocket,surfboard,Surfboard,singbox,sing-box,egern,Egern
preprocessors = HTML, Clash Pre-processor, Base64 Pre-processor, SSD Pre-processor,
                Full Config Preprocessor, Fallback Base64 Pre-processor
```

### 3.2 输入形态：**只有 Clash YAML 能用**

| 输入 | 结果 |
| --- | --- |
| base64 包裹的 URI 列表 | ❌ **0 节点、0 字符输出** |
| 纯文本 URI 列表 | ❌ **0 节点、0 字符输出** |
| **Clash/mihomo YAML** | ✅ **3/3 节点**，surge/qx/loon/stash/shadowrocket/egern/mihomo/sing-box/uri/json 全部出内容 |

这决定了**不需要等 `node_payloads`（P1）**——我们已经有 `crates/distribution::mihomo_yaml`，
直接喂它就行。**S56 里写的「格式扩充门禁」因此被绕开**，更正见 §10。

> 早先判断「URI/base64 已覆盖 v2rayN 等」在这个 build 上不成立，别拿 URI 列表当输入。

### 3.3 `data:` URL 可行，但**塞不进 GET**

`convert(url, target)` 内部是 `await fetch(url)`；Node 22 的 `fetch` 支持 `data:`，
所以**不需要给 sidecar 一个可抓取的 HTTP 地址**。但真实 52 节点订阅算出来：

```
data: URL length = 62,751
```

远超请求行/请求头限制 → 上游 `GET /sub?url=...` 的 API **不可用于真实订阅**。
→ 写我们自己的 POST 壳（保留它的 `convert()`，只换传输方式）。

### 3.4 真实 52 节点订阅验收（spike 阶段，直接调 core）

源：校机 `run2-sub-new.yaml`（42,038 B，36 vless + 16 hysteria2）。

| target | 输出 | 耗时 | 观察 |
| --- | --- | --- | --- |
| surge | 12,652 B / 52 行 | 136 ms | **52/52 满额** |
| stash / shadowrocket / egern / mihomo | ~18–20 KB / 53 行 | 47–69 ms | 满额 |
| sing-box | 28,412 B / 1,102 行 | 51 ms | 满额 |
| json | 24,437 B / 878 行 | 50 ms | 满额 |
| uri | 15,424 B / 52 行 | 52 ms | 满额 |
| **qx** | 10,223 B / 36 行 | 62 ms | **少 16 = QX 不支持 hysteria2** |
| **loon** | 9,549 B / 34 行 | 51 ms | 少 18 |
| **surfboard** | 4,546 B / 16 行 | 49 ms | 少 36 |

**节点数不同是 Sub-Store 按客户端能力过滤，属正确行为，不是丢数据**——必须写进文档，
否则以后会当成 bug 来查。

## 4. 架构与数据流

```
crates/distribution::mihomo_yaml(template, OutputNode[])
        │  Clash YAML（我们已有、已用真 mihomo 校验过）
        ▼
POST http://127.0.0.1:3000/convert?target=surge   (body = YAML)
        │
        ▼  deploy/substore-sidecar/server.mjs（我们写的薄壳）
        │      · 校验 target（对 produce 的 key 白名单）
        │      · 限流：body ≤ 8 MiB
        │      · YAML → data: URL → upstream convert()
        │      · 空产出 → 502（防「成功但 0 节点」的假订阅）
        ▼
text/plain 节点行 →（后续）套 format_configs 模板 → 返回给客户端
```

**为什么是常驻服务而非每次 spawn node**：503 KB bundle + peggy 语法解析只做一次；
实测常驻后单次转换 **49–136 ms**。

## 5. API 契约

| 方法 | 路径 | 请求 | 响应 |
| --- | --- | --- | --- |
| GET | `/health` | — | `{"ok":true,"target_count":20,"targets":[...]}` |
| POST | `/convert?target=<t>` | body = Clash YAML | `text/plain; charset=utf-8` 目标格式内容 |
| POST | `/convert/shape?target=<t>` | 同上 | `{"target":..,"ok":true,"lines":..,"bytes":..,"ms":..}` 诊断用 |

错误语义：
- 未知 `target` → `400`（并回可用列表）
- 空 body → `400`
- 转换抛错 / **产出为空** → `502`

`shape` 只报 `lines`/`bytes`，**不报 node 数**——各目标格式结构不同（surge 一行一节点、
sing-box/json 是缩进 JSON），单一「节点数」会是假数字。这是 spike 里 `nodes:0` 的教训。

## 6. 部署步骤（CI 可直接照抄）

```bash
cd deploy/substore-sidecar
pnpm install --frozen-lockfile          # 或 npm ci；依赖见 package.json，全部定版
node fetch-bundle.mjs                   # 按 commit pin 下载并校验 SHA-256
node fetch-bundle.mjs --check           # CI 常驻门禁：只校验不下载
node --check server.mjs                 # 语法门禁
node --check fetch-bundle.mjs
node server.mjs                         # 默认 127.0.0.1:3000
```

校机实测的等价路径（因为 node 不在 PATH）：

```cmd
cd /d C:\Users\YiZhidi\substore
"C:\Users\YiZhidi\shortcuts-verify\node.exe" server.mjs
```

> 版本门禁：`fetch-bundle.mjs` 里 `UPSTREAM_COMMIT` / `EXPECTED_SHA256` / `EXPECTED_BYTES`
> 三元组任一不符就**拒绝写入**，避免供应链漂移。

## 7. 端到端实测记录（真机 · 真实 52 节点）

启动后用 `client-test.mjs` 打满全链路：

```
[health] 200 {"ok":true,"target_count":20,"targets":[...20 个别名...]}

[shape] POST /convert/shape   （全部 200）
  surge 200 {lines:52,   bytes:13396, ms:116}   qx     200 {lines:36,   bytes:10691, ms:84}
  loon  200 {lines:34,   bytes:10067, ms:54}    stash  200 {lines:53,   bytes:20592, ms:53}
  shadowrocket 200 {lines:53, bytes:18982, ms:52}  surfboard 200 {lines:16, bytes:4822, ms:51}
  egern 200 {lines:53,   bytes:17956, ms:50}    mihomo 200 {lines:53,   bytes:18982, ms:63}
  sing-box 200 {lines:1102, bytes:29156, ms:53}  uri   200 {lines:52,   bytes:15424, ms:51}
  json  200 {lines:878,  bytes:25181, ms:48}

[convert] target=surge  status=200  chars=12652  lines=52

[errors]
  unknown target -> 400 {"error":"unknown target 'nope'",...}
  empty body     -> 400 {"error":"empty body (expected Clash YAML)"}
  garbage body   -> 502 {"error":"empty conversion output (no nodes parsed)"}
```

**结论：11/11 目标转换成功、3/3 错误路径正确。**

## 8. 版本 / 许可证 / 供应链

| 项 | 值 |
| --- | --- |
| 上游 | `tbxark/sub-store-convert` |
| **固定 commit** | `cddcab9c536da53c0f867cd6b5738791bc4e8121`（master HEAD，非 floating） |
| bundle | `packages/core/build/index.js` |
| **SHA-256** | `ee99f9a7fe2bb3a82321a5b94983ae3e223bfe02801fa55ef7c29478152027ef` |
| 字节数 | 503,527 |
| 运行依赖（定版） | js-base64 3.9.6 / json5 2.2.3 / lodash 4.18.1 / peggy 5.1.1 / yaml 2.9.1 |
| 许可证 | 仓库自称 **MIT**，但 vendor 并打包 **Sub-Store（AGPL-3.0）** → 本 sidecar 按 **AGPL-3.0** 分发；Gnosis 本身即 AGPL-3.0，兼容 |

固定 URL 已复核：`@cddcab9c…` 返回 200 且哈希与记录值一致（`HASH MATCH = True`）。
已登记 [`docs/THIRD_PARTY.md`](../THIRD_PARTY.md)。

## 9. 与现有模块的集成点（下一步的施工图）

1. **新 crate `crates/format-sidecar`**（`reqwest` 已是 workspace 依赖）：
   `convert(target: &str, clash_yaml: &str) -> Result<String>` + `health()` + 超时/重试/熔断。
   为什么单独 crate：`crates/distribution` 是纯渲染库，不该掺进 async HTTP。
2. **配置**：`PHLOGO_SUBSTORE_URL`（默认 `http://127.0.0.1:3000`）、超时、开关；
   sidecar 不可用时**只影响扩展格式**，mihomo/sing-box/base64 三条原生链路必须照常工作。
3. **`format_configs` 表**：为每个扩展格式加行，字段标明 `kind = sidecar` + `target`，
   并配**模板**（sidecar 只给节点行，需要补 `[Proxy]` / `[Proxy Group]` / `[Rule]` 等头尾）。
4. **API 出口**：`apps/curse/src/handlers/subscription.rs` 按 format 分派到原生渲染或 sidecar。
5. **compose**：加 sidecar 服务 + `healthcheck`，并把它标成**可选**（`profiles: ["extra-formats"]`）。
6. **CI**：§6 的四条门禁 + 真机验收脚本（`client-test.mjs` 收编进仓库）。

## 10. 两处对早先结论的更正

- **S56 写的「格式扩充被 `node_payloads`（P1）挡住」不成立**：sidecar 的输入是
  我们**已有的** `mihomo_yaml` 产物，不需要 URI producer、也不需要 `node_payloads`。
  不需要的东西就不要等。
- **S56 写的「base64 + URI 已覆盖 v2rayN/Shadowrocket 的读取」**：那说的是**输出**给客户端，
  与本 sidecar 的**输入**无关；实测这个 build 的 URI 输入解析出 0 节点，输入必须是 Clash YAML。

## 11. 风险与已踩的坑

| 坑 | 处理 |
| --- | --- |
| 上游 GET-only API + 62 KB `data:` URL | 自写 POST 壳 |
| `data:` URL 在 GET query 里会超请求头限制 | 同上 |
| URI 列表输入 0 节点 | 输入契约固定为 Clash YAML；空产出直接 502 |
| bundle 非自包含（缺 5 个包） | `package.json` 定版 + `pnpm install --frozen-lockfile` |
| 校机无 npm / Docker / Bun | 用 pnpm + 已签入的 build 产物 |
| **PS5.1 `Set-Content -Encoding UTF8` 写 BOM** | 本轮全部改用 Python / `.mjs` 显式编码；sidecar 输出 `text/plain; charset=utf-8` |
| `cmd /c start "" /B` 启动失败（srv.log 空） | 改 `.cmd` 包装 + 独立进程；实测前台可监听，E2E 用 Python `Popen` 保活 |
| QX/Loon 节点数变少被误判为丢数据 | §3.4 明确为客户端能力过滤 |
