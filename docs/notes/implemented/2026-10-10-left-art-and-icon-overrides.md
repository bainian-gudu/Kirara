# 左栏默认图与图片 / 图标自定义

Status: implemented

## Problem

安装界面左栏在包内没有内联图片时显示内置图。打包方不自备图片时它是界面上唯一的画面，因此需要是产品自己的形象图。

左栏图片与输出 exe 的图标若只由 `pack` 的命令行参数指定，三步打包就得各传一遍；漏传一步，该 exe 的界面或图标与其余 exe 不一致，而卸载器在安装期从安装器（更新时从更新器）的映像复制而来，会跟着一起不一致。

## Decision

内置默认图是 `web/left.webp`（399×454、含 alpha 的 WebP），由 rsbuild 按 `dataUriLimit` 以内联 base64 data URI 打进 `dist/index.html`。`UiState.theme` 为 `Image` 时左栏渲染包内图片 `/theme.webp`，其余取值渲染这张内置图。

`pack` 新增两个 pack-only 配置键，取出后从包内配置里删掉，`ProjectConfig` 不认识它们：

| 键 | 作用 | 缺省 |
| --- | --- | --- |
| `imageFile` | 内联左栏图片（`\0IMAGE` 槽位） | 内置 `web/left.webp` |
| `iconFile` | 输出 exe 的图标（rcedit 写入） | 内置 `resources/icons/icon.ico` |

两个键都相对配置文件所在目录解析，`pack` 的 `--image`（`-t`）/ `--icon` 优先于配置；点名了文件却读不到时打包失败，不静默回退。图标对三个 exe 都生效：安装器与更新器各自 `pack` 时写入，卸载器在安装期由安装器（更新时由更新器）的映像复制而来。

## Alternatives considered

- 沿用 `WizardArt` 线稿（见 [前端重写为 Preact 渲染器](./2026-09-02-frontend-preact-renderer.md)）：省下 27,677 字节未压缩体积，但打包方不自备图片时界面只剩通用线稿，丢掉产品形象。
- 只保留 `--image` / `--icon` 命令行参数：三步打包重复传参，漏传一步就得到资源不一致的 exe。
- 图标文件读不到时只警告：与 `agreementFile` 的宽容读法一致，但拼错的路径会静默退回内置图标，配置看起来生效。

## Verification

| 判据 | 结果 |
| --- | --- |
| 无内联图片时左栏显示内置图，有内联图片时显示 `/theme.webp` | PASS：`render.test.tsx` 断言 `img` 的 `src` |
| 图片内联进单文件产物，产物只有一个文件 | PASS：rsbuild 1.5.10 production 构建的 `dist/index.html` 90,051 字节，`dist/` 只有它，含一处 `data:image/webp;base64` |
| 配置键取出后不写进包内配置，相对与绝对路径都能解析，缺省回退内置资源 | PASS：devcheck logic 层 172 → 181 条断言全绿 |
| `pack` 在 Windows 上真编并通过行为测试 | PASS：CI `build` job |

## Consequences

- 收益：打包方不自备图片时界面是产品形象图；图片与图标随配置走，三步打包共用一份，卸载器与安装器、更新器天然一致。
- 代价：内置图让 `dist/index.html` 增加 27,677 字节（rsbuild 1.5.10 production 构建实测：去掉它 62,374、带上它 90,051；WebP 原始 21,750 字节），每个安装包都要多带这份体积。
- 配置点名了文件却读不到时打包直接失败，比只警告更容易打断下游构建。
