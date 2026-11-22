# 本地补丁台账

本仓库是 `YuehaiTeam/kachina-installer` 的**下游分支**（交付名 `kirara-builder.exe`）。
上游源码之上有一批本地改动，本文件是这些改动**唯一的现行清单**：改了什么、落在哪、
和上游是什么关系。每次同步上游后更新。

上游同步方式与最近一次合并的逐文件处置见
[`docs/upstream-sync-plan-2026-11-22.md`](docs/upstream-sync-plan-2026-11-22.md)；
迁移期（`refactor/v0.5.2` → `upstream/main@05a1410`）的逐项证据见
[`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。

## 现行补丁

| 编号 | 本地改动 | 落点 | 与上游的关系 |
| --- | --- | --- | --- |
| L1 遥测移除 | 物理删除 Sentry 错误上报与 `77.cocogoat.cn` 使用统计：无 `sentry.rs`、无 `Breadcrumb` 管道消息、无事件号文案、无 `@sentry/cli` 与 CI 上传 | `native/utils/{crash,time,code,error,taskdialog}.rs`、`native/ipc/{mod,manager}.rs`、`native/main.rs`、`native/installer/config.rs`、`native/host/native.rs`、`package.json`、`.github/workflows/build.yml`、`locales/*.tsv` | 上游仍保留 Sentry 与 `Breadcrumb`；但上游 f2a5184 已删 envelope 管道、GitHub JS 插件、图片 base64 往返（与本地同向） |
| L2 用户协议内联 | `agreementFile`/`agreementFormat`/`agreementTitle` 打包期内联、安全渲染、接受门槛、内联弹窗布局 | `native/builder/pack.rs`、`native/session/{types,state,plan,commands,ui}.rs`、`web/{agreement.ts,panels/AgreementPanel.tsx,App.tsx,screens/Ready.tsx}`、`locales/*.tsv`、README | 上游没有 |
| L3 改名兼容 | `legacyExeNames` / `legacyUninstallNames` / `legacyProgramFilesPaths` / `extraUninstallLnkNames` / `extraUninstallRegistry` / `extraUninstallScheduledTasks`：品牌改名后识别旧目录、旧进程、旧卸载器与旧快捷方式 | `native/session/types.rs`、`native/installer/{config,registry,pick path,uninstall}.rs`、`native/session/{state,commands,run}.rs`、`native/host/{bridge,native}.rs`、`native/builder/pack.rs` | 上游没有。上游 `utils/path.rs` 解决的是 8.3 短名归一，不是品牌改名 |
| L4 安全阀 | 卸载清单越界拦截、`%TEMP%` 白名单、跨用户重放、重解析点拦截、下载后执行验签、静默结束占用进程（按当前名 + 历史名，只动安装目录内实例） | `native/session/{plan,run}.rs`、`native/fs.rs`、`native/utils/{acl,secure_temp}.rs`、`native/installer/uninstall.rs`、`native/thirdparty/mirrorc.rs` | 部分上游化：提权管道 ACL 自 f2a5184 起取上游更严格的 SDDL（`SY`+`BA`）；上游 e29a11a「把管道锁到 helper」保留 |
| L5 标准构建目标 | 目标改 `x86_64-pc-windows-msvc`：去 `-Z build-std`、去 `rust-src`、去 win7 `ctor` patch、去 `STATIC_VCRUNTIME` 兜底 | `package.json`、`rust-toolchain.toml`、`Cargo.toml`、`scripts/merge-release-bundle.mjs`、`tests/prepare.mjs`、`.github/workflows/build.yml` | 上游仍是 `x86_64-win7-windows-msvc` + `rust-src` |
| L6 H3 = quinn + rustls | 传输层换 `quinn` + `rustls` + `h3-quinn`，删 `msquic-async`/`h3-msquic-async`/`seera-msquic`/`ctor` 四个 git patch 与静态 msquic 链接；证书固定语义与接口不变；`utils/net_failure.rs` 按 quinn 错误类型分类 | `native/capabilities/h3.rs`、`native/utils/net_failure.rs`、`Cargo.toml`、`Cargo.lock` | 上游是 msquic。**每次同步都要重贴，并在 PR 里写等价性核对结论** |
| L7 zip 去 fork | 用 crates.io `zip 9`，条目名按 `name_raw()` 自行 UTF-8 解码（非 UTF-8 走 lossy） | `Cargo.toml`、`native/thirdparty/mirrorc.rs` | 上游仍用 `xytoki/zip2` fork |
| L8 左栏图与图标配置化 | `imageFile`/`iconFile` 配置驱动左栏图与 exe 图标，自带内置默认左栏图 `web/left.webp` | `native/builder/pack.rs`、`native/session/commands.rs`、`web/App.tsx`、`web/left.webp`、README | 上游无位图默认图（用内联 SVG `WizardArt` 占位） |
| L9 界面与进度页 | 复刻 `refactor/v0.5.2` 外观；进度页固定 520×280 不撑高窗口；逐文件明细只记日志（界面只显示总体字节量与速度） | `web/{layout.css,App.tsx,screens/*,ui/icons.tsx}`、`native/session/{state,run}.rs`、`native/host/native.rs` | 上游 Running 页有自己的展示；上游 f2a5184 已删逐文件动作，本地随之删掉 `FileAction`，只保留完成日志 |
| L10 交付形态 | 交付 `kirara-builder.exe` + `SHA256SUMS.txt`；Release 申请 `contents: write`；CI 关 release PDB | `.github/workflows/build.yml`、`build.ps1`、`scripts/merge-release-bundle.mjs` | 上游交付 `kachina-builder.exe`，带 Sentry 上传 |
| L11 devcheck 快速门禁 | 七层本地体检（vendor/ps1/gen/rust/logic/front/ci）+ 17 个自检用例 + `devcheck.yml` | `tools/devcheck/`、`.github/workflows/devcheck.yml` | 上游没有。`front` 层自 f2a5184 起不再跑 vitest（随上游删除前端单测） |
| L12 P6 端到端重演 | 旧包新装 / 升级 / 卸载重演工作流与 `dump-offline-install` 用例 | `.github/workflows/p6-e2e.yml`、`tests/{p6-e2e,dump-offline-install}.mjs` | 上游没有 |
| L13 未识别配置键点名 | 打包期点名配置里未被识别的键，避免拼错键静默失效 | `native/utils/config_keys.rs`、`native/builder/pack.rs` | 上游没有 |

## 维护约定

1. **只登记现行改动**：上游吸收掉的（例如第 16 项 `mslnk` → Shell Link API）从本表移除，
   在 `MIGRATION_LEDGER.md` 留一条退役记录即可，不要两处重复。
2. **每条要能定位**：写清落点文件与上游对应实现（「上游没有」也是结论）。
3. **同步后必须复核 L1/L3/L5/L6/L7**：这五项每次都会被上游改动撞到；L6 还要在 PR
   里写明 quinn 侧与上游 msquic 修复的等价性核对。
4. `tools/devcheck` 的 `vendor` 层会断言遥测没被加回来、git 依赖锁到 commit、
   npm 依赖全部来自 registry —— 改依赖时先跑它。
