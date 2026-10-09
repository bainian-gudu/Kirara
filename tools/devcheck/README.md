# tools/devcheck — 不跑完整构建的本地体检

本仓库的完整构建要 nightly Rust、`x86_64-pc-windows-msvc`、pnpm 全家桶与 LTO，
本地跑一次好几分钟，CI 更久。结果是「改一行 Rust / TS，只能靠一次完整构建来发现
写错了」。

`devcheck` 解决这个问题：把**我们真正改过、且出事代价最大的那部分代码**放进最小
的检查环境里，用几秒到十几秒给出「会不会编译失败 / 安全阀有没有被改坏」的答案。

```powershell
# 仓库根目录
pwsh tools/devcheck/devcheck.ps1                 # 跑 all（vendor ps1 gen rust logic front ci）
pwsh tools/devcheck/devcheck.ps1 -Layer vendor,logic
pwsh tools/devcheck/devcheck.ps1 -SelfTest       # 自检：注入 17 个错误，确认每层都会报错
pwsh tools/devcheck/devcheck.ps1 -SkipInstall    # 缺工具链时 SKIP 而不是自动安装
```

任何一层失败 → 退出码 1。缺工具链的层标记 `SKIP` 并给出提示，不算失败。

这套检查也在 CI 里**自动执行**：`.github/workflows/devcheck.yml`（push 到 main、
任何 PR、手动触发；ubuntu + windows 双 runner）依次跑 `-Layer vendor` → `all` →
`-SelfTest`。完整的打包工作流 `build.yml`、端到端重演 `p6-e2e.yml` 见 `.github/workflows/`。

## 分层

| 层 | 检查什么 | 需要的工具 | 热跑耗时 |
| --- | --- | --- | --- |
| `vendor` | **源码只用仓库内的快照**：不是 submodule、快照完整、工作流与打包脚本里没有任何从外部拉源码 / 下二进制的动作、git 依赖锁到 commit、npm 依赖全来自 registry、遥测没被加回来、ARP 卸载命令行能被自己的 CLI 解析、停更依赖没回归 | pwsh 7 | ~1s |
| `ps1` | 仓库里全部 `.ps1` 的语法（PowerShell Parser） | pwsh 7 | <0.2s |
| `gen` | 从 `native/` 按名字抽出 logic 层要断言的 item | pwsh 7 | ~0.3s |
| `rust` | 根 crate 的 `cargo check --target x86_64-pc-windows-msvc --all-targets`（含 lib/bin/单测目标） | cargo + MSVC（`cl.exe`） | 首次数分钟，之后 ~1s |
| `logic` | 被抽出来的安全阀与纯逻辑的**行为断言**（160 条）：路径越界、删目录 / 删快捷方式 / 删计划任务 / 环境变量展开 / 跨用户重放 / `%TEMP%` 白名单、安装计划归一化与模板展开、未识别配置键点名、协议内联、包体 PE 识别、嵌入名规则、解包路径越界、zip 条目名解码、微软签名判定、H3 证书固定（用 openssl 交叉验证过的样例证书） | cargo | 首次 ~15s，之后 ~0.4s |
| `front` | 仓库自己的 `tsc --noEmit`（`web/` 全量 strict）+ `vitest run` + 协议渲染文件的 prettier | node + pnpm | ~10s |
| `ci` | 工作流里 action 的版本不低于下面登记的下限、每一层都真的接进了 `devcheck.yml`、构建入口与交付名一致 | pwsh 7 | ~1s |

全套热跑 ≈ 15 秒（`rust` 依赖 Windows + MSVC，缺平台时直接 SKIP）。

## `vendor` 层：源码只从本仓库拉

本仓库就是上游安装器的**源码快照**（仓库根即源码），构建必须完全基于它。这一层把
这条约束变成可执行的断言（十项，任何一项不满足就失败）：

1. 仓库根不存在 `.gitmodules`（源码不是 submodule）
2. 快照完整：`Cargo.toml` / `Cargo.lock` / `package.json` / `pnpm-lock.yaml` /
   `build.ps1` / `scripts/merge-release-bundle.mjs` / `native/main.rs` /
   `native/builder/pack.rs` / `native/installer/uninstall.rs` / `native/session/plan.rs` /
   `native/utils/config_keys.rs` 都在
