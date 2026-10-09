# kachina-installer 上游来源

本仓库是**上游源码 + 本项目本地修改**。本文件记录迁移基线与来源；逐项改动状态见
[`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。

| 项目 | 值 |
| --- | --- |
| 上游仓库 | https://github.com/YuehaiTeam/kachina-installer |
| 迁移基线 | `main` 提交 `05a14107fc024645e1ad350fd3b4dffa62ae1368`（2026-09-25） |
| 上游最新 tag | `0.5.1`（`ae461aa9`）；`05a1410` 为未发版的 `main` 头 |
| 用途 | 构建 `kirara-builder.exe`，供下游 HoYoEnhance 打包安装器 / 更新器 / 卸载器 |

## 为什么把源码放进仓库

安装器只允许 Kachina 一种实现。把打包工具源码放进仓库后：CI 不再依赖上游 Release
的可下载性；打包工具版本随本项目一起进版本控制，可复现、可审计；本地一条命令即可
从零构建（`pwsh build.ps1`）。

## 分支关系

迁移分支 `codex/migrate-upstream-main` 直接以 `05a1410` 为根建立，与本仓库的旧分支
（上游 `0.5.1` 快照 + 本地补丁，`18f9553`，现为默认分支 `main`，原名
`refactor/v0.5.2`）**没有共同 Git 祖先**。因此本地修改是按行为逐项移植，而不是合并
或 cherry-pick。迁移完成后的合并方向是 `codex/migrate-upstream-main` → `main`。

## 本地修改（重要）

本仓库**不是纯净快照**。旧分支 `refactor/v0.5.2` 积累了 1–18 项本地改动（协议渲染、
卸载安全加固、临时文件保护、遥测物理移除、依赖替换、品牌升级兼容等），迁移到本基线
时需逐项重做或确认等价。逐项处置、落点与状态见
[`MIGRATION_LEDGER.md`](MIGRATION_LEDGER.md)。

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
