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
