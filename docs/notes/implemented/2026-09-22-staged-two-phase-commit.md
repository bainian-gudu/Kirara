# 暂存目录 + 两阶段提交

Status: implemented

## Problem

安装器此前把每个文件直接写到安装目录：`Direct` 会先截断目标再写，`Patch` 在
`target.patching` / `target.old` 之间做三步 rename，Mirror酱逐文件 `File::create`。
下载中断、进程被结束或断电时，目标可能只剩半截文件、旧文件被改名为 `.old`，或者
一次多文件更新留下「一部分新版、一部分旧版」的混合目录；删除清单也直接 `remove_file`，
没有撤销路径。自更新失败还会把更新器的最后一份旧拷贝留在错误的位置。

本仓库的安装流程由前端驱动，和上游原生宿主不同：没有 Rust `session` 层持有暂存句柄，
但安装目录、提权进程和文件下载仍完全在本地。实现因此把暂存目录与提交协议做成四个 IPC，
让前端只在阶段一和阶段二之间传递路径。

## Decision

### 暂存目录

`src-tauri/src/fs/staging.rs` 已同步上游 `native/fs/staging.rs` 的选址与打开逻辑：

- 与安装目录同卷时使用 `%TEMP%\kachina-staged\<路径哈希>`，避免用户在安装目录旁
  看到暂存文件；跨卷时退回 `<安装目录>.kachina-staged`，保证 rename 不跨卷；
- 打开时同时检查同级目录与 `%TEMP%` 候选，保留第一个带 journal 的目录，清理无
  journal 的残留；旧版同级 `<安装目录>.kachina-staged` 仍能被发现并恢复；
- `enter_neutral_cwd`、`same_volume`、`free_space`、`scratch_file` 与路径安全函数
  `is_safe_rel` / `join_rel` 与上游一致。

