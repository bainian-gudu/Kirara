# kachina-installer 上游来源

本仓库是**上游源码 + 本项目本地修改**。本文件记录迁移基线与来源；逐项改动状态见
[`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。

| 项目 | 值 |
| --- | --- |
| 上游仓库 | https://github.com/YuehaiTeam/kachina-installer |
| 迁移基线 | `main` 提交 `05a14107fc024645e1ad350fd3b4dffa62ae1368`（2026-09-25） |
| 最后同步 | `main` 提交 `f2a5184`（2026-11-20），2026-11-22 合并（51 个上游提交） |
| 上游最新 tag | 仍是 `0.5.1`（`ae461aa9`）；`f2a5184` 为未发版的 `main` 头 |
| 用途 | 构建 `kirara-builder.exe`，供下游 HoYoEnhance 打包安装器 / 更新器 / 卸载器 |

## 为什么把源码放进仓库

安装器只允许 Kachina 一种实现。把打包工具源码放进仓库后：CI 不再依赖上游 Release
的可下载性；打包工具版本随本项目一起进版本控制，可复现、可审计；本地一条命令即可
从零构建（`pwsh build.ps1`）。

## 分支关系

`main` 从上游 `main` 的 `05a1410` 拉出，本地修改是它之上的独立提交（迁移期间的隔离
分支名是 `codex/migrate-upstream-main`，完成后改名 `main` 并设为默认分支）。与本仓库
旧分支（上游 `0.5.1` 快照 + 本地补丁，`18f9553`，原名 `refactor/v0.5.2`）**没有共同
Git 祖先**，因此旧分支的本地修改是按行为逐项移植，而不是合并或 cherry-pick。

**后续同步是普通合并**：`main` 与上游已有共同祖先，直接
`git fetch upstream && git merge upstream/main`。2026-11-22 已把 `f2a5184` 合并进来
（40 处冲突，决策与逐文件处置见 [`docs/upstream-sync-plan-2026-11-22.md`](docs/upstream-sync-plan-2026-11-22.md)）。
再同步时先跑 `git merge-tree --write-tree main upstream/main` 试算冲突，再决定排期。

## 本地修改（重要）

本仓库**不是纯净快照**。本地改动以 [`LOCAL_PATCHES.md`](LOCAL_PATCHES.md) 为唯一清单，
旧分支 `refactor/v0.5.2` 的 1–18 项迁移处置见 [`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。
每次同步上游后必须更新 `LOCAL_PATCHES.md`，否则下一次同步又要从头摸索。

其中**遥测移除是删除型改动**：上游几乎一定会带着 Sentry 与前端使用统计回来，迁移时
必须重做，不能沿用上游默认值。

## 构建产物

`pwsh build.ps1` 产出：

```
tools/kirara-builder.exe
```

它是 `pnpm build` 的结果——`merge-release-bundle.mjs` 把 cargo 编出的
`kachina-builder.exe` 与 `kachina-installer.exe` 拼成 `kachina-builder-bundle.exe`，
再由 `build.ps1` 以本项目名字拷贝到 `tools/`。一个文件同时包含「打包器 CLI」与
「安装器 GUI 模板」。`tools/*.exe` 已被 `.gitignore` 排除，不进版本库。
