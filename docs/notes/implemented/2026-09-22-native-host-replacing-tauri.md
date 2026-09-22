# 原生 Win32 + WebView2 宿主替换 Tauri（C10）

Status: implemented

## Problem

C9 落地后，本仓库的安装器仍由 Tauri 提供窗口、WebView2 生命周期、资源协议和命令
分发。Tauri 及其 GTK/WebKit/wry 传递依赖占据 `Cargo.lock` 717 个包中的大部分，
前端还要依赖 `@tauri-apps/api` 的 `__TAURI__` 注入；窗口、IPC 和资源服务的行为被
框架封装，无法直接复用上游原生宿主已经验证过的 WebView2 处理方式。

本次替换的目标是把宿主换成仓库自维护的 Win32 + WebView2 实现，同时保留现有 Vue
前端、前端驱动的 JSON IPC 与 C9 两阶段提交，避免把宿主替换和安装会话重写叠在一起。

## Decision

### 前端单文件与 IPC 桥

`rsbuild.config.ts` 开启 `inlineScripts` / `inlineStyles`、`all-in-one` chunk 与
10 MiB data URI 上限。生产构建只生成 `dist/index.html`，供 `build.rs` 压缩嵌入。

`src/host.ts` 直接使用 WebView2 的 `chrome.webview.postMessage` 和 `message` 事件，
保留原来的 `invoke` / `listen` / `Event<T>` / `getCurrentWindow` 形状；原
`src/tauri.ts` 及 `@tauri-apps/api`、`@tauri-apps/cli` 已删除。

### 原生宿主

`src-tauri/src/host/` 分为五个模块：

| 文件 | 职责 |
|---|---|
| `mod.rs` | Win32 消息循环、`HostHandle`、`UiAction`、启动看门狗 |
| `window.rs` | 窗口类、DPI、Mica、主题、图标、关闭清理 |
| `webview.rs` | WebView2 环境/controller、资源协议、主题背景、DevTools |
| `bridge.rs` | 前端命令分发到现有 Rust 函数 |
| `assets.rs` | 解压并返回 zstd 内嵌的 `index.html` |

`src-tauri/build.rs` 将 `dist/index.html` 压缩到 `OUT_DIR/index.html.zst` 并生成
`ui_assets.rs`；缺少前端产物时生成空资源表，避免干净检出上的 `cargo test` /
`cargo check` 被构建脚本阻塞。`resources/app.rc` 与 `app.manifest` 继续由
`embed-resource` 嵌入 exe。`src-tauri/src/main.rs` 的 `tauri_main` 改为
`native_main`，原 `#[tauri::command]`、`AppHandle` / `State` / `WebviewWindow`
参数全部移除；`ManagedElevate` 的进度事件经 `HostHandle.emit` 回到主线程。

窗口创建后由前端调用 `window_show` 显示；`WM_SIZE` 会同步 WebView2 controller
边界，`WM_CLOSE` 仍调用 `delete_self_on_exit` 保留 C9 的自更新清理语义。

### 依赖收敛与守卫

`Cargo.toml` 去掉 `tauri`、`tauri-build`、`tauri-utils`，改用与现有
`windows 0.61` 对齐的 `webview2-com 0.38`，并加入 `zstd` 与 `embed-resource`
构建依赖。`Cargo.lock` 从 717 个 `[[package]]` 降到 511 个；Tauri/wry/GTK/WebKit
条目清零。511 仍高于上游原生重构的 392，因为本仓库保留 H3/quinn、russh、
aws-lc 等本地依赖，不能直接套用上游包数口径。

`tools/devcheck` 的 `vendor` 层新增 Tauri 依赖、`@tauri-apps`、旧 `src/tauri.ts`
和 `tauri.conf.json` 回归断言，并检查 rsbuild 保持单文件内联；`logic` 层把
自更新退出清理断言从 Tauri `CloseRequested` 改到原生 `WM_CLOSE`。

## Alternatives considered

- **保留 Tauri，只做前端单文件化**：体积和依赖树没有实质改善，也无法取用原生
  WebView2 生命周期控制。
- **照搬上游 `native/`、Preact 与 session 层**：会同时改变前端框架、IPC 编码和
  安装会话，C9 及第 1～21 节加固需要整体重验，风险大于宿主替换本身。
- **用 wry 替换 Tauri 运行时**：仍保留框架资源协议和 IPC 形状，收益小于直接控制
  WebView2 controller。

## Verification

| 判据 | 结果 |
|---|---|
| 前端单文件 | PASS：`pnpm build:frontend` 后 `dist` 只有 `index.html` 与 `.gitkeep`；`index.html` 204,441 字节（gzip 85.1 KB） |
| TypeScript | PASS：`pnpm exec tsc --noEmit -p tsconfig.json` |
| Windows 目标类型检查 | PASS：`bash tools/devcheck/check-installer.sh --bin kachina-installer`（stub MSVC，仅类型/不链接） |
| devcheck 主体 | PASS：`vendor,ps1,gen,rust,logic,front,ci` 全绿；logic `PASS 250 / FAIL 0` |
| Tauri 依赖清理 | PASS：`Cargo.lock` 717 → 511 包，`rg` 无 `tauri` / `wry` / `@tauri-apps` |
| 宿主桥接文件 | PASS：`src/host.ts` 与 `src-tauri/src/host/{mod,window,webview,bridge,assets}.rs` 存在，旧 `src/tauri.ts` / `tauri.conf.json` 不存在 |
| 前端产物可嵌入 | PASS：`build.rs` 生成 `index.html.zst` 与 `ui_assets.rs` |
| 对话框父窗口 | PASS（结构）：`HostHandle::dialog_parent` 把原生 `HWND` 传给 `rfd`，覆盖选目录、错误、确认与启动看门狗 |
| 旧图标转换清理 | PASS：原生宿主直接使用 Win32 `ExtractIconExW`，无调用的 `utils/icon.rs` 已删除 |
| Windows 实机运行 | 待 CI：MSVC 链接、LTO、WebView2 资源协议、窗口缩放与 10 组安装行为测试 |

## Consequences

- 安装器运行期不再加载 Tauri 运行时；窗口、资源和命令分发都由本仓库代码控制，
  `Cargo.lock` 少 206 个包，依赖面明显收窄。
- 前端直接打开浏览器时没有 `chrome.webview` 桥，只有 WebView2 宿主内能执行
  选路径和安装命令；静态预览仍可查看页面，但不代表完整安装流程。
- 原生宿主需要自行维护 DPI、Mica、主题、窗口缩放和 COM 生命周期；本次已覆盖
  当前安装器的窗口尺寸与主题路径，后续新增原生能力时应继续在 `host/` 内实现。
- 上游若同步新的 `native/` 布局，第 22 节只能作为宿主替换参考；是否采用其
  session/Preact 协议需要单独评估，不能直接覆盖本仓库的 C9 与本地加固。
