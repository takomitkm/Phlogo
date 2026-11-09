## 6-03 订阅输出与 Token 认证 · Subscription Output & Token Auth

### 目标 / Goal

把 Phlogo 的公开订阅 `GET /{format}`（`internal/distribution/handler.go`）重写为带 **Token 认证 + 缓存失效** 的订阅链路，并产出三种格式：

- **Mihomo/Clash YAML**（复用 `internal/templates/clash.yaml` 模板：替换顶层 `proxies` 列表，并把每个节点名追加到每个 proxy-group）。
- **Sing-box JSON**（**独立转换器**，复用 `internal/templates/singbox.json` 模板：保留管理/路由 outbound，替换代理 outbound；**不是 Mihomo 配置的无损反向转换**，见 plan §10.1）。
- **Base64**（按目标客户端格式 base64 编码）。

新增：

- `subscription_profiles`（筛选 AST / 地区 / 健康 / 最低分 / 上限 / 排序 / 命名规则，方案 §10.2 + §5.1）。
- `subscription_tokens`（仅存哈希、可撤销、可轮换；profile 或节点状态变化时缓存失效，方案 §10.2）。

### 范围 / Scope

- **新建 / 修改** `crates/distribution/src/output/`：`mihomo.rs`（YAML 生成）、`singbox.rs`（独立 JSON 转换器）、`base64.rs`、`naming.rs`（节点命名，接 p5-01 AST）。
- **新建** `crates/distribution/src/handler.rs`（原 `internal/distribution/handler.go`）：`GET /sub/{token}.{yaml|base64|json}`。
- **新建** `crates/distribution/src/token.rs`：token 生成 / 哈希 / 撤销 / 轮换。
- **新建表**（迁移，方案 §5.1）：`subscription_profiles`、`subscription_tokens`；并把 `format_configs`（CONVENTIONS §2.3）的筛选/授权职责迁移过来。
- **复用**：p5-01 的 `filter` crate（AST 解析/SQL/求值）、p5-02 的 `select_with_preset`（候选集选区）。
- **不修改** Phlogo Go 源码（本任务只写文档）。

### 前置 / Prerequisites

- p5-01（Filter AST）与 p5-02（选区算法）已完成并可被 `distribution` crate 依赖。
- `internal/templates/clash.yaml`、`singbox.json`、`templates.go` 已读（模板形状与 `{"template": "<raw>"}` 包装格式，见 `templates.go` 的 `wrap`）。
- `internal/naming/naming.go` 已读：`defaultTemplate`、`regionOrder`、`flags.json`、评分阈值（`grade_a_min=0.75` 等）、`RegionName`/`flagFor`/`seqID`、非 common preset 用全称地区名（CONVENTIONS §2.6、§2.8）。
- `configs` 默认键已 seed（CONVENTIONS §2.6）：`max_distribute=200`、`filename_clash`/`filename_singbox`/`filename_base64`、`node_name_template`、`grade_*_min`。
- 目标 **Mihomo 版本已固定**（P0）；其 config-check 子命令已确认（见 §风险）。

### 步骤 / Steps

#### 1. 数据模型：`subscription_profiles` / `subscription_tokens`

基于方案 §5.1 的字段，并按 §10.2 扩充（每行一个列）：

```sql
CREATE TABLE subscription_profiles (
    id              UUID PRIMARY KEY,
    name            TEXT NOT NULL,
    filter_ast      JSONB,          -- p5-01 AST（plan §9.2 形状），可空=不过滤
    region          TEXT,           -- 地区条件（可选，叠加在 filter_ast 之上）
    health          TEXT DEFAULT 'alive',  -- 健康要求 alive|degraded|dead
    min_score       DOUBLE PRECISION,      -- 最低评分阈值（0..1）
    limit_count     INT  DEFAULT 200,     -- 节点数上限（与 max_distribute 取小）
    order_strategy  TEXT DEFAULT 'score_desc', -- 排序/抽样策略（接 p5-02 preset）
    preset          TEXT DEFAULT 'common', -- common|all|other（p5-02）
    output_format   TEXT NOT NULL,  -- mihomo|singbox|base64（或主格式 + 多格式）
    naming_rule     TEXT,           -- 命名模板（默认 node_name_template，CONVENTIONS §2.6）
    enabled         BOOLEAN DEFAULT TRUE,
    version         BIGINT NOT NULL DEFAULT 1  -- 缓存失效用：每次更新 +1
);

CREATE TABLE subscription_tokens (
    id           UUID PRIMARY KEY,
    profile_id   UUID NOT NULL REFERENCES subscription_profiles(id),
    token_hash   TEXT NOT NULL,     -- 仅存哈希，不存明文（见 §4）
    status       TEXT NOT NULL DEFAULT 'active', -- active|revoked
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ,
    UNIQUE (token_hash)
);
```

