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
| P3 卸载、提权与事务性安全 | 完成 | 卸载侧安全阀与扩展清理（第 1/1b/1c/8 项）、提权管道 ACL、重解析点拦截、下载后执行验签（第 3/9 项）均已落地 |
| P4 前端体验与隐私 | 部分 | 遥测移除（第 7 项）、协议弹窗与 footer 布局（第 2/5 项）已完成；历史版本提示待核 |
| P5 依赖、文档与 CI 门禁 | 部分 | CI 已关闭 release PDB；依赖替换与许可证全部落地（第 4/14/15/16 项）；README / 来源说明与快速检查门禁待收尾 |
| P6 端到端兼容、发布与回退 | 待办 | |

## 逐项处置

| 旧补丁 | 状态 | 落点 / 证据 |
| --- | --- | --- |
| 1 / 1b / 1c 卸载注册表、快捷方式、计划任务扩展 | 完成 | 见下「P3 已落地改动与证据」 |
| 2 协议文件、格式、标题、弹窗 | 完成 | 见下「P2 / P4 已落地改动与证据」 |
| 3 删除范围与提权面安全阀 | 完成 | 卸载侧四类判定、清单路径越界拦截、提权管道 ACL、下载后执行验签均已落地；宿主侧 `UninstallLauncher` / `ResolveDotNetCli` 不在本仓库（见下） |
| 4 vendored rcedit 兼容 | 完成 | `vendor/rcedit-rs/` 副本 + `rescle.cc` 的 `std::locale::classic()`；根 `Cargo.toml` 改 path 依赖（见下） |
| 5 弹窗 footer | 完成 | `.dialog` 改纵向 flex、footer 回文档流；见下 |
| 6 多用户与临时残留清理 | 完成 | `%VAR%` 展开、跨用户重放、`%TEMP%` 白名单、卸载前结束进程，见下 |
| 7 物理移除遥测 | 完成 | 见下「P4 已落地改动与证据」 |
| 8 卸载收尾、路径比较、提权状态、ARP 静默入口 | 完成 | ARP 加引号 + 静默入口 + 卸载时清 Mirror酱 CDK 凭据已落地；收尾顺序与路径比较由上游 `normalize_full` / 错误收集覆盖；提权状态由上游等价实现覆盖 |
| 9 安全临时文件、下载验签、提权管道 | 完成 | 清单越界、提权管道 ACL、日志/暂存重解析点、下载后执行验签与 `get_userprofile` 修复均已落地（见下） |
| 10 DFS 拆分/注释 | 待办 | 旧 `src/dfs.ts` 与新 `native/session/`（含 `download_plan.rs`、`rate.rs`）职责映射 |
| 11 编译告警治理 | 部分 | 上游 `05a1410` 已清理一批；剩余待构建后确认 |
| 12 改名后的升级兼容 | 完成 | `legacyExeNames` / `legacyProgramFilesPaths` / `legacyUninstallNames`，见下 |
| 13 Windows 10/11 标准目标 | 部分 | 目标、`-Z build-std`、`rust-src`、`ctor` patch、`STATIC_VCRUNTIME` 已处理（见下）；CI 的 `CARGO_PROFILE_RELEASE_DEBUG` 等细节待补 |
| 14 zip 去 fork 与中文名解码 | 完成 | 改用 crates.io `zip 8.6`；`native/thirdparty/mirrorc.rs` 按 `name_raw()` 自己解条目名（见下） |
| 15 H3 改 `quinn`/`rustls` | 完成 | `native/capabilities/h3.rs` 整份换成 quinn + rustls + h3-quinn；`[patch.crates-io]` 的 msquic fork 删除（见下） |
| 16 旧依赖替换与许可证 | 完成 | `mslnk` → Shell Link API、`nt_version` → ntdll、`libs/` 许可证补齐（见下） |
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
| 环境变量展开（第 6 项） | `native/installer/uninstall.rs` 的 `expand_env_vars` / `expand_path_list` | 在**卸载器进程内**展开 `%VAR%`（提权后 `%LOCALAPPDATA%` 是执行账户的），未知变量原样保留、`%%` 是字面 `%`；展开发生在安全阀之前 |
| 跨用户清理（第 6 项） | `native/installer/uninstall.rs` 的 `profile_relative_tail` / `loaded_profile_roots` / `collect_all_users_cleanup_targets` / `clean_per_user_leftovers` | 从 `ProfileList\<SID>\ProfileImagePath` 枚举已加载用户，把「相对用户目录的尾巴」重放到每个用户；只放行 `AppData` / `Documents` / `Desktop` 下的产品目录，`Desktop` 只认 `.lnk`，叶子命中 Shell 容器黑名单即拒绝；勾选语义自动跟随（没勾就没有 `userDataPath`） |
| `%TEMP%` 白名单（第 6 项） | `native/installer/uninstall.rs` 的 `is_installer_temp_artifact` / `clean_installer_temp_files` | 只认固定形状：WebView2 引导器、协议查看临时文件；只删文件不递归。运行时安装包与卸载器副本已由 staging 目录回收，不再单列。`KachinaInstaller.log` **不删**：它是本次会话的诊断记录，行为测试也要在进程退出后读它（见下「CI 回归」） |
| 卸载前结束进程（第 6 项） | `native/session/run.rs` 的 `prepare_process` / `run_uninstall_inner` | 卸载前按当前名与历史名找安装目录内的实例，询问后结束（静默卸载直接结束）；结束后等 1s 让 WebView2 缓存释放；用户拒绝则回到卸载页（`SessionResult::uninstall_cancelled`） |

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