3. `.github/workflows/*.yml`、`scripts/*.mjs`、`build.ps1` 里**没有任何**从外部拉取的
   动作：`releases/download`、`release-downloader`、`git clone`、`git submodule`、
   `Invoke-WebRequest`、`Invoke-RestMethod`、`DownloadFile`、`curl`、`wget`
   （只扫可执行内容，`#` 注释行与 `<# #>` 块跳过）。
   唯一的例外是 `p6-e2e.yml` 里 `workflow_dispatch` 的「冻结旧安装包地址」默认值 ——
   那是手动触发时喂给 `tests/p6-e2e.mjs` 的测试夹具输入，不是构建期 / 发布期的下载。
   例外按内容精确放行，且放行规则一旦匹配不上就报错（避免豁免悄悄失效）。
4. `build.yml` 确实调用 `pnpm build`、断言拼接体大于 cargo 裸 builder、交付
   `kirara-builder.exe` 并上传 artifact；会话 dump 的用例在 `unit-test` job 里用
   debug 产物跑（`cfg(debug_assertions)` 之后才有 dump）
5. 每个 `git = "..."` cargo 依赖都在 `Cargo.lock` 里锁到 40 位 commit
   （否则 CI 可能拉到漂移的分支），且没有一个指向上游仓库
6. npm 依赖全部是 registry 版本，没有 `git:` / `http:` / `github:` / `file:` / `link:` 形式
7. `rcedit-rs` 的 vendored 副本完整（含两份 LICENSE 与 `LOCAL_PATCHES.md`），
   `rescle.cc` 里没有 `std::locale::empty(`（MSVC 14.51 / VS 2026 已移除该非标准扩展，
   `windows-latest` 上必然 `error C2039`），`rcedit` 依赖是 `path` 形式，
   `Cargo.lock` 里不再出现 `git+https://github.com/Devolutions/rcedit-rs`
8. **遥测已物理移除，不许回归**（上游把安装器错误上报到 Sentry，前端还往统计端点
   POST 使用事件；本项目两条通道都拔了）。四组断言：
   - `native/utils/sentry.rs` 不许再出现；
   - `Cargo.toml` 不许再声明 `sentry` / `sentry-tracing` / `whoami`，`Cargo.lock` 里
     不许再锁 `sentry*` / `whoami` / `hostname` / `os_info` / `debugid`（依赖删了但
     lock 没重新生成时这条会响）；`package.json` 不许有 `@sentry/*`，
     `pnpm-lock.yaml` / `pnpm-workspace.yaml` 里不许有 `@sentry/` 条目；
   - Rust + 前端源文件**剥掉行注释**后不许出现 `sentry::` / `sentry_tracing` /
     `capture_anyhow` / `add_breadcrumb` / `start_transaction` / `configure_scope` /
     `sendInsight` / `getInsightBase` / `evCache`；
   - 兜底：整棵树的文本文件（不含 `.md`）里不许出现上报域名 `cocogoat`（DSN 与
     统计端点都带它）
