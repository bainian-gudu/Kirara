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
| P2 配置、打包与升级兼容 | 部分 | 协议内联（第 2 项）与改名兼容（第 12 项）已落地；未识别字段检测、打包器修复（第 17 项）待办 |
| P3 卸载、提权与事务性安全 | 部分 | 卸载侧安全阀与扩展清理已落地（第 1/1b/1c/3/8 项）；提权管道已有上游等价实现 |
| P4 前端体验与隐私 | 部分 | 遥测移除（第 7 项）、协议弹窗与 footer 布局（第 2/5 项）已完成；历史版本提示待核 |
| P5 依赖、文档与 CI 门禁 | 部分 | CI 已关闭 release PDB；依赖替换（第 4/14/15/16 项）待办 |
| P6 端到端兼容、发布与回退 | 待办 | |

## 逐项处置

| 旧补丁 | 状态 | 落点 / 证据 |
| --- | --- | --- |
| 1 / 1b / 1c 卸载注册表、快捷方式、计划任务扩展 | 完成 | 见下「P3 已落地改动与证据」 |
| 2 协议文件、格式、标题、弹窗 | 完成 | 见下「P2 / P4 已落地改动与证据」 |
| 3 删除范围与提权面安全阀 | 部分 | 卸载侧四类判定 + 清单路径越界拦截已落地；下载验签、提权管道 ACL 待办 |
| 4 vendored rcedit 兼容 | 待办 | 上游仍用 `Devolutions/rcedit-rs` git 依赖；核对 MSVC 14.51 是否需要 vendoring |
| 5 弹窗 footer | 完成 | `.dialog` 改纵向 flex、footer 回文档流；见下 |
| 6 多用户与临时残留清理 | 部分 | 扩展注册表/计划任务清理已落地；`%VAR%` 展开、跨用户重放、`%TEMP%` 白名单、卸载前结束进程待办 |
| 7 物理移除遥测 | 完成 | 见下「P4 已落地改动与证据」 |
| 8 卸载收尾、路径比较、提权状态、ARP 静默入口 | 部分 | ARP 加引号 + 静默入口已落地；提权状态由上游等价实现覆盖 |
| 9 安全临时文件、下载验签、提权管道 | 部分 | 清单越界已挡（见下）；下载后执行的验签、管道 ACL、日志重解析点检查待办 |
| 10 DFS 拆分/注释 | 待办 | 旧 `src/dfs.ts` 与新 `native/session/`（含 `download_plan.rs`、`rate.rs`）职责映射 |
| 11 编译告警治理 | 部分 | 上游 `05a1410` 已清理一批；剩余待构建后确认 |
| 12 改名后的升级兼容 | 完成 | `legacyExeNames` / `legacyProgramFilesPaths` / `legacyUninstallNames`，见下 |
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

## P4 已落地改动与证据（第 7 项：物理移除遥测）

两条外发通道（Sentry 错误上报、`77.cocogoat.cn` 使用统计）连依赖一起删除，
不是运行时关开关。

