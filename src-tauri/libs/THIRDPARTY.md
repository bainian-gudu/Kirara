# `src-tauri/libs` 里 vendored 的第三方源码

`hdiff-sys` 与 `hpatch-sys` 各自打包了一份 HDiffPatch 的 C/C++ 源码，
由 `build.rs` 用 `cc` 编成静态库，供 `src-tauri/src/fs.rs` 在增量更新时做
「旧文件 + 差异包 → 新文件」的解包。两个 crate 里都随源码放了上游的 `LICENSE`。

| | |
| --- | --- |
| 上游 | <https://github.com/sisong/HDiffPatch> |
| 快照版本 | `v4.8.0`（`hpatch_types.h` 里的 `HDIFFPATCH_VERSION_*` 也是 4.8.0） |
| 许可 | MIT：HDiffPatch 部分 Copyright (c) 2012-2023 housisong；`hdiff-sys` 里还带了 libdivsufsort，Copyright (c) 2003-2008 Yuta Mori，同样是 MIT。两份都在各自的 `LICENSE` 里 |
| 谁在用 | `src-tauri/Cargo.toml`：`hdiff-sys = { path = "./libs/hdiff-sys" }`、`hpatch-sys = { path = "./libs/hpatch-sys" }` |

## 与上游的差异

只 vendored 了用得到的部分：`hdiff-sys` 取 `libHDiffPatch/HDiff` + `libParallel`，
`hpatch-sys` 取 `libHDiffPatch/HPatch`（`HPatchLite` 与 `diff_for_hpatch_lite.h`
没有用到，没有放进来）。除此之外的本地改动：

1. **include 路径**：上游把 `HDiff` / `HPatch` / `libParallel` 放在同一个
   `libHDiffPatch/` 目录下，这里拆成了两个 crate（`hdiff-sys`、`hpatch-sys`），
   所以 `#include "../HPatch/..."` 之类的相对路径按新布局改成了
   `../../hpatch-sys/HPatch/...`。
2. **Rust FFI 出口**：`HDiff/diff.h` 里给「输出到 stream」的
   `create_single_compressed_diff(...)` 加了 `extern "C"` 声明，
   `binding.rs`（bindgen 生成）与 Rust 侧都按 C 链接调用；上游那份带
   `std::vector` 参数的 C++ 重载保持原样不动。
3. **失败时返回具体错误码**：`HPatch/patch.c` 里若干原本 `return _hpatch_FALSE;`
   的分支改成了具体数字（`getSingleCompressedDiffInfo` 的 1002–1006，
   `patch_single_stream` 的 2000/3000/4000/5000 等）。C 侧成功仍然是 1，
   Rust 侧 `fs.rs` 也按 `res == 1` 判成功，所以语义不变，只是日志里能看出
   到底是哪一步校验失败（差异包版本不对 / 尺寸对不上 / 内存不够 …）。
4. **注释**：这批源码的英文注释被译成了中文（历史遗留，部分译文质量不佳）。
   上游升级时建议直接以新版本覆盖，不要把这批注释当作参考。

## 升级到新上游版本

1. 从上游取新 tag 的 `libHDiffPatch/{HDiff,HPatch,libParallel}`；
2. 按上面第 1、2、3 条重新打一遍本地改动（`grep -n "extern \"C\""`、
   `grep -n "return [0-9]\{4\};"` 可以把改动点全找出来）；
3. `build.rs` 里列出的源文件清单要与新版本的目录结构核对一遍；
4. 跑 `pwsh tools/devcheck/devcheck.ps1 -Layer native`（Windows + MSVC）确认
   C/C++ 还能编过；增量更新的真实解包链路由 CI 的 `online-update` /
   `offline-update` 两组测试覆盖。