9. **写进 ARP 的卸载命令行必须能跑**：`UninstallString` 的值必须整体被引号包住
   （默认装在 `Program Files\` 下，不加引号时「应用和功能」会按第一个空格把命令
   截断成 `C:\Program`）；`QuietUninstallString` 里出现的每个 `-x` / `--xxx` 都要在
   `native/cli/mod.rs` 的 `OPTS` 表里真的声明过，且至少要有一个选项 —— 否则它跟
   `UninstallString` 没区别，静默卸载会弹界面
10. **停更 / 无保障的依赖不许回归**：`mslnk` / `nt_version` 换成了系统 API，
    msquic 系换成 `quinn` + `rustls`，zip 换回 crates.io 的 `zip` + 自己的条目名解码；
    `Cargo.toml` / `Cargo.lock` 里出现这些名字就报错。顺带断言
    `libs/{THIRDPARTY.md,hdiff-sys/LICENSE,hpatch-sys/LICENSE}` 都在 —— 那两份
    vendored 的 HDiffPatch 源码是 MIT，许可证必须随源码分发

第 5、7、8 项扫 `Cargo.toml` / `rescle.cc` / 源码时都会**先剥掉注释**：这些文件里的
注释本身就会写出「原为 `git = "..."`」「原为 `std::locale::empty()`」这类说明文字，
不剥掉就会自己误报自己。第 8 项的域名兜底还额外**跳过 `.md`**，并跳过
`tools/devcheck`、`.github`、`.agents` —— 那些地方写的是「哪些东西已被移除」，
不是随产物发布的代码。

> CI 仍然会联网取 crates.io / npm registry / rustup 工具链 / marketplace action ——
> 那是任何构建都免不了的；这一层保证的是**安装器本身**只来自本仓库。

## `logic` 层：抽出来的是哪些函数

清单在 `lib/Generate.ps1`，每个 item 都必须在源文件里找得到，否则 `gen` 层直接
抛错 —— 上游重命名 / 删除时 devcheck 立刻失败，而不是静默少测。抽取用「先把字符串
与注释挖空、再做括号配对」定位 item 边界，比按下标切片稳。

被抽出来的东西覆盖三类：**决定删什么的**（`native/installer/uninstall.rs` 的删目录 /
删快捷方式 / 删计划任务安全阀、跨用户重放白名单）、**决定放行什么路径的**
（`native/fs/staging.rs`、`native/session/plan.rs`、`native/builder/extract.rs`）、
以及**判定不可逆结论的**（微软签名、H3 证书固定的 SPKI 哈希、嵌入名保留字、
未识别配置键）。Windows 专有的 IO / 注册表 / 计划任务实现不在抽取范围内。

两处跨平台桩：`has_reparse_point`（真实实现调 Win32 API，桩按路径里有没有
`REPARSE` 判定），以及用 `SystemRoot` 环境变量驱动的 `is_under_system_root`（用例
自己设 `USERPROFILE` / `SystemRoot`，并在 `main()` 开头把其它环境变量清干净，
免得起因于本机环境的假失败）。

H3 证书固定的期望值由 openssl 算出，样例证书是自签的 EC prime256v1（CN=`devcheck.example`）。
复现命令（证书在 `src/main.rs` 里以 hex 内嵌）：

```bash
openssl x509 -in dc.crt -pubkey -noout | openssl pkey -pubin -outform DER | openssl dgst -sha256 -r
```

## `front` 层：为什么直接跑仓库自己的检查

旧架构下前端要装一整套 Tauri / Vue 依赖，所以 devcheck 自带一个最小 TS 工程。
现在前端就是仓库根的一份 Preact + TS 源码，根 `tsconfig.json` 才是构建真正用的
那份 —— 再维护第二份最小工程只会与它漂移。所以这一层直接跑
`pnpm exec tsc --noEmit`、`pnpm test`（vitest）以及协议渲染文件的 prettier。

prettier 只查 `web/agreement.ts`：`web/` 其余文件沿用上游既有格式，全量检查会把
上游代码的风格当成回归。

## `ci` 层：CI 不变量

工作流里 action 的版本下限：`actions/checkout` ≥ v4、`actions/setup-node` ≥ v4、`actions/upload-artifact` ≥ v4、`actions/download-artifact` ≥ v4、`pnpm/action-setup` ≥ v4、`Swatinem/rust-cache` ≥ v2、`softprops/action-gh-release` ≥ v2。

低于下限会在 runner 上打 Node 弃用告警；升级工作流时同步改这一行，`ci` 层按这一行
断言。除版本下限外，这一层还断言：`all` 集合里的每一层都在 `devcheck.yml` 里真的
跑过（新加一层却忘了接进工作流，本地和 CI 都会「绿」，那层等于没写）、`devcheck.yml`
里有 `-SelfTest` 步骤、`build.ps1` / `build.yml` / `p6-e2e.yml` 三处的「拼出 bundle
→ 断言它比 cargo 裸 builder 大 → 以 `kirara-builder.exe` 交付」一致、
`package.json` 的 `build` 脚本三步（前端产物 → 原生 release 产物 → 拼接）齐全。

## `-SelfTest`：证明这套检查不是空壳

检查工具最大的风险是「跑通了但其实什么都没查」。`-SelfTest` 先正常生成一次，然后
逐个注入错误，确认对应的层真的会失败；任何一层「注入了错误却没报错」= 自检失败。
17 个用例覆盖上面每一层，包括：源码变成 submodule、工作流里出现外部拉取、ARP 用了
不存在的选项、`rescle.cc` 用回 `locale::empty()`、遥测调用 / 依赖 / 上报域名回到树里、
停更依赖被写回、抽取清单里的 item 被改名、删目录安全阀被放宽、Rust 类型错误、
TS 类型错误、协议净化被改坏（只有 vitest 抓得到）、action 版本低于下限、
工作流不再跑 `all` 集合、`build` 脚本丢掉拼接一步。

`-SelfTest` 会临时改写**仓库里的真实文件**来验证各层真的会报错，因此有两个保护：

1. **仓库改动锁**（`tools/devcheck/.repo-lock`）：同一工作区里同时只允许一个自检
   进程。第二个进程会明确报「另一个 devcheck 正持有仓库改动锁（PID …）」而不是互相
   污染出一堆假失败。锁是原子改名创建的，拿锁进程被杀也不会留下永久锁（下次运行会
   看到 PID 不在了，删锁继续）。
2. **磁盘备份 + 启动清场**（`tools/devcheck/.selftest-backup/`）：每次注入前把原始
   内容落盘。进程被杀（Ctrl+C、CI 取消）后，下次运行会先按备份恢复被改的文件再开工；
   正常结束时备份目录会被删掉。

两个目录都在 `.gitignore` 里。`rust` 层的用例在非 Windows 上会记为「跳过」而不是
「通过」—— 这一层本来就只有 Windows 上跑得起来。

## 跨平台的坑

- **stdout / stderr 必须并发读**。`Invoke-Native` 用
  `ProcessStartInfo.RedirectStandardOutput/Error` 起子进程，两个流各自
  `ReadToEndAsync()` 之后再 `WaitForExit`：串行读在 Windows 上会死锁（管道缓冲只有
  4 KB，写满就互相等）。
- **stderr 一律排在 stdout 后面**。并发读的两个流分别读完再拼接，时间顺序会错位，
  所以两边都非空时插一行「以上 stdout / 以下 stderr（顺序不代表先后）」。
- **换行统一 LF**（`.gitattributes` 的 `* text=auto eol=lf`）。自检按内容做字符串
  替换来注入错误，CRLF 会让「注入点没匹配上」变成假失败；`Write-GeneratedFile`
  写生成物时也统一转成 LF。
- **`-Layer` 参数故意用 `[string]` 而不是 `[string[]]`**：
  `pwsh -File devcheck.ps1 -Layer vendor,logic` 用数组类型会把 `"vendor,logic"`
  当成一个值。
- **npm / npx / node 会打 DEP0040（punycode）、DEP0169（url.parse）这类弃用告警**，
  与本仓库无关。`devcheck.ps1` 设了 `NODE_NO_WARNINGS=1`，子进程继承。

## 日志里哪些 Warning / error 是正常的

- `cargo check` 的 `warning:` 会被汇总成一句「（N 条 warning）」；`rust` 层不因
  warning 失败，但条数会打出来，方便发现新增。
- `logic` 层跑 `resolve_agreement` 时会打印 `Warning: failed to read agreement file …`
  —— 那几组断言用的就是「配置文件里指向的文件不存在」这种输入，属于预期噪音。
- `-SelfTest` 期间每一层都会打印真正的报错，并在前面标一行
  「↓ 接下来这段报错是故意注入的」。看到这段报错才说明那一层没被架空。
- `front` 层首次运行会先 `pnpm install --frozen-lockfile`，输出较长属正常；
  只想跳过安装用 `-SkipInstall`。

## 维护约定

- 新增一层：在 `devcheck.ps1` 的 `$allLayers` 里登记、在 `switch` 里接上、在
  `devcheck.yml` 里确保被裸调用覆盖，然后加一个 `-SelfTest` 用例证明它会失败。
  `ci` 层会检查前三件事。
- 抽取清单（`lib/Generate.ps1`）里的 item 改名或删除时，**不要**顺手把清单改掉
  了事：先确认那确实是有意的行为变化，再同步清单，并在提交信息里写清原因。
- `vendor` 层的放行规则只有 `p6-e2e.yml` 那一条。要新增放行必须写清理由，并保证
  规则能匹配到内容（匹配不上会报错）。
