# 迁移台账：refactor/v0.5.2 → upstream/main@05a1410

记录把本仓库旧分支 `refactor/v0.5.2`（上游 `0.5.1` 快照 + 1–18 项本地改动）迁移到
上游最新架构（`upstream/main@05a1410`）的进度与证据。编号沿用旧分支的
`LOCAL_PATCHES.md`。

| 项目 | 值 |
| --- | --- |
| 迁移基线 | `upstream/main` `05a14107fc024645e1ad350fd3b4dffa62ae1368`（2026-09-25） |
| 旧分支 | `refactor/v0.5.2` `18f95534fba82c1d0d3a41c028af9c23b1a25d1c` |
| 迁移分支 | `codex/migrate-upstream-main` |
| 共同祖先 | 无（按行为逐项移植，不做 merge / cherry-pick） |

## 状态说明

- **完成**：改动已落地并有证据。
- **部分**：只完成了一部分，剩余点已列出。
- **待办**：尚未开始。
- **不适用**：新架构已有等价实现，或该改动在新架构下无意义（需写明依据）。

## 阶段状态

| 阶段 | 状态 | 说明 |
| --- | --- | --- |
| P0 冻结基线与建立隔离分支 | 完成 | 分支 `codex/migrate-upstream-main` 起自 `05a1410` |
| P1 标准目标 + 构建入口 + 产物接口 | 部分 | 目标与产物接口已改；**Windows 构建未执行**（本环境无 MSVC/WebView2） |
| P2 配置、打包与升级兼容 | 待办 | |
| P3 卸载、提权与事务性安全 | 待办 | |
| P4 前端体验与隐私 | 待办 | 遥测移除属此项 |
| P5 依赖、文档与 CI 门禁 | 待办 | |
| P6 端到端兼容、发布与回退 | 待办 | |

## 逐项处置

| 旧补丁 | 状态 | 落点 / 证据 |
| --- | --- | --- |
| 1 / 1b / 1c 卸载注册表、快捷方式、计划任务扩展 | 待办 | `native/installer/uninstall.rs`、`native/installer/registry.rs`、`native/session/`、`web/types.ts`、`web/`；先对齐上游 `install-identity-registry` 与 `tests/registry.mjs` |
| 2 协议文件、格式、标题、弹窗 | 待办 | `native/builder/pack.rs` 内嵌正文；`web/` 与原生简化 UI 显示协议 |
| 3 删除范围与提权面安全阀 | 待办 | 与上游 `4821e4d` 按会话提权、`2026-09-24-cross-account-elevation` 交叉核对 |
| 4 vendored rcedit 兼容 | 待办 | 上游仍用 `Devolutions/rcedit-rs` git 依赖；核对 MSVC 14.51 是否需要 vendoring |
| 5 弹窗 footer | 待办 | 新 Preact UI 与原生 TaskDialog 重做 |
| 6 多用户与临时残留清理 | 待办 | 与上游新清理逻辑合并前先比行为 |
| 7 物理移除遥测 | 待办 | `native/utils/sentry.rs`、`native/main.rs`、`native/session/ui.rs`、Cargo/pnpm lock、Release 上传步骤 |
| 8 卸载收尾、路径比较、提权状态、ARP 静默入口 | 待办 | 与上游 `07452f0` / `5dda0c5` 合并验证 |
| 9 安全临时文件、下载验签、提权管道 | 待办 | 与上游 `native/fs/staging.rs`、`native/ipc/` 协同 |
| 10 DFS 拆分/注释 | 待办 | 旧 `src/dfs.ts` 与新 `native/session/`（含 `download_plan.rs`、`rate.rs`）职责映射 |
| 11 编译告警治理 | 部分 | 上游 `05a1410` 已清理一批；剩余待构建后确认 |
| 12 改名后的升级兼容 | 待办 | `legacyExeNames` / `legacyProgramFilesPaths` / `legacyUninstallNames` |
| 13 Windows 10/11 标准目标 | 部分 | 目标、`-Z build-std`、`rust-src`、`ctor` patch、`STATIC_VCRUNTIME` 已处理（见下）；CI 的 `CARGO_PROFILE_RELEASE_DEBUG` 等细节待补 |
| 14 zip 去 fork 与中文名解码 | 待办 | 上游仍用 `xytoki/zip2`；`native/thirdparty/mirrorc.rs` |
| 15 H3 改 `quinn`/`rustls` | 待办 | 上游仍用 `msquic-async` 系列 fork |
| 16 旧依赖替换与许可证 | 待办 | 上游仍用 `mslnk` / `nt_version` |
| 17 打包器修复 | 待办 | PE 识别、嵌入名、extract 路径约束、`replace-bin`；可能已有上游等价实现，测试通过才标记已覆盖 |
| 18 安装行为测试与单测 | 部分 | 上游行为矩阵保留；旧分支特有的 `userdata-ignore` / `builder-extract-replace` 断言待补 |

## P1 已落地改动与证据

| 改动 | 文件 | 证据 |
| --- | --- | --- |
| 目标改为 `x86_64-pc-windows-msvc`，去掉 `-Z build-std` | `package.json`、`scripts/merge-release-bundle.mjs`、`tests/prepare.mjs`、`tests/builder-extract-replace.mjs` | `node --check` 通过；`rg x86_64-win7-windows-msvc` 在代码/CI 中无残留 |
| 去掉 `rust-src` 组件 | `rust-toolchain.toml`、`.github/workflows/build.yml` | — |
| 移除 win7 专用 `ctor` git patch | `Cargo.toml`、`Cargo.lock` | `cargo metadata` 成功；lock 中 `ctor` 回到 crates.io 0.6.3 |
| 去掉 `STATIC_VCRUNTIME` 兜底 | `.cargo/config.toml` | 标准目标自带 crt-static 特性 |
| 新增构建入口 | `build.ps1` | `pwsh` 解析通过；取拼接体并校验体积大于 cargo builder |
| 产物/发布资产改名 `kirara-builder.exe` | `.github/workflows/build.yml` | YAML 解析通过 |

## 未验证项（阻塞）

- **Windows 构建与运行验证尚未执行**：本环境为 WSL，无 MSVC 工具链与 WebView2，
  `pnpm build`（cargo 交叉编译到 `x86_64-pc-windows-msvc`）无法在此完成。
  P1 的退出门槛（干净 checkout 产出 exe、CLI 冒烟、二次拼接稳定）需要在 Windows 或
  CI 上补齐后才能勾选。
- 依赖替换（第 4 / 14 / 15 / 16 项）与遥测移除（第 7 项）等仍未开始。