> `format_configs` 表（CONVENTIONS §2.3）的「筛选/授权」职责并入上表；格式模板仍可由 `subscription_profiles.output_format` 选择 `clash.yaml` / `singbox.json` 内嵌模板。

#### 2. 候选集生成（串联 p5-01 + p5-02）

`handler.rs` 处理 `GET /sub/{token}.{fmt}`：

```
1. 解析 token → token_hash；查 subscription_tokens：
   - 不存在 / status=='revoked' → 401（见 §验收 3）。
2. 取 profile（enabled 校验）；用 profile.filter_ast → p5-01 parse → to_sql/matches 过滤候选
   （候选来自 P4 的存活节点缓存 / node_status，按 tag）。
3. 叠加 profile.region / health / min_score 硬条件（等价于在 AST 上追加 And 条件）。
4. preset 解析（URL preset > tag.default_preset > 全局 default_preset > common，CONVENTIONS §2.8）
   传给 p5-02 select_with_preset，limit = min(profile.limit_count, max_distribute)。
5. 对选中节点用 naming 规则（§5）生成名称，交由格式生成器输出。
6. 输出写入按 (token_hash, profile.version, checker_tag) 键的缓存（见 §6 失效）。
```

对应 Phlogo `handler.go` 的步骤：`format.Get` → 解锁过滤 → preset 选择 → `naming.Render` → `gen.Push` → `gen.Generate` → `Content-Disposition`（`filename_*` + RFC 5987 双 token，见 `contentDisposition`）。

#### 3. Mihomo/Clash YAML 生成（`mihomo.rs`，对应 `clash.yaml` 模板）

- 以 `internal/templates/clash.yaml` 为模板（通过 `templates.go` 的 `{"template":"<raw>"}` 包装读取）。
- 替换顶层 `proxies:` 列表为选中节点的 proxy 对象（每个节点由 `NormalizedNode` → clash proxy schema 转换）。
- 把每个生成节点名追加到模板中所有 `proxy-groups[].proxies`（模板里 `自动选择`/`故障转移` 两组 `proxies: []`，需填充）。
- 输出 YAML 含 `mixed-port`/`dns`/`rules` 等原样保留（CONVENTIONS §2.8 的公开订阅地址表：`/clash` 默认文件名 `Paily Connect.yaml`）。
- 设置 `Content-Type: text/yaml; charset=utf-8`、`Content-Disposition`（ASCII fallback + `filename*=UTF-8''` 双 token，与 Go `contentDisposition` 一致）。

#### 4. Sing-box JSON 生成（`singbox.rs`，**独立转换器**，plan §10.1）

- 以 `internal/templates/singbox.json` 为模板（`{"template":"<raw>"}`）。
- **保留**模板里的管理/路由结构：`inbounds`（tun/socks/mixed）、`route.rule_set`、`dns`、`experimental`、`log`。
- **替换**代理 `outbounds`：模板中 `outbounds` 含 `select`/`urltest`/`direct`，把 `urltest` 的 `outbounds: []` 与 `select` 的 `outbounds` 填充为选中节点转换出的 sing-box outbound 对象（**sing-box 独立 schema：`type`/`server`/`port`/`...` 与 clash 不同**）。
- **不得**把 Mihomo proxy 对象反向转成 sing-box（plan §10.1 明确要求「独立的 Sing-box 转换器，不能假设 Mihomo 配置可以无损反向转换」）；两者各有自己的 `NormalizedNode → outbound` 映射函数。
- 输出 `Content-Type: application/json; charset=utf-8`（`/singbox` 默认文件名 `Paily Connect.json`）。

#### 5. 节点命名（`naming.rs`，对应 `internal/naming/naming.go`）

