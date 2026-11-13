# 订阅模板

> **本实现**：模板经 `GET/PUT /api/v1/format-configs[/{format}]` 管理（`format_configs` 表），**写入时服务端用渲染同款解析器校验**——坏模板当场 400，不会等到订阅请求才失败。未存储覆盖时使用内嵌默认模板（`crates/distribution/src/output.rs` 的 `DEFAULT_CLASH_TEMPLATE` / `DEFAULT_SINGBOX_TEMPLATE`）。

Phlogo 在首次启动时为每个订阅格式提供内嵌默认模板。管理员可在管理界面（或 API）为 `clash` / `singbox` / `base64` 存放自定义配置。

当模板为空时，生成器只输出一个包含节点列表的极简文档。模板的具体处理规则由格式转换器实现（`crates/distribution/src/output.rs`），Clash 与 Sing-box 的规则不同。

## Clash 模板

Clash 模板必须是一份完整的 Clash/Mihomo YAML 配置。生成时：

1. 顶层的 `proxies` 列表会被替换为本次选中的全部节点。
2. 每个 `proxy-groups` 条目的 `proxies` 列表会追加本次全部节点的名称。

因此模板应保留完整的 `proxy-groups`、`rules` 等结构，而 `proxies` 处可以留空，生成时会被替换。

```yaml
mixed-port: 7890
allow-lan: true
mode: rule
proxies: []            # 会被替换为选中节点
proxy-groups:
  - name: 选择
    type: select
    proxies: []        # 会被追加全部节点名称
rules:
  - MATCH,选择
```

## Sing-box 模板

Sing-box 模板必须是一份完整的 sing-box JSON 配置。生成时：

1. 模板中的管理 outbound 会被保留：`selector`、`urltest`、`direct`、`dns`、`block`、`tproxy`、`redirect`、`tun`、`sniffer`。
2. 模板中原有的代理 outbound 会被丢弃，替换为本次选中的节点。
3. 每个 `selector` 与 `urltest` 的 `outbounds` 列表会追加本次全部节点的 tag。
4. 最终 `outbounds` 顺序为：`selector` / `urltest` + 代理节点 + `direct` 等基础设施。
5. Sing-box 不支持的协议会被跳过。

```json
{
  "outbounds": [
    { "type": "selector", "tag": "select", "outbounds": [] },
    { "type": "direct", "tag": "direct" }
  ]
}
```

## 制作步骤

1. 先在客户端验证一份完整可用的 Clash 或 sing-box 配置。
2. 记下其中的分组名称（Clash 的 `proxy-groups`，Sing-box 的 `selector` / `urltest`）和出站规则；`proxies` 或代理 outbound 可以留空，生成时会被替换。
3. `PUT /api/v1/format-configs/{format}`（body：`{"config": {"template": "<全文>"}}`），或在管理界面对应页面填入。
4. 访问 `/sub/{token}.{fmt}` 验证输出结果。

内置模板只在 `format_configs` 无该格式行时作为回退使用；删除对应行即可恢复内嵌默认。