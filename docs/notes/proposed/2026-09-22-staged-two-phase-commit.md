# 暂存目录 + 两阶段提交：把安装写入改成「先写、后换」

Status: proposed

## Problem

当前的写入模型是「装完即生效」：每个文件的下载直接落到安装目录的最终路径上，成功一个
生效一个。这带来三类后果，前两类已经用局部修补止血，第三类没有：

1. **自更新失败会丢更新器**——已由
   [自更新失败不再丢更新器](../implemented/2026-09-22-self-update-failure-keeps-updater.md)
   收掉（失败还原备份、成功才登记自删）。剩余的洞是**硬中断**：进程在两次 rename 之间
   被结束（断电、任务管理器），目标位置没有文件、旧版本只剩 `.instbak`。
2. **换文件的三步 rename 没有回滚**——已由
   [补丁换文件的三步 rename 补回滚](../implemented/2026-09-22-patch-swap-rollback.md)
   收掉进程内失败；同样剩下硬中断那一格。
3. **中断会留下混合版本**：`InstallFileMode::Direct` 是 `File::create(target)` 先截断再
   写，下载中断/被杀进程/断电都留下半截目标文件，而旧版本已经被截断掉了。多文件安装
   进行到一半时，安装目录是「一部分新版、一部分旧版」，应用启动大概率崩溃；用户并不知道
   要重跑一次安装器。
4. **删除不可回滚**：`latest_meta.deletes` 走 `IpcOperation::RmList` 直接 `remove_file`，
   结果被丢弃，没有撤销路径。
5. **取消没有落点**：没有暂存目录时，「取消」等价于留下混合版本，因此干脆没做。

上游在 `2026-09-02-atomic-file-commit.md` 里给出的解法是暂存目录 + journal + 两阶段
提交。本 note 记录把它适配到本仓库（前端驱动的 DFS 流程、Tauri 宿主、JSON IPC）的方案。

## Proposal

### 1. 暂存目录（`src-tauri/src/fs/staging.rs`，新增）

每次会话一个暂存目录，位置由安装路径确定性推导：

