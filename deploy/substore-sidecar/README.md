# Sub-Store conversion sidecar

Phlogo Gnosis 的格式转换旁挂服务。**我们只写一层薄 HTTP 壳**，转换逻辑全部来自
[tbxark/sub-store-convert](https://github.com/tbxark/sub-store-convert) 的已构建 core bundle
（其内部 vendor 了 [Sub-Store](https://github.com/sub-store-org/Sub-Store) 的 parser/producer）。

## 为什么需要这层壳

上游 Hono app 只有 `GET /sub?target=<t>&url=<url>`，`convert()` 内部是 `await fetch(url)`。
我们把已有的 **Clash/mihomo YAML** 用 `data:` URL 传进去是能跑通的（Node 22 的 `fetch` 支持
`data:`），但真实 52 节点订阅会生成 **62,751 字符**的 URL —— 远超请求行/请求头上限。
所以改成：**POST，把 YAML 放在 body 里**，并且让 503 KB bundle 与 peggy 语法只加载一次
（每次 `node` 启动都重新解析一遍太浪费）。

输入契约：必须是 **Clash YAML**（我们 `crates/distribution::mihomo_yaml` 的产物）。
实测：**纯/base64 的 URI 列表在这个 build 里解析出 0 个节点**，不要喂那个。

## 部署

前置：Node.js **>= 22**、pnpm 或 npm。

```bash
cd deploy/substore-sidecar
pnpm install --frozen-lockfile     # 或 npm ci
node fetch-bundle.mjs              # 按 pin 下载 index.mjs 并校验 SHA-256
node fetch-bundle.mjs --check      # CI 里每次都跑，只校验不下载
node --check server.mjs
node server.mjs
```

环境变量：

| 变量 | 默认 | 说明 |
| --- | --- | --- |
| `SUBSTORE_PORT` | `3000` | 监听端口 |
| `SUBSTORE_HOST` | `127.0.0.1` | **只绑回环**，不要直接暴露公网 |
| `SUBSTORE_MAX_BODY` | `8388608` | 请求体上限（8 MiB） |
| `SUBSTORE_BUNDLE` | `./index.mjs` | bundle 路径 |

## API

| 方法 | 路径 | 说明 |
| --- | --- | --- |
| GET | `/health` | `{ok, target_count, targets[]}`；做存活探针 |
| POST | `/convert?target=<t>` | body = Clash YAML → `text/plain` 目标格式内容 |
| POST | `/convert/shape?target=<t>` | 诊断：`{target, ok, lines, bytes, ms}` |

可用 target（20 个别名，去重后）：
`clash` `clash.meta` `clashmeta` `egern` `json` `loon` `meta` `mihomo` `quantumultx` `qx`
`shadowrocket` `sing-box` `singbox` `stash` `surfboard` `surge` `surgemac` `uri` `v2` `v2ray`

错误：未知 target / 空 body → `400`；转换抛错或**产出为空**（0 节点解析）→ `502`。
空产出单独报错很重要，否则客户端会拿到一份"看起来成功但没节点"的订阅。

## 目标格式的节点数会不一样（正常）

Sub-Store 会按客户端能力过滤不支持的协议。真实 52 节点（36 vless + 16 hysteria2）订阅实测：
`surge` 52 行、`qx` 36（QX 不支持 hysteria2，正好掉 16）、`loon` 34、`surfboard` 16。
**不是丢数据，是客户端表达不了**。

## 许可证

- `sub-store-convert` 仓库自称 MIT，但它 **vendor 并打包了 Sub-Store（AGPL-3.0）的源码**；
  本 sidecar 分发的 bundle 因此按 **AGPL-3.0** 处理。
- Phlogo Gnosis 本身就是 AGPL-3.0，兼容。
- 固定版本与哈希见 [`docs/THIRD_PARTY.md`](../../docs/THIRD_PARTY.md)；**禁止 floating 版本**。
  升级流程：先复核上游 diff → 更新 `fetch-bundle.mjs` 里的 `UPSTREAM_COMMIT` /
  `EXPECTED_SHA256` / `EXPECTED_BYTES` → 重新跑真机验收 → 才合入。
