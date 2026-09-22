# 补丁换文件的三步 rename 补回滚

Status: implemented

## Problem

`fs.rs::progressed_hpatch` 在补丁算完之后用三步 rename 把新文件换到目标上：

1. `rename(target → target.old)`
2. `rename(target.patching → target)`
3. `remove_file(target.old)`

三步之间没有任何回滚。第二步失败（目标被杀软、编辑器或另一个进程按住）时磁盘状态是
「目标不存在 + 旧文件在 `target.old` + 新文件在 `target.patching`」，函数直接
`?` 抛错；此时用户手上没有一个文件名正确的旧文件，安装器报的错也说不清东西去哪了。
进程恰好在这一步被结束，残留同样如此。

同一函数里还有两处同源问题：

- 第三步失败会把整次安装判成失败，可新文件其实已经就位了——调用方据此回滚一次成功的
  更新，或者让多文件安装停在半路。
- 返回码非 1 时先 `remove_file(new_target)?`：删不掉半成品就报
  `REMOVE_NEW_TARGET_ERR`，把「补丁本身失败」这个真正的原因盖掉。

## Decision

- 第二步 rename 失败时立刻 `rename(target.old → target)` 还原旧文件，并删掉
  `.patching` 半成品，再抛原始错误；还原本身也失败时记 error 并说明旧文件在哪。
  「目标是正在运行的 exe」那条分支同样处理，只是旧文件可能没被移动过（自更新路径下
  原名由 `.instbak` 备份承担），用 `moved_old` 记住是否真的动过。
- 第三步删 `.old` 失败降级为 `warn`：新文件已经就位，留一份垃圾不该让安装失败。
- 返回码非 1 时清理 `.patching` 降级为 `warn`，把 `PATCH_FAILED_ERR` 报出去。
- 函数开头补一次残留恢复：`.old` 存在而目标不存在（上次换到一半被结束进程）就把
  `.old` 改回原名；两者都在则说明 `.old` 是上次没删掉的残留，清掉。

## Alternatives considered

- 先写 `<target>.new` 再一次性 rename：效果一样，但补丁输出本来就在 `.patching`，
  多一次复制没有收益。
- 把 `.old` 留着不删、下次启动清理：多一份常驻垃圾，且没有解决「目标缺失」这个真正
  的中间态。
- 直接上暂存目录 + 两阶段提交（见上游 `docs/notes/implemented/2026-09-02-atomic-file-commit.md`
  对应的问题域）：那是彻底解法，会重写全部写入路径；本轮先把已存在的中间态收干净。

## Verification

| 判据 | 结果 |
|---|---|
| 第二步 rename 失败后目标存在且等于旧文件 | PASS（结构）：`if let Err(e) = rename(new_target, target_cl)` 分支内先 `rename(old_target, target_cl)` 再返回 |
| 第三步失败不再让安装失败 | PASS：`remove_file(old_target)` 改为 `if let Err(e) { tracing::warn! }` |
| 返回码非 1 时报 `PATCH_FAILED_ERR` | PASS：`remove_file(new_target)` 改为 `if let Err(e) { tracing::warn! }`，随后仍返回 `PATCH_FAILED_ERR` |
| 上次中断的 `.old` 被还原 | PASS（结构）：函数开头 `stale_old` 分支 |
| `cargo test --bin kachina-builder --locked` 与 e2e | 见 CI（Build / unit-test 两个 job） |

## Consequences

- 换文件的失败窗口从「目标可能永久缺失」缩到「毫秒级的两条 rename 之间」；进程恰在此
  刻被结束留下的 `.old` 会在下次补同一个文件时被自动还原。
- `.old` 是本次补丁的临时名，用户自己在目标旁边放一个同名 `.old` 文件会在补丁时被清掉
  ——这与改动前的行为一致（成功路径本来就会删它）。
- 还原失败时错误信息里给出 `.old` 的完整路径，用户仍可手工改名恢复。

> 后续的 [暂存目录 + 两阶段提交](2026-09-22-staged-two-phase-commit.md) 已把补丁输出
> 改到暂存目录，`progressed_hpatch` 不再执行本篇描述的三步换文件；本篇保留为那次止血
> 的决策记录。
