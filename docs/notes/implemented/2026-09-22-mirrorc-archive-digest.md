# Mirror酱归档校验下载内容摘要

Status: implemented

## Problem

Mirror酱接口 `get_mirrorc_status` 返回的 `sha256` 此前只被前端用来拼归档文件名
（`KachinaInstaller_Mirrorc_<sha256>.zip`），**下载下来的字节从未与它比对**：

- `ipc/operation.rs` 的 `IpcOperation::RunMirrorcDownload` 只有 `zip_path` 与 `url`；
- `thirdparty/mirrorc.rs::run_mirrorc_download` 收完流就返回；
- 解压 `run_mirrorc_install` 直接铺进安装目录。

镜像站返回了错误内容、代理截断响应、连接复用串了包，安装器都会把这份归档当成正常更新
解压进安装目录，且因为文件名带着「正确的」sha256，事后从文件名上完全看不出问题。

## Decision

- `utils/hash.rs::hash_reader` 增加 `sha256` 分支（此前只支持 `md5` / `xxh`），
  沿用同一套 1 MiB 顺序读循环；`hash_file("sha256", path)` 因此可直接复用。
- `run_mirrorc_download` 增加 `sha256: Option<&str>`：非空时对落盘的归档算 sha256，
  不等则删掉归档并以 `MIRRORC_HASH_ERR` 失败，绝不进入解压阶段。
- 提权 IPC 的 `RunMirrorcDownload` 变体增加 `#[serde(default)] sha256`，前端
  `ipcRunMirrorcDownload` 与 `App.vue` 的调用点同步传 `mirrorc_status.data.sha256`。
  字段可选：老调用方（没有摘要的镜像站应答）仍按原行为走，不因缺字段解析失败。

## Alternatives considered

- 在前端校验：归档落在安装目录、由提权进程写入，前端拿不到也不该拿这份字节。
- 只在解压时按 `changes.json` 里的逐文件摘要校验：那只覆盖变更清单里列出的文件，
  归档本身被截断（zip 中央目录损坏）仍会以解压错误的形式报出来，但错误码与原因对用户
  没有区分度，且校验发生在已经把文件铺了一半之后。
- 把 sha256 做成必填：会让没有该字段的镜像站应答直接解析失败，属于破坏性变更。

## Verification

| 判据 | 结果 |
|---|---|
| `hash_reader("sha256", b"hello")` 等于标准摘要 | PASS：devcheck logic 层 [22] 组断言 `2cf24dba…9824` |
| 跨 1 MiB 分块与一次性哈希一致 | PASS：devcheck logic 层 [22] 组断言 |
| 摘要不符时归档被删除且不进入解压 | PASS（结构）：不等分支先 `remove_file(zip_path)` 再返回 `MIRRORC_HASH_ERR` |
| 前端确实把接口的 sha256 传了下去 | PASS：`rg -n "sha256" src/App.vue src/api/ipc.ts` 命中调用点与接口字段 |
| e2e | 见 CI（Build / unit-test 两个 job）；Mirror酱 域名写死，无本地 stub，e2e 覆盖不到这条路径 |

## Consequences

- Mirror酱 路径多一次整包顺序读；归档大小通常在几十 MB 量级，与下载耗时相比可忽略。
- 摘要不符时用户看到的是明确的校验失败，且安装目录里没有半份更新。
- `MIRRORC_HASH_ERR` 是新增错误码；前端目前按通用错误呈现，未在 `locales` 里单列文案。
