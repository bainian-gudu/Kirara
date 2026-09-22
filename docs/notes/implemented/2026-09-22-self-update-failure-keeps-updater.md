# 自更新失败不再丢更新器

Status: implemented

## Problem

自更新时 `fs.rs::prepare_target` 先把正在运行的 exe 改名成 `<exe>.instbak` 腾出原名，
**并在同一处**把 `installer/uninstall.rs` 的 `DELETE_SELF_ON_EXIT_PATH` 设成这份备份。
从这一刻起磁盘上已经没有原名文件，而备份就是旧版本的最后一份拷贝。

紧接着的任何失败——网络中断、`verify_hash` 不符、磁盘满、目标被占用——都只是把错误
往上抛：`prepare_target` 设下的登记留着不动，用户关窗时 `host` 主循环调
`delete_self_on_exit()`，`del /f /q <exe>.instbak` 把备份也删掉。更新器与旧版本同时
消失，用户手上既没有能用的更新器也没有备份，只能重新下载安装包。

同一根因的三个变体：

- 直写模式（`InstallFileMode::Direct`）自更新：`File::create(target)` 先截断再写，
  失败留下半截 exe，备份照删。
- patch 模式：`progressed_hpatch` 返回非 1 或 `RUN_HPATCH_ERR` 时只删 `.patching`，
  不还原 `.instbak`，备份照删。
- Mirror酱 归档解压（`thirdparty/mirrorc.rs`）走到与当前 exe 同名的条目时自己写一次
  `DELETE_SELF_ON_EXIT_PATH`，随后 `File::create(out_path)` 失败或后面任何一个条目
  失败，同样是备份照删。

## Decision

**登记自删与「换掉 exe」分离，登记推迟到整次安装确实成功之后。**

- `fs.rs::prepare_target` 只做改名，返回备份路径；它不再碰
  `DELETE_SELF_ON_EXIT_PATH`。返回值是调用方必须收尾的义务，写在函数文档注释里。
- 新增 `installer/uninstall.rs::schedule_delete_on_exit` 作为静态量的唯一写入函数，
  并在注释里写清两个合法调用点（自更新/自卸载成功之后）。
- 新增 `fs.rs::commit_self_update_backup`（成功：登记退出自删）与
  `fs.rs::rollback_self_update_backup[_sync]`（失败：删掉可能写了一半的目标，把备份
  改回原名）。回滚删不掉备份时把备份留在磁盘上并记 error —— 名字带 `.instbak` 的旧
  安装器仍然可用，比两份都没有强。
- `ipc/install_file.rs` 的两条入口（`ipc_install_file`、`install_file_by_reader`）拆成
  「外层 prepare + 收尾」与「内层干活」两半，内层不再感知备份；外层用
  `finalize_self_update` 统一收尾，成功与失败各走一条路。
- `thirdparty/mirrorc.rs::run_mirrorc_install_sync` 同样拆成外层收尾 + 内层解压：备份
  路径由内层写进 `&mut Option<PathBuf>`，外层按解压整体结果登记或回滚。

## Alternatives considered

- 保留原结构，只在每个 `?` 前面补还原：入口里失败点有十几处（三种模式 × 下载/解包/
  哈希/清理），漏一处就等于没修。拆成「外层收尾」后失败点收敛成一个。
- 用 `Drop` 守卫在析构时回滚：`rollback` 需要文件 IO，`Drop` 里只能同步做，且
  `#[tokio::main]` 退出路径上的析构顺序不可控；显式收尾更容易读懂。
- 干脆不再改正在运行的 exe，自更新交给一个外部进程：那是彻底解法，但安装器本身没有
  第二个可执行文件可用（卸载器是它生成的，更新时可能还不存在）。本轮先把「失败不丢
  东西」做掉。

## Verification

| 判据 | 结果 |
|---|---|
| `DELETE_SELF_ON_EXIT_PATH` 的写入点全库唯一，且不在 `fs.rs` / `mirrorc.rs` 里 | PASS：`rg -n "DELETE_SELF_ON_EXIT_PATH" src-tauri/src` 只命中 `installer/uninstall.rs` 的静态量、`schedule_delete_on_exit`、`delete_self_on_exit` 与卸载流程内的两处读取 |
| `prepare_target` 不再写登记 | PASS：函数体内已无 `DELETE_SELF_ON_EXIT_PATH`，改由调用方拿返回值收尾 |
| 失败路径还原旧文件 | PASS（结构）：`finalize_self_update` 的 `Err` 分支调 `rollback_self_update_backup`；Mirror酱 路径的 `Err` 分支调 `rollback_self_update_backup_sync` |
| `cargo test --bin kachina-builder --locked` 与 e2e | 见 `tools/devcheck` 与 CI（Build / unit-test 两个 job） |

## Consequences

- 自更新失败后磁盘上留下的是「旧版本原地可用」的状态，用户重跑更新器即可；代价是失败
  时多一次 rename，且原名文件在两次 rename 之间仍有一个毫秒级缺口（进程恰在此刻被强制
  结束，备份会留在 `.instbak` 上，不会消失）。
- 多分块安装（`install_file_by_reader` 被 multipart/multichunk 复用）里，每个分块各自
  收尾：只有真正换掉 exe 的那个分块会登记自删，其它分块拿到 `None` 直接透传结果。
- 备份还原成原名后，`.instbak` 不再存在；下次自更新会重新创建它。回滚是幂等的。