## P3 已落地改动与证据（第 9 项：提权管道 ACL 与重解析点拦截）

提权 helper 是通过命名管道 `\\.\pipe\Kachina-Elevate-<uuid>` 驱动的，管道服务端由
中等完整性的 UI 进程创建，提权子进程作为客户端连上来。UAC 弹窗期间（可能几十秒）管道
空着等人，这段窗口就是攻击面。同一类问题也出现在「以管理员身份写固定路径文件」上：
普通权限进程预置符号链接 / junction，就能把提权进程的写入重定向到受保护文件。

| 改动 | 落点 | 说明 |
| --- | --- | --- |
| 管道 ACL 收紧 | `native/utils/acl.rs` 的 `create_security_attributes` | SDDL 由 `D:(A;;GA;;;AC)(A;;GA;;;RC)(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;BU)S:(ML;;NW;;;LW)` 改为 `D:(A;;GA;;;CO)(A;;GA;;;SY)(A;;GA;;;BA)S:(ML;;NW;;;ME)`：去掉 `BU`（所有本地用户）/ `AC`（AppContainer）/ `RC`（远程会话），改为只放行创建者（`CO`，解析成创建者 SID，UI 与提权子进程同一用户）与 `SY` / `BA`；完整性级别由 Low 提到 Medium，低完整性进程无法写入（no-write-up）。`reject_remote_clients` / `first_pipe_instance` 上游已开 |
| 重解析点判定复用 | `native/fs/staging.rs` 的 `has_reparse_point` | 从 `uninstall.rs` 提升为公共函数，供日志 / 暂存 / 卸载三处共用；沿父级逐层检查符号链接与 junction，`NotFound` 跳过继续看父级（首次写日志时文件本就不存在），其它读取错误按「是」处理 |
| 暂存文件拒写 | `native/fs.rs` 的 `create_staged_file` | 写入前先判定重解析点，命中即以 `FILE_IO_FAILED` 拒绝并附路径。staging 根可能在用户可写的 `%TEMP%`，且由提权 helper 写入，预置链接即可重定向 |
| 日志文件拒写 | `native/utils/log.rs` 的 `init` | 日志文件名固定，命中重解析点时只写控制台（stderr 提示），不打开文件 |

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo check --target x86_64-pc-windows-msvc --all-targets` 通过，**0 warning / 0 error**。
- SDDL 合法性由 `ConvertStringSecurityDescriptorToSecurityDescriptorW` 在真机运行时校验，
  `CO` / `ML;;NW;;;ME` 是标准 SDDL 记号；真机提权安装 / 卸载流程需 Windows 上确认。

## P3 已落地改动与证据（第 3 / 9 项：下载后执行链路的落地与验签）

「下载一个可执行文件、然后运行它」这条链路在新架构里有三处（.NET 运行时、VC++ 运行库、
WebView2 引导器）。上游的写法是 `%TEMP%` 里的固定文件名 + `CREATE_ALWAYS` 写入（会跟随
符号链接）+ 下完不验签直接运行，普通权限进程可以在「写完 → 启动」之间把文件换成自己的
exe，下载源被劫持时也照样执行。这里统一收口到 `native/utils/secure_temp.rs`。

| 改动 | 落点 | 说明 |
| --- | --- | --- |
| 新增落地 / 验签模块 | `native/utils/secure_temp.rs` | `package_dir`（优先 `%SystemRoot%\Temp`，退回 `%TEMP%`）、`package_path`（UUID 文件名）、`create_exclusive_file`（`create_new`，不跟随已存在链接）、`is_trusted_microsoft_signature`（纯函数）、`verify_microsoft_signed`（走 PowerShell `Get-AuthenticodeSignature`，失败删文件并报错） |
| WebView2 引导器 | `native/module/wv2.rs` | 由 `%TEMP%` 固定名 + `tokio::fs::write` 改为 `package_path` + 独占创建 + 写入 + 执行前验签，失败删文件并走原有错误对话框 |
| 运行时安装包验签 | `native/installer/runtimes.rs` | .NET 与 VC++ 两处在 `progressed_copy` 之后、`process::spawn` 之前调用 `verify_microsoft_signed`；落地目录仍是 staging `dl\`（提权时在安装目录旁、由提权进程写入，且 `create_staged_file` 已拒重解析点） |
| profile 目录解析 | `native/utils/dir.rs` | `get_userprofile` 由 `GetUserProfileDirectoryW(HANDLE::default(), …)` 改为 `FOLDERID_Profile`：前者需要有效用户令牌句柄，传空句柄会以 `ERROR_INVALID_HANDLE` 失败并让私有目录判定失效。新增 `path_is_equal_or_child`（统一分隔符、忽略大小写、整段比较）替换 `Path::starts_with` |
| Mirrorc ZIP 条目 | `native/thirdparty/mirrorc.rs` | 条目路径已过 `is_safe_rel`（绝对 / `..` / 系统目录），新增重解析点拦截；条目遍历用 `archive.len()`，不漏最后一项（上游已如此） |
| 卸载器镜像落盘 | `native/installer/uninstall.rs` 的 `stage_self_image` | 名字已过 `is_safe_rel`，新增重解析点拦截，避免提权进程跟随预置链接写到 staging 之外 |
| 卸载清 CDK 凭据 | `native/session/run.rs` 的 `run_uninstall_inner` | 卸载收尾时删除 Windows 凭据管理器里的 `KachinaInstaller_MirrorChyanCDK_<app>`；凭据不是注册表也不是文件，卸载器脚本覆盖不到 |

不在本仓库：旧 `LOCAL_PATCHES.md` 第 3 节列的 `UninstallLauncher.IsTrustworthyUninstaller`
与 `RuntimePrerequisite.ResolveDotNetCli` 属于**已安装产品**的宿主（`src/Host/`），两个分支
的安装器仓库里都没有这份代码，迁移不涉及。

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo check --target x86_64-pc-windows-msvc --all-targets` 通过，**0 warning / 0 error**
  （检查管道已用注入的编译错误验证过会真的失败）。
