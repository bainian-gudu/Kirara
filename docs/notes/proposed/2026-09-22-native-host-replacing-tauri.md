# 自研宿主替换 Tauri

Status: proposed

## Problem

上游 `main`（`a52a4c66`）已经不用 Tauri 了：`native/{session,fs,ipc,module,host,capabilities,installer}`
是自研宿主，前端是 Preact + rsbuild 打成的单个自包含 HTML，资源由
`build.rs` 用 zstd 压进二进制（见
[上游「zstd 内嵌 HTML」与 WebView2 兜底在本仓库的落点](../researched/2026-09-22-frontend-embedding-and-webview2-fallback.md)）。

本仓库仍是 Tauri 宿主。由此产生的具体代价：

1. **二进制体积**：Tauri + wry + WebView2 绑定占了安装器体积的大头；上游同款改动后
   Cargo.lock 从 717 包降到 392 包（本仓库重构后已把 git 依赖与 msquic 清零，但宿主层
   仍在）。
2. **IPC 形状**：本仓库的提权通道是 JSON（`IpcOperation` + `serde(tag = "type")`），
   上游已换成 postcard 帧，并因此去掉了 serde 的 `Content` 缓冲机制（省 75 KiB）。
3. **`LOCAL_PATCHES.md` 第 1–16 节的大部分是本仓库相对上游**快照**的偏差**：一旦同步到
   上游的新宿主，注册表清理、快捷方式、计划任务、安全阀这些「本地新增」需要在新架构上
   重新落位，而不是逐条套用。
4. **测试面**：上游有 `dfs2` 批量会话、`plugin-stub`、`dump-offline-install` 等 e2e，
   本仓库因为架构不同只能挑不依赖新架构的用例（见 `LOCAL_PATCHES.md` 第 18 节）。

## Proposal

分阶段推进，每阶段单独可验收，**不**做一次性重写：

1. **前置：暂存目录 + 两阶段提交**（见
   [暂存目录 + 两阶段提交](../implemented/2026-09-22-staged-two-phase-commit.md)）。上游的宿主层
   （`session/run.rs`）与提交协议是同一个设计的两个部分，先把提交协议在本仓库落地，
   宿主替换时才不用同时改两件事。
2. **前端单文件化**：rsbuild 配置成把 JS/CSS 内联进一个 `index.html`（上游同款），
   去掉 `static/` 目录；这一步在 Tauri 下也能做，验证成本低（`pnpm build` + 打开
   `dist/index.html` 能跑），做完才有「一个文件 `include_bytes!`」的前提。
3. **宿主抽象层**：把「谁在跑会话」抽出来——进度回调、取消、错误呈现、窗口/对话框
   各留一个 trait，Tauri 先做唯一实现。这一层不改变行为，但决定了后面能不能替换。
4. **自研宿主**：`host` 模块接管窗口创建（WebView2 直接 `CreateCoreWebView2EnvironmentWithOptions`）、
   资源服务（zstd 解压后的 `index.html`）、IPC（postcard 帧）、对话框（TaskDialog）。
   Tauri 依赖在最后一步才移除。
5. **依赖再收敛**：移除 Tauri 后重跑第 7 节的遥测移除核查与依赖清单，重新记录
   `Cargo.lock` 包数、git 依赖数、二进制体积。

### 有意排除

- Preact 重写前端与 i18n 插件系统：本仓库前端是 Vue 3，语言只有中文；换框架与语言
  对「宿主替换」不是必要条件，不在本 note 范围内；
- 上游的自研 Sentry 客户端（`native/utils/sentry.rs`）：本仓库物理移除遥测，不引入；
- `dfs2` 批量会话：它是上游服务端协议的扩展，与本仓库现有的 `dfs+` 路径并行，单独评估。

## Alternatives considered

- **保持 Tauri**：最省事，代价是体积与 IPC 形状（上面的 1、2），以及永远无法取用上游
  宿主层的测试与提交协议实现。若第 1 步落地后体积与测试覆盖已够用，可以停在这里。
- **一步到位照搬上游 `native/`**：会同时改动宿主、提交协议、前端框架与 IPC 编码，
  出错时无法定位是哪一层的问题；且 `LOCAL_PATCHES.md` 第 1–16 节的本地加固要在新架构上
  重新验证，一次性做等于把所有回归风险叠在一起。
- **换成 wry（去掉 Tauri 运行时但保留 WebView2 绑定）**：能去掉 Tauri 的 IPC/插件/
  资源服务，但体积收益小于自研宿主，且仍要自己实现资源服务与 IPC。

## Acceptance criteria

| 判据 | 期望 |
|---|---|
| 前置完成 | 两阶段提交的验收判据全绿（见 [暂存目录 + 两阶段提交](../implemented/2026-09-22-staged-two-phase-commit.md)） |
| 前端单文件 | `pnpm build` 只产出 `dist/index.html`；在浏览器里直接打开能完成一次「选路径」交互 |
| 宿主可替换 | 会话层不直接引用 `tauri::` 类型；`rg -n "tauri::" src-tauri/src/{fs,installer,thirdparty}` 零命中 |
| 行为等价 | `test` job 的 10 组 e2e 在新宿主上全绿，且不修改测试断言 |
| 体积 | 自研宿主替换后记录体积与包数变化（对比口径：同一 CI runner 的产物大小 + `cargo metadata` 包数） |
| 本地加固不丢 | `LOCAL_PATCHES.md` 第 1–16 节逐条在新架构上复核，结果写回该节 |

## Risks

- **WebView2 生命周期**：Tauri 现在负责窗口、DPI、WebView2 环境与 `webview_version()`
  探测，自研宿主需要自己实现同等逻辑；缺 WebView2 的兜底对话框必须保持不依赖 WebView。
- **`LOCAL_PATCHES.md` 第 1–16 节的回归**：这些是本仓库相对上游快照的加固（注册表清理、
  删除范围安全阀、`%VAR%` 展开、多用户清理、rcedit 副本、H3 传输层、停更依赖替换），
  宿主替换会作废其中与宿主/流程相关的部分，必须逐条重做并重新验证。
- **遥测回潮**：上游宿主自带自研 Sentry 客户端，同步时容易连带引入；第 7 节的核查
  （`tools/devcheck -Layer vendor` 的 4 组断言）是唯一防线。
- **收益不确定**：体积收益要到第 4 步才显现，前 3 步本身不减小二进制；若中途停下，
  投入没有回报，只是把「先写后换」和前端单文件化这两件本身有价值的事做完了。
