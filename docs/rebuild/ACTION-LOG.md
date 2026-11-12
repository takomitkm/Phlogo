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
DATABASE_URL=postgres://paily:change-me@127.0.0.1:5432/paily cargo test --workspace
cargo fmt --all / cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

**结果**：`build/test/fmt/clippy` 全 **0**；reconcile 6 场景全过（Replace 删消失+Orphaned、failed 保留、Accumulate 保留、删源不伤共享节点、空 Replace 全孤儿、死源守卫）。

**过程修正**：`database/Cargo.toml` 曾被 edit 拼坏（`}anyhow` 同行）修复；`registry` 补 `serde_json/serde/sqlx/thiserror`；`delete_source` 去掉 anyhow 依赖改直用 SQL；moved value（`error.clone()`）、`explicit_auto_deref`（`&mut *tx`→`&mut tx`）、`cloned_ref_to_slice_refs`（`from_ref`）逐个清掉。

**决策**：P2-04 **PASS**。**P2 阶段（fetch+SSRF / 解析 / 指纹 / 对账）全部完成**。下一步进入 P3（测活 check：icmp/tcp/http 探测、`node_status` 窗口）。

---

## [UTC 2026-11-11T02:30:00Z] S19 · P3-01 Apalis Check Job（PG 后端真机跑通；幂等副作用）

**动作**：`apps/worker` lib+bin 化：`CheckJob` + 幂等 `record/finish_check_run`（migration 0004 主库）+ Apalis `PostgresStorage` 装配；**Apalis 独立库 `paily_jobs`**。

**关键修正**：apalis 0.7.4 `setup()` 内嵌 migration 与业务 `migrate!` 共用 `_sqlx_migrations` 冲突（`VersionMissing(1)`）→ **队列独立数据库**（`PAILY_DATABASE_JOBS_DSN`，默认派生 `paily_jobs`），同时满足方案 §3.1 分离要求。`postgres` feature 属于 `apalis-sql`。