- 新增单测：`secure_temp` 的 4 个验签判定用例（有效微软签名 / 他人签名 / 冒名 Subject /
  非 Valid 状态）、`dir` 的 `private_folder_matching_ignores_case_and_separators`。
- 真机需确认两点：`Get-AuthenticodeSignature` 的证书 Subject 布局（不匹配时 fail-closed，
  错误信息带实际 status 与 subject）、`%SystemRoot%\Temp` 在标准用户下的可写性。

## P5 已落地改动与证据（第 4 / 16 项：依赖替换与许可证）

共同点：**只换实现，不换行为**；锁文件重新解析后没有顺带升级任何版本。

| 改动 | 落点 | 说明 |
| --- | --- | --- |
| rcedit 改为仓库内副本（第 4 项） | `vendor/rcedit-rs/`（12 个文件）、根 `Cargo.toml` | 上游 `Devolutions/rcedit-rs@1bfa3ee6` 的 `rescle.cc` 用了 MSVC 非标准扩展 `std::locale::empty()`，MSVC 14.51 起从 `<xlocale>` 移除（microsoft/STL#5834），`windows-latest` 已是 VS 2026 / MSVC 14.51，CI 必然失败。副本改 `std::locale::classic()`（基准 locale 不影响 `codecvt_utf8` 的转换结果，且不受 `locale::global()` 影响），并删掉上游 `[dev-dependencies] tempfile`（`tests/` 未纳入副本）。两份 LICENSE 原样保留，差异与升级步骤见 `vendor/rcedit-rs/LOCAL_PATCHES.md` |
| `.lnk` 改系统 API（第 16a 项） | `native/installer/lnk.rs` | `mslnk 0.1`（2022 年停更，自带约 1300 行手写 .lnk 序列化）换成 `IShellLinkW` + `IPersistFile::Save`；COM 初始化整段放进 `spawn_blocking`，`RPC_E_CHANGED_MODE` 视为可用但不配对 `CoUninitialize`；工作目录填目标文件所在目录。同时补回旧分支的路径校验（绝对路径、无 `..`、必须以 `.lnk` 结尾、目标过 `is_safe_delete_root`、`lnk` 非重解析点） |
| 版本号改 ntdll（第 16b 项） | `native/utils/os_version.rs`、`native/host/window.rs`、`native/capabilities/mod.rs` | `nt_version 0.1` 换成自己声明 `#[link(name = "ntdll")] RtlGetNtVersionNumbers`；`build` 的高 16 位标志位统一在 `get()` 里裁掉，两处调用点不再各自 `& 0xffff`。H3 的 Win11 门槛（`10.0` 且 build ≥ 22000）未放宽 |
| `libs/` 许可证（第 16c 项） | `libs/hdiff-sys/LICENSE`、`libs/hpatch-sys/LICENSE`、`libs/THIRDPARTY.md` | 补齐 HDiffPatch（MIT，Copyright (c) 2012-2023 housisong）与 libdivsufsort（MIT，Copyright (c) 2003-2008 Yuta Mori）的许可文本，并记录上游地址、快照版本 `v4.8.0`、本地四类差异（include 路径、`extern "C"` 出口、hpatch 具体错误码、注释中文化）与升级步骤 |
| 依赖删除 | 根 `Cargo.toml`、`Cargo.lock` | 去掉 `nt_version`、`mslnk`；`rcedit` / `rcedit-sys` 去掉 `source = "git+…"`。锁文件净减 27 行，只少了 `mslnk`、`nt_version` 与仅供 `mslnk` 使用的 `bitflags 1.3.2`，无版本变动 |
| zip 去 fork（第 14 项） | 根 `Cargo.toml`、`native/thirdparty/mirrorc.rs` | 上游的 `xytoki/zip2` fork 相对上游只把 `read.rs` 的 UTF-8 判定写死成 `true`（部分打包工具写中文名时不置该标志位，按标志位解码会退回 CP437 得到乱码，MirrorChyan 下发的包正属于这类）。改用 crates.io `zip 8.6` 后复刻同一语义：新增 `decode_entry_name`（`String::from_utf8_lossy`，与 fork 分支逐字一致），条目清单由 `file_names()`（按标志位解码）改为逐个 `by_index(i)` 取 `name_raw()` 再解码，前缀计算与路径安全判定都建立在这份名字上。`by_name()` 保留：zip 8.x 的名字索引按原始字节建表（`index_for_name` 用 `name.as_bytes()`），UTF-8 名字查得到。feature 保持与原 fork 相同的解压能力（`deflate-flate2-zlib-rs` / `deflate64` / `zstd`），flate2 走纯 Rust 的 zlib-rs 后端，不引入新的 C 依赖 |
| H3 换传输层（第 15 项） | `native/capabilities/h3.rs`（整份替换）、`native/capabilities/mod.rs`、根 `Cargo.toml` | 上游走 `h3-msquic-async` + `xytoki/msquic-async-rs` fork + 静态 `seera-msquic`（还要在 `[patch.crates-io]` 里重定向两个 git 源，构建时编上千个 C 文件）。换成 `quinn 0.11` + `rustls 0.23`（显式 ring 提供者）+ `h3-quinn 0.0.10` + `rustls-platform-verifier 0.7`，`[patch.crates-io]` 与 `h3-msquic-async` 依赖一并删除。对外行为逐条保持：连接池按 `(host, port, pin)` 复用、上限 32、空闲与死亡连接清扫；`PinningMode::Force` / `Add` 的判定顺序不变（`Add` 下系统信任即放行）；`PinTarget::Spki` 改为在证书 DER 上直接定位 SubjectPublicKeyInfo，字节与上游 `CryptEncodeObjectEx(X509_PUBLIC_KEY_INFO)` 一致，`openssl x509 -pubkey … | sha256sum` 的结果仍可直接当固定值；`http3://` 拦截、失败即 `disable_h3()`、UA 的 `h3/enabled`、`discover()` 接受任意证书并回传哈希都照旧；Win11+ 启用门槛保留（那是上游为 msquic + Schannel 定的，H3 的启用范围属于对外行为，不跟着依赖替换变） |
| 启动探测改为 QUIC 配置 | `native/capabilities/mod.rs` 的 `probe_h3_support` | 第三步由「建 msquic Registration 验证 DLL + Schannel」改为 `h3::probe()`：只建一次完整 QUIC 客户端配置，验证加密提供者、系统证书验证器与 QUIC 参数就绪，不开 socket |
| 删除 cmake 环境变量 | `build.ps1` | msquic 是唯一用 cmake 编 C 源码的依赖，替换后依赖图里已没有 `cmake` crate，`CMAKE_BUILD_PARALLEL_LEVEL` 的设置成为死代码 |

