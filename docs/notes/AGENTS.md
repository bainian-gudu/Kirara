# note 规范

note 是本仓库记录决策的体裁。本文定义 note 的生命周期、命名与格式。

上游（`YuehaiTeam/kachina-installer`）用同一套写法，`docs/notes/implemented/` 下的
文件名与结构对齐上游，便于升级时逐篇比对。

## 生命周期

note 的状态由所在目录编码，状态变化就是目录迁移，正文头部的 `Status:` 行随迁移保持
与目录一致。

| 状态 | 目录 | 含义 |
|---|---|---|
| proposed | `proposed/` | 提议与任务：已规划、尚未实施；验证计划与判据记录于此 |
| researched | `researched/` | 纯调研：只做了分析，未规划、未实施任何修改 |
| implemented | `implemented/` | 已实施：正文用现在时描述现状与验收结果 |
| rejected | `rejected/` | 被否决：`Status:` 行注明原因；仅当仍有防重议价值时保留 |

任务也是 note：任务派发就是写一篇 proposed，完成后把结果并入同一篇、迁入
implemented。仓库中不存在单独的任务文件体裁。

## 命名与引用

文件名取 `yyyy-mm-dd-<slug>.md`，日期为该主题首次提出之日。note 之间一律用库内相对
路径的 Markdown 链接互相引用，不写裸标题或数字代称。

## 格式

每篇开头固定为两段：

```text
# <标题>

Status: <proposed|researched|implemented|rejected[ — <原因>]>
```

状态行不写日期也不带括注。正文小节随状态而异，所有状态都以 `## Problem` 开头，动机
必须独立于方案成立。

- proposed：`## Problem`、`## Proposal`、`## Alternatives considered`、
  `## Acceptance criteria`、`## Risks`
- researched：`## Problem`、`## Findings`、`## No action`
- implemented：`## Problem`、`## Decision`、`## Alternatives considered`、
  `## Verification`、`## Consequences`
- rejected：冻结原 proposed 正文，只改 `Status:` 行

implemented 的 `Decision` 用现在时描述现状；`Verification` 逐条给出可复现的验收结果
（`| 判据 | 结果 |` 表格），而不是删除判据；`Consequences` 记录取舍付出的代价与换来
的收益。禁止出现 Plan、Migration plan 这类将来时标题。

## 与 LOCAL_PATCHES.md 的分工

`LOCAL_PATCHES.md` 是相对上游快照的改动台账：按顺序列全、便于升级时重新套用。note
记录的是**为什么这样定**、当时比较过什么、验证到什么程度。二者互相链接，不重复正文：
台账条目给出改动面与套用顺序，note 给出决策与验证。

## 何时写 note

凡是非平凡的变更——改变行为、契约、跨文件语义或流程——都应产生或更新一篇 note。已被
某篇 note 覆盖的决策，后续改名、改路径、改结构属于事实更新，直接改该 note；决策本身
被推翻则另立新 note 取代，旧 note 与新 note 交叉链接。纯机械、局部且语义无变化的编辑
可以豁免。
