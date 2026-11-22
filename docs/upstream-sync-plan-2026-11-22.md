# 上游同步方案：upstream/main `05a1410` → `f2a5184`

> 状态：**已执行**（阶段 0–3 完成，在本地分支 `sync/upstream-f2a5184` 上，未推送、未合并回 `main`）
> 生成时间：2026-11-22 ｜ 依据：本地 `main` `1c1530a`、`upstream/main` `f2a5184`

## 0. 结论摘要

| 项 | 结论 |
| --- | --- |
| 同步方式 | **正向合并**（`git merge upstream/main` 到本地 `main`），不 rebase、不重演迁移 |
| 可行性 | 可行。本地 `main` 与上游有共同祖先 `05a1410`，只需解决一次合并冲突 |
| 冲突规模 | 60 个双方都改过的路径，其中 **34 个内容冲突 + 6 个改/删冲突**，约 **85 处冲突块** |
| 冲突集中处 | `pnpm-lock.yaml`(13)、`native/session/run.rs`(11)、`native/session/state.rs`(6)、`native/ipc/mod.rs`(4)、`Cargo.lock`(4)，前 5 个占了 45% |
| 需要拍板的决策 | **2 个**：① H3 传输层保留本地 quinn+rustls 还是回上游 msquic；② 前端 vitest 测试保留还是跟随上游删除 |
| 上游已"顺手吸收"的本地补丁 | 第 16 项（`mslnk` → Shell Link API）、第 7 项的 envelope 管道 / GitHub JS 插件 / 图片 base64 往返 |
| 必须重做的本地补丁 | 第 7 项遥测（上游仍在用 Sentry + `Breadcrumb` + `@sentry/cli`）、第 13 项构建目标（上游仍是 `win7` + `rust-src`）、第 14 项 zip fork、第 15 项 H3 |
| 建议版本 | 合并后发 `v1.1.0`（含 `kachina-sfx` 新二进制与新测试矩阵） |

## 1. 现状事实（已实测）

### 1.1 仓库与基线

| 项 | 值 |
| --- | --- |
| `origin` | `git@github.com:bainian-gudu/Kirara.git` |
| `upstream` | `https://github.com/YuehaiTeam/kachina-installer.git` |
| 合并基线（merge-base） | `05a14107fc024645e1ad350fd3b4dffa62ae1368`（2026-09-25） |
| 本地 `main` | `1c1530a`，基线上 **42 个提交**，119 文件，+10,766/−2,248 |
| `upstream/main` | `f2a5184`（2026-11-20），基线上 **51 个提交**，132 文件，+10,821/−7,380 |
| 分叉计数 | `git rev-list --left-right --count main...upstream/main` → `42 51` |
| 上游 tag | 自 `0.5.1` 起未再发 tag（`git describe` = `0.5.1-138-gf2a5184`）；`v1.0.0`/`v1.0.1`/`v1.0.2` 是本仓库自己的发布 tag |
| 工作区状态 | 干净（`## main...origin/main`，无未提交改动） |

### 1.2 环境约束（本次实测，影响执行方式）

- **WSL 出网被本机 TUN 网卡劫持**：`.wslconfig` 为 `networkingMode=Mirrored`，Windows 侧 `Meta Tunnel` 适配器（`28.0.0.1/30`，route metric 1）把 WSL 默认路由抢走，直连 GitHub / 百度全部超时。
  可用姿势：命令前加 `export https_proxy=http://127.0.0.1:7897 http_proxy=http://127.0.0.1:7897 all_proxy=http://127.0.0.1:7897`（已实测 `github.com` 返回 200）。**不要把代理写进 `git config`**，只按命令注入。