验证证据（本机 WSL，非 Windows 运行验证）：

- `cargo metadata` 重新解析锁文件成功；`cargo check --target x86_64-pc-windows-msvc --all-targets`
  通过，**0 warning / 0 error**，日志里能看到 `Compiling rcedit-sys v0.1.0 (…/vendor/rcedit-rs/rcedit-sys)`
  —— 副本的 `build.rs` 与 `rescle.cc` 真被编了一遍（本机用 `/tmp` 的一次性假 `cl.exe`，
  只验证构建脚本与源码能被接受，不产出可链接产物）。
- `rg -n "nt_version|mslnk" native/ Cargo.toml` 无残留；`vendor/rcedit-rs/rcedit-sys/src/rescle.cc`
  里没有 `locale::empty(`。
- H3 替换后：`Cargo.lock` 里 `msquic` 出现 0 次，`quinn` / `quinn-proto` / `quinn-udp` /
  `rustls` / `rustls-platform-verifier` / `h3-quinn` 均已入图；`cargo metadata` 的包集合里
  不再有 `cmake`、`c-types`、`rangemap`、`ctor` / `dtor`（都只被 msquic 依赖链引入）。
  依赖侧有一条 future-incompat 提示（`quinn-udp v0.5.16`），不是本仓库代码的告警。
