# 无人值守运行不弹模态框

Status: implemented

## Problem

静默安装（`-S`）和非交互安装（`-I`）失败时会调用 `error_dialog`，后端直接执行
`rfd::MessageDialog::show()`。这个调用会阻塞等待用户点击；CI、控制面板和自动化调用
没有人可点击，于是失败路径永久挂起。`interrupted-download` 的首次更新正是这样在
CI 上超时，测试进程和更新器一直留在运行中。

## Decision

`installer/mod.rs` 提供纯函数 `should_show_dialog(silent, non_interactive)`：
只有交互运行才返回 `true`。`error_dialog` 在无人值守时写 `tracing::error!` 后立即
返回；`confirm_dialog` 写 warn 后返回 `false`（默认取消）。前端 `dialog_error`
在静默或非交互模式下关闭窗口，保证失败后进程能退出。

判定单独抽成纯函数，是为了让 `tools/devcheck` 的 logic 层能在任意平台断言，而不必
启动 WebView2 宿主或显示真实窗口。

## Alternatives considered

- 只在前端 `dialog_error` 里判断：后端其他调用点仍可能弹模态框，保护不完整。
- 给 `MessageDialog` 加超时：`rfd` 的同步 `show()` 没有超时接口，而且无人值守时本
  就不该出现需要用户操作的窗口。
- 仅静默模式（`-S`）不弹：非交互模式（`-I`）同样是无人值守语义，测试的开发环境
  也会用 `-I`，必须一起覆盖。

## Verification

| 判据 | 结果 |
|---|---|
| `should_show_dialog(false, false)` 为真 | PASS：devcheck logic 层 [25] 组断言 |
| 静默、非交互、两者同时为真时均不弹框 | PASS：devcheck logic 层 [25] 组 3 条断言 |
| 静默更新下载中断后失败进程退出，CI 不再等到 10 分钟超时 | PASS：`Build` run `35699771148` 的 `interrupted-download` job success，整条工作流 success |

## Consequences

无人值守失败的诊断信息进入日志，不再要求用户点击确认；交互运行的行为保持不变。
代价是调用方必须依赖退出码或日志判断失败，不能再假设失败时一定出现窗口。