- **WSL 可以承担静态门禁**：`rustup` 有 `nightly` 与 `x86_64-pc-windows-msvc` target，`cargo 1.100.0-nightly`、`node v22.23.4`、`pnpm 10.17.0` 就绪 → `cargo check/clippy --target x86_64-pc-windows-msvc`、`pnpm build`（rsbuild）可在 WSL 内跑。
- **WSL 跑不了**：`cargo test`（windows-rs 目标无法在 Linux 执行）、`cargo build --release`（需要 MSVC 链接器）、`tests/*.mjs` 行为矩阵（需要真实 Windows + 安装器）→ 这些只能交给 CI（`windows-2022`）。
- 已知沙箱问题：`workspace-write` 在本机 9P 工作区启动即失败，本会话用 `danger-full-access`。

## 2. 上游这 51 个提交做了什么（按主题归并）

| 主题 | 代表提交 | 与本地补丁的关系 |
| --- | --- | --- |
| **A. 新增 `kachina-sfx` 便携运行器**：sfx 自提权、给应用 electron-builder 式便携环境、快捷方式指向 sfx 以便固定、PE 夹具与快捷方式断言 | `9fd6ce3` `1bc3998` `0640699` `1ce1024` `e536c45` `6fb0868` `6c866bc` `88f9d91` `fbfcea7` `783c635` | 新能力，本地需全量接入（含 `merge-release-bundle.mjs` 拼包与 `build.yml` 产物） |
| **B. 安装/升级正确性**：8.3 短名统一为长形式、写后经同一句柄回读、离线包只嵌部分文件时按在线安装器处理、离线包 5 秒放弃更新检查、批量 range 与声明对齐、`>4GiB` 拒绝而非截断、`pack` 退出码 1、取消时停止哈希与 Mirror酱 解包 | `64a3c04` `385f34e` `2ed39d0` `c775273` `094c37b` `3e7ff9b` `20ed01e` `0362234` `1a651fc` `e6342bc` | 与本地第 1/1b/1c/3/6/9 项同区域，需逐 hunk 核对"上游是否已等价" |
| **C. 网络层 H3/msquic 稳定性**：从 fork 取 `h3-msquic-async` 让未命名状态的 transport close 掉失败而不是 panic、`ExitProcess` 避免 msquic 在打开连接下拆解、错误码按失败类型分类、local io code 过提权管道 | `f6d0048` `bb23ef9` `70a3824` `23f12ed` `c6a59bc` `233c725` | **与本地第 15 项直接冲突**（本地已换成 quinn + rustls），见 §3 决策 D1 |
| **D. 遥测与死代码清理**：删 `PipeMsg::Envelope` 管道、删 GitHub JS 插件、删图片 base64 往返、删客户端 alias、删 `PipeMsg::Disconnect` | `c4aff58` `fec120b` | 与本地第 7 项同向，冲突取"更彻底的删除"；但上游**仍保留** `sentry.rs` / `Breadcrumb` / `@sentry/cli` |
| **E. 文档与测试基础设施**：笔记只写契约/权衡/被推翻的决策、`proposed/*` 迁入 `implemented/*`、新增 `offline-update-check` / `dfs2` 分片 / `sfx-run` 用例、**删除整个 `web/__tests__`（vitest）** | `470436c` `ff40791` `8ec6e29` `e739e59` `4bea89f` | 文档直接取上游；测试取舍见 §3 决策 D2 |

## 3. 需要你拍板的两个决策

### D1 · H3 传输层：保留 quinn+rustls，还是回上游 msquic？

| | 方案 A（建议）保留本地 quinn + rustls | 方案 B 回上游 msquic |
| --- | --- | --- |
| 冲突处理 | `native/capabilities/h3.rs` 取本地（928 行），仅移植上游**错误分类**语义 | 取上游（851 行），丢弃本地整个重写 |
| 依赖 | 无 `[patch.crates-io]`、无静态 msquic 链接、无 4 个 git fork（`msquic-async-rs` ×2、`rust-ctor`、`seera-msquic`），构建更稳、供应链更短 | 恢复 4 个 git fork 依赖；上游 fork 可获取性受网络影响（本机 WSL 出网已需代理） |
| 已发布状态 | v1.0.0–v1.0.2 均基于 quinn 交付，行为已验证 | 需要重新验证整条 H3 下载路径 |
| 风险 | 上游 3 个 msquic 专属修复（`f6d0048` 未命名状态失败、`bb23ef9` 避免 panic 拆解）在 quinn 下不适用 —— 必须**逐条确认 quinn 侧不存在同类失败模式**，并补测试 | 回到上游实现，语义与上游完全一致 |

