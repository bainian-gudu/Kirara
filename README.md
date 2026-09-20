# Kirara — 安装器构建工具

HoYoEnhance 的安装 / 更新 / 卸载**只有 Kachina 一种实现**，本仓库就是那套工具链：
上游 [kachina-installer](https://github.com/YuehaiTeam/kachina-installer) 的源码快照
（tag `0.5.1`）+ 本地修改，产出 `kirara-builder.exe`。

```text
Kirara/                        仓库根目录就是安装器源码
├── src/                       Vue 3 前端（安装 / 卸载 / 更新界面）
├── src-tauri/                 Rust 侧（packer CLI + installer GUI 模板 + 卸载器）
├── vendor/rcedit-rs/          vendored cargo 依赖（上游 rcedit-rs + 1 行 C++ 修复）
├── public/                   图标等静态资源
├── tests/                    安装 / 更新行为测试（offline / online × install / update）
├── tools/
│   ├── ci/Import-DevCmd.ps1  注入 MSVC 开发环境（Ninja 生成器要用）
│   └── devcheck/             不跑完整构建的快速体检，见该目录 README
├── build.ps1                 唯一构建入口：源码 → tools/kirara-builder.exe
├── UPSTREAM.md               上游来源、快照版本、为什么把源码放进来
├── LOCAL_PATCHES.md          相对上游的全部本地修改（逐处说明 + 升级套用顺序）
└── UPSTREAM_README.md        上游自带的 README（含它自己的配置项文档）
```

下游项目 [HoYoEnhance](https://github.com/bainian-gudu/HoYoEnhance) 只保留
`packaging/packaging.config.json`（安装目录、ARP 名称、卸载清理范围、协议文件）与
`packaging/pack.ps1`（暂存载荷 → 调本仓库的 builder），构建时按固定 ref 检出本仓库。

## 构建

只在 Windows 上可构建（Tauri + MSVC + `windows` crate）：

```powershell
pwsh build.ps1            # 源码 → tools\kirara-builder.exe
pwsh build.ps1 -Force     # 忽略「产物比源码新」的判断，强制重建
```

需要 Rust nightly（含 `rust-src`，上游用 `-Z build-std`）、Node.js 20+、pnpm 10、
PowerShell 7 与 Windows MSVC / VS Build Tools；`build.ps1` 会检查工具链并安装
Rust 工具链与 `rust-src`，其余缺什么就报什么。

产物 `tools\kirara-builder.exe` 是「打包器 CLI + 安装器 GUI 模板」的二进制拼接体，
供下游按 `pack` / `gen` 子命令调用。

## 相对上游的改动

本仓库**不是纯净快照**：卸载器安全加固、卸载残留清理、可配置用户协议、MSVC 14.51
兼容（vendored `rcedit-rs`）、**物理移除全部遥测**（上游的 Sentry 上报与前端使用统计）
等改动都在里面，逐处说明见 [`LOCAL_PATCHES.md`](LOCAL_PATCHES.md)，上游版本与快照
来源见 [`UPSTREAM.md`](UPSTREAM.md)。

## 检查

```powershell
pwsh tools/devcheck/devcheck.ps1              # all：vendor ps1 gen rust logic native front ci
pwsh tools/devcheck/devcheck.ps1 -SelfTest    # 自检：注入错误，确认每层都会报错
pwsh tools/devcheck/devcheck.ps1 -Fix         # 只对我们维护的两个 .rs 跑 rustfmt
```

分层说明、覆盖范围与维护约定见 [`tools/devcheck/README.md`](tools/devcheck/README.md)。

## CI

| 工作流 | 触发 | 做什么 |
| --- | --- | --- |
| **Devcheck**（`.github/workflows/devcheck.yml`） | push 到 main、任何 PR、手动 | `vendor` 层 → `all` → `-SelfTest`（ubuntu + windows 双 runner） |
| **Build**（`.github/workflows/build.yml`） | push / PR / 手动；tag 触发 Release | 从源码构建 `kirara-builder.exe` → 跑安装 / 更新行为测试 → tag 时把产物挂到 Release |

`Build` 的 `test` job 会拿刚构建出的 builder 在 runner 上真装一次、真升一次
（`offline-install` / `online-install` / `offline-update` / `online-update` 四组）。
上游同款流程里有一处 Sentry 上传步骤，本仓库没有（遥测已移除）。

## 日志里哪些告警是正常的

| 字样 | 来源 |
| --- | --- |
| `warning: the following packages contain code that will be rejected by a future version of Rust: russh v0.54.5` | 上游依赖的 future-incompat 提示，升级 `russh` 才会消失 |
| `Could Not Find ...\target\x86_64-win7-windows-msvc\release\kachina-builder...` | tauri CLI 自己探测产物路径的输出；产物落在不带三元组的 `target\release\`，`build.ps1` 有兜底分支 |
| `NODE_NO_WARNINGS` 静音掉的 `DEP0040` / `DEP0169` | `actions/setup-node` 等 action 自己依赖的旧 API 告警，与本仓库无关 |

## workflow 里那些看着多余的设置

思路、踩坑过程与出处写在这里，代码里只留一句指针：

| 设置 | 为什么 |
| --- | --- |
| `RUST_TOOLCHAIN: nightly` + `-Z build-std` | 上游的构建方式，stable 工具链编不过 |
| `CMAKE_GENERATOR: Ninja` + `Enable Windows long paths` | `seera-msquic` 的静态构建会在极深路径下写 `.tlog`，超过 Windows 260 字符上限时 MSBuild 报 `error FTK1011`。Ninja 不写 `.tlog`，长路径是第二道防线。**副作用**：Ninja 不会自己去 VS 安装目录找 `cl.exe`，必须先跑 `tools/ci/Import-DevCmd.ps1` 注入 `PATH` / `INCLUDE` / `LIB` |
| `tools/ci/Import-DevCmd.ps1` | 按 `ProgramFiles` / `ProgramFiles(x86)` 枚举 `vcvarsall.bat`，用 `Start-Process` 跑一次子 cmd 拿全量环境变量，结果写进 `$GITHUB_ENV`；不依赖任何 Node 运行时，所以不会产生 action 弃用告警 |
| `NODE_NO_WARNINGS: "1"` | 压掉第三方 action 自己的 Node 弃用告警 |
| `.gitattributes`（`* text=auto eol=lf`） | windows-latest 的 git 默认 `core.autocrlf=true`，检出成 CRLF 后 `prettier --check` 在 Windows 上必挂 |
| `git config --global init.defaultBranch main`（放在 checkout 之前） | `actions/checkout` 会先 `git init`，ubuntu 镜像上默认分支名还是 `master`，每次打 8 行 hint |

工作流里 action 的版本下限（低于它的版本会在 runner 上打 Node 20 弃用告警）：`actions/checkout` ≥ v5、`actions/cache` ≥ v5、`actions/upload-artifact` ≥ v6、`actions/download-artifact` ≥ v7、`pnpm/action-setup` ≥ v6、`actions/setup-node` ≥ v5、`softprops/action-gh-release` ≥ v3。

`ci` 层会解析上面这一行并与 `.github/workflows/*.yml` 里实际用到的版本比对：
升工作流时忘了同步这里（或反过来）都会失败。

## 许可

- 本仓库自己维护的脚本、文档与检查（`build.ps1`、`tools/**`、`*.md`）：MIT，
  见 [`LICENSE`](LICENSE)。
- `src/`、`src-tauri/`、`public/`、`tests/`、`UPSTREAM_README.md` 等来自上游
  kachina-installer 的**源码快照**：上游仓库当前**没有声明任何许可证**，本仓库按原样
  保存并注明出处（见 [`UPSTREAM.md`](UPSTREAM.md)）。若要再分发这些文件，请自行确认
  上游的授权；本仓库不对上游代码授予任何额外许可。
- `vendor/rcedit-rs/` 的许可证随该目录（`LICENSE`、`LICENSE.rcedit`）。