- 复刻 `Render` 逻辑：评分阈值来自 config（`grade_a_min=0.75`/`grade_b_min=0.50`/`grade_c_min=0.25`），`gradeFromScore` → A/B/C/D；`flagFor`（emoji，`flags.json`）；`RegionName`（非 common preset 用全称）；进入模板的字段 `node.{region,region_flag,grade,score,seq,server,protocol,alive,streaming,sponsor}` + `unlock_flags` + `filter_mode`（同 `namingEnv`）。
- **命名模板表达式**：本步仍沿用 expr 风格 `node_name_template`（CONVENTIONS §2.6 默认值），其求值可暂用 expr 引擎或等价 AST；p5-01 已把「布尔筛选」迁到 AST，**命名模板迁移不在本步强制**（CONVENTIONS §2.8 的 expr→AST 覆盖项之一，可作为后续细化）。
- `seq` 为地区内两位序号（`%02d`）；最终 `seq_id` 形如 `hk01`（由 p5-02 `assemble_output` 给出）。
- 命名失败 fallback：`<region> <seq>`（对应 `naming.go` 的 `fmt.Sprintf("%s %s", region, seqID(...))`）。

#### 6. Token 生成 / 哈希 / 撤销 / 缓存失效

`token.rs`：

```rust
// 明文 token 仅在一次创建响应里返回给用户；库内只存哈希
pub fn issue() -> (plain: String, hash: String) {
    let plain = random_token(32);            // 32 字节随机，hex/base62
    let hash  = hash_token(&plain);          // 见下
    (plain, hash)
}
// 哈希：优先 argon2id（慢哈希，防库泄露）；或 sha256(plain + pepper)。
// 本页不指定算法，但要求：明文永不入库、可加盐、可轮换。
pub fn verify(plain: &str, hash: &str) -> bool { constant_time_eq(&hash_token(plain), hash) }
```

- **撤销**：`UPDATE subscription_tokens SET status='revoked', revoked_at=now() WHERE id=$1`。已撤销 → 订阅返回 401/空（§验收 3）。
- **轮换**：签发新 token（新 hash），旧 token 可保留为 `revoked` 或直接删除。
- **缓存失效**（`subscription_profiles.version` + 节点变化）：
  - profile 更新 → `version += 1` → 缓存键 `(token, version, tag)` 自然失效（读时按新 version 重算）。
  - 节点健康/评分变化（来自 P4 生命周期）→ 触发分发缓存重建（CONVENTIONS §2.8、§2.9），所有依赖该 tag 的订阅缓存键失效。
  - Token 撤销 → 命中 `subscription_tokens.status='revoked'` 直接拦截，不读缓存。

### 验收 / Acceptance

下列命令通过（退出码 `0`）：

```bash
cargo test -p distribution subscription::
```

1. **Mihomo YAML 通过目标版本校验**（方案 §15：「生成的 Mihomo YAML 能通过目标 Mihomo 版本校验」）：
   ```bash
   # 用目标 Mihomo 二进制的 config-check 模式（版本相关，见 §风险）
   mihomo check -d /tmp/mihomo-test <generated>.yaml
   echo $?   # 期望 0
   ```
   单测里：把生成器输出写到临时文件，调用该命令（或等价 schema 校验），断言退出码 `0`。同时断言顶层含 `proxies`（非空）且每个 proxy-group 的 `proxies` 已填充节点名。
2. **结果符合筛选条件**（方案 §15 + P5 验收）：对 `subscription_profiles` 设 `filter_ast = {"operator":"and","conditions":[{"field":"region","operator":"eq","value":"JP"},{"field":"score","operator":"gte","value":0.7}]}`，生成订阅后断言：每个输出节点的 `region=="JP"` 且 `score>=0.7`。
3. **已撤销 Token 无法读取**（方案 §15：「未授权或已撤销 Token 无法读取订阅」）：
   ```bash
   curl -s -o /dev/null -w '%{http_code}' /sub/<revoked_token>.yaml   # 期望 401
   # 且对 .base64 / .json 同口径返回 401
   ```
   单测：`issue()` → 写入 `active` → 断言 `verify` 通过且 `/sub/{token}.yaml` 返回 200；`revoke()` → 同 token 返回 401（或空体）。
4. **缓存失效被测试**：
   - 初次 `/sub/{token}.yaml` 写入缓存；`revoke(token)` → 再次请求 401（缓存不返回旧内容）。
   - `update_profile(profile_id)` 使 `version+=1` + 改变 `filter_ast` → 下次请求结果集变化（新 version 重算，旧缓存键失效）。
   - 模拟节点健康变化（P4 触发缓存重建）→ 再次请求输出反映新健康状态。
5. **Sing-box 是独立转换**：断言 `singbox.rs` 的输出对象带 sing-box 专属字段（`type`/`server`/`port`…），且**不是**由 Mihomo proxy 对象反序列化得到（代码层面两个映射函数各自独立；可加单测断言两者 schema 结构不同/字段集不重叠于 clash-only 字段）。