**建议 A**，理由：本地已是 quinn 且已发布验证；上游 fork 是个人仓库分支，可用性不可控。但必须在合并 PR 里写明"quinn 侧的等价性核对结论"，否则以后没人知道这笔账。

### D2 · 前端测试：保留本地 vitest，还是跟随上游删除？

- 上游删掉了 `web/__tests__/*`（`fixtures.ts`、`render.test.tsx`、`i18n-keys`、`plugin-host`、`setup.ts`）以及 `vitest`/`@testing-library/preact`/`jsdom` 三个 devDeps。
- 本地反而**新增**了 `web/__tests__/agreement.test.ts`（114 行，协议渲染/接受门槛）。
- 现状：本地 `package.json` 的 `test` = `vitest run`，上游没有；但上游重构了 `web/types.ts`(−84)、`web/state.ts`、`web/screens/Running.tsx`，本地 fixtures 必然要跟着改。

**建议保留**（本地是 web 层唯一的自动化覆盖），代价是合并时要重做 `fixtures.ts` 以对齐新组件 API，并接受"上游已放弃这块、以后每次同步都要自己维护"的长期成本。若你想减负，也可以跟随上游删除。

## 4. 执行步骤（分阶段，每阶段有门禁）

> 全程在 WSL 内执行；涉及网络的命令先注入代理三件套。

### 阶段 0 · 准备（不改代码，可回退）

```bash
cd /home/tushanhonghong/workspace/Kirara
export https_proxy=http://127.0.0.1:7897 http_proxy=http://127.0.0.1:7897 all_proxy=http://127.0.0.1:7897

# 0.1 当前状态打底（不含 .git 的完整快照，便于整体回退）
git bundle create ../Kirara-before-upstream-sync-20261122.bundle --all
git tag -a pre-upstream-sync-20261122 -m "同步上游 f2a5184 之前的本地 main 快照" main

# 0.2 建同步分支（不动 main）
git switch -c sync/upstream-f2a5184 main

# 0.3 记录基线，写进 UPSTREAM.md
git rev-parse upstream/main   # f2a5184...
```

**门禁**：`git status` 干净；bundle 与 tag 存在；分支为 `sync/upstream-f2a5184`。

### 阶段 1 · 先合并、后修（一次性产生冲突清单）

```bash
git merge --no-ff --no-commit upstream/main
git status --short | grep -E '^(UU|AA|DU|UD|AU|UA|DD)' | wc -l   # 期望 40
```

**门禁**：冲突数 = 40，与本文 §5 表格一致；不一致说明上游又动了，重跑 §1 的统计。

### 阶段 2 · 按"低风险 → 高风险"顺序收冲突

