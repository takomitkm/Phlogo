# 重构总行为清单 · Rebuild Master Action Log

> 目的：记录 Phlogo → Rust 重构过程中的**每一步与每一条命令**，每条目开头给出 **UTC 时间戳**。
> 用途：可复现、可审计、将来据此写 CI。约定见 [CONVENTIONS.md](CONVENTIONS.md)。
>
> 约定 / Conventions for this log:
> - 条目格式：`## [UTC yyyy-MM-ddTHH:mm:ssZ] <步骤号/标题>`，正文含 `动作`、`命令`、`结果`、`决策`。
> - 命令一律原样记录（含完整路径与参数），便于将来复制成 CI 步骤。
> - 重量级编译（cargo build/test、迁移、集成测试）**一律在校机执行**，见「执行环境」。

## 执行环境 · Execution Environment

| 角色 | 机器 | 说明 |
|---|---|---|
| 编辑/提交主机 | `tkmnopc`（用户 `takomi`） | 有 git、network、SSH client；**无 cargo/rustc/go/gcc** |
| 重量级编译校机 | `WIN11-HV1`（用户 `YiZhidi`），经 `ssh school-guest`（`127.0.0.1:13401`，key `C:/Users/Public/app/ssh-school/shell_key`） | Rust：`D:\yzd\tools\rust\cargo\bin\{cargo,rustc}.exe`；MinGW GCC 16.2.0：`D:\yzd\MSYS2\mingw64\bin\gcc.exe`；aarch64 交叉 GCC 10.3.1 见 `S:\work\WCIS\wiki\schoolhardware\toolchains.md` |

- 远端执行模板：`ssh -o BatchMode=yes -o ConnectTimeout=20 school-guest "cmd /c \"chcp 65001 >nul & <命令>\""`（`chcp 65001` 规避 GBK 乱码）。
- 待校机验证项：cargo 版本、`crates.io` 可达性、`sqlx-cli`/PostgreSQL 是否具备、headless 下 `cargo build` 能否无人值守运行。

---

## [UTC 2026-11-09T20:34:00Z] S0 · 编写重构步骤文档（P0–P7）

**动作**：以 `paily-core-rust-rebuild-plan.md` 为大纲，先摸清 Phlogo 真实代码（fork 自 `openpaily/paily-core`，Go 1.24 单体），再产出步骤文档。

**命令**（编辑主机）：
```
git clone --depth 1 https://github.com/takomitkm/Phlogo.git repos/Phlogo
# 逐文件读取：cmd/server/main.go、go.mod、internal/{db/model,checker,scoring,naming,distribution,filter,source,node}/*.go、api/openapi/spec.yaml、config.example.yaml
```

**结果**：写入 `docs/rebuild/` 共 **38 个 Markdown**：`CONVENTIONS.md`（事实来源）、`00-index.md`（总览）、8 个阶段目录（P0–P7，各 `index.md` + 更细子步骤页）。每页 9 节模板（目标/范围/前置/步骤/验收/现状对照/CI 提示/风险），中英双语，只描述未来 CI 命令、未落地任何 CI 文件。校验：`git status` 仅 `docs/rebuild/` 未跟踪、无源码改动、无 CI YAML 生成。

**决策**：本期只写文档，不写 CI；细节留待实施途中按需修订。

---

## [UTC 2026-11-09T21:06:56Z] S1 · 探明执行环境（编辑机 vs 校机）

**动作**：确认可编译环境，为「重编译去校机」建立通路。

**命令**（编辑主机 `tkmnopc`）：
```
hostname; whoami                                  # tkmnopc / tkmnopc\takomi
Get-Command cargo,rustc,go  # 均不在 PATH
Test-Path D:\yzd\MSYS2\mingw64\bin\gcc.exe        # False（本机无校机 D 盘）
Get-Command ssh                                   # C:\WINDOWS\System32\OpenSSH\ssh.exe（有）
Get-Content C:\Users\takomi\.ssh\config           # 定义 Host school-guest -> 127.0.0.1:13401, User YiZhidi, key C:/Users/Public/app/ssh-school/shell_key
```

**命令**（经 SSH 到校机）：
```
ssh -o BatchMode=yes -o ConnectTimeout=20 school-guest "cmd /c \"chcp 65001 >nul & where cargo rustc go node 2>nul & if exist C:\Users\YiZhidi\.cargo\bin\cargo.exe (echo CARGO_FOUND) else (echo CARGO_MISSING) & D:\yzd\MSYS2\mingw64\bin\gcc.exe --version\""
```

**结果**：
- SSH 通路可用（`school-guest` 成功连上校机）。
- 校机存在 Rust：`D:\yzd\tools\rust\cargo\bin\cargo.exe`、`D:\yzd\tools\rust\cargo\bin\rustc.exe`。
- 校机 MinGW：`gcc.exe (Rev6, Built by MSYS2 project) 16.2.0`。
- `where go`/`where node` 本次未命中（Go/Node 在 P0 验证阶段需要时再定位或安装）。

**决策**：
1. 采用「本机编辑+git 提交推送 / 校机 SSH 远程 `cargo` 编译」的双机工作流。
2. Rust 工程放 `Phlogo/gnosis-rs/`（Cargo workspace），保留 Go 树作参考，避免与 `go.mod` 根目录混编；待 P1 落地时据此细化目录与 CONVENTIONS §6。
3. 编译产物/日志从校机回传编辑机后再 `git add`/`commit`/`push`，满足「做一步 push 一步」。

---

## [UTC 2026-11-09T21:17:14Z] S2 · 校机 Rust 工具链冒烟验证

**动作**：验证校机 `cargo` 能离线编译、能经 crates.io 拉取并编译第三方 crate（决定后续 `cargo build/test` 是否真可无人值守跑）。

**通路**：编辑主机把本地脚本 Base64(UTF-16LE) 编码后用 `ssh -EncodedCommand` 送到校机执行，结果写文件回传（规避引号/GBK 乱码）。脚本：`.tmp/xiaoji-run.ps1`（runner）+ `.tmp/xiaoji-smoke.ps1`（smoke）。

**命令**（编辑主机）：
```
pwsh -NoProfile -ExecutionPolicy Bypass -File .tmp\xiaoji-run.ps1 -ScriptPath .tmp\xiaoji-smoke.ps1 -OutFile .tmp\xiaoji-smoke.out.txt
```

**结果**（校机）：
- `cargo 1.85.1` / `rustc 1.85.1`；host triple **`x86_64-pc-windows-gnu`**（链接走 MinGW `libgcc`，非 MSVC）。
- 离线 `cargo new + cargo build --offline` → `BUILD_OFFLINE_EXIT=0`。
- `cargo add serde --features derive`（经 `rsproxy-sparse` 镜像）→ `CARGO_ADD_EXIT=0`；`cargo build` → `BUILD_NET_EXIT=0`（8.4s）。

**决策**：
1. 后续 `cargo build`/`cargo test`/`cargo clippy` 一律在**校机**跑；本机仅编辑+git。
2. host 是 GNU（非 MSVC）：依赖里凡需要 MSVC 的（少见）要避开；GNU 下用 `rustls` 而非 `native-tls`，PostgreSQL 走 `sqlx` + `rustls` 无 C 依赖，OK。
3. `git push` 需带 PATH 的 git：校机 git 在 MSYS2 `usr/bin`（待用到时确认），本机推送即可，校机只编译不回推。

---

## [UTC 2026-11-09T21:06:56Z] S1b · 文档首推（docs/rebuild → GitHub main）

**命令**（编辑主机）：
```
git add docs/rebuild
git commit -m "docs(rebuild): add P0-P7 rebuild step docs, conventions, master action log"
git push origin main          # → dda4e6f..110d470 main -> main
git add docs/rebuild/ACTION-LOG.md
git commit -m "docs(rebuild): log校机 Rust toolchain smoke (cargo 1.85.1 GNU, crates.io OK)"
git push origin main          # → 110d470..182631b main -> main
```

**结果**：S0 + S2 均已推送（GCM 凭据缓存有效，HTTPS push 可用）。

---

## [UTC 2026-11-09T21:42:53Z] S3 · P0-01 VProxy 依赖面调查（信源：GitHub API + Cargo.toml）

**动作**：核查 `mateo-ki/vproxy` 是否可作 Rust 库依赖复用，替代读死文档。

**命令**（编辑主机）：
```
GET https://api.github.com/repos/mateo-ki/vproxy          # Rust, MIT, pushed 2026-11-09, main
GET https://raw.githubusercontent.com/mateo-ki/vproxy/main/Cargo.toml
```

**关键事实**：
- 单 crate 应用：`[lib] name=vproxy, crate-type=["rlib","cdylib","staticlib"]` + `[[bin]]`；**无 workspace 子 crate**。
- `default = ["gui"]`，GUI 才 optional；但**基础依赖硬带** `rhai`（脚本引擎）、`sqlx`（sqlite）、`axum`、`ring`、`tokio[full]`——即使 `default-features=false` 也会拖进 rhai/sqlite。
- build.rs + `embed-resource`；profile.release `lto="thin"`、`strip=true`。

**决策（记入 p0-01 结论）**：
1. **不把 vproxy 作为 lib 依赖**——违反方案 §3.1「不让上游类型/依赖渗透」且引入 rhai+sqlite 不需要的攻击面/体积。
2. 改为「**最小化移植**」路线：仅移植 `parser/fingerprint/export` 必需算法，保留 MIT 版权声明（方案 §3.1 末句授权的路线）。
3. P0-01 spike 的落地形式从「编译一个依赖 vproxy 的 scratch crate」改为「**在本机把 vproxy clone 下来，定位 parser/fingerprint/export 源文件并评估可移植性**」；`docs/THIRD_PARTY.md` 仍需锁 commit。

---

## [UTC 2026-11-10T00:04:00Z] S4 · P0-02 MiaoSpeed 固定版本：下载/清点/抽 BUILDTOKEN

**动作**：确定 MiaoSpeed 固定版本，取二进制基线，并从二进制抽出签名盐 `BUILDTOKEN`。

**命令**（编辑主机，脚本放 `.tmp/`）：
```
# 下载 4.7.9 windows-amd64 资产（Invoke-WebRequest，GitHub 可直连；git 直连超时但 https 下载可用）
Invoke-WebRequest https://github.com/AirportR/miaospeed/releases/download/4.7.9/miaospeed-windows-amd64-4.7.9.zip
Get-FileHash -Algorithm SHA256  # zip=685a8a58…e1d159 ; exe=5b0ec1ad…ac9611
# 校机运行 -version → 4.7.9, commit 0fafd30
# 扫描 exe 字符串表（ISO-8859-1 视图 + 正则 [A-Za-z0-9_]{2,}(?:\|…){2,}）→ 命中内嵌 BUILDTOKEN
```

**结果**：
- 内嵌编译 TOKEN = `MIAOKO4|580JxAo049R|GEnERAl|1X571R930|T0kEN`（与 `miaospeed.sh` 默认一致）。
- GitHub 预编译 commit `0fafd30` 在公开仓库 **API 返回 422（查无此 SHA）** → 预编译资产不可复现。

**决策**：PIN 采用 **自建（源码 tag `4.7.9`）**，不把预编译资产作供应链基线。

---

## [UTC 2026-11-10T00:16:00Z] S5 · P0-02 协议逆向 + Vendor 签名 quirk（Go 交叉验证）

**动作**：从 4.7.9 源码提取 WS 协议；用 Go 工具链做「标准答案」交叉验证 Rust 签名；发现并定位 `verify=false` 根因。

**命令**（校机 Go：`D:\yzd\magpie-go-toolchain-20261107\go\bin\go.exe`；GOPROXY=`https://goproxy.cn,...`）：
```
# a) 复刻 jsoniter+hashMiaoSpeed 的独立验证器（用镜像结构体）→ 与 Rust 逐字节一致
# b) 关键：在真实 miaospeed 模块内 import interfaces+utils，调 utils.SignRequest() 取真值
go run ./verifytool   # REAL_JSON 中 "Vendor":"" ；REAL_SIG=fKdMhefuCZz2RmiaQxar5SLkAMnLd5i7D1_ZUYSHCpQykOT_smWDJ-pnhmTgSyOnJl7c97mR90bw6UIe4hMJ9Q==
```

**关键发现**：
- 认证=逐请求内联 `Challenge`（`VerifyRequest: req.Challenge == SignRequest(req)`，服务端重序列化解码后的结构体再比对）。
- **致命 quirk**：`interfaces.SlaveRequest.Clone()` 4.7.9 版**漏拷 `Vendor`**，而 `SignRequest` 对 `Clone()` 结果签名 → 签名恒用 `"Vendor":""`。
  正确做法：**算签名用 `Vendor=""`，发送用真实 `Vendor`**。此前用 `Local` 算 → `verify=false`；改后 `verify=true`。

**决策**：P3 必须复刻此 quirk；建议 P3 契约测试用 `utils.SignRequest` 交叉验证一条固定样例，防源码行为漂移。

---

## [UTC 2026-11-10T00:24:00Z] S6 · P0-02 自建 4.7.9 + Rust spike 端到端验证（verify=true）

**动作**：从 tag `4.7.9` 源码自建二进制（补齐 embed 占位 + Go 代理镜像），用最小 Rust WS 客户端跑通全链路。

**命令**（校机脚本，`.tmp/p0-02-*.ps1`）：
```powershell
$env:PATH="D:\yzd\magpie-go-toolchain-20261107\go\bin;"+$env:PATH
$env:GOPROXY="https://goproxy.cn,https://goproxy.io,https://rsproxy.cn,direct"; $env:GOSUMDB="off"; $env:CGO_ENABLED="0"
# 补齐：utils/embeded/BUILDTOKEN.key + preconfigs/embeded/tls/miaoko.{crt,key}
go build -trimpath -o miaospeed-built.exe .           # sha256=2c302b29…6ab18, size=87,692,288
# Rust spike（tungstenite0.21 + url=2.5.0 钉旧版避开 icu）
cargo build --release ; .\spike.exe ws://127.0.0.1:18082/ SPIKETOKEN
```

**结果**：
- 服务端日志 `verify=true`；客户端依序收到 `Progress(Queuing)`→`Progress(单节点 InvokeDuration=5001ms, RTT/Conn=0)`→`Result(全量)`。
- Rust 计算签名 `fKdMhe…hMJ9Q==` 与 Go `utils.SignRequest` **逐字节一致**。`Value=0` 系校机直连 gstatic 超时（环境网络，非协议问题）。
- 协议字段全部冻结写入 `docs/rebuild/01-p0-upstream-verification/p0-02-miaospeed.md` 的 D 节；`docs/THIRD_PARTY.md` 登记 MiaoSpeed 固定版本 + 自建哈希。

**决策**：P0-02 判定 **PASS**；进入 P0-03（Apalis）/ P0-04（Vben）/ P0-05（授权收口）。

---

## [UTC 2026-11-10T00:31:00Z] S7 · 校机 Rust 工具链升级（1.85.1 → 1.99.1）+ GNU 构建 PATH 修复

**动作**：P0-03 构建反复撞 `icu_*/home/idna_adapter requires rustc 1.88`（校机仅 1.85.1）。根治：经 rsproxy rustup 镜像装新 stable。

**命令**（校机）：
```
$env:PATH="D:\yzd\tools\rust\cargo\bin;"+$env:PATH
$env:RUSTUP_DIST_SERVER="https://rsproxy.cn"; $env:RUSTUP_UPDATE_ROOT="https://rsproxy.cn/rustup"
rustup toolchain install stable --profile minimal   # → 1.99.1 (2026-10-13)
rustup default stable
```

**结果**：`rustc/cargo 1.99.1`（保留旧 1.85 toolchain）；后续所有 Rust 步骤默认用 1.99.1。
**附加发现**：GNU target（`x86_64-pc-windows-gnu`）构建必须把 `D:\yzd\MSYS2\mingw64\bin` 加入 `PATH`，
否则 `windows-sys/getrandom` 报 `error calling dlltool 'dlltool.exe': program not found`。

---

## [UTC 2026-11-10T00:40:00Z] S8 · P0-03 Apalis 队列行为验证（SQLite 运行 PASS + PG 编译验证）

**动作**：验证 Apalis 能否承接后台任务（enqueue/dequeue/process）。

**Pin**：`apalis 0.7.4` + `apalis-sql 0.7.4`（features `sqlite,postgres`）。最新稳定 `1.0.0`（后端拆分）列为升级候选。

**命令**（校机；crate `spikes/apalis-spike`，`PATH` 含 cargo bin + mingw64 bin）：
```
cargo build --release
.\target\release\apalis-spike.exe
```

**结果（实测）**：
- `BUILD_EXIT=0`；SQLite（`sqlite::memory:` + 官方 `SqliteStorage::setup` migration）运行时：
  `ENQUEUED 3 jobs → PROCESSED id=0/1/2 → PROCESSED_TOTAL=3 → RESULT=PASS`。
- Postgres 后端：`apalis-sql` 启 `postgres` feature + 引用 `PostgresStorage<DemoJob>` 类型 → 整体 `cargo build` 通过（**编译验证**）。
- **未在本机运行**（校机无 PG 服务）：retry/timeout/recovery 的运行断言 —— 留 CI（postgres sidecar + kill/restart harness）。

**决策**：P0-03 核心队列能力 **PASS**（SQLite 端到端 + PG 编译）；retry/timeout/recovery 运行验证转 CI。进入 P0-04（Vben 构建）。

---

## [UTC 2026-11-10T00:58:00Z] S9 · P0-04 Vben v5.7.0 构建 + OpenAPI→TS 生成（PASS）

**动作**：固定 Vben `web-ele` 版本，验证 `pnpm install`+`build:ele` 成功，并打通 `api/openapi/spec.yaml → TS` 契约生成。

**Pin**：Vben tag `v5.7.0`（commit `63a38dce49ba109f61607994e21ba921d8e970e9`）；`web-ele`=`apps/web-ele`（`@vben/web-ele`）。

**命令**（校机；`PATH` 含 `D:\yzd\tools\node\node-v24.22.2-win-x64` 与 pnpm；npmmirror 源）：
```
Expand-Archive vben.zip ; cd vue-vben-admin-5.7.0
pnpm install           # INSTALL_EXIT=0, 2m20s, pnpm v10.33.4
pnpm run build:ele     # BUILD_ELE_EXIT=0, turbo 11 tasks, vite built 4.34s, dist/ 产出
npx -y openapi-typescript@7 spec.yaml -o schema.d.ts   # OAPI_EXIT=0, 1256行→2125行, 72ms
```

**结果**：
- `node v24.22.2` / `pnpm 10.33.4`；`apps/web-ele/dist/`（css/js/jse/index.html/dist.zip）→ **构建 PASS**。
- `schema.d.ts` 含 `/api/v1/sources`、`/api/v1/nodes` 等；**但缺顶层 `/api/v1/checks` 与 `/api/v1/subscriptions`** → 契约缺口，P1(utoipa)/P6 补齐。
- 生成器本 spike 用 `openapi-typescript`（证 spec 可消费）；Vben 侧若要 typed client 可在 P6 换 `@hey-api/openapi-ts`。

**决策**：P0-04 判定 **PASS**（构建链路 + 契约生成链路均打通）。进入 P0-05（许可证/供应链收口 + 固化各 PIN）。

---

## [UTC 2026-11-10T01:06:00Z] S10 · P0-05 许可证/供应链收口 + P0 全阶段完成

**动作**：固化各上游 PIN 与许可证，落地 `deny.toml` 脚手架，`THIRD_PARTY.md` 定稿，更新 P0 阶段验收。

**命令/动作**（编辑机）：
```
GET https://api.github.com/repos/mateo-ki/vproxy/commits?per_page=1   # HEAD 19b9c147…（v1.16.9）
# 新建 deny.toml（仓库根，licenses/advisories/bans/sources；待 P1 gnosis-rs/Cargo.toml 后生效）
# 更新 docs/THIRD_PARTY.md：VProxy/Apalis/Vben/MiaoSpeed/Mihomo 全部真实版本+哈希
# 更新 p0-05 页面「固定基线」节 + P0 index 验收勾选
```

**结果（P0 收口）**：`docs/THIRD_PARTY.md` 固定版本一览——
- VProxy `19b9c147…`（v1.16.9，MIT，仅移植）；MiaoSpeed tag `4.7.9` 自建 exe `2c302b29…46ab18`（AGPL）；Mihomo `AirportR/mihomo v1.19.33-x365-miu.2`（GPL，随 MiaoSpeed）；Apalis `0.7.4`（MIT）；Vben `v5.7.0`（commit `63a38dce…70e9`，MIT）；phlogo AGPL 仅架构参考。
- `deny.toml` 已落地（P1 生效）；SBOM 计划：`cargo cyclonedx` + 二进制哈希登记比对。

**决策**：**P0 全阶段 PASS**（P0-01 VProxy / P0-02 MiaoSpeed / P0-03 Apalis / P0-04 Vben / P0-05 许可证供应链）。下一步进入 **P1（gnosis-rs workspace + 数据层）**，开始真正写 Rust 代码；重量级编译继续走校机（rustc 1.99.1 + MinGW PATH）。

> 环境基线（后续复用）：校机 `rustc/cargo 1.99.1`（rsproxy 升级）；GNU target 需 `D:\yzd\MSYS2\mingw64\bin` 于 PATH；cargo 依赖走 `rsproxy-sparse`；Go 依赖走 `goproxy.cn`；node `v24.22.2`+pnpm `10.33.4`（npmmirror）。

---

## [UTC 2026-11-10T01:18:00Z] S11 · P1-01 Cargo workspace 骨架（build/fmt/clippy 全绿）

**动作**：按 `docs/rebuild/02-p1-gnosis-skeleton-data/p1-01-workspace.md` 在仓库根建 Cargo workspace。

**新建**（编辑机 `repos/Phlogo`）：`Cargo.toml`（workspace, resolver=2, members=`apps/{api,worker}` + `crates/{domain,database,config,api-contract}`）、`rust-toolchain.toml`（stable+rustfmt+clippy）、`.gitignore`、各 crate `Cargo.toml`+占位 `main.rs/lib.rs`；配置 crate 包名 `app-config`。

