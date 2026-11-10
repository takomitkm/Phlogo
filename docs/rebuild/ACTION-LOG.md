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
2. Rust 工程放 `Phlogo/core-rs/`（Cargo workspace），保留 Go 树作参考，避免与 `go.mod` 根目录混编；待 P1 落地时据此细化目录与 CONVENTIONS §6。
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
# 新建 deny.toml（仓库根，licenses/advisories/bans/sources；待 P1 core-rs/Cargo.toml 后生效）
# 更新 docs/THIRD_PARTY.md：VProxy/Apalis/Vben/MiaoSpeed/Mihomo 全部真实版本+哈希
# 更新 p0-05 页面「固定基线」节 + P0 index 验收勾选
```

**结果（P0 收口）**：`docs/THIRD_PARTY.md` 固定版本一览——
- VProxy `19b9c147…`（v1.16.9，MIT，仅移植）；MiaoSpeed tag `4.7.9` 自建 exe `2c302b29…46ab18`（AGPL）；Mihomo `AirportR/mihomo v1.19.33-x365-miu.2`（GPL，随 MiaoSpeed）；Apalis `0.7.4`（MIT）；Vben `v5.7.0`（commit `63a38dce…70e9`，MIT）；paily AGPL 仅架构参考。
- `deny.toml` 已落地（P1 生效）；SBOM 计划：`cargo cyclonedx` + 二进制哈希登记比对。

**决策**：**P0 全阶段 PASS**（P0-01 VProxy / P0-02 MiaoSpeed / P0-03 Apalis / P0-04 Vben / P0-05 许可证供应链）。下一步进入 **P1（core-rs workspace + 数据层）**，开始真正写 Rust 代码；重量级编译继续走校机（rustc 1.99.1 + MinGW PATH）。

> 环境基线（后续复用）：校机 `rustc/cargo 1.99.1`（rsproxy 升级）；GNU target 需 `D:\yzd\MSYS2\mingw64\bin` 于 PATH；cargo 依赖走 `rsproxy-sparse`；Go 依赖走 `goproxy.cn`；node `v24.22.2`+pnpm `10.33.4`（npmmirror）。

---

## [UTC 2026-11-10T01:18:00Z] S11 · P1-01 Cargo workspace 骨架（build/fmt/clippy 全绿）

**动作**：按 `docs/rebuild/02-p1-core-skeleton-data/p1-01-workspace.md` 在仓库根建 Cargo workspace。

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

**动作**：`apps/api` 改 lib+bin；实现 Figment 配置（显式 PAILY_ env 覆盖）、auth 中间件、路由装配、OpenAPI JSON 端点。

**命令**（校机 `C:\Users\YiZhidi\phlogo-rs`，PATH 含 cargo+mingw64）：
```
cargo build --workspace --all-targets
cargo test -p api
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# 运行期冒烟：$env:PAILY_SERVER_PORT=28099; .\target\debug\api.exe & curl 200/401/404
```

**结果**：`BUILD_EXIT=0`、`TEST_EXIT=0`（4 passed：health/protected-401/unknown-404/openapi-200）、`FMT_EXIT=0`、`CLIPPY_EXIT=0`；
冒烟 **`health=200 post_sources=401 unknown=404`**（`PAILY_SERVER_PORT` 覆盖生效）。

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
psql -c "CREATE ROLE paily ...; CREATE DATABASE paily OWNER paily;"
$env:DATABASE_URL=postgres://paily:change-me@127.0.0.1:5432/paily
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

**动作**：新增 `crates/registry`、`database` 仓库层与模型、`apps/api` handlers + AppState（lazy pool），接入真实路由。

**命令**（校机，PG 18.6 + cargo）：
```
cargo build --workspace --all-targets
DATABASE_URL=postgres://paily:change-me@127.0.0.1:5432/paily cargo test --workspace
cargo test -p database --test pg_crud
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
# API e2e：起 api(PAILY_AUTH_SERVICE_SECRET=svc-secret) → curl POST /api/v1/sources 两次（同 body）
```

**结果**：`BUILD/TEST/FMT/CLIPPY` 全 0；`pg_crud` **2 passed**；**e2e `create1=201 create2=200`**（b1 `alreadyExisted=false`、b2 `true`，同一 id），`list_noauth=401`/`list_auth=200`。

**决策/修正**：
1. 去重竞态：新增 `0002_uq_sources`（`UNIQUE(type,content)`）+ `ON CONFLICT ... DO NOTHING`，并发安全（取代 `FOR UPDATE` 方案）。
2. `apps/api` 用 `database::connect_lazy` 构建 pool，未连库即可跑 health/401/404 单测。
3. handler `parse_id` 用 `#[allow(clippy::result_large_err)]`；rustfmt 差异按 `fmt.log` 逐处修正。

**决策**：P1-04 判定 **PASS**。**P1 阶段（骨架+数据层+CRUD）全部完成**。下一步进入 P2（fetch/parse 抓取解析）。