| 顺序 | 冲突簇 | 处置规则 | 涉及路径 |
| --- | --- | --- | --- |
| 1 | **锁文件** | 两个都**不手工合并**：`git checkout --ours Cargo.lock pnpm-lock.yaml`，等源码落定后 `cargo metadata` / `pnpm i` 重生成并提交新锁 | `Cargo.lock` `pnpm-lock.yaml` |
| 2 | **文档** | 取上游（`--theirs`），再把 `MIGRATION_LEDGER.md` 里指向被删 `proposed/*.md` 的链接改到 `implemented/*.md` | `README.md` `docs/notes/**` |
| 3 | **locales** | 取上游 + 补本地新增键（协议/图标/进度页），`pnpm run devcheck` 的 i18n 层会校验键对齐 | `locales/*.tsv` |
| 4 | **构建与依赖** | 保留本地：标准目标 `x86_64-pc-windows-msvc`、去 `rust-src`、去 `ctor`/`msquic`/`seera-msquic` patch、`zip 9` + `name_raw()` 解码；同时接入上游新增脚本（`offline-update-check`、`test:online-install-dfs2-sliced`、`test:sfx-run`） | `package.json` `Cargo.toml` `rust-toolchain.toml` |
| 5 | **CI** | 保留本地 `build.yml` 的 5 处改动（`CARGO_PROFILE_RELEASE_DEBUG`、去 `rust-src`、target 改标准、`kirara-builder.exe` 改名、Release `permissions: contents: write` + 校验值 + 去 Sentry 上传），并合入上游新增 job/步骤 | `.github/workflows/build.yml` |
| 6 | **遥测（第 7 项）** | 保留删除：`native/utils/sentry.rs` 保持删除；`utils/mod.rs` 取"本地模块集 ∪ 上游新增（`net_failure`/`path`/`shell_link`）− `sentry`"；`ipc/mod.rs` 的 `PipeMsg` 取上游形态**再去掉 `Breadcrumb`**（`Envelope`/`Disconnect` 上游已删）；`ipc/manager.rs`、`main.rs`、`installer/config.rs` 里的 sentry 调用点整段删除，上游新加的 `net_failure` 分类逻辑保留 | `native/utils/{sentry,mod,code,error,taskdialog}.rs` `native/ipc/{mod,manager}.rs` `native/main.rs` `native/installer/config.rs` |
| 7 | **快捷方式（第 16 项）** | 取上游：`native/utils/shell_link.rs` + 瘦身版 `installer/lnk.rs`；台账第 16 项从"本地补丁"改为"上游已吸收" | `installer/lnk.rs` |
| 8 | **卸载/安全阀（第 1/3/6/9 项）** | 以上游为骨架，逐 hunk 核对本地语义是否已等价：清单路径越界拦截、`%VAR%` 展开、跨用户重放、`%TEMP%` 白名单、卸载前结束占用进程、提权管道锁到 helper、重解析点拦截、下载后执行验签。**每一条都要么保留，要么在 PR 里写明"上游等价实现位于 X"** | `installer/uninstall.rs` `utils/acl.rs` `host/native.rs` `thirdparty/mirrorc.rs` |
| 9 | **session 层** | 冲突最密（11+6 处）：以**上游重构后的结构**为准（`plan`/`run`/`state`/`types`/`ui` 的职责切分变了），把本地两处语义**重新贴回去**：① 安装/更新/卸载静默结束占用进程（`8b4af64`）② 进度页固定窗口内收敛（`40017cd`） | `native/session/*.rs` `native/builder/pack.rs` |
| 10 | **H3（第 15 项）** | 按 D1 结论：取本地 `capabilities/h3.rs`，把上游错误分类/io code 过管道的改动手工移植 | `native/capabilities/h3.rs` |
| 11 | **前端** | 上游重构 + 本地外观复刻（`layout.css` 177 行、`agreement.ts` 323 行、`AgreementPanel`、`left.webp`、`ui/icons.tsx` 删 16 行）都要在**上游新结构**上重做；`web/types.ts` 上游 −84 行是类型收敛，取上游再补本地新增类型 | `web/**` |
| 12 | **测试** | 保留本地 `tests/dump-offline-install.mjs`、`tests/p6-e2e.mjs`、`web/__tests__/*`（若 D2 选保留），同时接入上游 `tests/sfx-run.mjs` 等新增；`tests/{prepare,utils,registry}.mjs` 取上游 | `tests/**` |

### 阶段 3 · 静态门禁（WSL 内）

