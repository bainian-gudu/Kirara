//! 安装器配置（包内 `\0CONFIG`）的顶层键名清单。
//!
//! 与 `native/session/types.rs` 的 `ProjectConfig` 的 serde 字段一一对应，由该文件
//! 的 `config_keys_match_the_struct` 单测钉住：改了结构体而没改这里，单测会失败。
//! builder 打包时用它点名拼错或过时的键——键名写错时 serde 静默忽略，配置看起来
//! 生效其实没有。

/// `ProjectConfig` 的顶层键，camelCase，与配置 JSON 一致。
pub const PROJECT_CONFIG_KEYS: &[&str] = &[
    "agreement",
    "appName",
    "description",
    "exeName",
    "extraUninstallLnkNames",
    "extraUninstallPath",
    "extraUninstallRegistry",
    "extraUninstallScheduledTasks",
    "ignoreFolderPath",
    "legacyExeNames",
    "legacyProgramFilesPaths",
    "legacyUninstallNames",
    "needWebView2",
    "programFilesPath",
    "publisher",
    "regName",
    "runtimes",
    "source",
    "title",
    "uacStrategy",
    "uninstallName",
    "updaterName",
    "userDataPath",
    "windowBorderless",
    "windowTitle",
];