预期输出：`test result: ok. N passed; 0 failed`（N ≥ 5 组）。

### 与 Phlogo 现状对照 / Mapping from current Phlogo

| Phlogo 现状 | 重构后 |
|---|---|
| `internal/distribution/handler.go` `Distribute` `GET /{format}`（无 token） | `crates/distribution/src/handler.rs` `GET /sub/{token}.{yaml|base64|json}`（带 token 认证） |
| `format.Get(formatName)` + `factory.NewGenerator/Generate` | `output/mihomo.rs` / `singbox.rs` / `base64.rs` 生成器 |
| `internal/templates/clash.yaml`（替换 `proxies`、填 proxy-groups） | `output/mihomo.rs` 模板内嵌，同逻辑 |
| `internal/templates/singbox.json`（替换代理 outbound） | `output/singbox.rs` **独立转换器** |
| `internal/templates/templates.go` `wrap` → `{"template":"<raw>"}` | config crate 同形状存储种子 |
| `assembleUnlockFlags` / `nodeMatchesUnlocks` / `nodeMatchesAnyUnlock` | `handler.rs` 解锁过滤（select=交集 / mark=并集，CONVENTIONS §2.8） |
| `naming.Render` + `flags.json` + `defaultTemplate` | `output/naming.rs`（评分/国旗/全称/seq），模板求值暂沿用 expr |
| `configs.filename_*` / `configs.node_name_template` | `subscription_profiles.naming_rule` + config 默认值（CONVENTIONS §2.6） |
| `format_configs` 表（筛选/授权混存） | 拆为 `subscription_profiles` + `subscription_tokens`（方案 §5.1） |
| 无 token / 无撤销 / 无缓存失效 | `subscription_tokens`（哈希/撤销/轮换）+ `version` 缓存失效 |

### CI 提示 / CI notes（仅描述，不落地 YAML）

- 单测：`cargo test -p distribution subscription::`（YAML 校验 / 筛选条件 / token 撤销 / 缓存失效 / singbox 独立，见 §验收 1–5）。
- 集成（`cargo test --test integration`，需 PostgreSQL + 固定版本 Mihomo 二进制）：
  - 起临时 Postgres → 插入 profile/token → 调订阅路由 → 取 YAML → 跑 `mihomo check` 断言退出码 0。
  - 撤销 token → 断言 401；更新 profile → 断言输出变化（缓存失效）。
- 格式 / Lint：`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets -- -D warnings`。
- 安全：`cargo audit`（确认无把 token 明文落库的依赖误用）；`cargo deny check licenses`。

### 风险 / Risks

- **Mihomo 版本耦合（最关键）**：config-check 子命令随版本变化（历史上 `mihomo check` / `clash` 子命令与 schema 均有差异）。P0 必须**固定目标 Mihomo 版本**并实测其校验命令；本页 §验收 1 的 `mihomo check -d <dir> <file>` 是常见形态，但**确切 flag 以该版本 `mihomo --help` 为准**，CI 里应 pin 版本 + 缓存二进制 + 记录 SHA-256（CONVENTIONS §2.11、方案 §16）。
- **Sing-box 不可由 Mihomo 反向**：plan §10.1 明确禁止「无损反向转换」。若偷懒把 clash proxy 转 sing-box 会丢字段（如 sing-box 的 `transport`/`tls` 结构与 clash 不同），必须各自实现 `NormalizedNode → outbound` 映射（§4）。
- **缓存失效时机错误**：profile 更新或节点健康变化若未正确 bump `version` / 触发重建，会出现「撤销后仍能读到旧缓存」或「筛选改了但订阅没变」——§验收 4 必须显式覆盖三条失效路径。
- **Token 明文泄露**：`issue()` 的明文 token 只应在创建响应返回一次；`subscription_tokens` 只存 `token_hash`（argon2id 或加盐 sha256），日志不得打印明文（方案 §16 第 8 条）。
- **命名模板仍依赖 expr**：本步命名沿用 `node_name_template` expr 求值，p5-01 仅替换布尔筛选；命名模板的 AST 化是后续细化项（CONVENTIONS §2.8），迁移前 expr 引擎不得删除，否则 fallback 名退化为 `<region> <seq>`。
- **`filename_*` 默认值**：CONVENTIONS §2.6 列出 `filename_clash/singbox/base64` 但本任务未读取其具体默认值（Go 里是 `config.example.yaml` 的 `filename_*`）。若走 `configs` 表种子，需确认种子包含这三个键，否则 `Content-Disposition` 缺文件名。