```bash
cargo metadata --format-version 1 > /dev/null          # 锁文件与 patch 段自洽
cargo fmt --all -- --check
cargo check  --all-targets --target x86_64-pc-windows-msvc
cargo clippy --all-targets --target x86_64-pc-windows-msvc -- -D warnings   # 本地第 11 项要求 0 warning
pnpm i --frozen-lockfile
pnpm build                                             # rsbuild 前端 + 产物合并脚本
node tools/devcheck/...                                # 本地快速门禁（见 .github/workflows/devcheck.yml 的调用形式）
```

**门禁**：全部 0 退出；`clippy` 0 warning；`pnpm build` 产出 `kirara-builder.exe` 且体积大于 cargo builder（脚本自带断言）。

### 阶段 4 · 提交与 CI 全绿

1. 分主题提交（建议 6–10 个提交：`chore(sync)` 锁文件 / `refactor(telemetry)` / `fix(security)` / `feat(sfx)` / `chore(web)` / `docs(sync)` 等），每个提交说明"取谁、为什么"。
2. 推送同步分支 → 开 PR（**需要你明确同意才推**）→ 等 `Build`（15 job）、`Devcheck`、`P6 E2E` 全绿。
3. 手工验收（CI 覆盖不到）：WebView2 界面渲染、UAC 提权弹窗、OneDrive 重定向目录安装、真实游戏进程占用下的静默结束、sfx 双击与固定到开始菜单。

### 阶段 5 · 版本与交付

```bash
# 合并到 main 后
# 1. 版本：0.1.0 保持（上游未改），发布 tag 用 v1.1.0
# 2. 更新 UPSTREAM.md：基线 → f2a5184、同步日期、新增/退役的本地补丁
# 3. 更新 MIGRATION_LEDGER.md：第 16 项标注"上游已吸收"，新增"上游 f2a5184 同步"一节
```

下游 `HoYoEnhance` 需要的 `kirara-builder.exe` 从 `v1.1.0` Release 取，校验值按既有约定（`sha256sum -c`）。

### 阶段 6 · 回退

| 场景 | 回退动作 |
| --- | --- |
| 阶段 2 中途放弃 | `git merge --abort`（或 `git reset --hard pre-upstream-sync-20261122`） |
| 已提交但 CI 不过 | `git switch main && git branch -D sync/upstream-f2a5184` |
| 已发布后发现问题 | 从 `pre-upstream-sync-20261122` 重发 `v1.0.3` 补丁版 |

## 5. 逐文件冲突处置表（40 条）

