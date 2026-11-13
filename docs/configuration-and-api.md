# 配置与 API

本文档介绍 Paily 主服务的配置层次、可调参数、筛选引擎、节点命名与 sponsor text，以及接口权限划分。

## 配置层次

Paily 主服务的配置分为两层：

1. **启动配置**：通过 YAML 文件或环境变量提供，涵盖监听端口、数据库、认证和测活后端列表等，需重启生效。
2. **动态配置**：保存在数据库中，涵盖评分、分发、历史保留和命名等参数，可在管理界面修改，无需重启。

启动配置的优先级为：

```text
环境变量 > YAML 配置文件 > 内置默认值
```

环境变量统一以 `PAILY_` 为前缀，嵌套键中的 `.` 替换为 `_`。例如 `server.port` 对应 `PAILY_SERVER_PORT`。

## 启动配置

| YAML 键 / 环境变量 | 说明 |
| --- | --- |
| `server.port` / `PAILY_SERVER_PORT` | HTTP 监听端口，内置默认值 `21380`。 |
| `server.proxy_protocol` / `PAILY_SERVER_PROXY_PROTOCOL` | 接受 PROXY protocol v1 头（反代后取真实客户端 IP，限流按真实 IP 分桶）。 |
| `server.expect_proxy` / `PAILY_SERVER_EXPECT_PROXY` | 与上项连用：**要求**每个连接都带 PROXY 头（API 端口不对外时的纵深防御）。 |
| `server.verbose` / `PAILY_SERVER_VERBOSE` | 是否记录 debug 级日志。 |
| `database.dsn` / `PAILY_DATABASE_DSN` | PostgreSQL DSN（**仅支持 PostgreSQL**）。 |
| `PAILY_DATABASE_JOBS_DSN` | Apalis 队列库 DSN（默认由主 DSN 推导出 `paily_jobs`）。 |
| `auth.admin_password` / `PAILY_AUTH_ADMIN_PASSWORD` | 管理员密码（登录用）。 |
| `auth.service_secret` / `PAILY_AUTH_SERVICE_SECRET` | 服务身份密钥（读面 + 源 intake + fetch trigger）。 |
| `auth.jwt_secret` / `PAILY_AUTH_JWT_SECRET` | JWT HMAC 密钥。 |
| `PAILY_LOGIN_RATE_LIMIT_PER_MIN` | 登录限流阈值（默认 10/min，按真实客户端 IP 分桶）。 |
| `PAILY_TOKEN_PEPPER` | 订阅 token 哈希 pepper。 |

## 动态配置

动态配置可在管理界面的「运行时配置」页面修改，首次启动时会写入缺失的键。

> **当前实现状态（2026-11-13 复核）**：✅ = 修改即生效；⏸ = 键已存在但暂无消费者（改它没效果，存在理由见表）。

| 分类 | 键 | 默认值 | 说明 | 状态 |
| --- | --- | ---: | --- | --- |
| 分发 | `max_distribute` | `200` | 单次订阅最多输出的节点数。 | ✅ |
| 分发 | `node_distribute_filter` | 空 | 命中表达式的节点不进入分发缓存。 | ⏸ 语义由订阅档案的 `filter_ast` 覆盖；键保留待清理 |
| 存活 | `node_alive_check_window_minutes` | `240` | 最新初筛的存活窗口，单位分钟。 | ✅ |
| 评分 | `score_decay_alpha` | `0.7` | 评分的时间衰减系数（小数或整数百分数均可）。 | ✅ |
| 评分 | `score_weight_latency` | `0.7` | 延迟权重。 | ✅ |
| 评分 | `score_weight_stability` | `0.3` | 稳定性权重。 | ✅ |
| 评分 | `score_weight_speed` | `0.55` | 有测速数据时的速度混合权重。 | ✅ |
| 评分 | `score_latency_curve_k` | `0.3` | 延迟曲线参数（须 < 2，≥ 2 会被当百分数）。 | ✅ |
| 评分 | `score_jitter_linear_threshold_ms` | `200` | 抖动分段阈值，单位毫秒。 | ✅ |
| 评分 | `score_min_samples` | `1` | 参与评分的最少样本数。 | ✅ |
| 命名 | `grade_a_min` | `0.75` | A 等级下限（订阅渲染消费）。 | ✅ |
| 命名 | `grade_b_min` | `0.50` | B 等级下限。 | ✅ |
| 命名 | `grade_c_min` | `0.25` | C 等级下限，低于此值为 D。 | ✅ |
| 生命周期 | `orphan_retention_days` | `14` | 孤儿节点保留天数，超期 Retired 后物理删除；`0` = 永不删。 | ✅ |
| 历史 | `fetch_dead_window_days` | `7` | 抓取失效判定窗口，单位天。 | ⏸ 待 B1 真测活后接线 |
| 历史 | `node_dead_window_days` | `4` | 节点测活失效判定窗口，单位天。 | ⏸ 同上 |
| 历史 | `fetch_history_window_days` | `7` | 抓取历史基础保留天数。 | ⏸ 抓取历史清理暂未实现 |
| 历史 | `check_run_history_days` | `14` | 测活明细保留天数（A5）；`0` = 永不删。 | ✅ 清理周期删除超期记录 |
| 历史 | `cleanup_interval_minutes` | `30` | 清理间隔，单位分钟。 | ✅ worker 调度消费 |
| 命名 | `node_name_template` | （expr 模板） | 节点命名模板。 | ⏸ 由订阅档案的 `naming_rule` 覆盖；键保留待清理 |
| 文件名 | `filename_clash` | `Paily Connect.yaml` | Clash 下载文件名。 | ⏸ 现由 URL 格式后缀决定；键保留待清理 |
| 文件名 | `filename_singbox` | `Paily Connect.json` | Sing-box 下载文件名。 | ⏸ 同上 |
| 文件名 | `filename_base64` | `Paily Connect.txt` | Base64 下载文件名。 | ⏸ 同上 |
| sponsor | `default_sponsor_text` | 空 | 无 sponsor 命中时的默认文案。 | ✅ sponsor 匹配落空时使用 |
| 判死 | `dead_latency_ms` | `3000` | 判死延迟门限。 | ⏸ 当前不参与存活判定；不可达以初筛延迟 `-1` 表示 |

