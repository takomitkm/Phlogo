# 分发

本文档介绍公开订阅接口、档案（profile）、地区策略与选节点算法。

> **本实现的差异**（相对上游 Go 版）：订阅地址为 **`/sub/{token}.{fmt}`**——必须持有订阅 token
> （哈希存储、可吊销、撤销即时 401）；上游的无认证 `/{fmt}` 公开地址已被此方案取代。
> 地区策略/配额算法与上游一致（`selector.go` 逐函数移植，E-S A-Res 加权抽样，统计验收）。

## 工作流程

终端用户请求 `GET /sub/{token}.{fmt}` 时按以下顺序处理：

1. **token 校验**：哈希比对，仅 `active` token 且其档案 `enabled` 才放行；否则 401。
2. **候选集**：`Linked` 节点 + 各自评分（后续接入档案 `filter_ast`/region/health/min_score 硬条件）。
3. **preset 选择**：按档案的 preset（`common`/`all`/`other`）执行地区配额与抽样。
4. **数量限制**：`min(档案 limit_count, max_distribute 配置)`。
5. **命名**：`{{field}}` 白名单模板（档案 `naming_rule` 或默认模板）+ **sponsor 文案注入**（priority 升序、后命中覆盖、无命中用 `default_sponsor_text`）。
6. **输出**：Clash YAML / Sing-box JSON / Base64，由档案的 `output_format` 决定。

## 订阅地址

```text
https://your-domain.example/sub/{token}.yaml
https://your-domain.example/sub/{token}.json
https://your-domain.example/sub/{token}.base64
```

- token 在**签发时一次性明文展示**（服务端只存哈希），丢失即作废重签。
- 管理界面 Subscriptions 页可签发/吊销；吊销即时生效（401）。
- 空候选时返回 404 `no nodes matched`（与上游"200 空内容"不同，客户端应同时处理两种）。

## 地区策略

### `common`

`common` 优先选择常用地区 `HK`、`TW`、`SG`、`US`、`JP`：

1. 尽量在有节点的常用地区之间平均分配配额。
2. 某常用地区的节点超过配额时，进行均匀随机抽样，避免每次请求都集中使用固定的高分节点。
3. 常用地区不足时，先从其他常用地区随机补足。
4. 仍不足时按评分从其他地区补足，最后使用 `UN` 未知地区。
5. 若没有任何常用地区节点，自动退化为 `all`。

### `all`

`all` 侧重地区分布：

1. 非 `UN` 地区按节点数量比例分配配额。
2. 节点数不超过配额时全部选取。
3. 节点数略多于配额时，优先选取评分较高的节点。
4. 节点数明显多于配额时，按评分加权随机抽样。
5. `UN` 节点仅在其他地区不足时补足。

加权随机使用 A-Res 抽样，权重为 `max(评分, 0.01)`。评分较低的节点概率较低，但不会被完全排除。

### `other`

`other` 排除 `HK`、`TW`、`SG`、`US`、`JP`，其余规则与 `all` 相同。

## 输出数量与排序

`max_distribute` 配置决定单次订阅的硬上限（默认 200），档案 `limit_count` 不得超过它。

输出按地区命名顺序排列，同一地区内按评分降序编号。节点名由命名模板生成：解锁标记、等级、国旗、地区、序号和 **sponsor 文案**，详见 [配置与 API](configuration-and-api.md)。
