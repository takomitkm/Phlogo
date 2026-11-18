# 16 M2 · CheckJob 接 MiaoSpeed 真测

> 状态：**完成**（接线 + mock 端到端 + **真 MiaoSpeed 4.7.9 跑通**）。
> 依据：方案 §8 / 09 §D.2（M2）；配置键由用户裁定为 `PHLOGO_CHECKER_WS_URL` / `PHLOGO_CHECKER_SECRET`。
> 提交：ACTION-LOG **S66**。

## 1. 之前是什么状态

`handle_check` 只做两件事：`record_check_run` → `finish_check_run`。**从没调用过测活适配器**，
所以 `node_check_results` 永远是空的，评分（M5）也就无从算起。适配器本身（P3-02）已实现但
`submit` + `collect` 这对方法在真实服务器上根本不成立（见 §3）。

## 2. 实现

| 位置 | 改动 |
| --- | --- |
| `crates/checker-miaospeed/src/adapter.rs` | 新增 **`MiaoSpeedAdapter::check(&SubmitRequest)`**：一次 roundtrip 到终态并映射结果；新增 `wire_nodes` 统一构造线上节点；`map_results` 把 `InvokeDuration<=0` 记为 `-1`（Phlogo 死亡约定） |
| `apps/worker/src/check_job.rs` | `run_check_once(pool, &job)` 全流程：记录 run → 读端点配置 → 取节点 + **解密载荷** → 组装 `SubmitRequest` → `adapter.check()` → `persist_check_run`（含 unreported=unknown）→ 逐节点 `upsert_node_status` → 终态 |
| `apps/worker/tests/check_wiring.rs` | mock MiaoSpeed（tokio-tungstenite）端到端 2 用例 |
| `apps/worker/tests/check_job.rs` | 状态断言由 `finished` 改为 **`success`**（`check_runs` 的词表就是 `success`，`source_fetch_runs` 才是 `finished`） |

失败路径同样收敛：没有端点配置 / 节点全无载荷 / 适配器报错 / 落库失败，都会把 run 标成
`failed` 并写日志，不会留下悬挂的 `started`。

**关键实现细节**

- `node_check_results.node_id` 是外键，所以「unreported」只允许包含**确实存在**的节点 id
  （`existing_ids` 先取交集），否则整批落库失败。
- 节点按 `checker_tag` 写 `node_status`；健康判定暂用 `latency_ms == -1 → dead`。
- Apalis 至少一次投递：重复投递**重跑一遍**而不是跳过（幂等写入保证不会重复计数），
  这样"跑到一半进程挂了"的投递还能补完。

## 3. 真机暴露的三条协议事实（mock 测不出来）

真 MiaoSpeed 4.7.9（`C:\Users\YiZhidi\p0-02\miaospeed.exe`，vendor mihomo v1.19.33）跑出来的：

1. **`Payload` 必须是字符串**。传 JSON 对象时服务端回显 `"Payload":""`，节点直接被丢；
   必须 `payload.to_string()` 后作为字符串发送。
2. **结果按 payload 里的 `name` 映射，不是 `Nodes[].Name`**。payload 缺 `name` 时
   `ProxyInfo` 全是空串（`Name:"",Address:"",Type:""`），`Nodes[].Name` 完全被忽略。
   适配器因此在发送前把 `vendor_id` 注入 payload 的 `name` 字段。
3. **`InvokeDuration = 0` 表示"没能建立连接"**，不是"0 毫秒很快"。因此 `<=0` 统一记为 `-1`。

## 4. 配置

| 变量 | 说明 |
| --- | --- |
| `PHLOGO_CHECKER_WS_URL` | MiaoSpeed 的 WS 地址，如 `ws://127.0.0.1:65432/`。**未设置时 run 直接 `failed`**（不静默成功） |
| `PHLOGO_CHECKER_SECRET` | 内联鉴权 token |
| `PHLOGO_CHECKER_VENDOR` | 默认 `Clash`（载荷是 Clash proxy 对象）；`Local` 走直连语义 |
| `PHLOGO_DEBUG_FRAMES` | **仅调试**：把每个 WS 帧打到 stderr（截断 4000 字符）。排障时开，平时别开 |

## 5. 验证

### 5.1 mock（无需外部依赖，CI 可跑）

`cargo test -p worker --test check_wiring` → **2 passed**：
- `check_run_persists_adapter_results`：两个有载荷节点 → 适配器回 37 → 落库 `alive` + `latency=37`，
  无载荷节点落成 `unknown`，不存在的 id 被过滤，run 终态 `success`；**重复投递不重复计数**；
- `missing_endpoint_fails_the_run`：无端点 → run `failed`。

`cargo test -p worker` 全量：**4 passed**（+ 既有 `no_duplicate_work`、`fetch_loop_end_to_end`）。

### 5.2 真机（校机）

`examples/live_check` 驱动真实服务器（节点指向本机关闭端口，测活结果本就该是 dead）：

```
vendor=Clash url=ws://127.0.0.1:65432/ results=1
node=… latency_ms=1 deep=false
```

终态帧里 `ProxyInfo.Name = "phlogo-live-0"`, `Address = 127.0.0.1:1`, `Type = Trojan`,
`InvokeDuration = 1` → **适配器把它映射回内部节点 id**，`live_check` 退出码 **0**。

同批的 vmess 节点被 mihomo 拒绝（`ProxyInfo` 为空）：我们只给了 `uuid/cipher`，
mihomo 的 vmess 还需要 `alterId` 等字段。**这是载荷完整性/保真度问题**，与接线无关，
登记给 M4（deep/保真）一起处理。

### 5.3 门禁

`rustfmt --check`（3 文件）clean；`cargo clippy -p worker -p checker-miaospeed --all-targets -- -D warnings` **0**。

## 6. 遗留

1. **单位归一化**：`InvokeDuration` 原值直接当毫秒存（除 0→-1 外）。p3-03 的 `normalize`/
   `reconcile` 有完整逻辑，M4 接上（含 deep 矩阵 → `avg/jitter/speed/region/streaming`）。
2. **deep 结果**：当前只映射 initial（`TEST_PING_RTT`）；`deep: None`。
3. **调度**：本页只保证"任务来了能跑完"。谁生成 CheckJob（定时/手动/按分数阈值）是 **M3**。
4. **vendor 语义**：`Clash` 期待单个 mihomo proxy 对象；`Local` 期待什么（直连 URL？）
   没有在真机上验证过，登记。
5. **`PHLOGO_DEBUG_FRAMES` 会打帧内容**（不含 token，但含节点地址），生产别开。