- 行为验证的两点缺口（真机才能确认）：H3 真实连接只能在 Windows 上跑，靠 CI 的 Build
  与安装 / 更新测试兜底；`packaging.config.json` 目前只有 `https://` 地址，`http3://`
  在正式安装流程里默认不会被走到。
- 快捷方式的**行为**只能实机验证：装一次看桌面 / 开始菜单的快捷方式能启动、工作目录正确，
  再卸载确认被清干净。

## CI 回归：卸载时删日志文件

第 6 项的 `%TEMP%` 白名单第一版把 `KachinaInstaller.log` 也列进了回收范围，行为测试
`test (uninstall registry)` 随即失败（run `37910792940`）：

```
Test failed: Helper uninstall removes the HKLM record:
  - elevation helper: operations did not go through the helper process
Log file not found at: ...\Temp\KachinaInstaller.log
```

`tests/registry.mjs` 的 `expectHelperUsed` 在卸载进程退出后读 `%TEMP%\KachinaInstaller.log`
断言提权助手确实被拉起；卸载时删掉日志等于把刚结束的会话现场一起销毁。日志是会话的
诊断产物（下一次运行继续追加），不是需要回收的残留，已从白名单移除并补断言
（`temp_artifact_whitelist_is_shape_based`）。其余 15 个测试 job 在该 run 中全绿，
说明失败点只有这一处。

## 未验证项（阻塞）

- **Windows 构建与运行验证尚未执行**：本环境为 WSL，无 MSVC 工具链与 WebView2，
  `pnpm build`（cargo 交叉编译到 `x86_64-pc-windows-msvc`）无法在此完成。
  P1 的退出门槛（干净 checkout 产出 exe、CLI 冒烟、二次拼接稳定）需要在 Windows 或
  CI 上补齐后才能勾选。
- 依赖替换（第 4 / 14 / 15 / 16 项）尚未开始。