**命令**（校机 `C:\Users\YiZhidi\phlogo-rs`，`PATH` 含 cargo bin + mingw64 bin）：
```
rustup component add rustfmt clippy --toolchain stable   # rsproxy 镜像
cargo build --workspace --all-targets
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

**结果**：`BUILD_EXIT=0`（33.18s，axum 0.7.9 / sqlx 0.8.6 / utoipa 5.5.0 均编译）、`FMT_EXIT=0`、`CLIPPY_EXIT=0`；`Cargo.lock`（242 包）回传仓库待提交。

**决策**：P1-01 判定 **PASS**。`apalis` 仅占位不 `use`，P3 再启用。进入 P1-02（Axum+分层认证骨架）。

---

## [UTC 2026-11-10T01:34:00Z] S12 · P1-02 Axum API 骨架 + 分层认证（build/test/fmt/clippy 全绿 + 冒烟 200/401/404）

**动作**：`apps/curse` 改 lib+bin；实现 Figment 配置（显式 PHLOGO_ env 覆盖）、auth 中间件、路由装配、OpenAPI JSON 端点。

**命令**（校机 `C:\Users\YiZhidi\phlogo-rs`，PATH 含 cargo+mingw64）：
```
cargo build --workspace --all-targets
cargo test -p api
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# 运行期冒烟：$env:PHLOGO_SERVER_PORT=28099; .\target\debug\api.exe & curl 200/401/404
```

**结果**：`BUILD_EXIT=0`、`TEST_EXIT=0`（4 passed：health/protected-401/unknown-404/openapi-200）、`FMT_EXIT=0`、`CLIPPY_EXIT=0`；
冒烟 **`health=200 post_sources=401 unknown=404`**（`PHLOGO_SERVER_PORT` 覆盖生效）。

**偏差**：`utoipa-swagger-ui 8.1.0` build.rs 与 `zip 2.6.1` 不兼容 → 本轮改用 `/api-docs/openapi.json` 提供 OpenAPI JSON（Swagger UI 静态资源后续钉版本再启用）。
**决策**：P1-02 判定 **PASS**。进入 P1-03（SQLx 迁移 + configs seed；校机无 PG，运行验证拟装便携 PG 或转 CI）。

---

## [UTC 2026-11-10T02:12:00Z] S13 · P1-03 SQLx 迁移 + configs seed（真实 PG 18.6 运行验证 PASS）

**动作**：写初始迁移与 KV seed，接入 `crates/database`，并在校机装 **PostgreSQL 18.6** 做真实运行验证。

**环境**：校机 `pacman -S --noconfirm mingw-w64-x86_64-postgresql`（MSYS2，二进制 `D:\yzd\MSYS2\mingw64\bin`）；PG 18.6。

**命令**（校机）：
```
# 迁移文件（SQLx 约定：文件而非目录）：migrations/0001_init.up.sql/.down.sql, 0002_uq_sources.up.sql/.down.sql
initdb -D C:\Users\YiZhidi\pgdata -U postgres --auth=trust -E UTF8 --locale=C
Start-Process postgres.exe -D C:\Users\YiZhidi\pgdata -p 5432 -c listen_addresses=127.0.0.1   # 不用 pg_ctl（管道挂起）
psql -c "CREATE ROLE phlogo ...; CREATE DATABASE phlogo OWNER phlogo;"
$env:DATABASE_URL=postgres://phlogo:change-me@127.0.0.1:5432/phlogo
cargo run -p database --example migrate
```

**结果（实测）**：`MIGRATE_OK configs_count=23`；表 `_sqlx_migrations,configs,node_sources,nodes,source_fetch_runs,sources`；
`configs=23`；`uq_sources_type_content` 存在；`_sqlx_migrations` v1(init)/v2(uq sources) 均 success。

**关键修正**：
1. **SQLx 不读子目录**——初版 `0001_init/up.sql` 目录形式导致扫到 0 迁移（`relation "configs" does not exist`）；改为 `0001_init.up.sql` 文件形式解决。
2. `pg_ctl start` 在捕获管道时挂起（postgres 继承 stdout）；改用 `Start-Process postgres.exe`。
3. `sources` 增唯一索引 `uq_sources_type_content`（`0002`），为 P1-04 竞态安全去重铺路。
4. seed 实为 **23** 个 KV 键（`filename_*` 3 个独立键），与文档「22」歧义说明已记录。

**决策**：P1-03 判定 **PASS**。进入 P1-04（Source/Node CRUD + 关系 + 集成测试）。

---

## [UTC 2026-11-10T02:40:00Z] S14 · P1-04 Source/Node CRUD + 关系（真实 PG 集成测试 + API e2e 全绿）

**动作**：新增 `crates/registry`、`database` 仓库层与模型、`apps/curse` handlers + AppState（lazy pool），接入真实路由。

**命令**（校机，PG 18.6 + cargo）：
```
cargo build --workspace --all-targets
DATABASE_URL=postgres://phlogo:change-me@127.0.0.1:5432/phlogo cargo test --workspace
cargo test -p database --test pg_crud
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# API e2e：起 api(PHLOGO_AUTH_SERVICE_SECRET=svc-secret) → curl POST /api/v1/sources 两次（同 body）
```

**结果**：`BUILD/TEST/FMT/CLIPPY` 全 0；`pg_crud` **2 passed**；**e2e `create1=201 create2=200`**（b1 `alreadyExisted=false`、b2 `true`，同一 id），`list_noauth=401`/`list_auth=200`。

**决策/修正**：
1. 去重竞态：新增 `0002_uq_sources`（`UNIQUE(type,content)`）+ `ON CONFLICT ... DO NOTHING`，并发安全（取代 `FOR UPDATE` 方案）。
2. `apps/curse` 用 `database::connect_lazy` 构建 pool，未连库即可跑 health/401/404 单测。
3. handler `parse_id` 用 `#[allow(clippy::result_large_err)]`；rustfmt 差异按 `fmt.log` 逐处修正。

**决策**：P1-04 判定 **PASS**。**P1 阶段（骨架+数据层+CRUD）全部完成**。下一步进入 P2（fetch/parse 抓取解析）。

---

## [UTC 2026-11-10T03:05:00Z] S15 · P2-01 抓取 + SSRF 防护（crates/fetch，测试 6/6）

**动作**：新建 `crates/fetch`：SSRF 黑名单 + 受控 fetch（手动重定向逐跳复核、大小/超时限制、allowlist）。

**命令**（校机）：
```
cargo build -p fetch --all-targets
cargo test -p fetch
cargo fmt -p fetch -- --check
cargo clippy -p fetch --all-targets -- -D warnings
```

**结果**：全 **0**；`6 passed`（黑名单表/良性/元数据/重定向元数据/超大/超时）。`reqwest 0.12.28` + `rustls-tls` 在 GNU/Windows 上编译成功（无 aws-lc C 依赖问题）。

**决策**：P2-01 **PASS**。DNS-rebinding 极小窗口以「解析后校验」缓解，彻底版（固定 IP 连接）列 P7 加固。进入 P2-02（VProxy 解析 Adapter → NormalizedNode）。

---

## [UTC 2026-11-10T03:28:00Z] S16 · P2-02 解析 Adapter（domain::NormalizedNode + Clash/Singbox/Base64，5 测试）

**动作**：定义 `domain::NormalizedNode`（身份字段 vs 展示字段分离 + `identity_material()`）；新建 `crates/vproxy-adapter` 自实现解析（P0-01 决定 VProxy 不作依赖）。

**命令**（校机）：
```
cargo build -p vproxy-adapter -p domain --all-targets
cargo test -p vproxy-adapter -p domain
cargo fmt -p vproxy-adapter -p domain -- --check
cargo clippy -p vproxy-adapter -p domain --all-targets -- -D warnings
cargo tree -p database | findstr vproxy    # 期望无
```

**结果**：全 **0**；`5 passed`（clash/singbox/base64/field_separation + domain identity 分离）；**`cargo tree -p database` 无 vproxy**（类型不渗透约束成立）。

**决策**：P2-02 **PASS**。percent-decode 与身份哈希留 P2-03。进入 P2-03（identity_hash + fingerprint_version + golden）。

---

## [UTC 2026-11-10T03:58:00Z] S17 · P2-03 节点指纹（14 golden 全过；v1 canonical 含 transport/tls）

**动作**：`crates/domain/src/fingerprint.rs`（`node_auth` + `normalize_credential` 反复 decode + sha256 前 16 字节 hex）+ `Credential` 扩枚举。

**命令**（校机）：
```
cargo build -p domain --all-targets
cargo test -p domain
cargo fmt -p domain -- --check
cargo clippy -p domain --all-targets -- -D warnings
```

**结果**：`build/test/fmt/clippy` 全 **0**；**14 passed**（golden 全表 + percent-decode 不变量 + 空值无身份 + 版本字段）。

**修正**：`decode_utf8_lossy` → `decode_utf8()`（非法转义保持原串，对齐 Go `url.PathUnescape` 报错语义）。

**决策**：P2-03 **PASS**。**v1 canonical = type|server|port|auth|transport|tls|network_options**（按方案 §7.3 纳入连接语义参数；Phlogo 四段式记为 version 0 兼容口径，不产出）。进入 P2-04（对账入库 + fetch run 历史）。

---

## [UTC 2026-11-10T16:30:00Z] S18 · P2-04 对账入库（Replace/Accumulate + 死源守卫 + 孤儿标记；6 集成测试全绿）

**动作**：migration `0003_node_membership`；`database::reconcile` 事务原语；`registry::reconcile_fetch_result` + `delete_source`；`registry/tests/reconcile.rs` 6 场景。

**命令**（校机，PG 18.6）：
```
cargo build --workspace --all-targets
DATABASE_URL=postgres://phlogo:change-me@127.0.0.1:5432/phlogo cargo test --workspace
cargo fmt --all / cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

**结果**：`build/test/fmt/clippy` 全 **0**；reconcile 6 场景全过（Replace 删消失+Orphaned、failed 保留、Accumulate 保留、删源不伤共享节点、空 Replace 全孤儿、死源守卫）。

**过程修正**：`database/Cargo.toml` 曾被 edit 拼坏（`}anyhow` 同行）修复；`registry` 补 `serde_json/serde/sqlx/thiserror`；`delete_source` 去掉 anyhow 依赖改直用 SQL；moved value（`error.clone()`）、`explicit_auto_deref`（`&mut *tx`→`&mut tx`）、`cloned_ref_to_slice_refs`（`from_ref`）逐个清掉。

**决策**：P2-04 **PASS**。**P2 阶段（fetch+SSRF / 解析 / 指纹 / 对账）全部完成**。下一步进入 P3（测活 check：icmp/tcp/http 探测、`node_status` 窗口）。

---

## [UTC 2026-11-11T02:30:00Z] S19 · P3-01 Apalis Check Job（PG 后端真机跑通；幂等副作用）

**动作**：`apps/worker` lib+bin 化：`CheckJob` + 幂等 `record/finish_check_run`（migration 0004 主库）+ Apalis `PostgresStorage` 装配；**Apalis 独立库 `phlogo_jobs`**。

**关键修正**：apalis 0.7.4 `setup()` 内嵌 migration 与业务 `migrate!` 共用 `_sqlx_migrations` 冲突（`VersionMissing(1)`）→ **队列独立数据库**（`PHLOGO_DATABASE_JOBS_DSN`，默认派生 `phlogo_jobs`），同时满足方案 §3.1 分离要求。`postgres` feature 属于 `apalis-sql`。

**命令**（校机，PG 18.6）：
```
cargo build --workspace --all-targets
DATABASE_URL=... cargo test --workspace      # + PHLOGO_DATABASE_JOBS_DSN=phlogo_jobs
cargo fmt --all -- --check / cargo clippy --workspace --all-targets -- -D warnings
# 真机 smoke：起 worker 12s，确认 phlogo_jobs 内 apalis 表 + 主库 check_runs finished 计数递增
```

**结果**：`build/test/fmt/clippy` 全 **0**；幂等测试过（重投 4 次仅 1 行）；真机 smoke 中 handler 消费 demo job（`check_runs` 2→3）。

**决策**：P3-01 **PASS**。进入 P3-02（MiaoSpeed WebSocket Adapter：Checker trait、握手/auth 签名、payload 提交）。

---

## [UTC 2026-11-11T03:10:00Z] S20 · P3-02 MiaoSpeed WS Adapter（P0-02 协议可执行化；golden 签名逐字节一致；7 测试全绿）

**动作**：新建 `crates/checker-miaospeed`：`protocol.rs`（jsoniter 字节级序列化 + hashMiaoSpeed + Vendor="" quirk）+ `adapter.rs`（Checker trait over WS，按 `ProxyInfo.Name` 回映射）+ 协议忠实 mock + 4 条验收测试。

**关键实现**：
- golden 签名测试与 P0-02 Go 交叉验证值**逐字节一致**（`fKdMhefu…9Q==`）——jsoniter 形态（字段序/内嵌/零值/`[]`）与 D.5 quirk 均被钉死。
- `serde_json` 全仓 `preserve_order`（协议需要插入序）→ P2-03 指纹 `canon_net` 改递归键排序 canonical JSON，`field_order` golden 保持通过。
- mock 用与服务端相同的验签路径（Vendor="" 克隆比对），错误 token → `{"Error":"cannot verify..."}` → `CheckerError::Auth`。

**命令**（校机）：`cargo build --workspace --all-targets` / `cargo test -p checker-miaospeed` / `cargo fmt --all -- --check` / `cargo clippy --workspace --all-targets -- -D warnings`。

**结果**：全 **0**；`7 passed`。

**决策**：P3-02 **PASS**。进入 P3-03（结果解析/单位规范化/迟到重复处理/跨 tag 合并）。

---

## [UTC 2026-11-11T03:25:00Z] S21 · P3-03 结果规范化与映射（迟到不覆盖 + 跨 tag 合并；11 测试全绿）

**动作**：`checker-miaospeed` 新增 `result.rs`（normalize/reconcile：单位缺省、按 node_id 去重、unreported→Unknown）与 `health.rs`（NodeHealth、`should_overwrite` 迟到规则、`cross_tag_alive` 窗口合并）。

**实测**（校机）：build/test(11 passed)/fmt/clippy 全 **0**。4 条验收全过：精准映射、迟到不覆盖、partial/duplicate/reconnect、跨 tag 合并（含 cutoff 边界）。

**决策**：P3-03 **PASS**。进入 P3-04（`check_runs`/`node_check_results`/`node_status` 持久化 + 幂等重试落库）。

---

## [UTC 2026-11-11T03:45:00Z] S22 · P3-04 测活历史持久化（幂等 + 迟到不覆盖落库；P3 全部完成）

**动作**：migration `0005_check_results`（`node_check_results` 合并初筛/复筛 + `node_status` per-tag 快照 + `check_runs.external_job_id`）；`database::check`（平面参数化，DB 层不依赖 adapter crate）。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**；`check_persist` 2 场景过——同 run_id 重跑 `inserted==0` 且计数不变、metrics JSONB roundtrip 逐字段一致（`speed_kbps=0` 保留）、`node_status` 迟到写（T+0 vs T+10）不覆盖、新写（T+20）生效。

**决策**：P3-04 **PASS**。**P3 阶段（Apalis Job / WS Adapter / 结果映射 / 历史持久化）全部完成**。下一步进入 P4（生命周期与评分：状态机、MarkDeadAndClean、评分引擎）。

---

## [UTC 2026-11-11T03:35:00Z] S23 · P4-01 状态机与事务化清理（lifecycle crate；4 测试全绿）

**动作**：新建 `crates/lifecycle`（四枚举 + 转换规则表 + `ops.rs`：`mark_dead_and_clean` 条件 UPDATE 仅赢家清理、孤儿/退役守卫、物理删除）+ migration `0006_lifecycle`（`retired_at` + `orphan_retention_days=14`）。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**；状态机 2/2（Dead 终态、Linked↛Retired 等非法转换拒绝）+ PG 集成 2/2（死源清理幂等、孤儿→Retired→复活幸存）。

**踩坑**：`make_interval(days => …)` 无 bigint 重载，绑 `i32`。

**决策**：P4-01 **PASS**。进入 P4-02（健康/孤儿清理 Job：跨 tag alive 物化到 nodes、清理调度）。

---

## [UTC 2026-11-11T04:00:00Z] S24 · P4-02 按 Tag 健康与孤儿/退役清理（cleanup cycle + 跨 tag 聚合；4 场景全绿）

**动作**：`lifecycle::ops` 扩展 `run_cleanup_cycle`（retire→delete 同 tick）与 `cross_tag_alive_map`（窗口内 `bool_or` 聚合，替代 `batchAliveDecision`）；`lifecycle/tests/p402.rs` 4 场景。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**；per-tag 独立、dead 不删关联、孤儿保留窗口（新鲜孤儿不动/超期一个 tick 删净含历史）、跨 tag 窗口聚合全过。迟到守卫由 P3-04 场景覆盖（同一 SQL）。

**决策**：P4-02 **PASS**。进入 P4-03（评分引擎：速度分支、jitter 阈值、等级映射、衰减）。

---

## [UTC 2026-11-11T04:30:00Z] S25 · P4-03 评分引擎（EWMA 移植 + golden 0.967722 更正；5 测试全绿；P4 全部完成）

**动作**：新 crate `crates/scoring`：`compute_score` 逐行移植 + `ScoreResult/components/version/InsufficientData`（net-new）+ `node_scores` 落库与 `nodes.score=AVG(score>0)` 回写；migration 0007/0008。

**三个开放决策定稿**：speed 分支按 **Go 代码基线**（始终混合，未测=0）；**jitter 权威阈值=200ms**（seed 值，Go 70 仅代码级 fallback）；取最近 20 条加 **`, id DESC`** tie-break。

**golden 更正**：规格手算 0.967718 有中间舍入误差；独立复算 `ln` 精确值 → `scoreLat=0.916511`、`quality=0.928270`、**score=0.967722**，与移植输出一致，文档与测试同改。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**；5/5（golden、不可达最差、6 位舍入、AVG 忽略零分、version/insufficient）。

**决策**：P4-03 **PASS**。**P4 阶段（状态机 / 健康+清理 / 评分）全部完成**。下一步进入 P5（过滤 AST + 订阅输出）。

---

## [UTC 2026-11-11T04:45:00Z] S26 · P5-01 类型化筛选 AST（白名单 + 参数化 SQL + 内存求值；7 测试全绿）

**动作**：新 crate `crates/filter`：`ast.rs`（手写 Deserialize 贴合 plan §9.2 形状）+ `parse.rs`（白名单/深度/叶子数限制）+ `sql.rs`（$n 占位符、零拼接、EXISTS 子查询）+ `eval.rs`（NodeView 纯函数求值）。

**实测**（校机）：build/test/fmt/clippy 全 **0**；`filter` 7/7（解析、双拒绝、参数化 SQL $2/$3/$4、限制、求值、空串+unlock）。

**关键实现点**：serde tag 分派会把叶子包一层 `operator:"cond"`——不符规格形状，改手写 Deserialize（顶层 `operator` 分派叶子/逻辑节点）；`in/not_in` 展开、`is_null` 忽略 value；`$1` 恒为 checker_tag。

**决策**：P5-01 **PASS**。存储迁移（configs/sponsors 列）随 p5-02/p5-03 消费端一并做。进入 P5-02（配额与限流）。

---

## [UTC 2026-11-11T05:00:00Z] S27 · P5-02 地区配额与选区算法（selector.go 逐函数移植；12 测试全绿）

**动作**：新 crate `crates/distribution`：common/all/other 三 preset、比例配额（末位吸收余数）、三分支池选择（≤quota/≤1.5×/加权 A-Res）、E-S 加权抽样（`max(score,0.01)`、`u==0→1e-10`、min-heap k）、UN 补足、naming order 输出+seq 编号。

**实测**（校机）：build/test/fmt/clippy 全 **0**；12/12（确定性 8 + 统计 2 + 端到端 2）。

**实现要点**：f64 无 Ord → 手写 `Entry(f64,usize)` 全序包装实现 min-heap；随机分支按规格只做种子化统计验收（不跨语言逐字节等价）；`common_fallback` 用同种子断言回退路径等价。

**决策**：P5-02 **PASS**。进入 P5-03（订阅输出：命名模板、Clash/Base64 渲染、订阅路由）。

---

## [UTC 2026-11-11T05:20:00Z] S28 · P5-03 订阅输出与 Token（三格式生成器 + 白名单命名模板 + token 哈希；24 测试全绿；P5 全部完成）

**动作**：`crates/distribution` 扩展 `token.rs`（sha256+pepper、明文不入库）、`naming.rs`（grade/flag/`{{field}}` 白名单模板——expr 命名模板的安全替代）、`output.rs`（mihomo YAML 模板填充 / **sing-box 独立转换器** / base64）、`subscription.rs`（AST 筛选→preset 选区→命名→渲染纯函数管线）；migration `0009_subscriptions`（profiles+tokens，仅存哈希）。

**实测**（校机）：build/test/fmt/clippy 全 **0**；`distribution` 24/24。

**决策**：P5-03 **PASS**。**P5 阶段（Filter AST / 配额选区 / 订阅输出）全部完成**。下一步进入 P6（Vben 管理端：p6-01 脚手架 / p6-02 页面）。

---

## [UTC 2026-11-11T05:40:00Z] S29 · P6-01 Vben 基座与 OpenAPI 契约链路（spec 生成 + TS 类型 + build:ele 复验全绿）

**动作**：
1. `crates/api-contract` 重写——utoipa 补 7 path + 5 schema（此前空文档），utoipa 加 `uuid` feature；实测 `/api-docs/openapi.json` 200 → 4580 字节 → 落 `api/openapi/spec.json`。
2. `web/vben-admin/`：`openapi-ts.config.ts` + `src/api/request.ts`（bearer/401/typed client）。**决策：Vben monorepo 不整仓 vendor**，构建用校机固定 checkout（v5.7.0/`63a38dce`，同 P0-04）。
3. 校机验证：`openapi-typescript@7` → schema.d.ts（12KB）GEN_EXIT=0；`pnpm build:ele` → **0**（11/11 tasks）。

**踩坑**：utoipa `#[path]` 宏要求类型在同作用域可解析（schemas 与 paths 需 glob 同文件作用域）；status 必须 literal。

**决策**：P6-01 **PASS**。进入 P6-02（业务页面：Sources/Nodes/Subscriptions）。

---

## [UTC 2026-11-11T06:00:00Z] S30 · P6-02 管理页面（Sources/Nodes/Scoring 三页 + config 端点；build:ele 11/11 绿）

**动作**：后端补 `GET/PUT /api/v1/config`（KV 读写，事务）；前端三页（sources 去重创建/删除/ignore-dead、nodes 列表删除、scoring 配置编辑）+ `phlogo.ts` 自含 API 层 + Phlogo 菜单路由。

**关键踩坑**：首次部署覆盖了 vben 原生 `src/api/request.ts` → `gnosis/auth.ts` 的 `requestClient/baseRequestClient` 导出丢失 → MISSING_EXPORT 构建失败。从本地 pristine v5.7.0 恢复原文件，自研层改独立 `phlogo.ts`（自含 fetch+bearer）。**教训：向第三方 monorepo 注入文件前先查同名导出的消费方。**

**实测**：Rust build/test/fmt/clippy 全 0；`pnpm build:ele` **0**（11/11 tasks，dist.zip 1.09MB）。

**契约缺口确认**：stats/checks/subscriptions 等页面所需端点能力在 crate 层已有，但未经 Axum 暴露——P7 装配路由时补 handler+`#[utoipa::path]`，页面随之补齐。

**决策**：P6-02 **PASS**。**P6 阶段（基座+页面）完成**（Dashboard/NodeDetail/Checks/Subscriptions 四页随 P7 契约扩展补齐）。下一步进入 P7（部署/可观测/加固）。

---

## [UTC 2026-11-11T06:20:00Z] S31 · P7-01 部署编排与备份（compose/Dockerfiles/Caddyfile/脚本；release 构建与拓扑 lint 全绿）

**动作**：`deploy/`（compose 六服务+debug profile、双多阶段 Dockerfile、Caddyfile、.env.example）+ `scripts/{backup,restore}.sh`（pg_dump -Fc / 恢复演练+行数校验）。

**实测**：校机 `cargo build --release --bin api --bin worker` **0**（api 15.6MB/worker 11.6MB，Dockerfile builder 段等价）；compose 拓扑 lint **13/13**（postgres/miaospeed 零公网端口、caddy 21380、6 服务、.env 4 键）。docker runtime 集成验收（整栈拉起/备份恢复演练）留 docker 环境的 CI job，文档如实标注。

**决策**：P7-01 **PASS**。进入 P7-02（可观测性）。

---

## [UTC 2026-11-11T06:35:00Z] S32 · P7-02 可观测性（tracing JSON + 全局 redaction + /metrics；测试全绿）

**动作**：新 crate `crates/observability`（JSON subscriber + redact_secret/subscription_url + RedactionLayer 违规告警）；`/metrics` 轻量 Prometheus 端点（进程计数 + DB 量规）；api/worker main 统一接 observability::init；Apalis 恢复引用 P3 既有实现与测试。

**设计取舍（写进文档）**：tracing Event 不可变 → RedactionLayer 不做就地改写，而是**违规可观测**（secret 字段名命中即 warn），主防线在调用点 redact_*。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**；observability 3/3（掩码、URL 剥 query、字段白名单）。