| 路径 | 冲突类型 | 冲突块 | 取值 |
| --- | --- | --- | --- |
| `pnpm-lock.yaml` | 内容 | 13 | 重生成 |
| `native/session/run.rs` | 内容 | 11 | 上游结构 + 本地语义 |
| `native/session/state.rs` | 内容 | 6 | 上游结构 + 本地语义 |
| `native/ipc/mod.rs` | 内容 | 4 | 上游 − `Breadcrumb` |
| `Cargo.lock` | 内容 | 4 | 重生成 |
| `docs/notes/proposed/2026-09-21-network-speed-and-offline-progress.md` | 内容 | 3 | 上游 |
| `package.json` | 内容 | 2 | 本地（目标/脚本）+ 上游新增脚本 |
| `native/utils/acl.rs` | 内容 | 2 | 本地安全阀 + 上游 |
| `native/thirdparty/mirrorc.rs` | 内容 | 2 | 本地 `name_raw()` + 上游 |
| `native/session/types.rs` | 内容 | 2 | 上游结构 + 本地字段 |
| `native/ipc/manager.rs` | 内容 | 2 | 去 sentry 分支 |
| `native/installer/uninstall.rs` | 内容 | 2 | 本地安全阀 + 上游 |
| `native/host/native.rs` | 内容 | 2 | 去 `capture_anyhow` |
| `native/builder/pack.rs` | 内容 | 2 | 上游 |
| `docs/notes/implemented/2026-09-02-frontend-preact-renderer.md` | 内容 | 2 | 上游 |
| `README.md` | 内容 | 2 | 上游 + 本地品牌名 |
| `Cargo.toml` | 内容 | 2 | 本地（去 patch、标准目标） |
| `web/types.ts` `web/state.ts` `web/App.tsx` `web/layout.css` `web/screens/Running.tsx` | 内容 | 各 1 | 上游结构 + 本地面板/样式 |
| `native/utils/mod.rs` | 内容 | 1 | 并集 − `sentry` |
| `native/utils/code.rs` | 内容 | 1 | 去 `event_id`/`should_report` |
| `native/session/ui.rs` `plan.rs` `commands.rs` | 内容 | 各 1 | 上游结构 + 本地语义 |
| `native/main.rs` | 内容 | 1 | 去 sentry 初始化 |
| `native/installer/lnk.rs` | 内容 | 1 | 上游（`shell_link.rs`） |
| `native/capabilities/h3.rs` | 内容 | 1 | 本地（D1） |
| `locales/en-US.tsv` `zh-CN.tsv` | 内容 | 各 1 | 上游 + 本地新增键 |
| `docs/notes/implemented/2026-09-02-ui-contract.md` | 内容 | 1 | 上游 |
| `native/utils/sentry.rs` | 改/删 | — | 保持删除 |
| `docs/notes/proposed/2026-09-21-download-planner.md` | 改/删 | — | 上游（已迁 `implemented/2026-09-21-download-planner.md`） |
| `docs/notes/proposed/2026-09-21-optional-performance-optimizations.md` | 改/删 | — | 上游（删除） |
| `tests/dump-offline-install.mjs` | 改/删 | — | 本地保留 |
| `web/__tests__/fixtures.ts` | 改/删 | — | 按 D2 决定 |
| `web/__tests__/render.test.tsx` | 改/删 | — | 按 D2 决定 |

## 6. 风险登记

| 风险 | 影响 | 缓解 |
| --- | --- | --- |
| 上游新增 `kachina-sfx` 二进制，拼包脚本 `merge-release-bundle.mjs` 与 `build.yml` 产物清单要一起改 | 打包出来少/多文件，下游静默失败 | 阶段 3 的 `pnpm build` 自带"bundle 必须大于 builder"断言；再手工核对产物 |
| 遥测"回潮"（`sentry.rs` 被别人 merge 回来） | 隐私契约破裂 | 阶段 2 第 6 步整段删除 + 加一条 `devcheck` 断言（`rg sentry native/ web/` 必须为空） |
| H3 选 quinn 后，上游 msquic 专属修复的等价性无法自动验证 | 特定 transport close 场景可能 panic | 补一条 `h3` 单测：未命名状态关闭时返回错误而非 panic；在 PR 里写核对结论 |
| `session/*` 冲突密集（17 处），语义重贴易漏 | 静默结束进程 / 进度页收敛退化 | 把 `8b4af64`、`40017cd` 两个提交的 diff 当作 checklist，逐条对照 |
| WSL 无直连网络 | 依赖安装、fetch 失败 | 代理三件套按命令注入；`cargo` 走代理 + 国内镜像 |
| 行为矩阵只能在 CI 跑 | 本地无法自证"能装能卸" | 阶段 4 必须等 CI 全绿；本机只做静态门禁 |
| 本地 quark 式补丁无台账（散在 42 个提交里） | 下次同步重复摸索 | 阶段 5 建 `LOCAL_PATCHES.md` 汇总"当前有效的本地补丁 + 上游对应关系" |

## 7. 同步后的长期机制（建议）

