# 15 M1 · 节点载荷加密落库（P1 第一片）

> 状态：**M1a 完成**（存储加密 + 明文清理）；M1b（`/sub` 真实渲染）与 M1c（base64 真实 URI）未做。
> 基线：`9ff1a7e`；本轮提交见 ACTION-LOG **S63**。
> 依据：方案 §7 / §16.8（不记录节点原始凭据）+ 用户裁定 A2（`XChaCha20-Poly1305` + `PHLOGO_PAYLOAD_KEY`）。

## 1. 为什么做：这不是理论问题

`nodes.raw_config_reference` 此前直接保存 vproxy-adapter 解析出的**原始 vendor payload**
（Clash 形状 JSON，含 password / uuid / psk / private key）。实测校机 dev 库 **473 个节点全部是明文**。
把它读出来就等于拿到全部节点凭据——这正是 §16.8 禁止的形态，也是 09 §D 的 **M1**。

同一处还牵连订阅：`/sub` 之所以只能发占位 `xxx://node/{id}`，正是因为「真实载荷」这条链路从未接通
（B2/M1）。本轮先解决**存储侧**（安全隐患），渲染侧留给 M1b。

## 2. 设计

| 项 | 选择 | 理由 |
| --- | --- | --- |
| 算法 | **XChaCha20-Poly1305**（`chacha20poly1305 0.10`） | A2 裁定；24B 随机 nonce，无需担心 nonce 复用；对称密钥无公钥面（后量子友好，见 10 §9 A2） |
| AAD | **节点 id 字符串** | 密文被搬到别的节点必然解密失败（防替换/错配） |
| 密钥 | `PHLOGO_PAYLOAD_KEY`：32 字节，**64 位 hex 或 base64/base64url**（带不带 padding 都可） | 运维常见两种写法都收 |
| 版本 | `key_version`（当前 `1`） | 未来轮换可逐行迁移，不用猜 |
| 表 | `node_payloads(node_id PK, key_version, nonce, ciphertext, updated_at)`，`ON DELETE CASCADE` | 与节点同生命周期 |
| 明文列 | `nodes.raw_config_reference` 只写 `''` | 彻底消除"顺手再写一次明文"的路径 |

**密钥缺失时的行为**（刻意设计）：未设置 `PHLOGO_PAYLOAD_KEY` 时生成**进程内临时密钥**——
开发/测试照常跑，但重启后已存载荷无法解密（订阅会取不到节点）。这让"忘记配置"在部署时就暴露，
而不是静默地以明文存了几周。**配置了但格式非法**则直接启动失败（worker 退出）。

密钥类型手写 `Debug` 输出 `PayloadKey(<redacted>)`，防止日志/断言把密钥打出来。

## 3. 迁移 `0012_node_payloads`

```sql
CREATE TABLE node_payloads (...);
-- 清除加密出现之前写入的明文。SQL 没有密钥，无法原地重封：
-- 每个源下一次抓取会重新写入加密载荷。
UPDATE nodes SET raw_config_reference = '' WHERE raw_config_reference <> '';
```

**不可逆**：down 迁移只删表，**不恢复明文**（原文已不存在）。这是有意的取舍——留着明文副本会让加密形同虚设。

## 4. 写路径

`crates/database/src/reconcile.rs::upsert_node_by_hash`：

1. `INSERT ... ON CONFLICT` 只写 `raw_config_reference=''`（不再绑定原始 payload）；
2. `RETURNING id` 拿到节点 id；
3. `payload_crypto::global().seal(aad=id, nn.raw_config_reference)`；
4. `payload_store::upsert_node_payload(conn, id, sealed)`。

整个过程在同一事务连接内（reconcile 的原子性不变）。`global()` 是进程级密钥（worker 启动时由
`init_from_env()` 安装），避免把密钥穿透 fetch → registry → database 四层。

## 5. 读路径（**未做**，M1b）

`apps/curse/src/handlers/subscription.rs` 仍是占位实现（yaml 发 `placeholder.invalid`、json 发
`direct`、base64 发 `xxx://node/{id}`）。M1b 要做的是：

- 读 `nodes`（Linked）+ `node_scores` + `node_payloads` → `open()` 解密 → 得到 Clash 载荷；
- 组装 `distribution::PipelineNode`，交给**已存在**的纯函数
  `distribution::subscription::render_subscription_with_sponsor`
  （它会应用 `filter_ast`、`region`、`min_score`、`preset`、`limit_count`、命名模板与 sponsor）；
- 三种格式分别返回真实内容。

**注意**：该纯函数目前对 `base64` 分支生成的是假链接（`{proto}://cred@{id}:443`），
所以 M1c 必须先补「Clash 载荷 → 真实 URI」的构造器，并用**我们自己的 vproxy-adapter 反解析**做往返校验；
在那之前 base64 不应被当作可用格式。

## 6. 验证（校机真 PG）

| 项 | 结果 |
| --- | --- |
| `cargo test -p payload-crypto` | **6 passed**（往返 / AAD 换节点失败 / 篡改失败 / 错密钥失败 / nonce 长度 / hex+base64 解析与 `global` 稳定） |
| `cargo test -p registry --test reconcile` | **7 passed**，新增 `payload_is_sealed_and_bound_to_its_node`：节点行列**为空**、`node_payloads` 有行、密文**不含** `TOP-SECRET-PW`、用节点 id 作 AAD 可回读、搬到另一节点**解密失败** |
| `rustfmt --check`（本轮 5 文件） | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | **0** |
| 迁移实跑 | `_sqlx_migrations` 版本 **12 = node payloads, success=t** |
| 明文清理 | `select count(*) from nodes where raw_config_reference <> ''` → **0** |
| 加密行 | `node_payloads` 11 行（本轮测试节点） |

依赖新增：`chacha20poly1305 0.10.1`（+ aead/cipher/poly1305 等 8 个 crate，`rsproxy` 拉取）。

## 7. 风险与遗留

1. **临时密钥重启丢载荷**：生产必须设置 `PHLOGO_PAYLOAD_KEY`（否则订阅无节点）。部署文档需加此项。
2. **轮换**：表里已有 `key_version`，但还没有「旧版本 key 列表 + 逐行重封」的工具；轮换前需要补。
3. **M1b/M1c**：`/sub` 仍发占位内容；base64 仍是假链接（见 §5）。
4. **§16.8 的另一半**：日志侧已由 `RedactionLayer` 处理；但**订阅 URL 里的 token** 与
   `sources.content`（订阅源 URL 本身可能含凭据）仍按原样存库——不在本轮范围，登记待评估。
5. **性能**：每个节点一次 AEAD（μs 级）+ 一次 upsert；抓取批量 500 节点量级无压力。

## 8. CI 提示（不落地 YAML）

- `cargo test -p payload-crypto` 与 `cargo test -p registry --test reconcile` 都可在无外部服务下跑
  （后者需 `DATABASE_URL`，与既有集成测试同门槛）。
- 建议 CI 断言两点：`nodes.raw_config_reference` 恒为 `''`；`node_payloads.ciphertext` 不含明文样本。
- 生产部署检查：`PHLOGO_PAYLOAD_KEY` 存在且能通过 `PayloadKey::from_env()`（无效即启动失败）。

## 9. 与 fork 源对照

上游 Go 版没有载荷加密层：`internal/node/upsert.go` 直接把原始节点配置写进库（`raw` 字段），
checker 拉取时整包下发。本实现是**新增的安全面**，不是等价移植；因此 `11-equivalence-audit.md`
的等价性口径不受影响（该表只覆盖 API 路径）。