**踩坑**：`DynFilterFn` 闭包是 2 参签名（去掉多余 filter 后绕开）。

**决策**：P7-02 **PASS**。进入 P7-03（安全加固：deny/audit、端口暴露 lint、contract 静态检查）。

---

## [UTC 2026-11-11T07:00:00Z] S33 · P7-03 安全加固（订阅路由+token 校验、限流层、/metrics 边界 403、supply-chain 脚本；全绿）

**动作**：`GET /sub/{token}.{fmt}` 实装（哈希校验→撤销/禁用即 401→p5-02 选区→命名→三格式）；`rate_limit.rs` 自研固定窗口 layer（429）；Caddyfile `/metrics` 显式 403；`scripts/supply-chain.sh`（deny/audit/SBOM/sha256）。

**实测**（校机 PG 18.6）：build/test/fmt/clippy 全 **0**（含 P2-01 SSRF 6 测试——验收 #4 引用）；compose lint 13/13。cargo audit 需 crates.io 网络在 CI 跑（校机 rsproxy 不承载 audit DB，如实标注）。

**决策**：P7-03 **PASS**。**P0–P7 全部页面完成**。做整体验收清点后收口。

---

## [UTC 2026-11-11T07:20:00Z] S34 · Podman 优先改造 + 真机冒烟（compose 解析/postgres 拉起/端口语义全过）

**动作**：新增 `scripts/runtime.sh`（podman compose → podman-compose → docker compose 解析链），backup/restore 改 source 它；README 改 podman-first；compose `WEB_IMAGE` 改默认 `nginx:alpine`（不再强制变量）。

**真机验证（本机 podman 6.1.4 + WSL machine）**：装 podman-compose 1.6.0；配 daocloud 镜像源（docker.io 直连超时）后 pull 成功；`podman compose config --services` 六服务识别 0；`up postgres` 0 + `pg_isready` accepting connections；**`compose port postgres 5432` 报错（无宿主映射）——§16.10 端口语义在 podman 下成立**；down 0。

**踩坑**：PS 5.1 无 `utf8NoBOM` Encoding；`podman machine ssh` 的 heredoc 会落到 Windows 侧 `%APPDATA%\containers\`（宿主与 machine 共读该配置目录），TOML 引号被剥导致解析失败——用 .NET WriteAllText 重写。

**决策**：Podman 兼容 PASS（deploy 全套双运行时可用）。

---

## [UTC 2026-11-11T07:40:00Z] S35 · 契约缺口收口：视图 + 订阅管理端点（端到端全链 PASS）

**动作**：`handlers/views.rs`（stats/checkers/logs/check/logs/fetch、nodes/{id}/checks/{initial,deep}）+ `handlers/subscriptions_admin.rs`（subscriptions CRUD、tokens 签发/撤销/列表）+ 路由挂载。

**端到端实测**（校机真实 PG）：stats/checkers/logs 全 200（真实数据：266 节点/363 源/3 tag）、无认证 401、
profile 创建 201 → token 签发 201（明文一次性）→ **用该 token 拉 /sub 200**——P5-03 管线与 P7-03 token 路由全链闭环。

**修正**：profile `output_format NOT NULL` 由 SQL 缺省 `'base64'`；`logs_fetch` success 改 `(status='finished')`；e2e JSON 体固定走文件。

**决策**：**契约缺口清零**——P6-02 四个待补页面（Dashboard/Checks/NodeDetail/Subscriptions）的数据源全部可用。

---

## [UTC 2026-11-11T08:00:00Z] S36 · 管理页面补齐（规格 7 页全部落地；build:ele 11/11 绿）

**动作**：新增 `dashboard.vue`（统计卡+分布）、`checks.vue`（tags+初筛日志，-1 红标）、`node-detail.vue`（身份+双时间线）、`subscriptions.vue`（profile CRUD+token 签发一次性弹窗/撤销）；`phlogo.ts` 扩至全部端点；路由七项；Nodes 页加详情入口。

**实测**：校机 `pnpm build:ele` **0**（11/11 tasks，7 页进 dist）。

**决策**：**P6 页面侧全部完成**（规格 7/7）。Go 源码保留为对照基准的决策由用户裁定（清掉需改文档行号锚点）。

---

## [UTC 2026-11-11T08:20:00Z] S37 · 自查补缺：Fetch 闭环接线 + 双循环调度 + 登录/触发端点（自补记录页建立）

**自查发现的缺口**：P2 四段实现各自测试绿但**从未被串起来跑**（fetch_job.rs 补闭环）；cleanup cycle 无定时触发；无登录端点（Vben 必挂）；抓取无手动触发；SSRF 守卫拦本地测试服务器。逐项补齐——详见 `docs/rebuild/09-self-completed-gaps.md`（含推迟项 B1-B6 与换构架评估输入）。

**端到端验证**：`fetch_loop_end_to_end` **PASS**（本地订阅服务器 → SSRF allowlist → 抓取 → 解析 2 节点 → 指纹 → 对账 → run 记录 → 二次幂等）——P2 四段第一次真正串起来跑通。

**决策**：自补完成，记录页落 `09-self-completed-gaps.md`。

---

## [UTC 2026-11-11T08:55:00Z] S38 · Go 源清除 + fork 源对照基准 + B4 audit 校机跑通

**Go 清除**：`internal/`(47) + `cmd/`(1) + `go.mod/go.sum/Dockerfile/oapi-codegen.yaml/config.example.yaml` 移除（-11,667 行，94 文件）；35 处文档「与 Phlogo 现状对照」改名「与 fork 源对照」+ 基准注释（**fork 根提交 `dda4e6f2`**，`git show dda4e6f2:<path>` 查原文）；CONVENTIONS 模板同步。对照语义不变——原文永存于 git 历史。

**B4 audit**：校机装 `cargo-audit 0.23` 但 advisory-db 硬编码 github（被墙，三种绕法实测失败）→ 自写 `audit-offline` 匹配器（scratch crate，`semver` 权威匹配）+ 本机 clone advisory-db 传入 → **校机 Rust 实跑 4 命中**（libsqlite3-sys[不可达]/ring[no-patch]/rsa[低危]/unsafe-libyaml[换 serde_yaml 1.0 可消]）。**校机实测无容器运行时**——compose 演练仍需容器环境。

**踩坑**：PS 5.1 UTF-8 无 BOM .ps1 中文乱码（脚本改 Python）；scp 传 bare clone 无工作树（audit 需普通 checkout）；release 下 `cfg(debug_assertions)` 诊断静默；scp 只传了 db 没传改过的 crate（旧二进制跑三遍才定位）。

**决策**：B3/B4 关闭。**B5（契约批量补）/B6（高级搜索二选一方案）已写明做法**，等需要时做。

---

## [UTC 2026-11-12T03:05:00Z] S39 · 旧评审交叉审查入库 + P0 安全修复（权限矩阵收紧 + 登录限流；全套测试真实 PG 全绿）

**文档**：`docs/rebuild/10-review-crosscheck.md`——旧 Go 评审逐节对照（§1–§9），区分已实现（附证据）/部分/缺口/被新架构取代；ErrorResponse+分页等契约改进**独立 P1**（用户裁定不混入本轮）。

**P0-01 权限矩阵**：`auth_kind(method,path)`（代码化矩阵，fail-closed AdminOnly）+ `require_api_auth` 替换整罩式层。**关键发现（实证）**：nest 内 middleware 收到剥 `/api/v1` 前缀的路径——无凭据 GET /stats 曾直接 200 吐数据（旧整罩层不查 path 所以掩盖至今）。修复=路径归一化。ServiceBearer 打管理写→403（区别 401）；合法服务面保留（GET 读、POST sources intake、POST fetch/trigger）。

**P0-02 登录限流**：login 挂 nest 外 + 独立 route_layer（10/min 默认）+ 桶键改 peer **IP**（原 SocketAddr 键换连接即绕过——实测发现修复）+ XFF 从不读 + `into_make_service_with_connect_info`。

**验证**：auth_matrix 7/7、login_limit 4/4（真 TCP 触发/XFF/并发/其他端点不受扰）、**全套 workspace 测试（真实 PG）EXIT=0**、fmt/clippy 0。**附带修复**：p402 测试绝对日期跨日出窗（改相对 now）。

**决策**：P0-01/P0-02 **关闭**。ErrorResponse+分页等 P1 待办见 10-review-crosscheck §5。

---

## [UTC 2026-11-13T01:55:00Z] S40 · 上游等价性审计（B3）+ 配置键悬挂审计 + 自检失误修复

**B3 等价性审计**（`docs/rebuild/11-equivalence-audit.md`）：以上游基线 `dda4e6f2` 的 26 条 API 路径逐条对照——**19 通过**（15 同形 + 4 架构取代 + 1 异形订阅 URL）、**7 条真缺失**（`auth/refresh`、`nodes|sources/search`、`filter/preview`、`sponsors` CRUD ×2、`format-configs` ×2），归为 3 个功能域。口径按用户裁定「多实现路径只需一条等价」。

**新发现：配置面悬挂**（比缺功能更危险）：
- **6 个键的值被静默忽略**——`score_decay_alpha`/`score_weight_*`/`score_latency_curve_k` 在 `configs` 里 seed 成小数（`0.7`），而 `scoring/params.rs:16-21` 按「整数百分数」`parse::<i64>()` → 解析失败 → **走 fallback**，改配置无效果（fallback 恰好等于 seed，所以一直没暴露）。
- **13 个键无任何消费者**：`grade_*_min`（阈值被 `GradeThresholds::default()` 硬编码）、`dead_latency_ms`、`fetch_dead_window_days`、`node_dead_window_days`、`fetch_history_window_days`、`check_run_history_days`、`cleanup_interval_minutes`、`default_sponsor_text`、`node_distribute_filter`、`node_name_template`、`filename_*`×3；另有 `score_min_samples` 被读但**从未 seed**。

**自检失误修复（C）**：
1. `login_limit.rs::xff_cannot_bypass` 原为 `assert!(saw_429 || true)`（恒真 = 假测试）→ 改为真断言：XFF 轮换 13 次，前 10 次必不 429、第 11 次必 429、第 13 次仍 429。
2. `09-self-completed-gaps.md` B5 行过期（仍写「未覆盖」）→ 改为已完成（`de57dcc`，19 路径）。
3. `00-index.md` 未收录 10/11 两份新文档 → 补索引行。

**决策登记**：A1 待定（§9 五方案对比）、A2 方向 XChaCha20、A3 选 AST JSON、A4 保持现状、A5 14 天且必须可配、A6 冻结、A7 CI 记录就绪、A8 容器转本机、B1/B2 通过——落 `10-review-crosscheck.md §8`。

---

## [UTC 2026-11-13T02:20:00Z] S41 · 评分参数修复 + A5 保留期接线 + B3 缺口 #1/#3 补齐（搜索/预览/模板配置）

**评分参数修复**（`crates/scoring/src/params.rs`）：原 `cfg_i64(...)/100.0` 按整数百分数解析，但 seed 是小数 `0.7` → 解析失败 → **静默走 fallback**（改配置无效）。改为 `cfg_ratio()`：小数原样、≥2 视作百分数（兼容旧写法）；单测 `decimals_and_percents_agree` 钉死。**现在改权重真的生效**。

**A5 落地**（14 天 + 可配）：
- `lifecycle::prune_check_history`（按 `check_run_history_days` 删除超期 `node_check_results`；`≤0` = 永不删）；
- `lifecycle::cleanup_interval_minutes` → worker 调度周期改读配置（默认 30 分钟），不再硬编码 600s；
- migration `0010_config_wiring`：把未被改过的 `check_run_history_days` seed 从 7 改 14；补 seed `score_min_samples`；加索引 `idx_node_check_results_node_time`（时间线查询 `(node_id, checked_at DESC)`）；建表 `format_configs`。

**B3 缺口补齐**：
- **#1 高级搜索 + 预览**：`apps/curse/src/handlers/search.rs`——`POST /nodes/search`、`POST /sources/search`（AST JSON body + 分页 ≤200）、`POST /filter/preview`（≤100）；filter crate 扩 `parse_with(whitelist)` / `matches_source` / `SOURCE_FIELD_WHITELIST`；权限矩阵纳入（读面 AdminOrService，fail-closed 保持）。
- **#3 模板配置**：`apps/curse/src/handlers/format_config.rs` + `format_configs` 表；**写入前用渲染同款解析器校验**（坏模板 400，不在每次订阅时 500）；`GET` 读面 AdminOrService、`PUT` AdminOnly。

**接线配置键**：`grade_*_min` → `handlers/config.rs::grade_thresholds()`（订阅渲染不再硬编码阈值）。

**契约**：utoipa 补 6 条路径 → **25 条**（`openapi.json` 10290 B）。

**验证**：校机真实 PG 全套 `FULL_TEST_EXIT=0`、`cargo fmt --check=0`、`clippy -D warnings=0`；新增 `apps/curse/tests/search_endpoints.rs`（未认证 401 边界 3 项）。

**已知限制（诚实记录）**：`unlock` 条件永不匹配（无 streaming 表）；节点 `region` 取自最近测活 metrics；搜索是内存过滤（与上游同语义，量大时应改 SQL 下推）。**sponsors 域仍缺，待用户裁定是否恢复**。

---

## [UTC 2026-11-13T04:55:00Z] S42 · A1 落地（PROXY protocol v1）+ 修限流桶键真 bug + 部署面收紧

**A1（用户裁定 PROXY protocol）已实现**：
- `apps/curse/src/proxy_protocol.rs`：v1 解析（`TCP4`/`TCP6`/`UNKNOWN`）+ **自带 accept 循环**（axum 0.7 的 `axum::serve` 内部直接建连交给 hyper，没有消费前置头的钩子——照 axum `serve.rs` 的写法自建 loop + `hyper::server::conn::http1`）；非 PROXY 连接首批字节用 `PrefixedStream` **回放**给 hyper；PROXY 头之后可能同包到达的 HTTP 字节也回放（`read_proxy_header` 返回 leftover）。
- 真实地址落点 `ConnectInfo<SocketAddr>`（与 axum 原生同构）→ `peer_addr()`/`peer_ip()` 统一取用；**从不读 XFF**。
- 部署：`deploy/Caddyfile` 三个 `reverse_proxy` 各加 `transport http { proxy_protocol v1 }`（**本机 podman `caddy validate` 实测 "Valid configuration"**）；`deploy/compose.yaml` 的 `api` **取消宿主端口**（只 expose），加 `PHLOGO_SERVER_PROXY_PROTOCOL=1` / `PHLOGO_SERVER_EXPECT_PROXY=1`（缺头即拒）。
- 配置：`crates/config` 新增 `server.proxy_protocol` / `server.expect_proxy`（默认关，向后兼容）。
- 测试：`apps/curse/tests/proxy_protocol.rs` 5 项（带头 200 / 无头不强制 200 / 无头强制拒 / `PROXY UNKNOWN` 不报错 / 头被正确消费）+ 模块内单测 5 项。

**修掉的真 bug（重要）**：`rate_limit.rs` 原读**裸 `SocketAddr`** 扩展，而 axum 插入的是 `ConnectInfo<SocketAddr>` → **永远取不到**，桶键恒为 `0.0.0.0`（全局单桶）。即"按 IP 分桶"此前**从未真正生效**。现已改经 `peer_ip()`。

**测试自身踩的坑**：`prefixed_stream_replays_bytes` 用 `duplex` 时写端未关闭 → `read_to_end` 永等 EOF → 测试挂 60 秒（首次跑被卡住）。修正为 `drop(w)`。

**验证**：校机真实 PG 全套 `FULL_TEST_EXIT=0`、`cargo fmt --check=0`、`clippy -D warnings=0`；本机 podman `caddy validate` 通过；`podman-compose config` 可解析。

**顺带修正文档错误**：`deploy/README.md` 里脚本路径写成 `scripts/runtime.sh`，实际在**仓库根** `scripts/`（git ls-files 确认）。

---

## [UTC 2026-11-13T05:55:00Z] S43 · B4 本机 compose 全栈演练 ✅（用户裁定容器用本机）+ 修 content 源 fetch 真 bug

**镜像构建**：`podman build`（Buildah）出 `phlogo-api:local` / `phlogo-worker:local`（rust:1.83-slim → bookworm-slim，多阶段）——**首次真正构建**（此前只有 Dockerfile）。

**部署缺口修复**：
1. **`phlogo_jobs` 库**：postgres 官方镜像只建 `POSTGRES_DB=phlogo`，worker 启动即挂（`database "phlogo_jobs" does not exist`）→ 新增 `deploy/postgres-init/01-jobs-db.sql` 挂 `/docker-entrypoint-initdb.d`（新卷自动建；已存在的卷需手动 `CREATE DATABASE`——本次演练即手动补的）；
2. **compose build 键**：podman-compose 1.6.0 不认 `build: {context, dockerfile}` 内联 map（报 "no Containerfile found"）→ 改多行 map + `image:` 指向预构建镜像，`podman compose`（provider 转发）`--no-build` 起；
3. **api 补 `PHLOGO_DATABASE_JOBS_DSN`**（fetch/trigger 端点要它入队）。

**演练结果（全部经 caddy:21380，即 PROXY protocol 链路）**：
- `/health` 200；api 日志实证 **"serving with PROXY protocol v1"**（expect_proxy=true）；
- **登录限流真实验证**：12 连发 `400×9,429×3`（PS 引号毁 body 致 400，但 **400 也计入桶**——符合"凭据错仍计桶"设计）→ **IP 分桶在真实反代链路上生效**；
- 权限矩阵实测：service bearer 打 `PUT /config` → **403**；admin JWT 全通过；
- **订阅全链路**：login 200 → 建源 201 → profile 201 → token 201（一次性明文展示）→ `/sub/{token}.base64` **200 + base64 内容**（`…://node/🌐 LINKED 01`，命名渲染正常）；错 token → 401；
- **新端点实测**：`nodes/search`、`sources/search`、`filter/preview`、`format-configs` 全 200；`PUT format-configs/clash` 存储成功（`source:"stored"`）；
- **fetch 闭环实测**：content 源 → `finished, node_count=1` → 节点入库 → stats `total_nodes=1`；url 源（假域名）→ `failed` 且**旧映射保留**（P2-04 不变量）。

**修复的真 bug（第 3 个，由演练挖出）**：`run_fetch` 无条件把 `content` 列当 URL fetch → **`type=content` 源永远失败**（`ssrf: invalid url: missing host`）。上游语义是"内容内联"。修：按 `source.type` 分支（`content` 直接用库内文本，其余走 HTTP+SSRF）。**这是上游也有的等价性缺口**（B3 审计补充）。

**环境坑（备忘）**：podman-compose 1.6 无 `--no-build`（`podman compose` 转发可用）；curl.exe 的 `--data-binary "@file"` 在 PS 里 `@` 被 splatting 吃掉（换 Python requests 驱动 e2e）。

**验证**：演练后 `deploy/compose.yaml` 的 `podman compose config` 可解析；Caddyfile `caddy validate` 通过；校机全套测试绿（见 S42）；本机全栈 5 服务 Up（postgres/api/worker/web/caddy）。

---

## [UTC 2026-11-13T06:50:00Z] S44 · 步骤2：搜索端点 SQL 下推（AST → 参数化 SQL，替换内存全量过滤）

**实现**：
- `crates/filter/src/sql.rs` 新增 **`to_search_sql`**：同 `to_sql` 的白名单编译，但对准 `handlers/search.rs` 的**节点聚合视图**（`v.health`/`v.score`/`v.region`——跨 tag 最新/max，不钉 checker tag）；参数从 `$1` 起；`source_id` 走 EXISTS（别名 `nsv` 防撞）；**`contains` → `ILIKE '%'||$n||'%'`**（首版漏了 contains 走 `=` 被演练抓出）；`unlock` → SQL `FALSE`（无 streaming 表，文档化限制）。
- `handlers/search.rs::load_nodes`：AST 编译进 WHERE，视图内联子查询 `FROM (SELECT … ) v WHERE …`；**删除内存复检**——`eval::matches` 不支持 `contains`/`is_null`，会把 SQL 已匹配的行错杀（第二版被演练抓出；SQL 是权威，preview/sources 侧保留内存求值因 `matches_source` 全支持）。
- 分页在过滤后的行上切片；坏字段仍白名单 400（SQL 前）。

**真实验证（本机 compose 栈， drill 节点：trojan/drill.example.com/无 score 行/无 status 行）**：
`server contains drill`=1、`protocol eq trojan`=1、`score gte 0`=0（NULL 三值逻辑，正确）、`health is_null`=1、`and` 组合=1、no-match=0、unlock=0、坏字段=400。校机全套测试 + fmt + clippy 全绿。

**踩坑记录**：① 首版 GROUP BY+HAVING 形态列引用歧义，改为内联视图 + WHERE；② contains 首版落默认分支变 `=`；③ 内存复检错杀——**教训：两个求值器并存时必须二选一作为权威**；④ podman `--no-cache` 重建遇网络死结，改 `down/up` 换镜像（本演练中"镜像未更新"与"内存复检"两个因素叠加，靠 psql 手动跑同形 SQL 定位到是代码而非部署）。

---

## [UTC 2026-11-13T07:00:00Z] S45 · 步骤3：空键收尾复查 + configuration 文档去陈旧

**逐键 grep 复查（用户裁定的收尾项）**：
- `node_distribute_filter` / `node_name_template` / `filename_*`×3：**确认无消费者**（仅注释提及）；语义分别由 `subscription_profiles.filter_ast` / `naming_rule` / URL 格式后缀覆盖 → 按裁定**保留**（不删行，避免无谓 migration），在配置文档标注"改它无效果"；
- `dead_latency_ms` / `*_dead_window_days`×2 / `fetch_history_window_days`：确认无消费者，但属**生命周期阈值**——B1 落地后必须接线（不是删）；
- `default_sponsor_text`：等 sponsor 域裁定；
- 最终清点：**12 键生效**（6 权重 + max_distribute + node_alive_check_window + orphan_retention_days + check_run_history_days + cleanup_interval_minutes + score_min_samples + grade_*×3）、**8 键空心但全有标注的存在理由**——"改了没反应"的静默陷阱在文档层面完全消除。

**`docs/configuration-and-api.md` 去陈旧**（该文档还是 expr-lang 时代写的）：
- 动态配置表加**状态列**（✅ 生效 / ⏸ 无消费者 + 原因），补 `score_min_samples`/`orphan_retention_days`；`check_run_history_days` 默认值改 14；
- 「筛选引擎」整节重写：expr-lang 字符串 → **Filter-AST JSON**（白名单字段/运算符/限制/已知 unlock 限制）；
- 「节点命名与 Sponsor」：`node_name_template` → `naming_rule`；**明确标注 sponsor 域未实现**（原来读起来像已实现）；
- 「格式配置」：改为实际的 `/api/v1/format-configs` 端点 + 写入时校验。

**决策**：三步全部完成（B4 演练 / SQL 下推 / 空键复查）。遗留待用户裁定：A1 已做（PROXY protocol）；sponsor 域恢复与否；B1 测活宿主机选择。

---

## [UTC 2026-11-13T07:25:00Z] S46 · sponsor 域恢复（用户裁定）+ B1 宿主机决策记录 + 演练栈 down

**用户三项裁定**：① sponsor 域**恢复**；② 校机走 mihomo 代理可当 B1 测活宿主；③ 演练栈 down。

**演练栈已 down**（`podman compose down`，pgdata 卷保留可复用）。