1. **每个上游 tag / 每月同步一次**：`git fetch upstream` → `git merge-tree --write-tree main upstream/main` 先试算冲突数（本方案 §1 的方法），冲突少就当场合，冲突多再排期。
2. **`UPSTREAM.md` 增加"最后同步"行**：记录 `upstream/main` 提交 + 日期 + 本次退役/新增的本地补丁编号。
3. **`LOCAL_PATCHES.md`（新建）**：把"本地改了什么、为什么、上游对应实现是什么"变成唯一事实源，替代现在散落在 42 个提交说明和 62KB 台账里的信息。
4. **可选：`schedule` 定时任务**每月 1 日跑一次"上游分叉试算"并把结果写成注释/消息；需要你点头再建。
5. **可选：`sync-check` workflow** 在 `main` 上每周跑 `git merge-tree` 并把冲突文件数写进 job summary，避免再次攒到 51 个提交才发现。

## 8. 需要你确认的事项

1. **D1 H3**：按建议保留 quinn + rustls？（还是回上游 msquic）
2. **D2 前端测试**：按建议保留本地 vitest？（还是跟随上游删除）
3. **执行范围**：只要方案，还是让我开始跑阶段 0–3（本地合并 + 静态门禁，全部在 `sync/` 分支上，不推送、不动 `main`）？
4. **推送与 PR**：阶段 4 的推送 / 开 PR 需要你单独一句"推"。

## 9. 执行记录（2026-11-22）

| 阶段 | 结果 |
| --- | --- |
| 0 准备 | 快照 `../Kirara-before-upstream-sync-20261122.bundle`（4.5 MB）+ tag `pre-upstream-sync-20261122`；分支 `sync/upstream-f2a5184` |
| 1 合并 | `git merge --no-ff upstream/main`：39 处未合并（33 内容 + 5 改删 + 1 删改），与预估的 40 处一致 |
| 2 收冲突 | 全部收完：锁文件重生成、文档取上游、locales 取上游后补回协议 3 键并去掉事件号文案、遥测调用点整段删除、`FileAction` 随上游删除只留完成日志、H3 取本地并把 `utils/net_failure.rs` 按 quinn 错误类型重写、前端保持本地展示 |
| 3 静态门禁 | `cargo check --all-targets --target x86_64-pc-windows-msvc` **0 error / 0 warning**（WSL + `cargo xwin` 0.23.1 + clang 21 + lld）；`tsc --noEmit` 通过；`rsbuild build` 通过；`cargo fmt --check` 仅格式差异（仓库历来不跑 fmt 门禁） |
| 3 已知偏差 | `cargo clippy -- -D warnings` 有 7 条**合并前就存在**的老 lint（`h3.rs`、`config.rs`、`registry.rs`、`uninstall.rs`、`state.rs`、`secure_temp.rs`），CI 与 devcheck 都不跑 clippy，本次未顺手改以缩小 diff |
| 3 验证技巧 | WSL 侧类型检查需要 `cargo-xwin`（MSVC SDK 从微软 CDN 下载）；第三方 `vendor/rcedit-rs` 的 `case reinterpret_cast<...>` 在 clang 下非法，检查时**临时**改成数字常量、检查后已还原（`git diff vendor/` 为空） |

### 与原方案的差异

- D2 选“跟随上游”：除 `web/__tests__/fixtures.ts`、`render.test.tsx` 外，本地 `agreement.test.ts` 与 `vitest.config.ts` 的依赖也一并删除；`tools/devcheck` 的 `front` 层与一条自检用例同步去掉 vitest。
- 上游在 `native/session/run.rs` 里给 Sentry 事务埋了点（`Transaction::timed`），本地没有 sentry 模块，合并时把两处 `txn.timed(...)` 拆成平铺调用。
- 本地原有的逐文件动作日志（`FileAction`）依赖上游已删除的协议字段，改为按新 `Snapshot` 只记完成行。
- 附加产物：新增 [`LOCAL_PATCHES.md`](../../LOCAL_PATCHES.md) 登记现行 13 项本地补丁；`UPSTREAM.md` 增加“最后同步”与合并方式；`MIGRATION_LEDGER.md` 增加本次同步小节并把第 16 项标注为上游已吸收。