- 与目标同卷：`%TEMP%\kachina-staged\<h>\`，`<h>` 是规范化后的安装路径的 sha256 前
  16 个十六进制字符；
- 跨卷：`<安装目录>.kachina-staged\`（rename 必须同卷才原子，跨卷会退化成复制）。

目录布局：`new\<相对路径>` 放产出、`old\<相对路径>` 放被换下的旧文件、`dl\` 放运行库
安装器与 Mirror酱 归档、`journal` 是提交清单、`lock` 内写持有进程 pid。

`Staging::open(install_dir)` 按「同级、`%TEMP%`」顺序看候选：锁被存活进程持有则挂
`STAGING_IN_USE`；第一个带 `journal` 的候选保留，其余删掉；都没有就新建。它以
`IpcOperation::OpenStaging` 运行在有写权限的一侧。

### 2. 两阶段提交（`src-tauri/src/fs/commit.rs`，新增）

**阶段一（写入）**：所有产出文件一律写到 `new\<相对路径>`，由 `fs::create_staged_file`
创建；`install_file.rs::finalize_staged` 在 `new\` 下完成 `clear_index_mark`、
`verify_hash` 与 `File::sync_all()`，任一失败删该文件并返回错误——**目标从未被触碰**。
`progressed_hpatch(old_path, diff, diff_size, out_path, on_progress)` 只读旧文件、写
`out_path`，不再有任何改名逻辑。

**阶段二（提交）**：`IpcOperation::Commit(CommitArgs { staging_root, install_dir, journal })`，
先写 journal 并 `sync_all`，再逐单元 rename：

| 单元 | 动作 |
|---|---|
| `file` | 目标存在则 `rename(目标 → old\<rel>)`，再 `rename(new\<rel> → 目标)` |
| `dir` | 目标目录整体 `rename` 进 `old\` 再换入；失败降级为逐文件 |
| `del` | `rename(目标 → old\<rel>)`，不存在则跳过 |
| `copy` | 重解析点子树内单文件：同目录 `<名>.kachina-tmp` → 校验 → 换入，回滚用 `.kachina-old` |

每次 rename 遇 os error 32/33/5 以 50/100/200/400/800 ms 退避重试五次（安全软件扫描
新写入文件的窄窗口）。全部完成后删 journal，返回 `CommitOutcome { self_replaced }`。

阶段二内失败：按逆序复原已换单元、删 journal 与暂存目录，挂 `FILE_IN_USE`（subject 为
相对路径），安装目录回到完整旧版。

### 3. 恢复与前滚（`IpcOperation::Recover` / `DiscardStaging`）

恢复时机在 metadata 已知之后、哈希扫描之前：比对 journal 记录的目标内容与本次要装的
内容（逐单元哈希、算法、归档摘要），相等则前滚完成剩余单元，不等则整目录丢弃。会话结束
（成功、失败、取消、已是最新）一律 `DiscardStaging`。

### 4. 前端（`src/App.vue` / `src/api/ipc.ts` / `src/types.ts`）

- `diff_files` 算完之后 `OpenStaging(install_dir)`，拿到 `staging_root`；
- 每个 `InstallFile` 的 `target` 改成 `new\<rel>`，新增 `old: Option<String>` 指向安装
  目录里的当前文件（只有 patch 模式用它作基文件）；
- 全部文件装完后 `Commit`；失败/取消走 `DiscardStaging`；
- `latest_meta.deletes` 不再走 `RmList`，改为提交里的 `del` 单元。

### 5. 自更新与自镜像

更新器（`updaterName`）与卸载器（`uninstallName`）是安装器自身生成的镜像，也走同一份
journal 提交，不再有 `.instbak`。`DELETE_SELF_ON_EXIT_PATH` 只在 commit 成功且 journal
已删之后登记（沿用
[自更新失败不再丢更新器](../implemented/2026-09-22-self-update-failure-keeps-updater.md)
里收敛出的唯一写入函数 `schedule_delete_on_exit`）。

### 6. 取消

会话层交出 `CancellationToken`，在 metadata、哈希扫描、下载前后各检查一次，下载循环
`select!` 它；命中即丢弃整个暂存目录。阶段二不检查 token（只有本地 rename，秒级）。

### 有意排除

- 磁盘空间预检（`GetDiskFreeSpaceExW` + `DISK_FULL`）：本轮先不做，空间不足由写入错误
  直接暴露；
- 目录单元在重解析点子树里的复制语义按上游规则实现，但**不**为链接目标跨卷做特殊处理；
- 应用侧的版本目录布局（彻底消除阶段二窗口）：超出安装器范围；
- `rm_list` 的卸载语义不动：安装/更新路径不再删文件，卸载仍直接删。

## Alternatives considered

- 只把 `Direct` 模式改成「写 `<target>.staging` 再换入」：改动最小，但 patch 与 Mirror酱
  两条路径仍是直写，混合版本的问题原样存在；而且硬中断留下的「目标缺失 + `.instbak`」
  需要恢复逻辑，半套方案反而多一个中间态。作为过渡可以，作为终态不行。
- 用 `Drop` 守卫代替 journal：进程被结束时不执行析构，正是要防的那一格。
- 先写 `.new` 再原子 `MoveFileEx(REPLACE_EXISTING)`：运行中的 exe 被占用时替换失败，
  自更新仍然需要先改名腾位，绕不开 journal。
- 每个文件保留 `.bak` 而不是 `old\` 目录：安装目录里会出现成倍的临时文件，且
  `latest_meta.deletes` 与用户数据目录混在一起，无法一次性清理。

## Acceptance criteria

| 判据 | 期望 |
|---|---|
| 安装路径不再直写目标 | `rg -n "File::create\\(" src-tauri/src` 的命中仅限写暂存目录、写 journal、`builder/` 与测试代码；`create_target_file` / `prepare_target` 不存在 |
| 临时文件名消失 | `rg -n "instbak\|patching\|patchold" src-tauri/src` 零命中（测试除外） |
| 自删登记唯一 | `DELETE_SELF_ON_EXIT_PATH` 的写入点只有 `schedule_delete_on_exit`，且调用点只在 commit 成功与自卸载停放之后 |
| 阶段一失败不触碰目标 | 单测：第二个文件写入失败时三个目标字节不变、无 journal、暂存目录被删 |
| 阶段二失败可回滚 | 单测：第二个单元 rename 失败后三个目标等于旧文件、暂存目录已删、错误码 `FILE_IN_USE` |
| 中断可前滚 | 单测：`stop_after` 钩子在第二个单元之后中断，恢复后三个目标均为新文件、journal 消失 |
| journal 版本门 | 单测：首行不是 `kachina-journal 1` 时不执行任何 rename，暂存目录被删 |
| 暂存目录不留在安装目录 | e2e `offline-install` 断言父目录无 `.kachina-staged`；`interrupted-download` 断言中断后安装目录字节不变、无暂存残留 |
| 取消可用 | 前端单测：`Running` 阶段有取消按钮、`commit` 阶段没有；取消后暂存目录被删、目标未变 |
| 体积代价可接受 | CI 产物大小变化记录在 Verification（上游同款改动本机口径约 +95 KiB） |

## Risks

- **磁盘峰值**：从「单个文件 + 暂存」变成「现有安装 + 本次全部变更」，空间不足直接失败；
  此前能「边下边换勉强装完」的用户现在装不了。
- **`sync_all` 开销**：每个产出文件一次刷盘，数万小文件时累计秒到十秒级。
- **阶段二窗口**：仍是单元数次本地 rename，只是把最常见的全新安装缩为一次目录 rename；
  彻底消除需要应用侧版本目录。
- **前端驱动的流程**：本仓库没有上游的 Rust 会话层，暂存句柄由前端持有、写入发生在提权
  进程，`OpenStaging` / `Commit` / `Recover` / `DiscardStaging` 四个新 IPC 变体的参数与
  返回值要一并进 `tools/devcheck` 的检查，否则两侧签名漂移不会被发现。
- **`RmList` 从安装路径消失**：删除改成 rename 进 `old\`，卸载流程仍在用 `rm_list`，
  两条路径的语义差异需要在实现时写清，避免误删。
- **回滚本身失败**：复原时同样可能被占用；那时保留 journal 与暂存目录、报错，让用户
  重试或下次会话前滚。