**sponsor 域实现**（B3 gap #2 关口）：
- **migration 0011**：`sponsors` 表（`filter_ast` JSONB 替代上游 expr 字符串；priority 索引）；
- **`crates/registry/src/sponsor.rs`**：CRUD + `match_text`——**上游 `Service::Match` 语义逐条对齐**（priority 升序遍历、后命中覆盖前者、无命中落 `default_sponsor_text`、解析失败的 sponsor 跳过不报错）；
- **`handlers/sponsor_admin.rs`**：`GET/POST /sponsors`、`GET/PATCH/DELETE /sponsors/{id}`（AdminOnly 面，写入前 AST 白名单校验）；
- **订阅注入**：`/sub` 渲染时按节点视图求 sponsor 文案注入 `{{sponsor}}` 字段；DB 错误降级为默认文案，绝不 fail 订阅；
- **契约**：utoipa +2 → **27 条路径**（11562 B）。

**验证**：校机真实 PG 全套 `FULL_TEST_EXIT=0`（含 0011 迁移）、fmt 0、clippy 0（修了 4 个小错：Cargo 依赖重复 / match_text 签名 / too_many_arguments / if-same-then-else 死分支）。

**B1 宿主机决策记录**：校机跑 MiaoSpeed + mihomo 代理出站——worker 连校机 MiaoSpeed WS（校园网内可达），MiaoSpeed 经 mihomo 出墙测活。待实施（纯部署配置：校机起 mihomo + MiaoSpeed `-proxy` 参数或系统代理）。

**等价性终态**：26 条上游路径 25/26 通过，唯一剩余 `auth/refresh`（单管理员重新登录覆盖，暂不做）。

---

## [UTC 2026-11-13T07:55:00Z] S47 · sponsor 管理前端页面（p6 增补）+ `build:ele` 绿

**新增**（照 subscriptions.vue 既有惯例：Element Plus 表格 + 对话框表单 + typed API 层）：
- `web/vben-admin/src/api/phlogo.ts` 补 sponsor 段（`SponsorView/SponsorUpsert/listSponsors/createSponsor/updateSponsor/deleteSponsor` + `simpleCondition` AST 构造辅助）；
- **`views/phlogo/sponsors.vue`**：分页表格（priority 可排序 / 注入文案 / AST 预览）+ 新建/编辑对话框——表单把「字段+运算符+值」三个简单控件拼成 **Filter-AST 单叶条件**（空值 = 匹配全部节点），提交前本地构造、服务端白名单兜底；删除带确认；
- 路由 `PhlogoSponsors`（`/phlogo/sponsors`）。