## 筛选引擎

筛选已从上游的 expr-lang 字符串表达式迁移为**类型化 Filter-AST JSON**（字段/运算符双白名单 + 参数化 SQL，杜绝注入）。用于节点/来源搜索（`POST /api/v1/nodes/search`、`POST /api/v1/sources/search`）、筛选预览（`POST /api/v1/filter/preview`）和订阅档案的 `filter_ast`。

AST 形状（逻辑节点带 `operator: and|or|not`，叶子是条件对象）：

```json
{"operator":"and","conditions":[
  {"field":"protocol","operator":"eq","value":"vless"},
  {"field":"score","operator":"gte","value":0.7},
  {"field":"region","operator":"ne","value":"UN"}
]}
```

节点字段白名单：`health, region, score, protocol, server, source_id, unlock`。
来源字段白名单：`id, identifier, type, status, info`。
运算符白名单：`eq, ne, gt, gte, lt, lte, in, not_in, contains, is_null`。
限制：AST 深度 ≤ 8、条件数 ≤ 32；未知字段/运算符返回 400。

已知限制：`unlock`（流媒体解锁）条件在 SQL 下推中恒为假（`node_streaming` 表随 deep-check 接线后启用）。

## 节点命名与 Sponsor

节点命名由订阅档案的 `naming_rule`（`{{field}}` 白名单模板，可用字段 `grade/region/region_flag/seq/unlock_flags/sponsor`）驱动，默认输出形如：

```text
[解锁服务] 等级 - 国旗 地区 序号 Sponsor 文案
```

序号按地区内评分排序后单独编号，例如 `hk01`、`hk02`、`tw01`。解锁标记仅在指定了流媒体筛选且满足显示条件时出现。

**Sponsor（赞助商文案注入）已实现**：`GET/POST /api/v1/sponsors`、`GET/PATCH/DELETE /api/v1/sponsors/{id}`（AdminOnly）。每条 sponsor 带 `text`、`filter_ast`（同筛选引擎的 AST JSON）和 `priority`。匹配规则与上游一致：**priority 升序依次求值，后命中覆盖前者，数值更大的 priority 最终获胜**；没有任何命中时使用 `default_sponsor_text`（默认空 = 不加文案）。命中的文案注入节点名的 `{{sponsor}}` 字段。

## 格式配置

`GET/PUT /api/v1/format-configs[/{format}]` 管理 `clash`、`singbox`、`base64` 各自的 JSON 配置（`format_configs` 表）。**写入时服务端会用渲染同款解析器校验模板**（坏模板当场 400，不会等到订阅请求 500）；未存储覆盖时使用内嵌默认模板。Clash 与 Sing-box 的模板编写方法见 [订阅模板](templates.md)。

## API 与权限

三层身份，按**路由权限矩阵**强制（代码：`apps/api/src/auth.rs::auth_kind`；**fail-closed**——任何未列入矩阵的 `/api/v1` 路由一律要求 AdminJWT）：

| 身份 | 凭据 | 可访问范围 |
| --- | --- | --- |
| 终端用户 | 订阅 token（URL 内） | `GET /sub/{token}.{fmt}`；以及 `/health`、`/api-docs/openapi.json`。 |
| 管理员（AdminJWT） | 登录签发的 access JWT（24h） | 全部管理 API（含所有写操作）。 |
| 服务身份（ServiceBearer） | `service_secret` | **最小面**：全部 GET 读、`POST /api/v1/sources`（intake）、`POST /api/v1/fetch/trigger`。**管理写（PATCH/DELETE/PUT）返回 403**。 |

- MiaoSpeed WebSocket 是 worker 内部调用，不经 HTTP 权限面。
- 登录限流：按真实客户端 IP（PROXY protocol）分桶，默认 10 次/分钟，超限 429；`X-Forwarded-For` 永不信任。
- 错误码语义：401 = 缺失/无效凭据；403 = 凭据有效但身份越权（ServiceBearer 打管理写）。

完整接口定义见运行时 `GET /api-docs/openapi.json`（utoipa 源头生成，27 条路径），源文件 `crates/api-contract/src/`。