- `new\` 放阶段一产出，`old\` 放被换下的旧文件，`dl\` 放 Mirror酱归档；
- `journal` 记录哈希算法、可选归档摘要、Kirara 兼容版本行和上游单元格式；
- `lock` 记录持有进程 pid；打开时通过 `OpenProcess` +
  `GetExitCodeProcess` 判断 pid 是否仍存活。

有 journal 时保留暂存内容，交给恢复流程；没有 journal 的残留目录在重新加锁后清空
`new\` / `old\` / `dl\`，避免上次阶段一的半成品混进下一次提交。恢复版本或内容不匹配
时整目录丢弃，前端会重新打开一个干净目录再继续。

### 阶段一：只写暂存

`fs::create_staged_file` 统一创建暂存文件并自动创建父目录；`progressed_hpatch` 只读
安装目录里的旧文件，把补丁结果写到 `out_path`，不再有 `.patching`、`.patchold`、
`.old` 或 `.instbak`。Direct、Patch、HybridPatch 和 Mirror酱解压都走同一条路径，
写完校验并 `sync_all`。HybridPatch 的 `<target>.hybrid-base` 在补丁后必须删除，
删不掉就让本次安装失败，不能把临时基文件当成产品文件提交。

`InstallFileArgs` 增加 `old: Option<String>`：只有补丁模式需要它，指向安装目录里的
当前文件；`target` 始终是暂存 `new\` 下的输出。

### 阶段二：逐单元提交

`src-tauri/src/fs/commit.rs` 已同步上游的 journal v1、目录单元、复制单元、重试和恢复
状态机。提交前扫描 `new\`，对每个文件计算 SHA-256，并记录安装目录中旧文件的
SHA-256（不存在则为空）；删除单元也记录旧摘要。前端仍只传 `version + deletes`，
后端兼容层据此构造与上游相同的 journal。journal 写盘并 `sync_all` 后再开始 rename：

| 单元 | 动作 |
|---|---|
| `file` | 目标存在则先 rename 到 `old\`，再 rename `new\` 到目标 |
| `dir` | 干净目录整体 rename 到 `old\`，再换入 `new\` 下同名目录；根目录支持不存在或为空 |
| `copy` | 仅用于 reparse point 子树，在链接目标同卷内复制并校验，再改名换入 |
| `del` | 目标存在则 rename 到 `old\` |

每次 rename 对 os error 32 / 33 / 5 做 50、100、200、400、800 ms 退避重试。目录
单元在提交前会重新检查干净度，用户在等待期间放入文件时降级为逐文件提交。进程内
失败按逆序恢复已提交单元；恢复失败时保留 journal、`new\`、`old` 并返回
`ROLLBACK_FAILED`，前端也不会再无条件删除暂存目录。

### 恢复与自更新

`Recover` 先校验 journal 版本和哈希算法，再按上游状态机逐单元判断：

- 目标已经是新摘要：已完成；
- 目标仍是旧摘要且暂存新文件存在：前滚；
- 目标缺失、`old\` 有旧文件且新文件丢失：恢复旧文件并丢弃暂存；
- 其他情况：视为目录已被用户或外部程序改变，丢弃暂存并重新扫描。

全部提交完成后删除 journal；若本次换掉了正在运行的安装器，保留暂存目录，把退出自删
登记到暂存根，避免在自更新成功前删掉唯一旧镜像。其他情况立即清理暂存。

提权模式下提交跑在 `headless-uac` helper 里，helper 没有 UI 窗口，不会触发
主窗口的 `WM_CLOSE` 清理路径。`uac_ipc_main` 在管道线程结束后主动调用一次
`delete_self_on_exit`，让已登记的暂存根由提权 cmd 在 helper 退出后删除。

### 前端流程

`src/App.vue` 在 metadata 和哈希算法确定后 `OpenStaging`；有 journal 时先
`Recover`。恢复完成就直接收尾，恢复返回未完成则重新 `OpenStaging` 再扫描。所有
`InstallFile` 的目标改为 `staging\new\...`，补丁额外传安装目录旧路径；下载完成后
`Commit`，失败/取消时 `DiscardStaging`。Mirror酱也走同一套 IPC，归档下载到
`staging\dl\`，删除清单交给 `Commit`。

## Alternatives considered

- 只在 `Direct` 模式写临时文件：补丁、HybridPatch 和 Mirror酱仍会留下混合版本，不能
  覆盖「一次更新整体生效」这条保证。
- 使用 `Drop` 守卫清理：进程被任务管理器结束或断电时析构不会执行，正是要处理的场景。
- 每个文件单独保留 `.bak`：安装目录会被临时文件污染，且删除清单仍不可回滚。
- 不移植上游完整 `session` 层：Kirara 仍由 Vue 前端驱动 JSON IPC，直接覆盖
  `native/session` 会同时改变 IPC、安装计划和恢复时机，风险远高于本次需要的内核同步。
- 继续使用同级暂存根：跨卷场景仍需它保证 rename 同卷，但同卷时不再把暂存目录暴露在
  安装目录旁。

## Verification

| 判据 | 结果 |
|---|---|
| 安装路径不再直写目标 | PASS：运行路径已改用 `create_staged_file`，`prepare_target` / `create_target_file` 不再被安装流程调用 |
| 阶段一产出统一落到 `new\` | PASS：Direct / Patch / HybridPatch / Mirror酱解压均传暂存路径 |
| journal v1、目录/复制单元与摘要 | PASS：`fs/commit.rs` 单测 `journal_roundtrip_and_version_gate` 覆盖版本、算法、目录/复制单元与格式错误 |
| 提交可换入、可删除、整目录替换 | PASS：上游 `commit_swaps_three_files_and_removes_journal`、`root_unit_missing_and_empty_install_dir`、`dir_unit_degrades_when_no_longer_clean`、`copy_unit_via_junction` 已保留 |
| 后续单元失败可回滚已换文件 | PASS：`commit_rolls_back_when_second_unit_is_locked` 断言错误码、旧内容和暂存清理 |
| 中断后可前滚 | PASS：`interrupted_commit_recovers_forward` 覆盖 journal 保留、前滚和暂存清理 |
| 新文件丢失/目录被改可恢复或丢弃 | PASS：`recovery_with_new_dir_deleted_rolls_back_swapped_units`、`recovery_discards_when_target_was_overwritten` |
| 暂存锁不会抢占活进程 | PASS（结构）：`Staging::open` 使用 `OpenProcess` + `GetExitCodeProcess`，无 journal 的残留才清空 |
| 前端恢复不继续使用已删除路径 | PASS：恢复未完成时重新 `OpenStaging`；回滚失败时保留 staging |
| 提权 helper 退出清理 | PASS（结构）：`uac_ipc_main` 退出时调用 `delete_self_on_exit`，devcheck [26] 同时断言主窗口与 helper 两条路径都在 |
| 本地类型检查 | PASS：`bash tools/devcheck/check-installer.sh --tests` |
| 前端 SFC / tsc | PASS：`devcheck -Layer front` |
| Windows 真实文件单测 | PASS：CI run `35713140127` 的 `cargo test --bin kachina-installer --locked` |

## Consequences

- 阶段一不再触碰安装目录；取消或下载失败只会留下可丢弃的暂存目录。
- 提交阶段需要再次顺序读取暂存文件和旧目标来计算 SHA-256，换来恢复时能区分
  「已完成、待前滚、旧文件丢失、目标被改」四种状态；大安装的提交会多一轮本地 IO。
- 同卷安装的暂存目录不再出现在安装目录旁；跨卷安装仍使用同级目录，因为 rename 必须
  同卷。正常成功、明确失败和版本不符都会清理，回滚失败则有意保留给下一次恢复。
- 目录单元把全新安装和整目录替换从 N 次 rename 缩短为一次；散落更新仍是逐文件提交。
- 复制单元只用于用户自行建立的 reparse point 子树，避免 rename 跨链接目标卷。
- 当前仍不移植上游完整 session 层，因此取消按钮、Rust 状态机和安装计划仍由现有前端
  负责；这不影响提交协议的正确性。