**校机构建**：checkout 实际在 `C:\Users\YiZhidi\p0-04\vue-vben-admin-5.7.0`（根下那个不存在）；node v22.22.2 在 `C:\Users\YiZhidi\shortcuts-verify\`、pnpm 10.33.4 只有 `.tools\pnpm\10.33.4\bin\pnpm.CMD`（无全局 exe）——PATH 组合后 **`build:ele` 11/11 successful**（1m04s）。

---

## [UTC 2026-11-13T08:40:00Z] S48 · 全仓改名：paily→phlogo、core→gnosis、项目名→Phlogo Gnosis、api→curse

**用户裁定**（三轮问答确认边界）：
1. **scope**：全量替换 `core→gnosis`（含目录/进程名）；
2. **runtime**：运行期标识符全改，含数据库迁移；
3. **upstream**：保护上游外部引用（`openpaily/paily-*`）不改；
4. 追加裁定：**品牌词组 `Paily Connect` → `Phlogo`**（`Connect` 是品牌名的一部分，不是符号——`ConnectInfo`/`PgConnection` 等技术标识符一律不动）；**api→curse 只改服务名**（HTTP 路由前缀 `/api/v1` 与 OpenAPI/openapi-ts/api-contract 等标准与工具名保持不动）；
5. 命名组合：**Phlogo Gnosis / gnosis-curse**。

**改名映射**：

| 原 | 新 |
| --- | --- |
| `paily` / `Paily` / `PAILY_*` | `phlogo` / `Phlogo` / `PHLOGO_*`（含环境变量前缀） |
| `core` / `Core`（产品语境） | `gnosis` / `Gnosis` |
| `core-api` / `core-worker` | `gnosis-curse` / `gnosis-worker` |
| `Paily Connect` / `Paily Connect Core` | `Phlogo` / `Phlogo Gnosis` |
| crate `api`（`apps/api`） | crate `curse`（`apps/curse`），二进制 `curse` |
| 镜像 `phlogo-api` / `phlogo-worker` | `phlogo-gnosis-curse` / `phlogo-gnosis-worker` |
| 前端 `src/api/paily.ts`、`src/views/paily/`、路由模块 `paily.ts` | `src/api/phlogo.ts`、`src/views/phlogo/`、`modules/phlogo.ts` |
| 队列库 `paily_jobs` / 库 `paily` / PG role `paily` | `phlogo_jobs` / `phlogo` / `phlogo` |
| compose 服务 `api` | `curse` |
| `deploy/Dockerfile.api` | `deploy/Dockerfile.curse` |

**保护表**（脚本内置，dry-run 断言零违规）：`openpaily/paily-core|fetch|check`、任何含 `score` 的标识符（`score`/`node_scores`/`score_decay_alpha`…）、第三方 crate 内嵌 core（`futures-core`/`sqlx-core`/`axum-core`/`serde_core`/`rand_core`/`parking_lot_core`/`core-foundation-sys`/`apalis-core`/`tracing-core`/`time-core`/`windows-core`/`icu_locale_core`）、技术标识符 `ConnectInfo`/`PgConnection`/`CheckerError::Connect`。

**改名脚本**（`.tmp/rename-apply.py` 等）：先收集全部替换（位置基于原文）再**从右往左**应用——第一版按顺序改字符数组导致长度变化后索引错位，产出 `gnosiscore` 这类污染（`score` 被拆），改为右到左后归零。

**误伤修复**（23 文件）：`openphlogo/*` → `openpaily/*`；`phlogo-gnosis-rust-rebuild-plan.md` → `paily-core-rust-rebuild-plan.md`（用户本地方案文件名，磁盘上就是这个名）；`phlogo-gnosis` → `paily-core`（其余均源自上游项目名）。

**校机数据库迁移**（运行期改名）：`ALTER ROLE paily RENAME TO phlogo` + `ALTER DATABASE paily|paily_jobs RENAME TO phlogo|phlogo_jobs`（须用 `postgres` 超级用户；`ALTER ROLE` 以自身身份连接会报 "session user cannot be renamed"）。

**验证**：校机 `cargo test --workspace -- --test-threads=1` → **FULL_TEST_EXIT=0**（43 个测试二进制全绿）、fmt 0、clippy 0；前端 `build:ele` **11/11 successful**（含新路由 + 8 页面）。
> 首次 workspace 并发跑时 `check_persist` 2 个用例 30s 超时——**并发下 PG 连接池争用**，单线程重跑通过，非改名引入。

**顺带发现并修正**：校机前端 checkout 此前**从未 ship 过 paily 视图**（`src/views/` 只有 vben 自带页面），此前的 "build 绿" 是构建缓存产物的假绿；本次显式 `mkdir` + 逐文件 ship 后才是真绿。

---

## [UTC 2026-11-13T09:05:00Z] S49 · 品牌修正（去掉误加的 Wish）+ 对外兼容面审计

**用户更正**：项目名应为 **Phlogo Gnosis**（不是 "Phlogo Wish Gnosis"）；「Connect」是**品牌词**的一部分（原名 `Paily Connect` → 现 `Phlogo`），**不是符号**——`ConnectInfo`/`PgConnection`/`CheckerError::Connect` 等技术标识符一律不动（动了会破坏 axum/sqlx 兼容）。

**修正内容**：
- `Phlogo Wish Gnosis` → **`Phlogo Gnosis`**（12 文件：README、utoipa 契约标题、crate doc 注释、领域文档）；
- `Phlogo Wish` → **`Phlogo`**；
- 订阅下载文件名默认值 → **`phlogowish.yaml` / `phlogowish.json` / `phlogowish.txt`**（用户裁定；注意这三个 `filename_*` 键当前**空心**——实现无 `Content-Disposition`，实际文件名由 URL 后缀决定，故此项仅文档层）；
- ACTION-LOG 中记录的「Connect→Wish」裁定同步改写为真实裁定；
- 领域文档的「Phlogo 主服务」→「Phlogo Gnosis」（`docs/rebuild/` 里的 "Phlogo" 指**上游 Go 项目**，保留原样）。

**对外兼容面审计**（脚本对比改名前 `cb0c8ca` 与改名后 `HEAD`）：

| 面 | 结果 |
| --- | --- |
| HTTP 路由（`/api/v1/**`、`/api-docs`、`/sub/**`、`/health`） | **SAME** |
| JSON 响应字段名 | **SAME** |
| DB 表名（migrations 全量） | **SAME** |
| MiaoSpeed WS 帧字段名（`Basics.ID/Slave/SlaveName/Invoker/Version` 等） | **SAME** |
| 环境变量前缀 | DIFF：`PAILY_*` → `PHLOGO_*`（**用户裁定彻底改名、不管兼容**） |

- **MiaoSpeed 侧无破坏**：`Basics.Invoker` 对应 MiaoSpeed 的 `-whitelist` bot id；P0-02 实测捕获时上游用的就是 `phlogo`（原提交 `51e6f50` 的 golden fixture 已是 `phlogo-spike`/`phlogo`，非本次改名所致），**保持不变即兼容**。`SlaveName`/`Invoker` **不参与签名**（签名只覆盖 vendor 与固定字段序），改名不影响 golden。
- **签名契约未动**：`golden_signature_matches_p0_02` 的期望值仍是 P0-02 用 Go 交叉验证过的字节级签名。
- **无 `Content-Disposition`**：订阅下载文件名不由本服务控制，故品牌改名不改变客户端可见行为。
- **DB schema 无品牌词**：迁移未引入/污染列名，仅库名/角色名改为 `phlogo`（内部标识符，与其他项目无互操作关系）。

### S49b · `paily` 残留审计（改名完成度）

全仓逐处扫描（脚本 `.tmp/paily-audit.py`）：**125 处，23 个文件，全部在 markdown 文档内；非 markdown 文件（代码/配置/契约/前端/迁移/脚本）零残留**。

剩余的 125 处按性质分三类，**全部保留**：

| 类别 | 数量 | 例子 | 保留理由 |
| --- | --- | --- | --- |
| **上游真实项目名** | ~90 | `openpaily/paily-core`、`paily-fetch`、`paily-check`、`openpaily/paily-fetch/format` | 真实存在的上游仓库/服务名（`openpaily/paily-core` 已 archived）；改成 phlogo 会变成**错误描述**，且链接失效 |
| **改名历史记录** | ~30 | ACTION-LOG 的映射表「`paily`/`Paily`/`PAILY_*` → `phlogo`…」、S48 标题「paily→phlogo」 | 记录**改名前的原名**才能读懂；改了就失真 |
| **磁盘上的真实文件名** | ~5 | `paily-core-rust-rebuild-plan.md` | 用户本地方案文件名（`S:\Downloads\`，2026-11-09 修改），文档按名引用它 |

**唯一清理**：README 的「从旧版（`paily` 命名）升级」说明段——用户裁定「彻底改名、不管兼容」，故删除（保留它反而暗示需要迁移路径）。

> 判定口径（供后续复查复用）：**引用上游真实名字 ≠ 改名遗漏**。凡 `paily` 指向一个真实存在的外部仓库/服务/文件名，就不是残留。

---

## [UTC 2026-11-13T10:00:00Z] S52 · 最新 mihomo / sing-box 协议支持度审计与修复

**触发**：用户报「校机有 mihomo 在跑，检查对最新 mihomo 和 sing-box 的支持」。

**环境事实**：校机 mihomo **v1.19.32**（run2 监听 mixed 7890 + controller 9090），真实订阅样本 4 份（各 52 节点：**vless 36 + hysteria2 16**），校机**无 sing-box**（清单取自 sing-box v1.14.0 官方文档）。

**审计发现（真 bug）**：
- `vproxy-adapter::cred_for` 只认 5 类协议 → **hysteria2 / anytls / hysteria / tuic / snell / wireguard / socks5 / http / mieru / sudoku / ssr 全部静默丢弃**；
- 真实样本实测 **keep_rate 69.2%**（156 节点丢 48）；
- 讽刺的是 **身份层早就支持**——`domain::Credential` 有 `Tuic`/`PrivateKey`/`UsernamePassword`/`Psk`/`Key`，`fingerprint::node_auth` 也逐协议对齐了上游 `nodeAuth`；**缺的只是解析层接线**；
- sing-box 发射层是硬编码简版（只认 vmess/vless/trojan）→ hysteria2 节点输出时**丢 password**。

**修复**：
1. `cred_for` 按上游 `nodeAuth` **逐协议**补齐 11 类（含 tuic v5 的 `uuid:password`）；**移除 `cipher` 回退**——加密算法不是凭据，会污染身份；
2. 新增 `singbox_outbound_from_clash(payload, name)`：从完整 Clash payload 做全保真 sing-box 转换（`cipher`→`method`、`private-key`→`private_key`、Clash 扁平 TLS→sing-box `tls` 对象、`network`+`ws-opts`/`grpc-opts`→`transport`、丢弃 Clash 专有键）；`render_subscription` 改用它；
3. 回归测试 5 个（解析 2 + 发射 3），含「凭据口径与上游 `nodeAuth` 逐条一致」断言。

**修复后真实样本回归**：**keep_rate 69.2% → 100.0%**（156/156）。

**踩坑**：① transport 转换必须早于「丢弃 Clash 专有键」，否则 `network` 先被删 → transport 变 Null（顺序 bug）；② `json!({"host": ...})` 赋给 `t["host"]` 时 `unwrap_or_else` 需显式 `Value`（类型推断走偏）；③ 测试块插入位置撞到原 doc comment（clippy `empty line after doc comment`）。

**文档**：新增 [`12-protocol-coverage-audit.md`](12-protocol-coverage-audit.md)（含仍未覆盖项的如实清单：shadowtls / wireguard 多字段 / hysteria2 obfs 与端口跳跃 / mihomo 1.19 的 ech-opts 与 x25519mlkem768）。

**校机验证**：`cargo test --workspace` **FULL_TEST_EXIT=0**（43 个测试二进制）、fmt 0、clippy 0。

---

## [UTC 2026-11-13T11:15:00Z] S53 · 校机装 sing-box v1.14.4 实测 + Sub-Store 对比 + 修出 REALITY 缺 uTLS 的真 bug

**用户要求**：① 校机装 sing-box 实测；② 回答「订阅转换现在用的是什么」并对比 Sub-Store。

**环境**：校机新装 **sing-box v1.14.4**（go1.26.10，with_utls/with_quic/with_clash_api 等全标签）；
mihomo v1.19.32 作对照（已在跑，7890）。

**转换现状**：**自研，无外部转换依赖**——`crates/vproxy-adapter`（解析）→ `crates/domain::NormalizedNode`
（类型化中间表示）→ `crates/distribution::output`（Clash/sing-box/Base64 三 producer）。
VProxy 因「单 crate 应用无法轻依赖 + 方案禁上游类型渗透」被 P0-01 判为不可用（见 `p0-01-vproxy.md`）。

**与 Sub-Store 的定位差异**：Sub-Store 是「多格式转换服务」（十余种目标格式 + 脚本/规则/聚合）；
我们是「节点池 + 评分 + 筛选 + 按档案生成订阅」，转换只是最后一公里。Sub-Store 不具备的能力
（身份去重/评分/测活/地区配额/token 鉴权/sponsor/权限分层）恰是本项目的核心。
借鉴点已部分采用：统一中间表示 + 每目标独立 producer + 显式字段映射。

**实测方法**（真实验证，非纸面推导）：用我们的转换规则把**校机真实订阅**（52 节点）转成 sing-box 配置
→ 交 **真 sing-box 二进制** `check` → `run` → `curl --proxy` 实测出网。

**结果**：
- `sing-box check`（52 outbound 全量）**EXIT=0**；`run` 起来并监听 127.0.0.1:2080；
- **逐节点类型隔离**：`hysteria2` → **204 OK（1.83s）**；`vless+REALITY+vision` → **204 OK（0.95s）**
  → **我们的转换配置真实代理出了网**；
- 失败两类（vless-ws 18 个、vless 无 reality 的 vision 2 个）经对照判定为**节点/网络侧**：
  mihomo 走同一订阅 204 OK，ws 节点后端 172.64.229.88 是 Cloudflare（校园网不可达）。

**实测抓出的真 bug（已修）**：**REALITY 在 sing-box 侧必须带 uTLS**，否则 sing-box 拒绝启动
（`FATAL initialize outbound[7]: uTLS is required by reality client`）。
Clash 用 `client-fingerprint` 表达，sing-box 要 `tls.utls{enabled,fingerprint}` —— S52 的转换漏了，
会导致整份 sing-box 配置起不来。修复：`client-fingerprint` → `tls.utls`，并顺带透传 `flow`（vision）；
新增回归测试 `singbox_from_clash_maps_client_fingerprint_to_utls`。

**文档**：新增 [`13-substore-compare-and-singbox-test.md`](13-substore-compare-and-singbox-test.md)
（含与 Sub-Store 的逐维度对比表、仍存在的缺口清单——`shadowtls` / `hysteria2` 的 obfs 与端口跳跃 /
wireguard 多字段 / mihomo 1.19 的 ech-opts 与 x25519mlkem768）。

**方法论沉淀**：**新增协议字段以真实二进制校验为准**，不纸面对照字段表——Sub-Store 的 sing-box producer
正是这样处理的（uTLS/obfs/server_ports/method 都显式映射），我们此前没有，因而被实测打脸。

---

## [UTC 2026-11-13T11:45:00Z] S54 · 协议缺口全部补齐（逐协议 × 双真实二进制校验）

**用户指令**：「把缺口全补上」——S52/S53 文档里列出的缺口逐条补齐。

**方法**：为 13 种协议各生成**单节点配置** + **全协议配置**（用我们自己的转换规则的 Python 等价实现），
交校机**真 sing-box v1.14.4**（`check`）与**真 mihomo v1.19.32**（`-t`）校验，**任何 FAIL 都算未完成**。

**最终结果**：**sing-box 12/12 PASS、mihomo 13/13 PASS**（wireguard 按设计只在 mihomo 侧）。

**被二进制打脸、必须改实现的 10 个问题**（全部已修）：
1. sing-box 不认 `type: ss` → 改写为 `shadowsocks`；
2. `shadowsocks`/`snell`/`socks`/`http` 无 `tls` 字段 → 按协议条件化（`SINGBOX_NO_TLS`）；
3. `hysteria2.up_mbps` 要 int（mihomo 给 `"100 Mbps"`）→ `mbps_of()` 带单位解析；
4. `tuic.heartbeat` 要 duration 字符串（mihomo 给 int）→ `duration_ms()`；
5. **单位分两套**：`hop-interval` 是**秒**（`duration_secs`），`heartbeat` 是**毫秒**（`duration_ms`）——混用会被拒；
6. `server_ports` 是**数组**且元素用**冒号**（`bad port range: 60000-65530`）→ `server_ports_of()` dash→colon；
7. `plugin_opts` 是**字符串**（mihomo 给 map）→ map → `k=v;k2=v2`；
8. **sing-box 1.13 已移除 wireguard outbound**（迁 endpoint 形态）→ 加入 `SINGBOX_UNSUPPORTED`；
9. mihomo `hysteria2.obfs` 是**类型**、`obfs-password` 才是密码 → 正确拆分 `obfs{type,password}`；
10. **hysteria v1 的凭据是 `auth-str`**（mihomo）不是 `password` → 解析层加回退（否则丢节点）。

> 第 6 条值得单记：sing-box 官方文档写冒号、上游 issue #2416 声称应改 dash——**实测二进制只接受冒号**，
> 即 issue 的结论是反的。又一次证明「以二进制为准」。

**行为变更（重要）**：sing-box 对未知类型/字段是**整份配置拒绝启动**，故不可表示的协议
（`mieru`/`sudoku`/`ssr`/`shadowtls`/`wireguard`）在 **sing-box 输出中完全剔除**（含 selector/urltest 的
引用列表，否则引用不存在的 tag 会让客户端起不来）；**Clash/mihomo 输出不受影响**。
新增 `singbox_supports(protocol)` 对外暴露该判定。

**解析层**：补 hysteria v1 的 `auth-str`/`auth` 回退（mihomo 用它，sing-box 用 `password`）。

**回归测试**：新增 7 个（hysteria2 端口跳跃+obfs、tuic、ss plugin、tls 条件化、跳过不可表示协议、snell obfs、socks5→socks）。

**校机验证**：`cargo test --workspace` **FULL_TEST_EXIT=0**（43 个测试二进制；distribution 35 个用例）、fmt 0、clippy 0。

---

## [UTC 2026-11-13T12:10:00Z] S55 · 复核 issue #2416 并更正表述 + 修出「裸端口」真漏洞

**触发**：用户追问「上游 issue #2416 声称该改成 dash，给我链接和分析」。

**核查来源**（全部走 REST，无浏览器）：
- issue 正文与评论：`api.github.com/repos/SagerNet/sing-box/issues/2416`（+ `/comments`）
- 结案指向的提交：[sing-quic 8e6a84e](https://github.com/SagerNet/sing-quic/commit/8e6a84e8211b040a976509381436c53f2eba508e)（*Add port hopping for hysteria2*）
- 该提交里 `parsePorts` 的实际实现（强制冒号）
- v1.11.0-beta.17 的 `protocol/hysteria2/outbound.go`（确认解析当时在 sing-quic 依赖里）

**事实链（更正 S54 的表述）**：
- issue #2416 于 2024-12-29T05:12Z 提在 **1.11.0-beta.17**，日志 `bad port range: 2080:3000`；
  作者断言「文档的冒号错了，应该改 dash」。
- **真实原因是 beta 期代码与文档不一致**（不是文档笔误）：解析在 sing-quic 里，当时要求 **dash**。
  **直接证据**：beta.17 的 `go.mod` pin 的是 `sing-quic v0.4.0-beta.1`，该 tag 的 `parsePorts` 判定
  `strings.Contains(portRange, "-")`（dash）——所以 beta.17 拿到 `"2080:3000"` 必然报 `bad port range`，
  **提案人的现象观察是对的**。
- 维护者同日 07:40Z 在 sing-quic 把该函数改成**冒号版**（`strings.Contains(portRange, ":")`）
  ——**改的是代码（向冒号靠）**，提案里 dash 的修法方向没被采纳；提案人随后自行关闭 issue。
- 因此 S54 写的「该 issue 报告的是文档笔误但结论反了」**措辞不准确**；准确说法是
  **现象真实、修法相反**。已在 `13-substore-compare-and-singbox-test.md` §5.3 更正为完整事实链 +
  两版 `parsePorts` 源码对照。
- 真二进制 A/B（v1.14.4）：`["60000:65530"]` ✅、`["2080:3000"]` ✅、`["60000:"]` ✅（开口）；
  `["60000-65530"]` ❌、`["60000"]` ❌、`["...","443"]` ❌（`bad port range: 443`）。

**由此修出真漏洞**：`parsePorts` 要求**每个元素都含冒号**，而 mihomo 允许写单端口（`ports: "443"`）。
原 `server_ports_of()` 只做 dash→colon，会产出裸端口 `["443"]` → sing-box 拒收（整份配置起不来）。
修复：新增 `normalize_port_range()`，替换 dash 后若不含冒号则补成 `"N:N"`（`"443"`→`"443:443"`，
`"60000:"` 保持开口）。回归测试 `singbox_port_ranges_always_carry_a_colon`（4 组输入）。
二进制复核：`["443:443"]` ✅、`["443:443","8443:8444"]` ✅。

**过程教训（假绿）**：首次验证时 `cargo test` 显示 `0 passed / 35 filtered out` —— 新测试根本没编译进去。
原因：`scp -r localdir host:destdir` 在 destdir 已存在时会把 localdir 复制**到 destdir 内部**（得到
`crates/crates`），真实树保持陈旧。原 `rename-full-verify2.ps1` 第 10 行有前置 `rmdir` 才是正确姿势。
**判据：测试名过滤后若 `0 passed` 且 `N filtered out`，就是没编译进去，不是通过。**

**校机验证**：`cargo test --workspace` 全绿、fmt 0、clippy 0。

> ⚠️ **时间戳口径更正**：S48–S56 各条开头写的 `…Z` 实际记的是**本机墙钟（Asia/Taipei, +08:00）**，
> 少减了 8 小时，并非真 UTC。自本条起按真 UTC 记（本机 12:30 +08 = **04:30Z**）。
> 换算：条目 `HH:MM Z` → 真 UTC `HH-8:MM Z`。顺序与相对间隔不受影响。

---

## [UTC 2026-11-13T04:30:00Z] S57 · 决策 A 落地：Sub-Store 转换旁挂服务（spike → 薄壳 → 真机 E2E → 固化 pin）

**用户指令**：「A. 旁挂 sub-store-convert」。

### 执行的每一步 / 每条命令

| # | 命令 / 脚本 | 目的与结果 |
| --- | --- | --- |
| 1 | `python -X utf8 .tmp\substore-convert-probe.py` | 读 `tbxark/sub-store-convert` README：API 是 `GET /sub?target=&url=`，只收 URL；Node 22+；仓库自称 MIT 但 vendor 了 Sub-Store |
| 2 | `python -X utf8 .tmp\npm-check.py` | `@sub-store-convert/{core,cli,app}` 全 **404 未发布到 npm** |
| 3 | `ssh school-guest "node --version && (docker --version …) && (bun --version …)"` | node 不在非交互 PATH → 换全路径 |
| 4 | `ssh school-guest "C:\Users\YiZhidi\shortcuts-verify\node.exe --version && …"` | **Node v22.22.2**；**无 Docker、无 Bun** |
| 5 | `python -X utf8 .tmp\substore-core-probe.py` | 读 `packages/core/src/index.js` / `app/src/index.js` / `build/package.json`；确认 `convert(url,target)` 内部 `await fetch(url)`；**build/index.js 503,495 B 已签入** |
| 6 | `python -X utf8 .tmp\run-substore-spike.py` | scp `probe.mjs` + curl 下载 bundle 到校机 `C:\Users\YiZhidi\substore\index.mjs`；`node probe.mjs` → `IMPORT FAILED: Cannot find package 'peggy'` |
| 7 | `ssh … dir /b shortcuts-verify & dir /b pnpm…\bin` | 无 npm，有 `pnpm.CMD` (10.33.4) |
| 8 | `python -X utf8 .tmp\run-substore-spike2.py` | 探 npm 候选路径全 MISSING → 改 pnpm |
| 9 | `python -X utf8 .tmp\run-substore-spike3.py` | `pnpm add peggy js-base64 json5 lodash yaml`（PATH 注入 node 目录）→ 成功；`probe.mjs` **IMPORT OK，20 targets** |
| 10 | `probe.mjs` 三种输入对照 | **base64 URI / 纯 URI → 0 节点、0 字符**；**Clash YAML → 3/3 节点**，surge/qx/loon/stash/shadowrocket/egern/mihomo/sing-box/uri/json 全出内容 |
| 11 | `python -X utf8 .tmp\run-substore-accept.py` | 真实 52 节点订阅验收：surge **52 行满额**；qx 36 / loon 34 / surfboard 16（**客户端能力过滤，非丢数据**）；`data:` URL 长 **62,751** |
| 12 | 写 `deploy/substore-sidecar/server.mjs` | 自写 POST 薄壳（GET 塞不下 62 KB）；`/health`、`/convert`、`/convert/shape`；空产出 → 502 |
| 13 | `python -X utf8 .tmp\run-sidecar-e2e.py` | `start /B` 启动失败（srv.log 空） |
| 14 | `python -X utf8 .tmp\diag-sidecar.py` | `node --check server.mjs` = 0；前台跑 .cmd → **core loaded; 20 targets; listening on 3000**（服务本身没问题，是 `start /B` 引号坑） |
| 15 | `python -X utf8 .tmp\run-sidecar-e2e3.py` | Python `Popen` 保活 + netstat 解析 PID → **E2E 全绿**：11/11 格式 200、49–119 ms、3/3 错误路径正确；精确 kill |
| 16 | `python -X utf8 .tmp\pin-substore.py` | `git ls-remote` → **commit `cddcab9c536da53c0f867cd6b5738791bc4e8121`**；bundle **SHA-256 `ee99f9a7…ef`**、503,527 B |
| 17 | `python -X utf8 .tmp\verify-pin.py` | 固定 commit 的 CDN URL 返回 200 且 `HASH MATCH = True` |
| 18 | 写 `package.json` / `fetch-bundle.mjs` / `README.md` / `docs/rebuild/14-substore-sidecar.md` | 定版依赖、pin 校验脚本、部署说明、详细大步骤页（含 CI 门禁命令） |
| 19 | 更新 `docs/THIRD_PARTY.md` / `00-index.md` | 登记新组件（AGPL 判定、commit、哈希、定版依赖）；索引加第 14 页 |

### 关键结论

- **输入契约固定为 Clash YAML**（我们已有的 `mihomo_yaml` 产物）→ **不需要等 `node_payloads`（P1）**；
  S56 里写的「格式扩充被 P1 挡住」**不成立**，已更正。
- `data:` URL 免掉了「给 sidecar 一个可抓地址」的问题，但 62 KB 塞不进 GET → 自写 POST 壳；
  常驻服务让 503 KB bundle + peggy 只解析一次。
- **QX/Loon/Surfboard 节点数少于 52 是客户端能力过滤**，不是 bug。
- **许可证**：上游仓库 MIT，但 vendor 打包了 Sub-Store（AGPL-3.0）→ 本 sidecar 按 AGPL-3.0 分发；
  Gnosis 本身即 AGPL-3.0，兼容。

### 明确未做（下一步）

Rust `crates/format-sidecar` 客户端、`format_configs` 扩展格式行与**模板包装**（sidecar 只给节点行）、
`apps/curse` 分派、compose 服务与 healthcheck、CI job。施工图见 `14-substore-sidecar.md` §9。

### 本步踩的坑

`cmd /c start "" /B` 引号导致进程没起来（srv.log 空）；前台验证 OK 后改用 Python `Popen` 保活 +
netstat 解析 PID 精确停止。另：PS5.1 `Set-Content -Encoding UTF8` 写 BOM 会让 sing-box 拒收配置，
本轮全部改用 Python / `.mjs` 显式编码。

---

## [UTC 2026-11-13T07:10:00Z] S58 · 生产就绪只读审计 → 并入缺口总账（M1–M17 **全部要做**）

**用户指令**：问「上游为什么说不适合生产？我们这版适合了没有？」+ 5 个必确认项 +
README 与 Rust Workspace 一致性提醒；随后裁定「把这些核查记录进去，说这些还没做都要做的」。

**执行方式**：**只读取证**——未改任何代码、未跑构建、未联网发帖。仅用 `web_fetch`（CDN README）、
`grep`、`read`。

### 执行的每一步 / 每条命令

| # | 动作 | 结果 |
| --- | --- | --- |
| 1 | `read README.md`、`grep 「不适合\|生产环境\|P1」` | 本仓 README 已是纯 Rust；命中 `10-review-crosscheck.md` 的缺口表 |
| 2 | `web_fetch https://cdn.jsdelivr.net/gh/openpaily/paily-core@main/README.md` | 取到上游 `[!WARNING]`「该代码库非直接生产可用」原文三条理由（早期开发 / 可能数据丢失 / **近期无推进计划**） |
| 3 | `read 10-review-crosscheck.md:52-85` | §2.3 现成缺口：流媒体分别评分 ❌、场景化配置 ❌、增量测活 ❌、多 Checker 置信度 ❌ |
| 4 | `grep "Matrix\|preset\|download\|unlock\|streaming" *.rs` | 定位 `checker-miaospeed/types.rs`、`result.rs`、`filter/sql.rs` |
| 5 | `read p0-02-miaospeed.md:137-262` | D.3 `Configs`（`PingAddress`/`DownloadURL`）、D.7 矩阵白名单、D.8 vendor |
| 6 | `read apps/worker/src/check_job.rs` | `handle_check` **只记账**（`record`+`finish`），注释自陈「real MiaoSpeed call is p3-02+」 |
| 7 | `grep "CheckJob {\|MiaoSpeedAdapter\|scoring::"` | `CheckJob {` 仅定义+测试；`MiaoSpeedAdapter` 仅在自身单测；**`apps/` 对 scoring 零引用** |
| 8 | `read apps/worker/src/main.rs`、`grep router.rs routes` | 调度只有 fetch 300s + cleanup；**无 check tick / 无 `/check/trigger`** |
| 9 | `read adapter.rs:1-95,95-164` | `PingAddress` 硬编码 gstatic、`DownloadURL=""`、时长/线程全 0；`map_results` **`deep: None`** |
| 10 | `read scoring/engine.rs:86-125`、`grep alpha` | EWMA `alpha=0.7`；**速度只取 `records[0]` 不平滑**；`w_speed=0.55` |
| 11 | `read handlers/subscription.rs` | `/sub` 输出 `{}://node/{}` **占位**；`let _ = _filter_ast; let _ = preset;`，preset 硬编码 `"common"` |
| 12 | `grep deploy/compose.yaml`、`Dockerfile.curse/worker` | compose 有 `miaospeed`/`web`，README quickstart 未起；miaospeed 为 `:latest` 浮标；产物是 `--bin curse`/`--bin worker` |
| 13 | `grep naming.rs` | 分级阈值 A/B/C/D = 0.75/0.50/0.25 → 无测速时 `score ≤ 0.45`，**A/B 永不出现** |
| 14 | `edit 09-self-completed-gaps.md`（新增 §D）、`edit 00-index.md` | 把核查结论并入既有缺口台账，并加「阶段完成 ≠ 可部署」告警 |

### 结论（已写入 `09-self-completed-gaps.md` §D）

- **上游警告不自动继承**：①②是 Go 那版代码状态，③意味着不能等上游修；我们版合格与否看自己。
- **我们版目前不适合生产**，硬阻塞：`/sub` 假节点、测活从未发生、无 CheckJob 入队、评分零接线、
  deep 被丢、`filter_ast`/`preset` 被忽略、`node_streaming` 表不存在。
- **5 问逐条**：① 不能模拟真实 API 请求 ② 不能区分流媒体/AI ③ 不支持按服务评分
  ④ 当前零带宽消耗但**分数封顶 0.45**、开启前必须定配额 ⑤ 算法够灵敏但链路是断的、速度无平滑。
- **README 一致性**：本仓 README 已是 Rust；Go 段属**上游 README**。真实不一致三处：
  quickstart 少起 miaospeed/web、miaospeed `:latest` 浮标、rebuild 文档残留 `gnosis-api`/`-p api`。

### 口径变更（用户裁定）

- **B1 → M2、B2 → M1**：状态由 `⏸ 明确推迟` 升级为 **`MUST-DO / P0`**，重启条件视为已满足。
- 新增 **M1–M17**（含 M15–M17 文档一致性、M11/M12 按服务评分为**新设计**非接线）。
- **上线总门禁**：抓取 → 真测活 → deep 落库 → 评分更新 → `/sub` 出真实节点 → `mihomo -t` 通过 → 真代理出网 204。
- 本节**只做记录与排期，未改代码**；代码实现按 M1→M2→M3→M4→M5 逐步落地，每步照惯例「做一步、写一页、push 一步」。

---

## [UTC 2026-11-13T07:30:00Z] S59 · 追加「生产级能力 5 项」到缺口总账（M18–M22，全部要做）

**用户指令**：「这些呢：按业务选择出口 / 按用户地理位置调度 / SLA 保障 / 故障自动恢复 / 权限、审计、合规」——
5 行能力对照（**无表头，不臆测两列指向哪两个对象**），延续 S58 的裁定「都要做」。

### 执行的每一步 / 每条只读核查

| # | 命令 / 动作 | 结果与证据 |
| --- | --- | --- |
| 1 | `grep migrations "audit\|sla\|geo\|subscription_profiles\|CREATE TABLE"` | 迁移里**无 `audit*` / `sla` 表**；`subscription_profiles` 只有 filter_ast/region/health/min_score/preset 等（`0009:2-16`） |
| 2 | `grep deploy/*.yaml "healthcheck\|restart\|depends_on\|PROXY"` | compose 有 `depends_on` + PROXY v1 头，**无 `healthcheck` / `restart` / `stop_grace_period`** |
| 3 | `grep crates+apps "client_ip\|geoip\|x-forwarded\|audit\|SLA\|SLO"` | 全仓仅 1 条命中（config.rs 注释里的 "B3 audit"）→ **无用户侧 GeoIP、无审计表、无 SLO** |
| 4 | `read filter/src/parse.rs:1-30` | Filter-AST 白名单 7 字段（health/region/score/protocol/server/source_id/unlock），**无 service/业务维度** |
| 5 | `read apps/curse/src/handlers/subscription.rs` + `grep "ConnectInfo\|client\|ip\|geo\|region\|preset"` | `/sub` 不读客户端 IP；`filter_ast`/`preset` 被 `let _ =` 丢弃、preset 硬编码 `"common"`（M6 复证） |
| 6 | `read deploy/compose.yaml:20-64` | PROXY v1 头送 curse；`depends_on: [postgres]`（无健康检查条件）；miaospeed `:latest` 浮标复证 |
| 7 | `grep docs/configuration-and-api.md "审计\|SLA\|告警\|合规\|retention"` | 文档只有业务日志/历史保留键，**无管理操作审计、无 SLA/告警定义** |
| 8 | `edit 09-self-completed-gaps.md`（新增 §D.5）、`edit 00-index.md` | 写入 5 项现状表 + M18–M22 + 两道门禁口径 |

### 5 项逐条判定（均**未达完整档**，多数比"有限/部分"更弱）

| 能力 | 我们这版 | 关键证据 |
| --- | --- | --- |
| 按业务选择出口 | **未接线**（比"有限"更弱） | `subscription.rs:46-84` 丢弃 filter_ast/preset；无 per-service 维度（M11/M12）；上游 Go 的"按检测视角分发"也没接 |
| 按用户地理位置调度 | **未实现**（不是"非核心能力"） | PROXY 真实 IP 只用于登录限流分桶；`region` 是节点出口地区且 deep 被丢（M4）；零 GeoIP 代码 |
| SLA 保障 | 不完整且更薄 | 只有 `/health` + 最小 `/metrics`；compose 无 healthcheck/restart；无 SLO/告警/降级；B4 容器演练未做 |
| 故障自动恢复 | 部分（**仅应用层**） | Apalis at-least-once + `ON CONFLICT` 幂等 + `should_overwrite` ✅；编排层无 restart/断路器/supervision；M2 未接线 |
| 权限、审计、合规 | 权限 ✅ / 审计合规 ❌ | fail-closed 矩阵（P0-01）、限流（P0-02）、token 哈希、redaction、SSRF 已有；**无 audit 表**、`/logs/*` 只是业务日志、SBOM 未产出 |

### 口径

- 新增 **M18–M22**（按业务选出口 / 用户地理调度 / SLA / 编排层故障恢复 / 审计与合规），各带验收标准与依赖。
- **两道门禁**：可部署门禁 = M1–M5 + M15；生产级门禁 = M6–M14 + M18–M22。**全部要做**（用户裁定），
  区别只在顺序与验收时点，不是做不做。
- 本节同样**只做记录与排期，未改代码**。

---

## [UTC 2026-11-13T18:25:00Z] S60 · 最初计划逐节核查 → 并入缺口总账（M23–M27，全部要做）

**用户指令**：「检查一下最初的计划是不是都完成了」（对照 `S:\Downloads\paily-core-rust-rebuild-plan.md`）；
随后裁定「要」——把对照结果并入 §D 台账。

### 执行的每一步 / 每条只读命令

| # | 命令 / 动作 | 结果 |
| --- | --- | --- |
| 1 | `grep "^#{1,3} " 计划文档` | 提取 50 个标题定位章节（§14 在 533 行、§15 在 614 行） |
| 2 | `read 计划 533-712`（§14 分阶段验收 / §15 核心验收 17 条 / §16 安全 10 条 / §17 MVP 不做 / §18 冻结规格 10 份 / §19 技术基线 / §20 索引） | 取得全部验收条款原文 |
| 3 | `glob docs/*.md` | §18 十份规格文件**只有 `THIRD_PARTY.md` 存在**，其余 9 份缺 |
| 4 | `read migrations/0007_node_scores.up.sql` | `PK(node_id,checker_tag)` 仅当前行 → **P4「评分历史/快照」未做** |
| 5 | `grep score_history\|score_snapshot\|identity_material` | 评分历史零命中；指纹 golden 位于 domain/vproxy-adapter |
| 6 | `grep p2-03 "name\|测试"` | golden 全表：`name_change` 相同 / 各字段变化不同 / `field_order`、`null_default` 相同（14 passed） |
| 7 | 复用 S58/S59 证据（`check_job.rs`、`adapter.rs:112`、`subscription.rs`、`compose.yaml`、`Dockerfile.*`） | P3/P5/P7 的 ⚠️ 判定 |
| 8 | `edit 09-self-completed-gaps.md`（新增 §D.6 + M23–M27）、`edit 00-index.md`（总账 M1–M27 + 两道门禁） | 记录落地 |

### 判定（三层，已写入 §D.6）

1. **阶段实现层基本完成**：P0–P7 全有实现+测试+文档；⚠️ 四个：P3（M2/M3 桩）、P4（M5 未接线 + **M23 评分历史缺**）、
   P5（M6 丢档案 + M27）、P7（M14 CI + M25 SBOM + M26 演练）。
2. **计划验收条款未全达成**：§15 **15/17 有测试但零件级**（无一条在真实 MiaoSpeed + 真实订阅下端到端）；
   #15/#17 缺（M27）；§16 四条 ⚠️（M1/M25/M26）；**§18 冻结规格 9/10 缺（M24）**——含 p0-02 遗留未勾选项
   `MIAOSPEED_PROTOCOL.md` 与 crosscheck 引用悬空的 `LIFECYCLE.md`。
3. **系统目标未达**：即 D.2 的 M1–M5。

### 口径

- 新增 **M23–M27**（评分历史 / §18 规格九份 / SBOM+deny+ScanCode / 备份演练 / §15 两条验收），
  总账 **M1–M27 全部要做**（用户裁定）。
- 门禁：可部署 = M1–M5 + M15；生产级 = M6–M14 + M18–M27。
- 本节**只做记录与排期，未改代码**；VProxy「最小移植」与 sing-box 完整转换为已记录的决策分歧/计划外新增，不算未完成。

---

## [UTC 2026-11-18T10:45:00Z] S61 · 人工复核三项（权限隔离真 HTTP / 限流边界 / 错误冻结）

**用户指令**：只查三处——① 权限隔离必须**真 HTTP 集成验收**（不信 `auth_kind` 单测；曾出现无凭据
`GET /stats` 200 泄露统计）；② 登录限流的**部署边界**（反代后是否共用一个桶）写进部署文档；
③ 交叉审查文档有没有**错误冻结**（已完成项是否有源码/测试证据、Rejected 是否真被取代、P1 是否偷跑）。
并明确要求：不要让 Agent 自主扩大范围去实施 P1。

### 执行的每一步

| # | 动作 | 结果 |
| --- | --- | --- |
| 1 | `git status/fetch/rev-list` 同步体检 | 干净、与 origin 一致、HEAD `f9da308` |
| 2 | 读 `auth.rs` / `router.rs` / `rate_limit.rs` / `proxy_protocol.rs` / `main.rs` | 确认矩阵实现、nest 装配、桶键来源（`ConnectInfo`，PROXY 优先） |
| 3 | 校机体检 | Win11-HV1、cargo 1.99.1、PG 18.6 但**未运行** |
| 4 | 启动 PG | 首次被 ssh 会话回收；改为「一次 ssh 内启库→跑测→停库」 |
| 5 | 同步源码（先删远端 `apps/crates/migrations` 再 scp） | 形状正确，无嵌套 |
| 6 | `cargo build -p curse` | 0（28.7s） |
| 7 | `cargo build -p worker` | 首次缺 `gcc.exe`（`libsqlite3-sys`），补 `D:\yzd\MSYS2\mingw64\bin` 到 PATH 后 0 |
| 8 | 校机跑 `.tmp/remote_auth_matrix.py`（raw socket，32 用例） | **30 项符合预期，2 项是脚本预期写错**（见下） |
| 9 | PROXY 模式 4 用例 | 4/4 通过（无头被断 / 带头 200 / 10×200→429 / 换 IP 重新计数） |
| 10 | 改 `deploy/README.md` | 新增「限流与反向代理的边界（部署前必读）」 |
| 11 | 改 `10-review-crosscheck.md` | 修正 6 处错误冻结 + 新增 §10 人工复核记录 |

### ① 权限隔离：真实 HTTP 结果（校机真二进制 + 真 PG）

- 无凭据 `GET /api/v1/stats` → **401**（历史漏洞已修）；ServiceBearer/AdminJWT → 200，body 为真实统计。
- ServiceBearer 打管理写：`DELETE /sources/{id}` / `PATCH /sources/{id}` / `PUT /config` /
  `/subscriptions` / `/sponsors` → 全部 **403**（`service identity may not perform admin writes`）。
- ServiceBearer `POST /fetch/trigger` → **200** `{"enqueued":562}`；无凭据 → 401。
- AdminJWT `DELETE /sources/{id}` → **204**。
- 未匹配路由 → **404**（无凭据与带 service 相同）；`auth_kind` 的 fail-closed 只对「已注册但未列入矩阵」生效。
- 尾斜杠 `/stats/` → 404；`/api/v1/api/v1/stats`、`/../stats`、`/%2e%2e/stats` → 404。
- `POST /stats` 无凭据 → **401**（鉴权先于方法解析）；带 service → 403；`HEAD /stats` → 401/403。
- 两处「FAIL」是脚本预期错：`POST /stats` 原写 405、`DELETE` 原写 200——实际行为更严格/更符合 REST。

### ② 限流边界（写入 `deploy/README.md`）

代码事实：桶键 = `ConnectInfo<SocketAddr>`；PROXY 模式=头里的真实 IP，直连模式=TCP peer；**从不读 XFF**。
残余限制：代理不支持 PROXY v1 时所有客户端共用一个桶（10/min 全局）；`EXPECT_PROXY=1` 则直接拒绝。
文档给出三条不改代码的处置，并说明 PROXY 头只在「API 端口不可直连」时可信任。

### ③ 错误冻结：修正 6 处 + P1 越界 1 项

1. AC-2「caddy 反代下共用一个桶」→ 已被 S42 PROXY v1 取代；
2. 「缺 `(node_id, checked_at)` 索引」→ `0010` 已建；
3. 「测活历史只增不减」→ `check_run_history_days`(14) + `lifecycle/ops.rs` 清理；
4. A5「该键无消费者」→ 已有消费者；
5. B3「19/26 通过」→ 现 **25/26**（仅剩 `auth/refresh`）；
6. 「mihomo check ❌」→ 转换映射层已用真二进制校验（S54），完整模板仍缺（M27）；
7. 「19 路径」→ 实际 **26 条唯一路径 / 37 操作**（含 `README.md`、`configuration-and-api.md` 的「27 条路径」）；
8. 「SQLx 0001–0009」→ 现 **0001–0011**。

**P1 是否偷跑**：ErrorResponse/分页/ETag/请求 ID/filter 类型检查/评分重算/增量测活 **均未实施**（grep 零命中）；
但 **P1 #8 复合索引** 随 `0010` 做了、**无单独裁定** → 记为**范围外实施**（无害，但应先问）；
P1 #9 历史保留有 A5 裁定，属合规执行。

### 口径

- 本轮**未改产品代码**，只改了两份文档（部署限制 + 交叉审查更正）。
- 新增用例脚本在校机跑，产物 `auth_matrix_result.json` / `auth_matrix_run.log` 留在 `.tmp/`。

---

## [UTC 2026-11-18T15:20:00Z] S62 · P0 收尾：队列核查清理 + 最小权限收紧 + 可销毁库验收

**用户裁定**：① S61 真实 HTTP 验收的 7 项**通过**；② 宽匹配前缀**现在就收紧**（默认 AdminOnly、
ServiceBearer 仅完整 Method+路径白名单、只保 `POST /sources` + `POST /fetch/trigger`、完整段数匹配）；
③ **先核查 `enqueued:562` 的去向再启动 Worker**，不得清空整个队列；④ §10.3「6 处」改「9 处」；
⑤ 只做这三件，不开展其他 P1。

### A. 562 个任务的去向（核查前**未启动 Worker**）

在持久 ssh 会话里查 `phlogo_jobs`（PG 18.6，`apalis.jobs`）：

| 发现 | 数量 | run_at |
| --- | --- | --- |
| `FetchJob` Pending | **562** | 2026-11-18 18:41:43（= 我们 `POST /fetch/trigger` 的那一批） |
| `FetchJob` Done | **543** | 18:41:34 / 18:41:35 |
| `FetchJob` Running | **18** | 18:41:35 |
| `CheckJob` Done | 1 | 2026-11-11（**测试前就有**，保留） |

**关键发现（比 562 更值得记）**：S61 脚本里为建 apalis 表而起的 **8 秒 worker warm-up**，
其调度器 `tokio::time::interval` 的**首个 tick 立即触发**，对全部 active source 又入队了一批
（约 561 条），并在 8 秒内**实际执行了 543 条**。所以「562」只是明面上的那一批，
真实污染是 **1123 条队列记录 + 543 条抓取记录**。

主库影响核查：

- `source_fetch_runs`：18:41:35–18:41:42 新增 **543 条，全部 `failed`**（校机无出站）；
- `sources`：`updated_at >= 18:41` 的 **0 条**（失败不判死，符合设计）；
- `nodes`：`updated_at >= 18:41` 的 **0 条**，总数 473 未变（失败保留旧关联）。

**清理（先落盘快照，再按精确时间窗删，未清空整队列）**：

| 步骤 | 结果 |
| --- | --- |
| 快照 `jobs-s61-snapshot.csv` | 1123 行（id/job_type/status/run_at/attempts/lock_by） |
| 快照 `fetch-runs-s61-snapshot.csv` | 543 行 |
| `DELETE FROM apalis.jobs WHERE run_at >= '18:41:30+08' AND run_at < '18:45:00+08'` | **DELETE 1123** |
| `DELETE FROM source_fetch_runs WHERE started_at >= '18:41:00+08' AND started_at < '18:45:00+08'` | **DELETE 543** |
| 清理后 | `apalis.jobs` 仅剩 1 行（2026-11-11 CheckJob）；`source_fetch_runs` 435 行（历史） |

### B. 最小权限收紧（`apps/curse/src/auth.rs`）

矩阵由「前缀通配」改为**精确 Method + 完整 segment 向量**：

- ServiceBearer 白名单**恰好两条**：`POST sources`、`POST fetch/trigger`；
- 其余全部 `AdminOnly`（含所有 GET、`POST */search`、`POST /filter/preview`）；
- `POST /sources/{id}/anything` 等**不会继承** `POST /sources`；
- 删除 `require_admin_or_service`（整罩式弱检查，无调用者）与 `UuidLike`（旧形状分流）。
- 矩阵差异表与理由（读会暴露 `SourceView.content` / 整张 config KV / 运行历史）见
  `10-review-crosscheck.md` **§11**。

### C. 验收

| 项 | 结果 |
| --- | --- |
| `cargo test -p curse`（校机） | **33 passed**（含新 `auth_boundaries.rs` 5 项 + 重写 `auth_matrix.rs` 6 项） |
| `cargo test --workspace -- --test-threads=1` | **EXIT 0** 全绿 |
| `cargo clippy -p curse --all-targets -- -D warnings` | **0** |
| `rustfmt --edition 2021`（本轮 3 文件） | **0**；包内另 5 个文件为**既有**漂移，未顺手格式化 |
| 真实 HTTP + **可销毁 jobs 库**（14 项） | **14/14**：5 个敏感 GET 全 403；AdminJWT 3 个 GET 200；`POST /sources` 201；`POST /fetch/trigger` 200 且 **577 条全部进临时库**；**生产队列 1→1**；临时库 `dropdb` 后存在数 0 |

可销毁库做法：`createdb phlogo_jobs_auth_matrix_tmp` → `pg_dump --schema-only --schema=apalis phlogo_jobs`
导入 → curse 的 `PHLOGO_DATABASE_JOBS_DSN` 指向它 → 跑完 `dropdb --force`。
`phlogo` 角色无 CREATEDB，临时库的建/删用集群超管 `postgres`（pg_hba 对 loopback 是 `trust`），
**应用仍以 `phlogo` 运行**。

### 口径与遗留

- 本轮只改：`apps/curse/src/auth.rs`、`apps/curse/tests/{auth_matrix,auth_boundaries}.rs`、
  `apps/curse/Cargo.toml`（dev-deps）、4 处文档；**未动任何 P1 项**。
- `cargo test --workspace` 会在共享 dev 库留测试夹具（sources 561→576、dead 72→74），是**既有行为**；
  本轮验收脚本自己创建的 `auth-matrix-*` 源已用 AdminJWT 删除（残留 0）。
- 仍遗留：通用 `/api/v1/*` 限流只挂了 login；M14 CI 未写。

---

## [UTC 2026-11-18T15:55:00Z] S63 · P1 第一片：M1a 节点载荷加密落库

**用户指令**：「继续」——按已定顺序进入 P1，先做 M1（真实载荷 + 凭据加密，堵 §16.8 明文凭据）。
本轮只做**存储侧**（M1a），不动 `/sub` 渲染（M1b）与 base64 URI（M1c）。

### 执行的每一步

| # | 动作 | 结果 |
| --- | --- | --- |
| 1 | 查 `Cargo.lock` 与校机 registry 缓存 | `chacha20poly1305` 不在缓存；`rsproxy.cn` / `index.crates.io` 均 **200** 可达 |
| 2 | 新建 `crates/payload-crypto`（XChaCha20-Poly1305；AAD=节点 id；hex/base64 密钥；`key_version`；手写 `Debug` 脱敏） | 6 单测 |
| 3 | 迁移 `0012_node_payloads` | 建表 + **清空 `nodes.raw_config_reference` 明文**（不可逆，页内说明） |
| 4 | `database::payload_store`（upsert/load） | 新增模块 |
| 5 | `upsert_node_by_hash`：节点行只写 `''`，载荷 seal 后写 `node_payloads` | 同一事务连接内 |
| 6 | worker 启动 `payload_crypto::init_from_env()` | 无键→临时键（warn）；**键非法→启动失败** |
| 7 | registry 集成测试新增 `payload_is_sealed_and_bound_to_its_node` | 见下 |
| 8 | 校机跑 gates（crypto 测试 / reconcile 测试 / rustfmt / workspace clippy） | 全绿 |
| 9 | 校机实跑迁移并核对 | 版本 12 success=t；明文节点 **0**；`node_payloads` 11 行 |

### 验收数字

- `cargo test -p payload-crypto`：**6 passed**
- `cargo test -p registry --test reconcile`：**7 passed**（新增项断言：`nodes` 侧为空、密文不含
  `TOP-SECRET-PW`、AAD 换节点解密失败、可回读）
- `rustfmt --check`（5 个文件）：clean；`cargo clippy --workspace --all-targets -- -D warnings`：**0**
- 依赖：`chacha20poly1305 0.10.1` + 7 个传递 crate（rsproxy 拉取），`Cargo.lock` 已更新

### 途中修正（记下来，免得重犯）

1. ssh 多行命令**被前导空行吞掉**（脚本 `exit 0` 但什么都没跑）→ 改成**单行 `&&` 链**。
2. `cargo test` 报 `PayloadKey` 无 `Debug`（`unwrap_err` 需要）→ **手写 `Debug` 输出 `<redacted>`**，不用 derive。
3. `fetch_one(conn)` 把 `&mut PgConnection` move 掉 → 改 `fetch_one(&mut *conn)`。

### 遗留

- **M1b**：`/sub` 仍发占位内容（yaml `placeholder.invalid` / json `direct` / base64 `xxx://node/{id}`）。
- **M1c**：`render_subscription` 的 base64 分支是假链接，需先补「Clash → 真实 URI」构造器并用
  vproxy-adapter 反解析做往返校验。
- 密钥轮换工具（`key_version` 已有，重封流程未做）；`sources.content` 里可能含凭据的订阅 URL 未评估。

---

## [UTC 2026-11-18T16:58:00Z] S64 · 部署文档补密钥 + M1b：`/sub` 真实渲染

**用户指令**：「部署文档更新，接着做 m1b」。

### A. 部署文档（`PHLOGO_PAYLOAD_KEY` 必填）

| 文件 | 改动 |
| --- | --- |
| `deploy/.env.example` | 新增 `PAYLOAD_KEY=`，附生成命令（`openssl rand -hex 32`）与"换钥/丢钥会让旧载荷不可读"的警告 |
| `deploy/compose.yaml` | `curse` 与 `worker` 都注入 `PHLOGO_PAYLOAD_KEY: ${PAYLOAD_KEY:?set PAYLOAD_KEY in .env}`（缺省即 compose 报错，不静默） |
| `deploy/README.md` | 新增「载荷加密（M1）：`PAYLOAD_KEY` 必填」：四种情形（两端一致 / 任一端缺失 / 非法 / 不一致）、症状（`/sub` 503 `no usable node payloads`）、轮换临时办法 |
| `docs/configuration-and-api.md` | 启动配置表新增 `PHLOGO_PAYLOAD_KEY` 行，指向部署文档 |
| `README.md` | quickstart 的 `.env` 填写清单加上 `PAYLOAD_KEY` |

编辑途中一次误删（`worker` 的 `depends_on` 与 `miaospeed:` 服务头）**当场发现并修回**，随后逐行核对 compose 结构。

### B. M1b 实现

`apps/curse/src/handlers/subscription.rs` 整体重写：

1. token → hash（`PHLOGO_TOKEN_PEPPER`）→ profile（active + enabled）；
2. 单查询取全部 `Linked` 节点 + 分数（每节点最大）+ 地区（最新 check 的 `metrics->>'region'`）
   + 健康（`node_status` 跨 tag alive>degraded>dead）+ 密封载荷；
3. 逐节点解密（AAD=节点 id），失败**跳过并计数**；全不可用 → 503 并提示检查密钥；
4. `PipelineNode` → `render_subscription_with_sponsor`：**档案筛选真正生效**（M6 一并落地）；
5. sponsor 改为一次查询（新增 `registry::sponsor::match_texts`，不再每节点重读 sponsors 表）；
6. `yaml`/`json` 返回真实内容；`base64` → **501**（M1c 之前不发假链接）。

`apps/curse/src/main.rs` 增加 `payload_crypto::init_from_env()`（非法键→启动失败）。

### C. 验收（校机真 PG）

| 项 | 结果 |
| --- | --- |
| `cargo test -p curse --test subscription_render` | **1 passed**（真实 TCP） |
| 该用例断言 | `.yaml` 200 且含真实 `jp.example` + 密码；`.json` 200 且 outbound 含 `jp.example`；档案 AST 把 `us.example` 排除；`.base64` **501**；吊销 token **401**；结束清理夹具 |
| `cargo test -p curse` | **34 passed**（10+5+6+4+5+3+1） |
| `rustfmt --check`（3 文件） | clean |
| `cargo clippy -p curse --all-targets -- -D warnings` | **0** |

### 途中修正

1. `identity_hash` 是 `varchar(64)`，`format!("{:x}", Uuid)` 实际按 Display 输出 36 字符 ×2 = 72 → 改
   `Uuid::simple()`（32 字符）拼接。
2. clippy `type_complexity`：profile 的 7 元组抽成 `type ProfileTuple`。

### 遗留

- **M1c**：base64 真实 URI（构造器 + vproxy-adapter 往返校验）。
- `crates/registry/src/sponsor.rs` 有**既有** fmt 漂移（本轮只新增 `match_texts`，未顺手格式化整文件）。
- `/sub` 响应仍无 Cache-Control（P1 项，凭据在响应体里，长期应禁止缓存）。

---

## [UTC 2026-11-18T17:10:00Z] S65 · M1c：base64 真实 URI + 真 mihomo 验证

**用户指令**：「继续，然后没问题先推送再 m2」。

### A. 实现

- 新增 `crates/distribution/src/uri.rs::uri_from_clash`：把 Clash 载荷转真实分享链接
  （ss SIP002 / vmess base64 JSON / vless 含 reality 与 ws / trojan / hysteria2 / tuic / socks5 / http）。
- 纯函数 `subscription.rs` 的 base64 分支改用真实 URI；**不支持的协议省略**（不造假链接），
  全不可转 → 422。
- `apps/curse` 的 base64 分支从 **501** 改为正常返回 `text/plain`。
- 新增 `crates/distribution/examples/uri_export.rs`：把**真实订阅文件**走同一条链路导出 base64，
  供外部二进制验证。

### B. 验证策略的修正（重要）

原计划「用我们自己的 vproxy-adapter 反解析做往返校验」**行不通**：grep 证实 `vproxy-adapter`
**没有 URI 解析**（只支持 Clash YAML / sing-box JSON 及其 base64）。这同时暴露一个独立采集缺口：
以 base64 URI 列表分发的源目前会 `UnrecognizedFormat`（已登记，不在 M1 范围）。

改为**真 mihomo 验证**：

1. `cargo run -p distribution --example uri_export -- run2-sub-new.yaml` → `nodes=52 links=52 skipped=0`，17,924 B；
2. 本地 `python -m http.server 18080` 提供为订阅 URL；
3. mihomo（`D:\yzd\proxy\run2\mihomo-windows-amd64-v3.exe`）配 `proxy-provider type: http` 指向它；
4. 首跑 **解析 0**——日志显示 `[TCP] mihomo --> 127.0.0.1:18080 match Match using G[COMPATIBLE]`：
   连 provider 抓取也被规则 `MATCH,G` 送进了**空**代理组。加 `IP-CIDR,127.0.0.1/32,DIRECT,no-resolve` 后复跑；
5. 结果：**mihomo 解析出 52 个代理，`{Vless: 36, Hysteria2: 16}`，与源完全一致**，`providers.phlogo` 无错误。

### C. 门禁

| 项 | 结果 |
| --- | --- |
| `cargo test -p distribution` | **42 passed**（含 6 个 URI 用例） |
| `cargo test -p curse --test subscription_render` | **1 passed**（base64 断言改为：200 + 解码后含真实 `trojan://…jp.example:443` + 不含 `us.example`） |
| `rustfmt --check`（3 文件） | clean（`distribution/src/subscription.rs` 测试模块的**既有**漂移未顺手格式化） |
| `cargo clippy -p distribution -p curse --all-targets -- -D warnings` | **0** |

### 遗留

- URI 只覆盖 8 协议；mieru/sudoku/ssr/shadowtls/wireguard 与带 plugin 的 ss 在 base64 里被省略
  （yaml/json 仍包含）。
- `vproxy-adapter` 缺 URI 解析（采集缺口，登记）。
- 下一步：**M2**（CheckJob 接 MiaoSpeed 真测 + 结果落库）。

---

## [UTC 2026-11-18T18:08:00Z] S66 · M2：CheckJob 接 MiaoSpeed 真测（mock + 真机）

**用户裁定**：配置键用 `PHLOGO_CHECKER_WS_URL / PHLOGO_CHECKER_SECRET`；范围含**真 MiaoSpeed 跑通**。

### 实现

- `MiaoSpeedAdapter::check()`：一次 roundtrip 到终态并映射（`submit`+`collect` 在真机上不成立）。
- `run_check_once()`（worker）：记录 run → 端点配置 → 节点 + **解密载荷** → `check()` →
  `persist_check_run` → `upsert_node_status` → 终态；失败路径统一 `failed`。
- `check_runs` 词表统一为 **`success`**（原 `finish_check_run` 写的 `finished` 与 p3-04 冲突）。

### 真机暴露的三条事实（mock 测不出，全部已修）

1. `Payload` 必须是**字符串**：传 JSON 对象时服务端回显 `"Payload":""`。
2. 结果按 **payload 的 `name`** 映射，`Nodes[].Name` 被忽略；缺 `name` → `ProxyInfo` 全空。
3. `InvokeDuration = 0` = 连接没建立 → 统一记 `-1`。

### 验收

| 项 | 结果 |
| --- | --- |
| `cargo test -p worker --test check_wiring` | **2 passed**（落库 + unknown 节点 + 不存在的 id 过滤 + 重复投递幂等；无端点 → failed） |
| `cargo test -p worker` | **4 passed** |
| 真 MiaoSpeed 4.7.9（`C:\Users\YiZhidi\p0-02\miaospeed.exe`） | `live_check` 退出 **0**：`ProxyInfo.Name=phlogo-live-0 / Address=127.0.0.1:1 / Type=Trojan / InvokeDuration=1` **映射回内部节点 id** |
| fmt / clippy | clean / **0** |

### 途中修正

- `node_check_results.node_id` 外键 → unreported 先与 `nodes` 取交集。
- `identity_hash` varchar(64) 与 `"{:x}"` 的 Display 坑（同 S64）。
- 两个测试共用进程环境变量 → `tokio::sync::Mutex`（`std::Mutex` 会被 clippy 判 `await_holding_lock`）。
- `rustfmt --check apps/worker/src/lib.rs` 会**递归检查整个 crate**，牵出 `fetch_job.rs` 的既有漂移 → 只对具体文件跑。

### 遗留

- `InvokeDuration` 单位未归一化、deep 矩阵未映射（M4 接 p3-03 `normalize`）。
- vmess 载荷被 mihomo 拒（缺 `alterId` 等）→ 载荷保真度问题，随 M4。
- 谁生成 CheckJob（定时/手动/阈值）= **M3**。
- `PHLOGO_DEBUG_FRAMES` 是调试开关，production 不要开。

---

## [UTC 2026-11-18T21:30:00Z] S67 · 复审裁定落地：Cache-Control + health 语义修正 + URI 解析闭环

**用户裁定**（对 `b2bd9fd`/`1063986` 源码复审后）：M1b 需收尾（问题 1、2 属实），M1c 要求建立
URI 转换 + 反解析闭环，不许只数节点数验收。**时间线说明**：裁定到达时 M1c（`5c54424`）与
M2（`fdd0f67`）已按前一条「继续，然后没问题先推送再 m2」完成推送；本条落实裁定中仍有效的部分，
M2 不回滚。

### 1. 问题 1 属实 → 修：`Cache-Control: no-store`

`/sub` 所有响应（200 yaml/json/base64、401、403、404、422、503）统一经 `no_store()` 打头。
理由（裁定原文照录）：订阅 URL 本身携带访问凭据，响应体包含节点密码，不能依赖浏览器、代理或
CDN 的默认缓存行为。

### 2. 问题 2 属实 → 修：health 过滤语义

复审确认 `to_node_view(n, &profile.health)` 把**档案期望值**当节点实际状态喂给 AST。
修正：

- `PipelineNode` 新增 `health` 字段（`node_status` 跨 tag 合并的真实值），AST 条件
  `{"field":"health",...}` 现在看**实际状态**；
- `ProfileSpec.health` 明确为**硬条件**（与 region/min_score 同级）：alive(3) > degraded(2) >
  dead(1) > unknown(0)，节点 rank ≥ 门槛才通过；`""`/`"any"` 关闭；
- 组合测试：distribution 用 alive/degraded/dead/unknown × 门槛 alive/degraded/关闭 +
  **AST 死节点用例**（证明 AST 看真实状态）；curse 端到端新增
  `profile_health_bar_excludes_unhealthy_nodes`（dead 节点被 bar 排除，healthy 留下）。

### 3. URI 解析闭环（裁定第三阶段要求）

`vproxy-adapter` 新增 `src/uri.rs`：

- `parse_uri_line`：ss(SIP002) / vmess(base64 JSON) / vless / trojan / hysteria2 / tuic /
  socks5 / http 逐字段解析（TLS/SNI/Reality pbk/flow/ws path+host/obfs/congestion 等）；
- `looks_like_uri_list` + `parse_subscription` 接入：URI 列表在 YAML/JSON 之前尝试，
  base64 包装解码后再试一次——**我们自己发出的订阅现在能被自家采集器吃回去**；
- **凭据缺失的行直接跳过**（http 无 userinfo → None），不造 ghost 节点；
- 解析产物重建完整 Clash JSON 载荷（transport/tls/credential 全部落回 `raw_config_reference`），
  身份哈希与再渲染路径不受影响；
- **逐字段往返测试**（裁定明确不许只数数量）：
  `builder_output_roundtrips_through_the_parser` 比对 protocol/server/port/**credential**；
  trojan 单测再比对 TLS/SNI/ws；vless 比对 reality pbk + flow；vmess 比对 alterId/cipher/ws-opts；
  hysteria2 比对 obfs/obfs-password/skip-cert-verify；tuic 比对 congestion/udp-relay-mode。

### 4. 验收

| 项 | 结果 |
| --- | --- |
| `cargo test -p distribution` | **43 passed**（+1 health 组合测试） |
| `cargo test -p vproxy-adapter` | **16 passed**（+10 URI 解析用例） |
| `cargo test -p curse` | **35 passed**（subscription_render 2：真实渲染 + 健康门端到端） |
| `rustfmt --check`（本轮 5 文件） | clean |
| `cargo clippy -p vproxy-adapter -p distribution -p curse --all-targets -- -D warnings` | **0** |

### 途中修正

- 测试种子节点没有 `node_status` 行 → 实际健康 `unknown`，rank 0 < alive，被新门拦掉（**这正是
  修正后的正确行为**）→ 种子补 status 行；另加 dead 节点端到端用例证明门生效。
- profile `filter_ast` NOT NULL 为 jsonb：空串不是合法 JSON → 用 `{"operator":"and","conditions":[]}`。
- clippy `collapsible_match`（first_scheme_default 占位 match）→ 删占位，直接 `443` 默认。
- `Credential` 不是 Option（`NormalizedNode.credentials: Credential`）→ 解析器以「跳过该行」
  表达无凭据，而不是 None 凭据。

### 遗留（裁定中确认的后续事项，均维持登记）

- **`key_version` 未按版本选钥**：轮换 = 多版本密钥 + 逐行重封，独立任务。
- **`sources.content` 可能含凭据**（订阅原文/URL）：M1a 只解决节点载荷，未解决此暴露面。
- M2 已完成（`fdd0f67`）；下一步按序 M3（入队/调度）。

---

## [UTC 2026-11-19T17:45:00Z] S68 · M1c 字段保真补齐 + M2.1 加固 + M3 调度完整交付

**用户裁定**（2026-11-19 复审 `9767908`/`5c54424`/`fdd0f67`）：连续执行 M1c 补齐 → M2.1 加固 →
M3 完整交付；完成后停止，不提前进入 M4。

### A. 阶段一：M1c 字段保真（裁定：闭环要完整，不是核心字段闭环）

| 缺口 | 修复 |
| --- | --- |
| VLESS `reality-opts.short-id` 丢失（rebuild 固定 `""`） | `build_node` 先把 URI query 的 `sid` 种进 payload 骨架；`rebuild_raw` 改为**合并**进已存在的 `reality-opts`，只覆写 `public-key` |
| VLESS `client-fingerprint`（`fp=`）丢失 | 同上：query `fp` → payload `client-fingerprint` |
| VMess `fp`/ALPN 不恢复 | `parse_vmess` 恢复 `alpn`（逗号拆分）入 `TlsSettings`，并把 `alpn`/`client-fingerprint` 写进重建 payload |

映射表已写进测试注释（Clash field → URI part → parsed slot → rebuilt payload）；
`sid`/`fp` 标注为 parser-only home（`NormalizedNode` 无对应槽位）。

**身份影响核查（裁定第 4 点）**：`vless_reality_full_fidelity_roundtrip` 断言
`compute_identity(from_clash(原始payload)) == compute_identity(from_clash(往返重建payload))`
—— 字段保真修复**不改变**现有节点身份；14 个 golden 指纹测试全部通过（未触碰 `fingerprint.rs`）。

**`/sub` 头部断言（裁定）**：新增 `every_sub_response_is_no_store`（真实 TCP）——
yaml/json/base64/未知格式/未知 token 五种响应全部断言 `Cache-Control: no-store`。

### B. 阶段二：M2.1 加固（裁定 A/B/C）

- **A 终态帧**：`roundtrip()` 增加 `saw_terminal` 标志——连接在终态 `Result` 前关闭 →
  `CheckerError::Protocol("connection closed before the terminal Result frame")`。
  新测试 `early_close_fails_the_run`：无终态帧 → run `failed` 且 **0 行结果**。
- **B 调试脱敏**：`PHLOGO_DEBUG_FRAMES` 改为结构化输出 `debug_frame()`：frame id/terminal 标志/
  error 文本/queuing/index/记录名与延迟/结果数——**永不输出 Payload（含节点凭据）与 token**，
  上限 40 帧。部署文档同步（`docs/configuration-and-api.md`）。
- **C 持久化一致性**：新 `persist_all()` 把 `persist_check_run_in` + `upsert_node_status_in` +
  `finish_check_run_in` 放进**一个事务**（`database::check.rs` 新增三个 `*_in` 变体接受
  `&mut PgConnection`）；`existing_ids()` 的 DB 错误不再 `unwrap_or_default()` 而是让 run 失败。
  迟到结果保护（late-result guard）语义未动，schema 未动。

### C. 阶段三+四：M3 调度 + 手动触发

- **周期调度**（`apps/worker/src/main.rs` + `check_job.rs::enqueue_scheduled`）：
  - 配置键 `check_schedule_interval_minutes`（默认 30）与 `check_batch_size`（默认 50），
    迁移 `0013_check_schedule`；**明确不使用** `node_alive_check_window_minutes` 作间隔（裁定）。
  - 只调度 **Linked 且有可读密封载荷** 的节点（`schedulable_node_ids`）；首批只用 `TEST_PING_RTT`。
  - **确定性去重**：`schedule_id = v5(NS, period_label)`，`run_id = v5(schedule_id, batch)`；
    先 `record_check_run`（幂等 INSERT），**赢的实例才 push job** —— 多 worker/重启不重复。
  - **首次 tick**：`interval_at(now + interval, ...)` 显式推迟一个整周期（裁定：不依赖 interval
    隐含的立即首 tick）。
- **手动触发** `POST /api/v1/check/trigger`（`handlers/check_trigger.rs`）：
  AdminJWT only（**不扩 ServiceBearer 白名单**，fail-closed 403）；API 只入队，绝不调 MiaoSpeed；
  显式节点列表先过滤为可调度节点（无 ghost 调度），上限 500 节点、batch clamp [1,100]；
  返回 **202** + `{enqueued_batches, duplicate_batches, nodes, batch_size}`；全不可调度 → 422；
  label 带 uuid4 后缀 → 同秒两次触发也是两个独立逻辑运行。
- OpenAPI 增加 `CheckTrigger` 操作（401/403/202/422/503）。

### D. 验收（一次性 `phlogo_jobs_test_m3`，创建→测试→销毁；不触碰生产队列）

| 套件 | 结果 |
| --- | --- |
| worker（含 M3 调度/去重/手动触发/早断连 5 用例） | **7 passed** |
| curse（含权限矩阵真 TCP 401/403/202/422） | **37 passed** |
| database（含事务变体回归） | **4 passed** |
| distribution | **43 passed** |
| vproxy-adapter（含 sid/fp/alpn 保真 + 身份哈希往返） | **18 passed** |
| checker-miaospeed | **11 passed** |
| workspace `rustfmt --check`（含此前两处既有漂移文件，本轮顺手修正） | **clean** |
| workspace `cargo clippy --all-targets -- -D warnings` | **0** |

### E. 途中修正（记要点）

1. 测试假设 dev 库只有自己的夹具 → 调度测试改**相对断言**（同周期 push==n、重跑全 skip）。
2. manual 触发的 label 秒级时间戳会撞车 → 服务端 label 加 uuid4 后缀（同秒两次也独立）。
3. 显式节点列表里的未知 id 会产生 ghost 调度 → 服务端先过滤为可调度节点（`n3` 从 1 改 0）。
4. `uuid` crate 缺 `v5` feature → workspace 开启。
5. **校机 C 盘满**（0 GB）→ `cargo clean` 释放 21.9 GB 后重跑。
6. clippy `explicit_auto_deref`（`&mut *tx` → `tx.as_mut()`）。
7. `distribution`/`adapter` 合跑一次出现 mingw linker 偶发失败与磁盘错误 → 分包重跑通过。

### 遗留（裁定明确不混入本轮）

- 增量测活、连续失败降级、评分接入（M5）；deep 矩阵与 `InvokeDuration` 归一化（M4）。
- `enqueue_for_active_sources`（fetch）仍为周期全量入队，无 period 去重（另行裁定是否复用同策略）。

---

## [UTC 2026-11-19T20:40:00Z] S70 · M3.1 调度可靠性收尾（复审裁定五点全落地）

**用户裁定**（复审 `30ea99c`）：M1c/M2.1 通过；M3 主体落地但多实例去重与任务恢复有实际缺口，
**暂不完全验收**；只做小范围 M3.1，不进 M4（冻结）。五点全部执行：

### 1. 多实例周期身份（裁定问题 1）

period 标签原取 `Utc::now()` 分钟格式，与各 worker 自己的 `interval_at` 起点无绑定 → 不同启动
时刻的 worker 对同一逻辑周期派生不同 run_id，唯一约束防不了重复调度。修复：
`aligned_period_label(now, interval)` 把时间**对齐到周期边界**（epoch 分钟 div_euclid），
标签形如 `period-20261120T1200+30m` —— 同槽任何启动时刻同标签。测试
`aligned_period_label_is_stable_across_start_times`。

### 2. 筛选改真实可解密（裁定问题 2）

`schedulable_node_ids` 原只判"有载荷行"；现在逐节点 `open()` + JSON 解析，不可解密排除并计数
（只报数量）。测试 `undecryptable_payloads_are_excluded_from_scheduling`：另一把密钥封印的
节点 → 调度后 jobs 表 grep 该节点为 **0**。

### 3. handler 失败重试（裁定问题 3）

`handle_check` 改返回 `Result<(), anyhow::Error>`；check worker 加
`.retry(apalis::layers::retry::RetryPolicy::retries(3))`（workspace apalis feature +`retry`）。
测试 `handler_returns_err_on_failure`：handler 返回 Err 且 run 行 `failed`。

### 4. 崩溃窗口恢复（裁定问题 4）

`check_runs` 新增 `pushed boolean`（迁移 0013 扩充——**事后学到的纪律：迁移一旦应用不可再改，
本次 dev 库手动回滚 0013 重放**）；`claim_for_push()` 三态（New / LostBeforePush /
AlreadyClaimed），push 成功 `mark_pushed`、失败 `mark_unpushed` 下轮重试。手动触发走同一协议。
测试 `lost_push_is_recovered_by_the_next_pass`：全 push → 重置 batch 0 为 unpushed → 恰好
`re_pushed==1、pushed==0、skipped==n1-1`。

### 5. 测试覆盖与基建（裁定表格五项）

- 四类故障注入/并发测试全部补齐（上 1–4 + 501 id）。
- API 501 个 id 由静默截断改 **422 明确拒绝**（新增断言）。
- jobs 测试库**强制** `PHLOGO_TEST_JOBS_DSN`，缺省跳过（worker + curse），不再回退 DATABASE_URL。

### 门禁

worker **11** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11**
全绿；workspace `rustfmt --check`（含递归）+ `clippy --all-targets -- -D warnings` **0**。
一次性 `phlogo_jobs_test_m3` 创建→测试→销毁，生产队列零接触。

### 途中修正（记要点）

1. 0013 迁移事后修改 → sqlx 拒绝迁移 → dev 库手动回滚重放（见 17 页 §8.6 纪律条目）。
2. push 成功未置位 `pushed` → 第二轮误判"丢失"重推 → 补 `mark_pushed`（测试当场抓住）。
3. 恢复测试断言 `s>=1` 写错（场景里没有 already-pushed 批次）→ 重写为两轮 push/重置/恢复。
4. apalis `retry` 是独立 feature（workspace 只开了 tracing）→ 加 feature 后
   `apalis::layers::retry::RetryPolicy` 才可见。
5. fmt 清单误包含 `.sql` 文件 → rustfmt 报 token 错 → 移除。

### 遗留

M4 冻结（deep 矩阵、InvokeDuration 归一化）；评分（M5）未动；push 失败秒级恢复扫描器未做
（按裁定保持最小范围）。

## [UTC 2026-11-22T02:55:00Z] S71 · M3.1-2 再收紧：投递状态机重写（二轮复审裁定六点）

**用户裁定**（复审 `4abd0fd`）：五项里 2/3/5 部分接受；**`claim_for_push()` 否**——`pushed` 布尔把
「正在 push」与「已 push 成功」压成一个状态位，同时存在并发双推（New vs LostBeforePush）、认领后崩溃
永久漏推、push 成功后崩溃重推三个窗口；另外 timer 相位仍挂在各进程启动时刻、手动触发换 uuid4 无法
恢复、0013 迁移被事后修改。**M4 继续冻结**。本条是六点全落地。

### 1. timer 锚到全局 UTC 边界（裁定 1）

`next_boundary_after(not_before, interval)` 返回「首个不早于 not_before 的边界」+ 该槽标签；worker 以
`startup + interval` 作 `not_before`，`interval_at(首边界, check_secs)`，之后每 tick 标签按**算术推进**
（`planned_at += interval`），**不再**用 `Utc::now()` 重算——延迟 tick 必须用原计划标签，补齐的连续 tick
也要逐个走过各自槽位，否则同一逻辑周期会被重复执行。
测试 `boundary_timer_shares_identity_across_start_times`。

### 2. 投递状态机重写（裁定 2，主项）

迁移 `0014_check_push_state`：`push_state`（pending/dispatching/enqueued）+ `lease_until` + `lease_owner`
+ 恢复索引。`claim_for_push` 收敛成**单条 UPSERT**
（`INSERT ... ON CONFLICT DO UPDATE ... WHERE pending OR 过期 dispatching AND status='started' RETURNING xmax=0`），
新建与恢复同一条竞争路径 → 缺陷 A 结构性消失；认领只写租约，崩在 push 前 → 租约过期可重认领 →
缺陷 B 消失；push 成功后 `mark_enqueued(owner 守卫)`，若崩在标记前则过期后**再推一次**，即
**at-least-once**，重复执行由执行侧闸门挡住：`run_check_once` 见 `status='success'` 直接返回，不再跑
真实 MiaoSpeed。`let _ = mark_*` 全部删除，DB 错误一律上抛；owner 守卫导致 0 行时明确 warn。
查证 apalis 0.7.4 **没有**确定性 job id（`jobs.id` 由存储生成），故不依赖它。
对外语义写 **at-least-once**，跨主库/jobs 库不声称 exactly-once。

### 3. 手动触发可恢复（裁定 3）

请求体加 `idempotency_key`；label = `manual-<key>`，verbatim 派生 schedule_id。同键重发 → 已 enqueued
则跳过、崩溃未推则按过期租约恢复；无键仍是随机 label（一次性语义不变）；响应回显 `period`。
权限面不变（AdminJWT-only / ServiceBearer 403 / 匿名 401 / >500 id 422）。

### 4. 可解密筛选测试：假通过 → 真证据（裁定 4）

`pushed_jobs()` 反序列化 `apalis.jobs.payload` 取 `node_ids`，断言 healthy UUID 在、wrong-key UUID 不在
（原 `payload LIKE` 恒为 0，无效）。**本轮真正的卡点是夹具污染**：固定 server 名 + `SELECT id WHERE
server=...` 选中历史 panic 留下的行，那些行由上个进程的临时密钥封印（`PHLOGO_PAYLOAD_KEY` 缺省即
ephemeral）→ 本进程解不开 → 触发返回 `(0,0,0)`。修法：server 每次唯一 + 直接用 `seed_node` 返回的 id。
另修 `total >= 2` 的口径错误（`total` 是筛选后计数）。测试不再手抄 v5 recipe：`schedule_id_for_label` /
`scheduled_run_id` 从 worker 导出后直接调用（上一轮手写版本多包命名空间 + 多 `manual-` 前缀，把恢复用的
UPDATE 打到不存在的行）。

### 5. 故障注入测试（裁定 5，九项）

一次性 `PHLOGO_TEST_JOBS_DSN`（缺省跳过，不回退 DATABASE_URL）：`concurrent_claims_yield_exactly_one_winner`、
`concurrent_recovery_of_expired_lease_has_one_winner`、`succeeded_run_is_not_reexecuted_on_redelivery`、
`state_transition_errors_propagate`、`manual_trigger_idempotency_key_recovers`、
`undecryptable_payloads_are_excluded_from_scheduling`、`handler_returns_err_on_failure`、以及新增
**`retry_layer_redelivers_until_exhausted`**（裁定 5.6 的真实重投集成测试：真 `Monitor` + 真队列，
handler 执行 **4** 次 = 1+3 retries、队列 `attempts=4`、终态 **`Killed`**（apalis 重试耗尽走 kill，
不是 Failed，也不再被 fetch）。用测试自有 job 类型 `RetryProbe` 靠 `type_name::<T>()` 划 namespace，
不干扰别的测试的 CheckJob 行）。

### 6. 迁移纪律（裁定 6）

`0013_check_schedule.{up,down}.sql` **逐字节**恢复 `30ea99c`（连注释也不加：sqlx 校验和是整文件哈希）。
新增 0014：up 建状态列 + 索引并 `DROP COLUMN IF EXISTS pushed`（清被否决的列，全新库 no-op）；down 只
回滚自己加的。一次性库 `phlogo_mig_scratch` 实测 0001…0014 → (3 列, pushed 0, 索引 1, configs 2) →
`0014.down` → (0, 0, 0, configs 仍 2) → `0013.down` → configs 0 → 删库。dev 库历史修复（仅 dev）：删
13/14 两行 + 清 0013 的 configs 行，让 sqlx 按序重放（14 的 up 全 `IF NOT EXISTS`）。

### 门禁

worker **16** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11** 全绿；
workspace `rustfmt --check` + `clippy --all-targets -- -D warnings` **0**。一次性 `phlogo_jobs_test_m3`
建库→测试→销毁，`apalis.jobs` **12** 条（非空跑）；生产队列零接触。

### 途中修正（记要点）

1. 夹具复用固定 server 名 → 历史行被选中且载荷不可解密 → 触发 `(0,0,0)`；改唯一 server + 用返回 id。
2. 交接时把「测试派生 run_id」的诊断做反了（去掉命名空间又保留 `manual-` 前缀）→ 改为直接调用生产函数。
3. gates 脚本里 `SELECT count(*) FROM jobs` 漏 schema：apalis 用 **`apalis` schema**；且该诊断行用 `&&`
   串在 DROP DATABASE 之前，一失败就跳过销毁，把测试库留在机器上 → 改 `apalis.jobs` 并加 `|| echo`。
4. 屏幕上 `manual_trigger_idempotency_key_recovers` 先前「通过」其实是陈旧二进制；每次同步后必须重编。

### 遗留

M4 冻结（deep 矩阵、InvokeDuration 归一化）；M5 未动。投递语义 at-least-once，崩溃窗口内可能重复
push（执行侧收敛）；手动触发恢复依赖客户端重发同键，统一 outbox 扫描器未做；租约 60s 可按部署调整。

## [UTC 2026-11-22T10:45:00Z] S72 · M3.1-3 执行侧原子认领（三轮复审裁定）

**用户裁定**（复审 `e1d5105`）：六项里 timer / 手动幂等 / 错钥测试 / 迁移纪律 **通过**，投递状态机、
故障注入与 RetryLayer **基本通过**；但发现**执行侧没有互斥**，因此 M3.1-2 **暂缓最终验收**。原文要点：
`run_check_once` 里 `SELECT status` 是**读**，不是认领——push 成功后崩溃、租约过期重推，队列里同一
`run_id` 会有两条 job，两个 worker 都读到 `started` 就都调 MiaoSpeed。**M4/M5 继续冻结**，不回滚 `e1d5105`。

### 1. 根因

`record_check_run` 的返回值只表示「是不是我建的行」——只有首次投递为真，重投永远假，换不来执行权。
投递侧（0014 的 `claim_for_push`）解决的是「谁可以 push」；**执行侧必须另有租约**。

### 2. 执行租约 + 原子认领（迁移 0015）

`check_runs` 加 `exec_owner uuid` / `exec_lease_until timestamptz` + 部分索引
`idx_check_runs_exec_lease (exec_lease_until) WHERE exec_lease_until IS NOT NULL`；**0014 不动**。
`claim_for_exec` 一条语句：

```sql
UPDATE check_runs SET exec_owner=$2, exec_lease_until=now()+make_interval(secs=>$3),
       status='started', finished_at=NULL
 WHERE id=$1 AND status IN ('started','failed')
   AND (exec_lease_until IS NULL OR exec_lease_until < now())
RETURNING exec_owner
```

- 行锁串行化 → 恰好一个 `Some(owner)`；其余 `None`，对 Apalis 是 **no-op（Ok）**，不造假失败。
- 认领**开启新尝试**（重置 `status`/`finished_at`），否则 `failed` 的重试永远写不进 `success`
  （终态写入带 `finished_at IS NULL` 守卫）。
- `EXEC_LEASE_SECS = 900`，超时即「死亡执行者让位」。
- 终态：成功走 `finish_check_run_owned_in`（**与结果同一事务**，`WHERE id=$1 AND exec_owner=$2`，
  0 行即整事务回滚）；失败走 `fail_check_run(owner, …)`（同守卫 + 释放租约，0 行只 warn 不覆盖新主人）。

### 3. 证据（含基线对照）

- `exec_claim_is_exclusive`：6 路并发（独立池）认领同一 run → **恰好 1 个赢家**，行内 `exec_owner`
  与之相符（无时序依赖）。
- `concurrent_executions_of_one_run_id_hit_miaospeed_once`：两 handler（屏障 + 两池）同时执行同一
  `run_id`，mock 把首请求挂 400ms → checker **只收到 1 次** submit、输家 `Ok`、run `success`、
  结果 1 行、租约已清。
- **基线对照**：换回 `e1d5105` 版 `check_job.rs`/`lib.rs` 跑同一测试 → `left: 2, right: 1`
  （旧代码确实打两次）；恢复后 1。测试有区分力。

### 4. 迁移纪律与实测

`phlogo_mig_scratch` 全序 0001…0015 → (push_state 3、`pushed` 0、恢复索引 1、configs 2、
执行租约两列 2、执行租约索引 1) → `0015.down` 只清执行租约（2→0、索引 1→0、push_state 仍 3）→
`0014.down` → `0013.down` → 删库。

### 5. 文档语义修正（复审明确要求）

- 不再声称周期任务可自动恢复：**没有** outbox/过期租约扫描器，过期租约只是「可被再认领」；周期任务
  的下一周期是**新** `run_id`，卡住的旧记录不会自己好。可自动恢复的只有手动重发同一 key。
- 幂等键**不绑定请求内容**（无 fingerprint）：同 key 配不同 `node_ids`/`batch_size` 不报错，后续应存
  请求摘要并在不匹配时拒绝。

### 门禁

worker **18**（`check_wiring` 16，含 11 项故障注入）/ curse **37** / database **4** /
distribution **43** / adapter **18** / checker **11** 全绿；workspace `rustfmt --check` +
`clippy --all-targets -- -D warnings` **0**；一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，
生产 `phlogo_jobs` 零接触。

### 途中修正（记要点）

1. **终态守卫踩到 M2.1 的既有行为**：`persist_check_run_in` 在同一事务里已写 `status='success'`、
   `finished_at=now()`，所以终态守卫**不能**再要求 `finished_at IS NULL`（永不命中）——首版即报
   `execution lease lost before the terminal write`；改为只按租约 owner 判归属、靠事务回滚兜底。
2. 基线对照脚本的清理链返回 2（DROP DATABASE 被残留连接挡住），测试库留在机器上 → 手工 DROP 并复查
   只剩 `phlogo` / `phlogo_jobs`。
3. **gates 脚本尾部命令（DROP DATABASE / 停 PG）一直在「看似执行、实际没执行」**：把 ~6 分钟的
   cargo 测试链整条内联进一条 ssh 命令时，尾部输出会丢、顺序会乱，链尾的 DROP 从未真正跑过，
   而 `EXIT=0` 看起来一切正常（这就是前几轮测试库反复残留的真因；此前误记为「被残留连接挡住」）。
   修法：把步骤写成校机上的 `.cmd` 文件（每步 `if errorlevel 1 goto gatefail`，失败即中止），
   输出重定向到 `gates.out` 再 `scp` 回本地解析；并把「测试库是否真的删掉」做成**显式断言**
   （同一条链内 `echo TESTDB_LEFT-BEGIN` / `psql -A -t -c "SELECT count(*) …"` / `END`，
   解析必须为 `0`）。修完实测：`==GATES-OK==` + `disposable-db-check: count='0'`。
   另记一个 cmd 陷阱：SQL 里带 `|`（如 `'X|'||count(*)`）会被 cmd 当管道拆开，即使整段在双引号里
   —— 校验查询因此改成不带 `|` 的 `SELECT count(*)`。

### 遗留

无后台恢复扫描器（投递/执行租约都没有）；幂等键缺请求摘要；M4/M5 冻结未动。

## [UTC 2026-11-22T12:50:00Z] S73 · 执行侧复核轮：挂死测活兜底 + 三个常数的依赖链（自检 + 独立复核）

**触发**：本轮起按新工作方式走——干活与自检由本会话（work routing group）负责，复核与裁定交给
**独立复核 agent**（smart routing 角色；`magpie/group/group-2` 在本会话未被授权，实际用
`magpie/group/group-3`，见「路由限制」）。复核对象是 S72 的 `88316be` + `94593eb`。

**复核裁定：REJECT**，两条缺陷：
1. **[blocking]** `run_check_once` 里 `adapter.check(&request).await` **没有任何超时**：一个「接了
   WebSocket 但永不发终态帧」的 MiaoSpeed 会把 worker 槽位永久占住，并且执行会跨过执行租约边界 ——
   按文档写的「同 key 重发恢复」就能让第二条投递在租约过期后赢认领，**两个执行者同时进 MiaoSpeed**，
   正好破掉本轮要保证的「一个 run_id 只有一个执行者」。adapter 自身确无超时（查证
   `crates/checker-miaospeed`），且父提交 `e1d5105` 同样如此（既有问题，但本轮把它变成承重项）。
2. **[minor]** `crates/database::check::finish_check_run_in` 在 `persist_all` 改用 owner 守卫版本后
   **已无调用方**，却仍是 `pub` 且不带租约守卫——未来误用会重新打开「标 success 但留下陈旧
   `exec_owner`」的口子。

**自检追加（比复核更严重的一条）**：租约原为 900s，而 apalis-sql 0.7.4 默认
`reenqueue_orphaned_after=300s`、`keep_alive=30s`，且 `reenqueue_orphaned()` 会把「worker 心跳已死」的
`Running` job 重置回 `Pending` 重投。**重投（≈300s）会早于租约到期（900s）**：`claim_for_exec` 返回
`None` → 该投递按 no-op 返回 `Ok` → Apalis 把它标成 done → **唯一一次恢复机会被烧掉**，run 永远停在
`started`。这比「没有扫描器」严重得多，因为它会把 apalis 自带的自愈能力变成反效果。

### 修法（三个常数组成一条依赖链）

```text
CHECK_RUN_TIMEOUT_SECS (180s) < EXEC_LEASE_SECS (240s) < apalis reenqueue_orphaned_after (300s)
   活着的执行者一定在租约被抢     死掉的执行者，等到孤儿重投时          于是那次重投能赢认领，
   走之前结束                     租约已过期                            而不是空转
```

1. `run_check_once` 用 `tokio::time::timeout` 包住 `adapter.check`：超时 → `fail_check_run`（释放
   租约）+ 返回 `Err`（Apalis 可重试）。默认 180s，`PHLOGO_CHECK_TIMEOUT_SECS` 可覆盖，且被**夹到
   租约以下**（`EXEC_LEASE_SECS - 30`）。
2. `EXEC_LEASE_SECS` 900 → **240s**（低于 apalis 孤儿窗口 300s，理由如上）。
3. 删除 `database::check::finish_check_run_in`；worker 侧 test-only 的 `finish_check_run` 保留并加
   ⚠️ 文档（无租约守卫、禁止生产路径调用）。
4. 新测试 `hung_checker_times_out_and_releases_the_lease`：mock 收下请求后 30s 不回终态帧，
   `PHLOGO_CHECK_TIMEOUT_SECS=1` → 断言返回 `Err`、耗时 < 10s、只 1 次 submit、run `failed`、
   租约字段清空。

### 文档同步（把「过度悲观」也一起纠正）

- §10.2 增加不等式链与三个数字的来源（apalis 默认值 300s/30s），并说明 `180s` 只够 ping，
  **M4 深度矩阵必须同时抬高 timeout 与租约**（或改心跳续租）。
- §10.4 重写：执行侧崩溃**可以**被 apalis 孤儿重投自动恢复（前提是租约短于该窗口）；我们自己仍无
  outbox/过期扫描器，剩余缺口是「队列行已被消费」的情形与「周期任务不回头救旧周期」。
- §11 把常数标成「互相约束」，改一个必须同时检查另外两个；新增 M4 提示条目。
- `p3-01-apalis-check-job.md`（设计稿记的是 30 分钟硬超时）加「实现现状：180s + 依赖链」注。
- `configuration-and-api.md` 补「单次执行硬超时 180s < 租约 240s」。

### 门禁（本轮实测）

worker **19**（`check_wiring` 17，含 12 项故障注入）/ curse **37** / database **4** /
distribution **43** / adapter **18** / checker **11** 全绿；workspace `rustfmt --check` +
`clippy --all-targets -- -D warnings` **0**；迁移 0001…0015 全序 up + 分步 down 已在一次性库验证；
一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，**并用链内断言确认删除（`TESTDB_LEFT` 计数 = 0）**。

### 路由限制（需要用户处理）

本会话的子代理模型白名单只有 `magpie/group/group-3`（短上下文 routing group）；指定
`magpie/group/group-2` 会被拒（`child LLM route ... is not allowed for this Session`）。所以
「检查与裁定交给 group/group-2」目前只能退化为 group/group-3，或由用户放开该路由。

### 遗留

无 outbox/过期扫描器（依赖 apalis 孤儿重投 + 手动同键重发）；幂等键缺请求摘要；M4 深度矩阵需重算
三个常数；M4/M5 仍未动。

## [UTC 2026-11-22T13:25:00Z] S74 · 钉住 apalis 的孤儿窗口/心跳 + 把不变量链变成编译期断言（复核轮 2 收尾）

**触发**：goal 循环第 2 轮独立复核对 `59b7580` 判 **ACCEPT**，但留下一个 minor：
「不变量链的上界 `apalis reenqueue_orphaned_after = 300s` 只是**上游默认值**，项目从未显式设置；
将来 apalis 升级若把它降到 ≤ 租约（240s），孤儿重投就会退化成 no-op `Ok`，run 永远停在 `started`
—— 而这条危险不需要改我们仓库的代码就能被触发」。

### 修法

1. `check_job.rs` 新增 `APALIS_ORPHAN_REENQUEUE_SECS = 300` / `APALIS_KEEP_ALIVE_SECS = 30`；
   `jobs_storage(pool)` 用 `apalis_sql::Config::new(type_name::<CheckJob>())`
   `.set_keep_alive(..).set_reenqueue_orphaned_after(..)` 构造 —— namespace **刻意保持**
   `type_name::<CheckJob>()`，因为它是 `apalis.jobs.job_type` 的判别列（改掉会孤儿化已入队任务）。
2. **编译期断言**把链钉死：`const _: () = assert!(CHECK_RUN_TIMEOUT_SECS < EXEC_LEASE_SECS);`
   与 `assert!((EXEC_LEASE_SECS as u64) < APALIS_ORPHAN_REENQUEUE_SECS);` —— 把超时抬到租约之上、或把
   租约抬到孤儿窗口之上，直接**编译失败**，而不是等到线上出现「空转烧掉唯一恢复机会」。
3. 所有构造 `CheckJob` 队列的位置改用 `jobs_storage`：`main.rs` check worker、`trigger_check_batch`、
   `check_wiring.rs` 三处测试。
4. 文档：§10.2 新增「这条链不靠上游默认值碰巧成立」（含升级 apalis 时的注意点：若上游改了 orphan 语义
   要重评 §10.4.1）；§11.6 改为「由编译期断言强制」。

### 测试区分力的实测证据（复核 J 项）

把 `run_check_once` 的 `tokio::time::timeout` 包裹**临时去掉**、其余不动，重跑
`hung_checker_times_out_and_releases_the_lease`：`FAILED … finished in 31.14s`（31s 正是 mock 的 30s
静默不回帧），带包裹时 1s 返回 `Err` 并通过。结论：该测试确实在测超时本身，不是假通过。之后已把校机
文件还原（本地/远端 md5 一致），一次性测试库删除并复查只剩 `phlogo` / `phlogo_jobs`。

### 门禁（本轮实测）

worker **19** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11**
全绿；`rustfmt --check` + `clippy --all-targets -- -D warnings` **0**；`cargo check --workspace
--all-targets` 0；一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，链内断言确认删除（`TESTDB_LEFT` 计数 = 0）。

### 复核结论

第 2 轮 **ACCEPT**（无阻塞缺陷；唯一 minor 即本条已修）。第 3 轮针对本提交复核 namespace 一致性与
断言有效性，结果见下一条目（若有）。

### 路由限制（仍未解决，需用户处理）

本会话子代理白名单只有 `magpie/group/group-3`；指定 `magpie/group/group-2`（用户指定的 smart
routing group）会被拒。所以「检查与裁定交给 group/group-2」目前退化为 group/group-3。

## [UTC 2026-11-22T14:20:00Z] S75 · smart 侧直接复核 HEAD：抓到并修掉第 3 个租约泄漏（既有复核未覆盖）

**角色变化**：用户要求「干活由 group/group、检查与裁定由 group/group-2」。本会话当前**已运行在
`group/group-2`**（模型切换），因此本轮检查由本会话直接做；但「用 group/group-2 作为子代理路由」仍被
会话策略拒绝（`child LLM route "magpie/group/group-2" is not allowed for this Session`），子代理白名单
只有 `magpie/group/group-3`。

**复核对象**：`1b22ba7`（工作树干净、本地=远端 0/0；门禁跑的校机文件与本地 md5 逐个一致）。

### 发现的缺陷（`run_check_once` 的第 4 条错误路径）

`claim_for_exec` 之后到终态写入之间的所有错误路径本应 `fail_check_run`（释放租约），但
`existing_ids()`（`adapter.check` **返回之后**读主库的一步）是裸 `?`：

```rust
let known = existing_ids(pool, &job.node_ids).await
    .map_err(|e| anyhow::anyhow!("existing_ids: {e}"))?;   // 直接把租约一起带走了
```

后果链条（比「没有扫描器」更硬）：主库一次抖动 → 该次投递返回 `Err` 但**仍持有 `exec_owner` + 未过期
租约** → run 停在 `started` → worker **心跳还活着**，apalis 不会把它当孤儿重投 → Apalis 立即重试的那
两三次全部撞上有效租约、按 no-op 返回 `Ok` 并完成 → 再也没有投递 → **run 永久卡在 `started`**。

### 修法与证据

1. 改走 `fail_check_run`（owner 守卫 + 同语句释放租约 + 标记 `failed`），使重试能重新认领。
   并把「claim 之后所有早退路径都经 `fail_check_run`」写成注释约束。
2. 新测试 `db_error_after_checker_releases_the_execution_lease`：用**真实的 DB 错误**制造该路径 ——
   测试在 mock 回帧后对 `nodes` 取 `ACCESS EXCLUSIVE` 锁，worker 侧用带 `lock_timeout=300ms` 的池，
   于是 `existing_ids` 超时失败；断言 `Err` + 只 1 次 submit + `status='failed'` + 租约字段清空。
3. **基线对照**：把修复回退成裸 `?` 后同一测试失败，`left: "started" / right: "failed"` —— 精确复现
   「带租约卡死」，证明测试有区分力（修完恢复，校机文件 md5 与本地一致）。

### 门禁（本轮实测）

worker **19**（`check_wiring` 17，含 13 项故障注入）/ curse **37** / database **4** /
distribution **43** / adapter **18** / checker **11** 全绿；`cargo check --workspace --all-targets` 0；
`rustfmt --check` + `clippy --all-targets -- -D warnings` 0；一次性 `phlogo_jobs_test_m3` 建库→测试→
销毁并由链内断言确认（`TESTDB_LEFT` 计数 = 0）；迁移 0001…0015 的 up/down 已在一次性库验证。

### 结论

`1b22ba7` + 本条修复之后，claim→终态之间不再存在不释放租约的退出路径；三组常量由编译期断言钉住；
恢复边界（apalis 孤儿重投可救、我们无自建扫描器、周期任务不回救旧周期）在文档里如实写清。

### 仍未决（需用户）

子代理路由 `magpie/group/group-2` 未授权 —— 若希望「检查由 group-2 的**独立代理**做」而不是由本会话
以 group-2 身份做，需要在会话/插件配置里放开该路由。

## [UTC 2026-11-22T15:10:00Z] S76 · smart 侧复核续：投递侧 push 失败释放认领 + 形状守卫的盲区自查

**角色**：本会话已在 `group/group-2` 上运行，故本轮「检查/裁定」由本会话直接承担（子代理路由
`magpie/group/group-2` 仍未授权，白名单只有 `group/group-3`）。上一条 S75 修掉了执行侧第三处租约泄漏；
本轮继续按「逐条早退路径」与「投递侧租约归属」两条线扫 HEAD。

### 发现 1：投递侧的「无人回收」缺口（已修最常见入口）

实测确认：迁移 0014 建了 `idx_check_runs_push_recovery` 索引，但**没有任何代码查询它** —— 没有扫描器、
没有后台任务。而 `enqueue_scheduled` / `trigger_check_batch` 里 `storage.push(job).await?` 失败时直接
`?` 返回，该行停在 `dispatching`：

- 周期调度**不会**回头重推它：下一周期是**新** `run_id`（`scheduled_run_id(schedule_id, batch)` 按
  周期 label 派生），旧 run_id 再也不会被 `claim_for_push` 命中；
- 手动触发可以救：同一 `idempotency_key` 重发 → 同一 label → 同一 run_id → 命中「过期 dispatching」。

修法（不扩裁定范围、不写扫描器）：push 失败时调用新增的 `release_push_claim()`，把该行从
`dispatching` 放回 **`pending`**（owner 守卫：租约已被接管时不动别人的行），于是下一次尝试可以
**立即**重试，不必等 60s 租约过期。两处 push 调用点（调度路径 + 手动触发路径）都改。
仍无自动回收的是「进程在 push 之后、mark 之前硬死」那一类 —— 那才是需要扫描器的。

测试 `failed_push_releases_the_delivery_claim`：把 storage 指向一个**不存在的数据库**让 push 必然
失败，断言调度器派生出的那一行回到 `pending`，并可立即重新认领。

### 发现 2：我自己写的守卫有盲区（**S79 才真正修好**，此处按 S79 更正）

`every_early_exit_after_the_exec_claim_releases_the_lease` 第一版只匹配 `return Err(`，而 S75 那个泄漏
恰恰是**裸 `?`**（没有 `return`）—— 也就是说，我为它写的守卫**抓不到它自己要防的形状**。

> ⚠️ **S76 时我以为已经修好了，这里更正**：S76 只把谓词改成「以 `?` 结尾」，但泄漏的实际形状是
> `...)?;`（**带分号**），仍然匹配不到；而当时那句「基线对照再跑即失败并点名那一行」是**不准确的**
> —— 守卫之所以失败，只是因为 `checked` 计数从 7 掉到 6（它以为代码形状变了），**并没有**指出泄漏行。
> 真正的修复在 S79：谓词同时匹配 `?` 与 `?;`，且判定改为**向前扫描到语句块边界**（原先的固定 4 行
> 窗口够不到 `Err(...) => { ... }` 分支里的 `fail_check_run`）。验证方式也从「回退整段代码」换成
> **注入式控制实验**：注入 `?;` 形状的泄漏 → 守卫失败并点名那一行；注入合法的（已守卫）早退 →
> 守卫保持沉默（无误报）。

### 文档

- §10.4 新增第 4 条「投递侧过期租约也没有人回收」（含索引存在但无人查询的实测），并说明 S76 堵住了
  push 失败这一入口、剩下的类别仍需扫描器；
- §11.5 同步（执行侧靠 apalis 孤儿重投自愈，投递侧连 apalis 也救不了 —— 队列里根本没有那个 job）。

### 门禁（本轮实测）

worker **22**（`check_wiring` 20，含 15 项故障注入）/ curse **37** / database **4** /
distribution **43** / adapter **18** / checker **11** 全绿；`rustfmt --check` +
`clippy --all-targets -- -D warnings` 0；一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，链内断言确认
删除（`TESTDB_LEFT` 计数 = 0）。

## [UTC 2026-11-22T15:40:00Z] S77 · smart 侧续：幂等键长度上限 + 修掉 curse 测试的顺序依赖

**角色**：本会话已在 `group/group-2` 上运行，本轮检查由本会话直接承担（子代理路由 `group/group-2`
仍未授权）。本轮沿两条线继续扫 HEAD：API 输入面、以及「测试之间是否有隐藏依赖」。

### 1. `idempotency_key` 无长度上限（已修）

key 会被拼进 period label（原样进 v5 派生、**并回显在响应里**），但此前没有任何长度限制 —— 调用方
可以把任意长度的字符串塞进响应与日志。新增 `MAX_IDEMPOTENCY_KEY_LEN = 128`，超限一律 **422 明确拒绝**
（与 `node_ids > 500` 同一口径：不静默截断）。测试断言 129 字符 → 422；128 字符 → 不因长度被拒。

### 2. curse 的 API 测试**依赖 worker 测试的副作用**（已修，且做了基线判定）

单跑 `cargo test -p curse --test check_trigger` 失败：

```
admin must get 202: error returned from database: relation "apalis.jobs" does not exist
  left: 500  right: 202
```

真因不是产品缺陷，而是 curse 测试的 `setup()` 只 migrate **主库**，**从不建 jobs 库的 apalis 表**；
它在全量门禁里能过，只是因为 worker 测试二进制先跑、顺手建了表 —— 一个**隐藏的测试顺序依赖**。

基线判定（重要）：把 S76 的 push 失败处理退回成原来的 `storage.push(job).await?` 后**同样失败**
（错误消息少了 `push: ` 前缀而已）⇒ **该失败是既有的**，不是最近几轮引入的回归。
（顺带说明：想用 `e1d5105` 整体做基线是不行的 —— 那版 `check_job.rs` 与现版 `lib.rs` 不兼容，
编译不过；改用「只回退这一处」的基线。）

修法：curse 测试自己 `PostgresStorage::setup(&jobs_pool)` 建 apalis schema。现在**单跑也通过**。

### 3. fetch 侧同类问题：如实登记，未擅自扩范围

`handle_fetch` 把错误吞进日志（`let _ = registry::reconcile_fetch_result(..)`）且 fetch worker **没有**
`.retry(RetryPolicy::..)` —— 与 M3.1 裁定第 3 点在 check 侧修过的问题同构（Apalis 认为成功、不重试）。
但 fetch 属 **P2 阶段**，裁定明确「M3 只做小范围、不进 M4」，故本轮**只登记不修**，写在 §10.4 第 5 条
与 §11（业务上不会永久漏抓：下一个 5 分钟周期会重新入队；缺的是单次失败的重试）。

### 门禁（本轮实测）

worker **22** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11**
全绿；`rustfmt --check` + `clippy --all-targets -- -D warnings` 0；一次性 `phlogo_jobs_test_m3`
建库→测试→销毁，链内断言确认删除（`TESTDB_LEFT` 计数 = 0）。另单独验证：`curse` 的
`check_trigger_permission_matrix` **单跑**通过（此前必须依赖 worker 测试先跑）。

## [UTC 2026-11-23T00:30:00Z] S78 · 移除无守卫的终态写出口 + 同步脚本加哈希校验（并记一次门禁拦住我）

### 1. 移除 `finish_check_run`（无守卫的终态写）

复核（round-2 minor #2 的同类）留下的最后一口子：worker crate **导出**了一个**无 owner 守卫、
不释放租约**的终态写 `finish_check_run` —— 任何 crate 都能 import 它，把一个「有租约在身」的 run
标成 `success` 并留下陈旧 `exec_owner`。本轮直接**删除**该函数（而不是降可见性）：
删掉后 `cargo check` 报 `never used`（说明它确实只有那一个测试调用方），于是把
`apps/worker/tests/check_job.rs` 的终态幂等断言改为走**有守卫**的路径 ——
`claim_for_exec` 拿 owner → `database::check::finish_check_run_owned_in` → 断言「连调两次仍只生效一次」。
现在 `check_runs` 的终态写**只有一条**路径，且带 owner 守卫。

### 2. 同步脚本的「静默不同步」被封死（本轮真实踩到）

删函数后我跑 `cargo check`，校机报的还是**旧代码**的警告（`finish_check_run` is never used 仍在），
而本地早就删了 —— 因为**上一次 `p86-sync-src.py` 实际没同步成功**（`rmdir` 后 `scp -r` 的目录状态
问题），脚本却仍然返回 0，紧接着的 `cargo check` 又命中缓存，于是「看起来编译过了」。
这是继 S72「长链尾部不执行却 EXIT=0」之后的**第二例「工具静默失败」**，同一个根因：成功与否没有
被断言。

修法：`p86-sync-src.py` 现在在拷贝后**逐文件比对 MD5**（本地 vs 校机 `certutil -hashfile`），
8 个关键文件全部打印 `hash OK` / `hash DIFF`，不一致即返回 5 并打印 `!! SYNC MISMATCH`。
本轮实测 8/8 OK。

### 3. 门禁真的拦住了我（clippy）

删函数时我留了一段悬空的 doc 注释 → `clippy -D warnings` 报
`empty_line_after_doc_comments` → 门禁 `==GATES-FAILED==`（我新链路里 `if errorlevel 1 goto gatefail`
起作用了）。修正后重跑 `==GATES-OK==`。顺带说明：新链路的失败**不再被吞**，且失败时
`disposable-db-check` 会报 `count='missing'`（而不是假 0）。

### 门禁（本轮实测）

worker **22** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11**
全绿；`rustfmt --check` + `clippy --all-targets -- -D warnings` 0；同步 8/8 哈希一致；
一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，链内断言确认删除（`TESTDB_LEFT` 计数 = 0）。

## [UTC 2026-11-23T01:05:00Z] S79 · 复核 DEFECT #1 认账并修实：形状守卫此前**抓不到自己要防的形状**

独立复核（round-5，累积 3 提交）判 **ACCEPT**（生产代码无缺陷），但给出 1 个 major + 3 个小项。
**major 那条我认，而且比复核说的更糟**：复核说守卫「只匹配裸 `?`、漏了 `?;`」；我自己用控制实验
量出来，它**连自己写的那个泄漏形状都看不见**，而且我此前在 commit message / §10.3 / ACTION-LOG 里
写的「基线对照会失败并**点名那一行**」是**不准确的**——基线之所以失败，只是因为 `checked` 计数
从 7 掉到 6（守卫「以为代码形状变了」），**不是**因为它指出了泄漏行。

### 缺陷与真实情况

1. **[major] 形状守卫的 `?` 判定漏 `?;`** —— 复核正确：`.map_err(...)?;` 以 `?;` 结尾，`ends_with('?')`
   匹配不到。更糟的是**第二个盲点**：判定「同一语句块里有 `fail_check_run`」用的是**固定 4 行窗口**，
   而 `fail_check_run` 常在 `Err(reason) => { ... }` 分支里、离 `return Err(..)` 5 行以上 → 窗口取不到
   → 合法路径也可能被误判。
   **修法**：(a) 谓词同时匹配 `?` 与 `?;`；(b) 判定改为**向前扫描到语句块边界**（而不是固定行数）。
   **两个控制实验验证**（不再靠「回退整段代码」这种间接法）：
   - 注入 `?;` 形状的泄漏 → 守卫失败并**点名那一行**：`4: .map_err(|e| anyhow::anyhow!("injected: {e}"))?;`
   - 注入**合法**的（已 `fail_check_run`）早退 → 守卫**保持沉默**（无误报）。
2. **[minor] `mark_enqueued` 上方有一段重复的 doc 块**（S78 插入时的手滑）→ 已删。
3. **[minor] 幂等键上限按字节算、文档写「字符」** → 改为 `chars().count()`（与文档一致），并补测试：
   128 个多字节字符**不得**因长度被拒（按字节算会在 ~43 字符就拒）。
4. **[minor] §10.5 门禁数字过期**（写 21、实际 22；`failed_push_releases_the_delivery_claim` 漏在 §10.3 表里）
   → 已更新，并把 15 项故障注入逐项列出。

### 门禁（本轮实测）

worker **22** / curse **37** / database **4** / distribution **43** / adapter **18** / checker **11**
全绿；`rustfmt --check` + `clippy --all-targets -- -D warnings` 0；同步 8/8 哈希一致；
一次性 `phlogo_jobs_test_m3` 建库→测试→销毁，链内断言确认删除（`TESTDB_LEFT` 计数 = 0）。

### 方法学记要（本轮最值得记的）

「守卫测试」本身也会有盲区，而且**它的基线对照可能以错误的方式失败**（计数变化而非点名问题）。
可靠做法是**注入式控制实验**：分别注入「有缺陷的形状」与「合法的同形形状」，要求守卫**前者报错并
指名、后者沉默**。本轮就是这么发现固定窗口这个第二盲点的 —— 只靠「回退整段代码」是看不出来的。