| 改动 | 落点 |
| --- | --- |
| 删除手写 Sentry 客户端 | `native/utils/sentry.rs`（整份删除），`native/utils/mod.rs` 去掉 `pub mod sentry;` 与 `get_device_id()` |
| 时间格式化独立 | 新增 `native/utils/time.rs`（`rfc3339`），`log.rs` / `taskdialog.rs` 改用它 |
| 崩溃收尾去遥测 | 新增 `native/utils/crash.rs`（panic hook 写 stderr + 日志 + 拉起 `crash-dialog`，不再上报、不再带事件号） |
| 错误对话框去掉事件号 | `native/utils/taskdialog.rs` 的 `ErrorDialog.event_id` 字段、footer 行与复制内容里的 `event:` 行 |
| 码表去掉上报判定 | `native/utils/code.rs` 删除 `Coded.event_id`、`should_report` / `should_report_error` |
| 错误类型去掉上报入口 | `native/utils/error.rs` 删除 `report_if_needed()` |
| IPC 去掉遥测通道 | `native/ipc/mod.rs` 删除 `PipeMsg::Envelope` / `PipeMsg::Breadcrumb`；`native/ipc/manager.rs` 删除 envelope/breadcrumb 分支与 outbox |
| 会话不再发事务与计数 | `native/session/run.rs` 删除 `Transaction`（含 5 处 `timed` / 4 处 `set_measurement`）、`emit_insight` / `insight_base` / `prepare_event` / `source_id` / `txn_status`；失败分类改为本地 `tracing::warn!` |
| 使用统计函数删除 | `native/session/ui.rs` 删除 `send_ev_insight` 与 `encode_uri` |
| 安装配置不再上报 | `native/installer/config.rs` 的 `set_context` 换成一条本地 `tracing::info!` |
| CLI 去掉事件号参数 | `native/cli/arg.rs`、`native/cli/mod.rs`：`Command::CrashDialog` 变单元变体 |
| 前端契约去掉事件号 | `web/state.ts`、`web/__tests__/fixtures.ts` |
| 文案去掉事件号 | `locales/{en-US,zh-CN}.tsv`：`dialog.crash` 去掉 `{event_id}`，删除 `dialog.event_id` / `dialog.unknown_event` |
| 构建期依赖删除 | `package.json` 去掉 `@sentry/cli`；`pnpm-workspace.yaml` 去掉白名单项；`pnpm-lock.yaml` 用 `pnpm install --lockfile-only` 重新生成（净减 165 行，无版本顺带升级） |
| CI 去掉上传步骤 | `.github/workflows/build.yml` 删除 Release job 的 "Sentry upload" |
| `whoami` 降为 dev-dependency | `Cargo.toml`（`get_device_id()` 删除后只剩 `session::state` 测试用） |

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo check --target x86_64-pc-windows-msvc --all-targets` 通过，**0 warning / 0 error**
  （含 `--force-warn=unused_crate_dependencies` 与全部单测目标）。
  本机没有 MSVC/CMake，native 依赖的 build script 用一次性假工具链（`cl.exe` / `lib.exe` / `cmake`，
  仅存在于 `/tmp`）绕过；不产生可链接产物，也不写入仓库。
- `pnpm exec tsc --noEmit` 通过；`pnpm test`（vitest）3 个文件 30 个用例全绿。
- `rg -n "sentry|77\.cocogoat|capture_anyhow|report_if_needed|event_id"` 在源码、配置、
  工作流与两个锁文件中均无残留（`docs/notes/` 里的历史设计文档除外）。

## P2 / P3 已落地改动与证据（第 1 / 1b / 1c / 8 / 12 项）

上游新架构把「卸载要删什么」集中在 `native/installer/uninstall.rs`，调用点在
`native/session/run.rs`（不再是 Vue 前端拼 IPC 参数），所以旧分支改前端的部分
在新架构里对应到 Rust 侧。

| 改动 | 落点 | 说明 |
| --- | --- | --- |
| 配置字段 | `native/session/types.rs` 的 `ProjectConfig` | 新增 `legacyExeNames` / `legacyUninstallNames` / `legacyProgramFilesPaths` / `extraUninstallLnkNames` / `extraUninstallRegistry` / `extraUninstallScheduledTasks`，全部 `#[serde(default)]`；配置 JSON 原样内嵌，键名与旧分支一致 |
| 旧主程序名识别 | `native/installer/config.rs`、`native/installer/mod.rs` | `CURRENT_DIR` / `PARENT_DIR` / `REG` / `REG_FOLDED` / `DEFAULT` 探测都接受旧名；注册表缺失时按 `legacyProgramFilesPaths` 兜底并给出新的 `DEFAULT_LEGACY` 来源 |
| 旧卸载器识别 | `native/installer/config.rs` | 自身文件名命中 `uninstallName` 或 `legacyUninstallNames` 即按卸载流程走 |
| 旧进程结束 | `native/session/run.rs` 的 `prepare_process` | 当前名 + 历史名一起枚举，只结束安装目录内的实例 |
| 旧文件清理 | `native/session/run.rs` 的 `legacy_delete_names` | 更新时把旧 exe / 旧卸载器名并入 `deletes`（只接受纯文件名，含分隔符的一律丢弃） |
| 快捷方式 | `native/session/run.rs` 的 `create_shortcuts` | 开始菜单项在更新时也重建；桌面图标更新时只重建**已存在**的（含历史命名），不给没勾选的用户补建 |
| 扩展清理参数 | `native/installer/uninstall.rs` 的 `RunUninstallArgs` | 新增 `extra_uninstall_shortcuts` / `extra_uninstall_registry` / `extra_uninstall_scheduled_tasks` / `reg_name`，全部 `#[serde(default)]` |
| 注册表清理 | `native/installer/uninstall.rs` | `clean_extra_registry`：`HKCU` 时额外遍历 `HKEY_USERS` 下已加载的用户配置单元（跳过 `*_Classes` / `.DEFAULT` / `S-1-5-18`），错误只记日志 |
| 计划任务清理 | `native/installer/uninstall.rs` | `is_safe_task_name`（必须以 `regName` 开头、字符白名单、≤100）+ `clean_extra_scheduled_tasks`（`schtasks.exe` 参数数组、无 shell） |
| 删除范围安全阀 | `native/installer/uninstall.rs` | 文件清单过 `is_safe_rel`；`userDataPath` / `extraUninstallPath` 过 `is_safe_delete_root`（绝对、无 `..`、非重解析点、不在 `%SystemRoot%`、至少两级、非受保护位置本身）；快捷方式过 `is_safe_shortcut_path`（只放行 `.lnk`） |
| 清单越界拦截 | `native/session/plan.rs`、`native/fs/commit.rs` | 网络元数据里的文件名 / 残留清单先过 `is_safe_member`（非空 + `is_safe_rel`）；提交单元再做一次，越界的 `rel` 会让 `join_rel` 退化成安装目录本身，所以直接以 `FILE_IO_FAILED` 拒绝 |
| ARP 卸载入口 | `native/installer/registry.rs` | `UninstallString` 整体加引号；新增 `QuietUninstallString`（`-U -S -I` 短选项，长名会被 clap 拒绝）；`tests/utils.mjs` / `tests/registry.mjs` 同步断言 |
| 提权管道存活 | `native/ipc/manager.rs` | 不适用：上游已用 `fail_all_pending` 让在途请求立刻报错，`run()` 在发送失败时也提前返回 `IPC_ERR`；且 `ManagedElevate` 按会话创建，重试即新起 helper，不存在「永远转圈」 |

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo check --target x86_64-pc-windows-msvc --all-targets` 通过，**0 warning / 0 error**
  （假工具链同第 7 项，仅存在于 `/tmp`）。
- `pnpm exec tsc --noEmit` 通过；`pnpm test` 30 个用例全绿。
- 新增单测 `legacy_exe_name_counts_as_an_existing_install`（旧名目录算可升级）、
  `scheduled_task_names_must_belong_to_this_product`（通配符 / 目录形式 / 别人的任务名 /
  空名 / 超长名 / 命令行注入全部拒绝）。
- 真机行为（旧目录原地升级、卸载后 ARP 与计划任务残留消失）需 Windows + CI 行为测试确认。

## P2 / P4 已落地改动与证据（第 2 / 5 项：用户协议与弹窗布局）

新架构的确认界面有两套：装了 WebView2 时是 Preact 的 `Ready` 页（`gui_entry` 直接进
`prepare_gui`），没装 WebView2 时是原生 TaskDialog 的 ready 页（`native_entry`）。两套
都要有协议合同，正文一律在**打包期**内联进配置，离线包 / 更新器 / 卸载器共用同一份。

| 改动 | 落点 | 说明 |
| --- | --- | --- |
| 协议内联 | `native/builder/pack.rs` 的 `resolve_agreement` | 读 `agreementFile`（相对配置文件目录）、`agreementFormat`、`agreementTitle`，生成 `agreement: {title, format, content}` 并删掉三个源字段；BOM 去掉、CRLF 归一；读不到只警告，链接退化为纯文字 |
| 配置字段 | `native/session/types.rs` 的 `AgreementConfig` | `ProjectConfig.agreement`，`#[serde(default)]` |
| 渲染契约 | `native/session/state.rs` 的 `ProjectView.agreement` | 随 `UiState` 推给 WebView；原生路径直接用同一份 |
| Web 净化渲染 | `web/agreement.ts`（新增） | 纯文本转义 + 极简 Markdown + HTML 统一过白名单净化：删 `script`/`style`/表单控件/`iframe`/`object`/`svg` 等，剥 `on*` 与 `style`/`srcdoc`，URI 只放行 `http(s)` / `mailto` / `#` |
| Web 协议弹窗 | `web/panels/AgreementPanel.tsx`（新增）、`web/screens/Ready.tsx` | 链接打开弹窗看全文；正文可滚动、可选中；正文里的链接交给系统浏览器打开，不在窗口里导航；「我已阅读并同意」顺带勾选 |
| 接受门槛 | `web/screens/Ready.tsx` | 有正文时默认**不勾选**，安装按钮禁用；没正文时保持上游的默认勾选 |
| 原生路径合同 | `native/host/native.rs`、`native/utils/taskdialog.rs` | 新装且有正文时，第一遍弹窗的勾选项是「我已阅读并同意」，勾选后第二遍才显示常规选项；「查看协议」把正文写到 `%TEMP%\kachina-agreement.txt`（先拒重解析点）再交给系统默认文本查看器，长文可滚动 |
| 弹窗 footer | `web/layout.css` | `.dialog` 纵向 flex + `overflow:hidden`；`.dialog-body` 吃剩余高度；`.dialog-footer` 回文档流；footer 里的 `.btn-install` 覆盖为 `position: static` 的 28px 小按钮，按钮与正文不再重叠 |

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo check --target x86_64-pc-windows-msvc --all-targets` 通过，**0 warning / 0 error**。
- `pnpm exec tsc --noEmit` 通过；`pnpm test` 4 个文件 37 个用例全绿。
- 新增单测：`agreement_inlines_the_file_and_drops_the_source_keys`（BOM/CRLF、缺文件、
  未配置三种情形）、`web/__tests__/agreement.test.ts`（5 个：纯文本转义、Markdown 子集、
  净化负例、注释剥离）、`render.test.tsx` 的接受门槛用例（默认禁用 → 打开弹窗 → 接受后可用）。
- `pnpm exec rsbuild build` 通过，PurgeCSS 未误删 `.agreement-body` / `.dialog-footer .btn-install`。
- 真机观感（窄窗、高 DPI、暗亮主题、原生弹窗两遍流程）需 Windows 上确认。

## 未验证项（阻塞）

- **Windows 构建与运行验证尚未执行**：本环境为 WSL，无 MSVC 工具链与 WebView2，
  `pnpm build`（cargo 交叉编译到 `x86_64-pc-windows-msvc`）无法在此完成。
  P1 的退出门槛（干净 checkout 产出 exe、CLI 冒烟、二次拼接稳定）需要在 Windows 或
  CI 上补齐后才能勾选。
- 依赖替换（第 4 / 14 / 15 / 16 项）尚未开始。
