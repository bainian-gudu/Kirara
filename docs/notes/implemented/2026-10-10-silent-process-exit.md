# 安装与卸载静默结束占用进程

Status: implemented

## Problem

安装、更新与卸载都要在自己产品的进程持有目标文件之前把它们结束掉，否则文件替换或删除会失败。把「是否结束」交给用户决定，等于在几秒的流程里插一次模态询问，而拒绝之后流程只能中止。

## Decision

`native/session/run.rs` 的 `prepare_process` 在安装、更新与卸载三条路径上都直接结束安装目录内的当前名与历史名实例：先按配置的 `elevate` 结束，失败再按提权重试一次，仍失败挂 `PROCESS_KILL_FAILED`；结束前按 pid 记一条 info 日志。卸载路径结束后仍等 1s 让 WebView2 缓存释放。

会话没有进程询问这个 `PromptKind`，`prompt.process_running.*` 文案键、`SessionResult::uninstall_cancelled` 以及对应的渲染夹具与用例一并不存在。文件占用提示（`occupied_files`）与版本不匹配提示（`version_mismatch`）不受影响，仍是询问式。

## Alternatives considered

- 保留询问：模态询问的合理答案只有「继续」，拒绝只能中止整个流程，而安装流程本身只有几秒。
- 只在卸载时静默：安装与更新面对的是同一个「文件被自己产品的进程占用」问题。
- 结束失败时要求用户手工关闭再重试：`PROCESS_KILL_FAILED` 已带失败原因，用户重试安装即可。

## Verification

| 判据 | 结果 |
| --- | --- |
| 三条路径都不再弹出询问 | PASS：`PromptKind` 无进程变体，`locales/` 无 `prompt.process_running.*`，渲染用例与夹具无对应项，`SessionResult` 无 `uninstall_cancelled` |
| Rust 侧编译 | PASS：`cargo check --target x86_64-pc-windows-msvc --all-targets` 0 warning / 0 error（本机 WSL，用仅存在于临时目录的假 MSVC 工具链让 cc-rs 通过） |
| 前端与门禁 | PASS：`pnpm test` 41 个用例全绿；`tools/devcheck/devcheck.ps1` 七层通过（rust 层因缺 MSVC 跳过） |
| 结束进程的行为本身未变 | PASS：CI 的 `occupied-process` 行为测试断言更新时结束占用进程 |

## Consequences

- 用户没有界面内的办法阻止安装器结束自己产品的进程；要保留运行中的程序只能先退出安装器。
- 结束失败只留错误码与日志，界面不再提供「是否继续」的选择。