**命令**（校机，PG 18.6）：
```
cargo build --workspace --all-targets
DATABASE_URL=... cargo test --workspace      # + PAILY_DATABASE_JOBS_DSN=paily_jobs
cargo fmt --all -- --check / cargo clippy --workspace --all-targets -- -D warnings
# 真机 smoke：起 worker 12s，确认 paily_jobs 内 apalis 表 + 主库 check_runs finished 计数递增
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

**动作**：后端补 `GET/PUT /api/v1/config`（KV 读写，事务）；前端三页（sources 去重创建/删除/ignore-dead、nodes 列表删除、scoring 配置编辑）+ `paily.ts` 自含 API 层 + Paily 菜单路由。

**关键踩坑**：首次部署覆盖了 vben 原生 `src/api/request.ts` → `core/auth.ts` 的 `requestClient/baseRequestClient` 导出丢失 → MISSING_EXPORT 构建失败。从本地 pristine v5.7.0 恢复原文件，自研层改独立 `paily.ts`（自含 fetch+bearer）。**教训：向第三方 monorepo 注入文件前先查同名导出的消费方。**

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

**动作**：新增 `dashboard.vue`（统计卡+分布）、`checks.vue`（tags+初筛日志，-1 红标）、`node-detail.vue`（身份+双时间线）、`subscriptions.vue`（profile CRUD+token 签发一次性弹窗/撤销）；`paily.ts` 扩至全部端点；路由七项；Nodes 页加详情入口。

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
- **#1 高级搜索 + 预览**：`apps/api/src/handlers/search.rs`——`POST /nodes/search`、`POST /sources/search`（AST JSON body + 分页 ≤200）、`POST /filter/preview`（≤100）；filter crate 扩 `parse_with(whitelist)` / `matches_source` / `SOURCE_FIELD_WHITELIST`；权限矩阵纳入（读面 AdminOrService，fail-closed 保持）。
- **#3 模板配置**：`apps/api/src/handlers/format_config.rs` + `format_configs` 表；**写入前用渲染同款解析器校验**（坏模板 400，不在每次订阅时 500）；`GET` 读面 AdminOrService、`PUT` AdminOnly。

**接线配置键**：`grade_*_min` → `handlers/config.rs::grade_thresholds()`（订阅渲染不再硬编码阈值）。

**契约**：utoipa 补 6 条路径 → **25 条**（`openapi.json` 10290 B）。

**验证**：校机真实 PG 全套 `FULL_TEST_EXIT=0`、`cargo fmt --check=0`、`clippy -D warnings=0`；新增 `apps/api/tests/search_endpoints.rs`（未认证 401 边界 3 项）。

**已知限制（诚实记录）**：`unlock` 条件永不匹配（无 streaming 表）；节点 `region` 取自最近测活 metrics；搜索是内存过滤（与上游同语义，量大时应改 SQL 下推）。**sponsors 域仍缺，待用户裁定是否恢复**。

---

## [UTC 2026-11-13T04:55:00Z] S42 · A1 落地（PROXY protocol v1）+ 修限流桶键真 bug + 部署面收紧

**A1（用户裁定 PROXY protocol）已实现**：
- `apps/api/src/proxy_protocol.rs`：v1 解析（`TCP4`/`TCP6`/`UNKNOWN`）+ **自带 accept 循环**（axum 0.7 的 `axum::serve` 内部直接建连交给 hyper，没有消费前置头的钩子——照 axum `serve.rs` 的写法自建 loop + `hyper::server::conn::http1`）；非 PROXY 连接首批字节用 `PrefixedStream` **回放**给 hyper；PROXY 头之后可能同包到达的 HTTP 字节也回放（`read_proxy_header` 返回 leftover）。
- 真实地址落点 `ConnectInfo<SocketAddr>`（与 axum 原生同构）→ `peer_addr()`/`peer_ip()` 统一取用；**从不读 XFF**。
- 部署：`deploy/Caddyfile` 三个 `reverse_proxy` 各加 `transport http { proxy_protocol v1 }`（**本机 podman `caddy validate` 实测 "Valid configuration"**）；`deploy/compose.yaml` 的 `api` **取消宿主端口**（只 expose），加 `PAILY_SERVER_PROXY_PROTOCOL=1` / `PAILY_SERVER_EXPECT_PROXY=1`（缺头即拒）。
- 配置：`crates/config` 新增 `server.proxy_protocol` / `server.expect_proxy`（默认关，向后兼容）。
- 测试：`apps/api/tests/proxy_protocol.rs` 5 项（带头 200 / 无头不强制 200 / 无头强制拒 / `PROXY UNKNOWN` 不报错 / 头被正确消费）+ 模块内单测 5 项。

**修掉的真 bug（重要）**：`rate_limit.rs` 原读**裸 `SocketAddr`** 扩展，而 axum 插入的是 `ConnectInfo<SocketAddr>` → **永远取不到**，桶键恒为 `0.0.0.0`（全局单桶）。即"按 IP 分桶"此前**从未真正生效**。现已改经 `peer_ip()`。

**测试自身踩的坑**：`prefixed_stream_replays_bytes` 用 `duplex` 时写端未关闭 → `read_to_end` 永等 EOF → 测试挂 60 秒（首次跑被卡住）。修正为 `drop(w)`。

**验证**：校机真实 PG 全套 `FULL_TEST_EXIT=0`、`cargo fmt --check=0`、`clippy -D warnings=0`；本机 podman `caddy validate` 通过；`podman-compose config` 可解析。

**顺带修正文档错误**：`deploy/README.md` 里脚本路径写成 `scripts/runtime.sh`，实际在**仓库根** `scripts/`（git ls-files 确认）。

---

## [UTC 2026-11-13T05:55:00Z] S43 · B4 本机 compose 全栈演练 ✅（用户裁定容器用本机）+ 修 content 源 fetch 真 bug

**镜像构建**：`podman build`（Buildah）出 `paily-api:local` / `paily-worker:local`（rust:1.83-slim → bookworm-slim，多阶段）——**首次真正构建**（此前只有 Dockerfile）。

**部署缺口修复**：
1. **`paily_jobs` 库**：postgres 官方镜像只建 `POSTGRES_DB=paily`，worker 启动即挂（`database "paily_jobs" does not exist`）→ 新增 `deploy/postgres-init/01-jobs-db.sql` 挂 `/docker-entrypoint-initdb.d`（新卷自动建；已存在的卷需手动 `CREATE DATABASE`——本次演练即手动补的）；
2. **compose build 键**：podman-compose 1.6.0 不认 `build: {context, dockerfile}` 内联 map（报 "no Containerfile found"）→ 改多行 map + `image:` 指向预构建镜像，`podman compose`（provider 转发）`--no-build` 起；
3. **api 补 `PAILY_DATABASE_JOBS_DSN`**（fetch/trigger 端点要它入队）。

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
- `web/vben-admin/src/api/paily.ts` 补 sponsor 段（`SponsorView/SponsorUpsert/listSponsors/createSponsor/updateSponsor/deleteSponsor` + `simpleCondition` AST 构造辅助）；
- **`views/paily/sponsors.vue`**：分页表格（priority 可排序 / 注入文案 / AST 预览）+ 新建/编辑对话框——表单把「字段+运算符+值」三个简单控件拼成 **Filter-AST 单叶条件**（空值 = 匹配全部节点），提交前本地构造、服务端白名单兜底；删除带确认；
- 路由 `PailySponsors`（`/paily/sponsors`）。

**校机构建**：checkout 实际在 `C:\Users\YiZhidi\p0-04\vue-vben-admin-5.7.0`（根下那个不存在）；node v22.22.2 在 `C:\Users\YiZhidi\shortcuts-verify\`、pnpm 10.33.4 只有 `.tools\pnpm\10.33.4\bin\pnpm.CMD`（无全局 exe）——PATH 组合后 **`build:ele` 11/11 successful**（1m04s）。
