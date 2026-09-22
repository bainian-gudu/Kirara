# 本地扫描改单趟枚举并看见不受管文件

Status: implemented

## Problem

`fs.rs::check_local_files` 是安装计划的唯一输入，它把「本地已有哪些文件」算成
`Vec<Metadata>`，做法是：遍历安装目录的每一个文件，对每个文件**线性扫一遍元数据清单**，
每次比对都 `path.to_lowercase().replace("\\", "/")` 与
`file.to_lowercase().replace("\\", "/")` 各分配一次字符串。

两个后果：

- **代价**：几万文件的安装目录（`node_modules` 类）乘上千条清单，是几千万次字符串分配，
  且全部发生在异步工作线程上。
- **语义**：比对用的是整串 `ends_with`，清单里的 `d.dll` 会被 `.../ad.dll` 命中，
  前端拿到一个哈希对不上的假条目；反过来，安装目录里**不在清单里**的文件对计划完全
  不可见——旧版本残留、用户自己放进去的文件都不会出现在任何输出里。

同一函数还有一处会崩的写法：

```rust
if res.is_err() && writable { return Err(res.err().unwrap()); }
let hash = res.unwrap();
```

文件存在但既读不动也写不动（被占用 / 权限不足）时 `writable` 为 false，第一个分支不
进，`res.unwrap()` 直接 panic 在 blocking 线程上，整次扫描以 `HASH_THREAD_ERR` 收场，
前端连「哪些文件被占用」都拿不到。

## Decision

- 清单先归一化成一个 `HashSet`（小写 + `/` 分隔），每个目录项也归一化一次，然后按
  **路径组件**逐级去掉前缀查表：单趟 O(1) 查表，且 `d.dll` 只认名为 `d.dll` 的文件。
- 返回值改为 `LocalScan { files, unmanaged }`：`files` 语义不变（`file_name` 仍是绝对
  路径，前端继续 `replace(source, '')`）；`unmanaged` 是本地存在但不在清单里的文件，
  相对安装目录、小写、`/` 分隔。
- 哈希阶段不再 panic：读不动又写不动时只标 `unwritable` 并把 `hash` 留空（前端据此
  走「文件被占用，是否继续」的确认），能读却读失败才把错误抛出去。
- 前端 `App.vue` 的调用点改为读 `local_scan.files`，并在 `unmanaged` 非空时记一条 warn
  （列出前 20 条），让「装完还剩奇怪文件」这类反馈有线索。

## Alternatives considered

- 保持 `ends_with` 语义、只把清单做成表：能拿到 O(1)，但字符串后缀误命中还在。
- 把 `unmanaged` 做成 `Metadata` 条目混进 `files`：前端要按哈希空值过滤，且会让
  「清单里有几条」这个语义变模糊。
- 直接把 `unmanaged` 的文件删掉（当成旧版本残留清理）：用户自己放进安装目录的文件
  会被无提示删除，风险远大于收益。
- 按上游做法一并做「目录单元」规划（干净目录整目录替换）：那是暂存目录 + 两阶段提交的
  一部分，本轮只把可见性做出来，`unmanaged` 正是那件事的输入。

## Verification

| 判据 | 结果 |
|---|---|
| 不再有按清单线性扫描的循环 | PASS：`file_list.iter().for_each` 与 `ends_with` 均已消失 |
| `unmanaged` 能列出不受管文件 | PASS（结构）：非命中分支按 `source` 前缀去掉后 push 相对路径 |
| 读不动又写不动不再 panic | PASS：`res` 改为 `match`，`!writable` 时 `hash` 留空 |
| 前端类型与调用点同步 | PASS：`devcheck -Layer front`（tsc --strict + SFC 编译）通过 |
| 安装行为回归（下载计划不变） | 见 CI 的 e2e（`online-install` / `online-update` / `userdata-ignore`） |

## Consequences

- 比对语义收紧后，清单里写 `sub/foo.dll` 而磁盘上是 `foo.dll` 的情况会被判为「本地没有」
  并重新下载——这与前端本来就按相对路径严格比对的行为一致，此前 Rust 侧的宽松匹配只会
  产出前端用不上的假条目。
- `unmanaged` 包含安装器自己的产物（卸载器、更新器、Mirror酱 归档）与用户数据目录，
  目前只进日志，不参与任何删除决策。
- 扫描仍是全目录遍历，哈希仍是读盘瓶颈；本 note 只消除了比对侧的多余分配。
