# 上游「zstd 内嵌 HTML」与 WebView2 兜底在本仓库的落点

Status: researched

> 2026-09-22 更新：C10 已按
> [原生 Win32 + WebView2 宿主替换 Tauri](../implemented/2026-09-22-native-host-replacing-tauri.md)
> 实施，`dist/index.html` 已由 `src-tauri/build.rs` 用 zstd 嵌入。以下正文保留当时在
> Tauri 宿主下的调研结论，其中「暂不做」只代表当时状态。

## Problem

上游 `build.rs` 做两件与本仓库体积/健壮性有关的事，评估其中哪些能在当前（Tauri 宿主）
架构下直接取用：

1. 把 `dist/index.html` 用 zstd 压成 `index.html.zst` 后 `include_bytes!` 进二进制，
   运行时按路径从一张 `match` 表里取（`native/build.rs`）；
2. 缺 WebView2 时用 TaskDialog 而不是依赖 WebView 的对话框兜底。

## Findings

**1. zstd 内嵌 HTML 的前提在本仓库不存在。** 上游那套的前提是「前端被 rsbuild 打成
单个自包含 HTML + 一张 `i18n.tsv`」，然后由**自研宿主**自己提供资源协议。本仓库走
`tauri.conf.json` 的 `frontendDist: ../dist`，由 `tauri-build` 把整个目录编译进二进制、
由 Tauri 的资源协议按路径服务；`dist/` 现在是 `index.html` + `static/`（228 KB，其中
`static/` 220 KB）。

- Tauri 自己的压缩是 `tauri-utils` 的 `compression` feature（`brotli`，见
  `~/.cargo/registry/.../tauri-utils-2.*/src/assets.rs`），本仓库的 `tauri` 依赖
  只开了 `wry` / `devtools`，**没有**开 `compression`，所以 228 KB 是原样进二进制的。
- 但把 `compression` 打开要连带把 brotli 解压器编进来，而本仓库已经在依赖树里有
  `zstd`（`async-compression` 的 zstd 用于 DFS 负载）。用 zstd 替掉 Tauri 的资源
  服务等于自己实现一套 `Assets`，与框架正面冲突；只开 brotli 又不一定净减体积。
- 结论：这一步不是「小件」，它属于宿主替换（见
  [自研宿主替换 Tauri](../implemented/2026-09-22-native-host-replacing-tauri.md)）的
  一部分——只有不再用 Tauri 服务前端时，才轮到「压成一个文件自己 `include_bytes!`」。

**2. WebView2 兜底已经在本仓库里。** `main.rs` 在 `tauri::webview_version()` 失败时把
命令改成 `Command::InstallWebview2`，由 `module/wv2.rs::install_webview2()` 处理，而它
全程不依赖 WebView：

- 进度对话框是裸 `TaskDialogIndirect`（`TASKDIALOGCONFIG` + `TDF_SHOW_MARQUEE_PROGRESS_BAR`）；
- 所有错误路径走 `rfd::MessageDialog`（Windows 后端是原生 MessageBox/TaskDialog）；
- 引导器下载到 `utils/secure_temp` 的临时文件、验签后才执行；
- 另有启动看门狗：5 秒内没收到 `APP_BOOT_SIGNAL` 就用原生对话框报
  「Initialization failed due to webview2 fault」再退出。

唯一与上游的差别是文案语言（上游英文、本仓库中文）和「没有进度对话框的取消按钮接线」。

## No action

本调研只做了分析，未规划也未实施任何修改。

- 第 1 项（zstd 内嵌 HTML）不做：它的前提是自研宿主，归入
  [自研宿主替换 Tauri](../implemented/2026-09-22-native-host-replacing-tauri.md) 的前置条件。
- 第 2 项（WebView2 兜底）已经满足，无需改动；若将来把 `rfd` 依赖去掉，需确认替代的
  错误呈现同样不依赖 WebView。
