//! 生成式类型检查 crate 的根模块。
//!
//! `src/gen/` 下的文件由 `tools/devcheck/devcheck.ps1 -Layer rust` 从
//! `src-tauri/src/` 生成（不要手改，也不会入库）：
//!
//! - `gen/uninstall.rs` —— `installer/uninstall.rs` 原样复制（兼容旧快照时去掉 `#[tauri::command]`）
//! - `gen/utils_error.rs` —— `utils/error.rs` 原样复制
//! - `gen/lnk.rs` —— `installer/lnk.rs` 原样复制（兼容旧快照时去掉 `#[tauri::command]`）
//! - `gen/utils_dir.rs` —— `utils/dir.rs` 原样复制
//! - `gen/utils_os_version.rs` —— `utils/os_version.rs` 原样复制
//!
//! 本文件负责把这些文件挂到与上游相同的模块路径上（`crate::utils::error`、
//! `crate::utils::dir`、`crate::installer::uninstall`、`crate::installer::lnk`），
//! 并为重量级依赖提供最小桩：
//! `crate::dfs::InsightItem`（只用到类型本身）、`crate::local::get_base_with_config`
//! （只用到「返回一个能 AsyncRead 的东西」），以及 `uninstall.rs` 的暂存、错误码和
//! 哈希调用面。
//!
//! `lnk.rs` 是「用系统 API 换掉 mslnk」那次重构的落点（`IShellLinkW` + `IPersistFile`），
//! 那段代码只在 Windows 上跑，本地无从执行 —— 挂进来至少保证 COM 接口名、参数类型与
//! 调用顺序在 `x86_64-pc-windows-msvc` 上编得过，改错 API 时 devcheck 当场就响。
//! `utils/os_version.rs` 同理：它是换掉 `nt_version` 的 `ntdll` 声明，
//! `unsafe extern` 那几行写错同样只有 Windows 构建才会发现。
//!
//! 桩与上游不一致时会直接编译失败，所以这个 crate 顺便也盯着上游签名变化。
#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    unused_mut,
    unused_assignments,
    clippy::all
)]

/// 对应上游 `src/dfs.rs` 的 `InsightItem`（字段需保持一致）。
pub mod dfs {
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Deserialize, Serialize, Debug)]
    pub struct InsightItem {
        pub url: String,
        pub ttfb: u32,
        pub time: u32,
        pub size: u32,
        pub error: Option<String>,
        #[serde(default)]
        pub range: Vec<(u32, u32)>,
        #[serde(default)]
        pub mode: Option<String>,
    }
}

#[path = "gen/utils_error.rs"]
pub mod gen_utils_error;

#[path = "gen/utils_dir.rs"]
pub mod gen_utils_dir;

#[path = "gen/utils_os_version.rs"]
pub mod gen_utils_os_version;

// 上游这里有个 `pub mod sentry { capture_anyhow }` 桩：`utils/error.rs` 序列化时会顺手
// 把错误上报到 Sentry，`super::sentry` 指向 crate 根。本项目已把 Sentry 连依赖一起拔掉
// （LOCAL_PATCHES.md 第 7 节），error.rs 里那句调用也没了，所以桩不需要。

/// 让 `use crate::utils::error::{return_ta_result, TAResult}`、
/// `use crate::utils::dir::get_dir` 与 `crate::utils::os_version::get()` 能解析到真实文件。
pub mod utils {
    pub use crate::gen_utils_error as error;
    pub use crate::gen_utils_dir as dir;
    pub use crate::gen_utils_os_version as os_version;

    /// `uninstall.rs` 只构造一个裸错误码；完整模块还依赖 serde_json 等非检查依赖，
    /// 所以这里只保留调用面。
    pub mod code {
        use std::fmt;

        pub const FILE_IO_FAILED: &str = "FILE_IO_FAILED";

        #[derive(Debug)]
        pub struct Coded {
            code: &'static str,
        }

        impl Coded {
            pub fn bare(code: &'static str) -> Self {
                Self { code }
            }
        }

        impl fmt::Display for Coded {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.code)
            }
        }

        impl std::error::Error for Coded {}
    }

    /// `uninstall.rs` 的暂存镜像流程只需要异步哈希接口；真实文件 IO 与 Windows
    /// 顺序读标志由完整构建覆盖。
    pub mod hash {
        pub async fn run_hash(
            _hash_algorithm: &str,
            _path: &str,
        ) -> anyhow::Result<String> {
            Ok(String::new())
        }
    }
}

/// `installer/uninstall.rs` 的暂存镜像路径调用面。实现只保留真实签名；
/// `fs.rs` 的完整实现依赖 reqwest / WebView2 等宿主组件，不应拖进检查 crate。
pub mod fs {
    pub async fn sync_staged_file(_path: &str) -> Result<(), anyhow::Error> {
        Ok(())
    }

    pub mod staging {
        use std::path::{Path, PathBuf};

        pub fn is_safe_rel(rel: &str) -> bool {
            let normalized = rel.replace('/', "\\");
            if normalized.starts_with('\\') || normalized.contains(':') {
                return false;
            }
            normalized
                .trim_end_matches('\\')
                .split('\\')
                .filter(|part| !part.is_empty())
                .all(|part| part != "." && part != "..")
        }

        pub fn join_rel(base: &Path, rel: &str) -> PathBuf {
            if !is_safe_rel(rel) {
                return base.to_path_buf();
            }
            rel.split(['/', '\\'])
                .filter(|part| !part.is_empty())
                .fold(base.to_path_buf(), |path, part| path.join(part))
        }
    }
}

/// 对应上游 `src/local.rs`：真实实现要 mmap 自身并解析内嵌索引，
/// 这里只需满足 `uninstall.rs` 的用法（`tokio::io::copy(&mut reader, ..)`）。
pub mod local {
    pub async fn get_base_with_config() -> anyhow::Result<tokio::io::Empty> {
        Ok(tokio::io::empty())
    }
}

// 注意：这里不能写成 `pub mod installer { #[path = "../gen/uninstall.rs"] ... }`，
// rustc 拼出来的路径需要中间目录真实存在（src/installer/ 并不存在）→ ENOENT。
// 所以先在 crate 根挂上生成文件，再用 re-export 拼出上游的模块路径。
#[path = "gen/uninstall.rs"]
pub mod gen_uninstall;

#[path = "gen/lnk.rs"]
pub mod gen_lnk;

pub mod installer {
    pub use crate::gen_uninstall as uninstall;
    pub use crate::gen_lnk as lnk;
}
