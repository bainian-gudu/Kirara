use anyhow::Context;
use std::io;
use std::path::{Path, PathBuf};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

use crate::fs::staging::{staging_root, Staging};
use crate::utils::code::{Coded, FILE_IO_FAILED};
use crate::utils::error::TAResult;
use crate::utils::process;

lazy_static::lazy_static!(
    /// Staging directory to remove after this process exits. It holds the
    /// running executable under `old\` (self-update or self-uninstall), which
    /// can be renamed but not deleted while it runs.
    static ref DELETE_SELF_ON_EXIT_PATH: std::sync::RwLock<Option<String>> = std::sync::RwLock::new(None);
);

/// The only writer of the exit-time cleanup path. Called once the swap that
/// moved the running executable has fully succeeded.
pub fn schedule_delete_on_exit(staging_root: &str) {
    DELETE_SELF_ON_EXIT_PATH
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .replace(staging_root.to_string());
}

#[cfg(test)]
pub fn delete_on_exit_path() -> Option<String> {
    DELETE_SELF_ON_EXIT_PATH
        .read()
        .unwrap_or_else(|e| e.into_inner())
        .clone()
}

#[cfg(test)]
pub fn clear_delete_on_exit() {
    DELETE_SELF_ON_EXIT_PATH
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .take();
}

pub fn run_clear_empty_dirs(path: &Path) -> Result<(), std::io::Error> {
    let entries = std::fs::read_dir(path)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            run_clear_empty_dirs(&path)?;
            let entries = std::fs::read_dir(&path)?;
            if entries.count() == 0 {
                std::fs::remove_dir(&path)?;
            }
        }
    }
    Ok(())
}

pub fn delete_dir_if_empty(path: &Path) -> Result<(), std::io::Error> {
    let entries = std::fs::read_dir(path)?;
    if entries.count() == 0 {
        std::fs::remove_dir(path)?;
    }
    Ok(())
}

pub async fn rm_list(key: Vec<PathBuf>) -> Vec<String> {
    let mut set = tokio::task::JoinSet::new();
    for path in key {
        set.spawn_blocking(move || match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("Failed to remove file {}: {e}", path.display())),
        });
    }
    let mut errs = Vec::new();
    while let Some(res) = set.join_next().await {
        match res {
            Ok(Ok(())) => {}
            Ok(Err(e)) => errs.push(e),
            Err(e) => errs.push(e.to_string()),
        }
    }
    errs
}

pub async fn clear_empty_dirs(key: String) -> anyhow::Result<()> {
    tokio::task::spawn_blocking(move || {
        let path = Path::new(&key);
        run_clear_empty_dirs(path)?;
        delete_dir_if_empty(path)?;
        Ok(())
    })
    .await
    .context("CLEAR_EMPTY_DIR_ERR")?
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct RunUninstallArgs {
    pub source: String,
    pub files: Vec<String>,
    pub user_data_path: Vec<String>,
    pub extra_uninstall_path: Vec<String>,
    /// 宿主自建/改名的快捷方式：删不掉只算清理不干净，不让卸载失败。
    #[serde(default)]
    pub extra_uninstall_shortcuts: Vec<String>,
    /// 宿主自己写过的注册表项（开机自启等）。
    #[serde(default)]
    pub extra_uninstall_registry: Vec<RegistryCleanupItem>,
    /// 安装期登记的登录计划任务（开机自启 + 自动管理员）。
    #[serde(default)]
    pub extra_uninstall_scheduled_tasks: Vec<String>,
    /// 计划任务安全阀用：任务名必须以产品注册名开头。
    #[serde(default)]
    pub reg_name: String,
    pub uninstall_name: String,
}

/// 卸载时要清掉的一条注册表项：给了 `value` 只删该值，没给则删整个子键。
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct RegistryCleanupItem {
    pub hive: String,
    pub key: String,
    #[serde(default)]
    pub value: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, Default)]
pub struct UninstallOutcome {
    /// Paths that could not be removed. Removal continues past each failure.
    pub errors: Vec<String>,
    /// Staging root the running uninstaller was parked in (`old\<name>`); the
    /// session schedules it for deletion at exit.
    pub self_moved_to: Option<String>,
}

pub async fn run_uninstall_with_args(args: RunUninstallArgs) -> TAResult<UninstallOutcome> {
    run_uninstall(
        args.source,
        args.files,
        args.user_data_path,
        args.extra_uninstall_path,
        args.extra_uninstall_shortcuts,
        args.extra_uninstall_registry,
        args.extra_uninstall_scheduled_tasks,
        args.reg_name,
        args.uninstall_name,
    )
    .await
}

/// Park the running executable under the staging directory's `old\` so the
/// install directory can be emptied. Same volume is guaranteed by
/// `staging_root`; a drive-root install has nowhere to go and is refused.
async fn park_self(exe_path: &Path, source: &str, uninstall_name: &str) -> anyhow::Result<String> {
    let root = staging_root(source, false)?;
    let staging = Staging::at(&root);
    staging.ensure_layout()?;
    let parked = staging.old_path(uninstall_name);
    let _ = tokio::fs::remove_file(&parked).await;
    tokio::fs::rename(exe_path, &parked)
        .await
        .context("SELF_UNINSTALL_ERR")?;
    Ok(root.to_string_lossy().to_string())
}

/// Remove each existing file or directory tree; missing paths are skipped and
/// a failure does not stop the rest. Returns one message per failed path.
pub async fn remove_paths(paths: &[String]) -> Vec<String> {
    let mut errs = Vec::new();
    for pathstr in paths {
        let path = Path::new(pathstr);
        if !path.exists() {
            continue;
        }
        let removed = if path.is_file() {
            tokio::fs::remove_file(path).await
        } else {
            tokio::fs::remove_dir_all(path).await
        };
        if let Err(e) = removed {
            errs.push(format!("Failed to remove {pathstr}: {e}"));
        }
    }
    errs
}

fn path_eq(a: &Path, b: &Path) -> bool {
    crate::session::plan::normalize_full(&a.to_string_lossy())
        == crate::session::plan::normalize_full(&b.to_string_lossy())
}

/// 路径本身或任一父级是符号链接 / junction。属性读不到时按「是」处理：
/// 判不出来就别动它。
fn has_reparse_point(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(p) = current {
        match std::fs::symlink_metadata(p) {
            Ok(meta) => {
                if meta.file_type().is_symlink() || crate::fs::commit::is_reparse(&meta) {
                    return true;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return false,
            Err(_) => return true,
        }
        current = p.parent();
    }
    false
}

fn is_under_system_root(path: &Path) -> bool {
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".to_string());
    let root = crate::session::plan::normalize_full(&root);
    if root.is_empty() {
        return true;
    }
    let target = crate::session::plan::normalize_full(&path.to_string_lossy());
    target == root || target.starts_with(&format!("{root}\\"))
}

/// 受保护位置**本身**：盘符根 / UNC 根、系统与用户环境变量指向的目录，
/// 以及用户配置目录下一层的 Shell 容器。产品自己的目录一定在它们下面至少一层。
fn is_protected_root(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    const VARS: &[&str] = &[
        "SystemRoot",
        "SystemDrive",
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "USERPROFILE",
        "HOMEDRIVE",
        "HOMEPATH",
        "APPDATA",
        "LOCALAPPDATA",
        "PUBLIC",
        "TEMP",
        "TMP",
    ];
    for name in VARS {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() && path_eq(path, Path::new(value)) {
                return true;
            }
        }
    }
    let mut shell_dirs: Vec<PathBuf> = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        for sub in ["Desktop", "Documents", "Downloads"] {
            shell_dirs.push(Path::new(&profile).join(sub));
        }
    }
    if let Ok(appdata) = std::env::var("APPDATA") {
        let start_menu = Path::new(&appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu");
        shell_dirs.push(start_menu.join("Programs"));
        shell_dirs.push(start_menu);
    }
    if let Ok(public) = std::env::var("PUBLIC") {
        shell_dirs.push(Path::new(&public).join("Desktop"));
    }
    shell_dirs.iter().any(|dir| path_eq(path, dir))
}

/// 「删除配置声明的目录」通道的安全阀：绝对路径、无 `..`、不是符号链接、
/// 不在 `%SystemRoot%` 内、至少两级、且不是受保护位置本身。
pub fn is_safe_delete_root(path: &Path) -> bool {
    if !path.is_absolute() {
        return false;
    }
    if path
        .to_string_lossy()
        .split(['/', '\\'])
        .any(|seg| seg == "..")
    {
        return false;
    }
    if has_reparse_point(path) || is_under_system_root(path) {
        return false;
    }
    let depth = path
        .components()
        .filter(|c| matches!(c, std::path::Component::Normal(_)))
        .count();
    depth >= 2 && !is_protected_root(path)
}

/// 「尽力删除」通道的安全阀：只放行 `.lnk`，且不含 `..`、不是符号链接。
pub fn is_safe_shortcut_path(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|seg| seg == "..")
        && path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("lnk"))
        && !has_reparse_point(path)
}

/// 配置里的 hive 名（`HKCU` / `HKLM` / `HKCR` / `HKU` 及其全称）。
fn cleanup_root(hive: &str) -> Option<&'static windows_registry::Key> {
    match hive.to_ascii_uppercase().as_str() {
        "HKCU" | "HKEY_CURRENT_USER" => Some(windows_registry::CURRENT_USER),
        "HKLM" | "HKEY_LOCAL_MACHINE" => Some(windows_registry::LOCAL_MACHINE),
        "HKCR" | "HKEY_CLASSES_ROOT" => Some(windows_registry::CLASSES_ROOT),
        "HKU" | "HKEY_USERS" => Some(windows_registry::USERS),
        _ => None,
    }
}

fn apply_registry_cleanup(root: &windows_registry::Key, key: &str, value: Option<&str>) {
    let result = match value {
        Some(name) => root
            .options()
            .write()
            .open(key)
            .and_then(|sub| sub.remove_value(name)),
        None => root.remove_tree(key),
    };
    if let Err(err) = result {
        tracing::warn!("registry cleanup {key} failed: {err}");
    }
}

/// 清理宿主自己写过的注册表项。`HKCU` 还要遍历已加载的用户配置单元：卸载器
/// 通常以管理员身份运行，此时 `HKCU` 指向管理员账户，而登记来自登录用户。
/// 键不存在、无权限都只记日志，卸载不因此失败。
pub fn clean_extra_registry(items: &[RegistryCleanupItem]) {
    for item in items {
        if item.key.is_empty() {
            continue;
        }
        let Some(root) = cleanup_root(&item.hive) else {
            tracing::warn!("unknown hive in extraUninstallRegistry: {}", item.hive);
            continue;
        };
        let value = item.value.as_deref().filter(|v| !v.is_empty());
        apply_registry_cleanup(root, &item.key, value);
        if !matches!(
            item.hive.to_ascii_uppercase().as_str(),
            "HKCU" | "HKEY_CURRENT_USER"
        ) {
            continue;
        }
        let users = windows_registry::USERS;
        let Ok(sids) = users.keys() else {
            continue;
        };
        for sid in sids {
            if sid.ends_with("_Classes") || sid == ".DEFAULT" || sid == "S-1-5-18" {
                continue;
            }
            apply_registry_cleanup(users, &format!("{sid}\\{}", item.key), value);
        }
    }
}

/// 任务名安全阀：必须以产品注册名开头、只含字母数字与 `._- `、长度 ≤ 100。
/// 卸载器通常以管理员身份运行，配置里写个 `*` 就等于删掉整台机器的计划任务。
pub fn is_safe_task_name(product: &str, name: &str) -> bool {
    !product.is_empty()
        && !name.is_empty()
        && name.len() <= 100
        && name.starts_with(product)
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
}

/// 回收安装期登记的登录计划任务。名字不合法、任务不存在、`schtasks` 调用失败
/// 都只记日志。
pub async fn clean_extra_scheduled_tasks(product: &str, names: &[String]) {
    if names.is_empty() {
        return;
    }
    let root = std::env::var_os("SystemRoot")
        .unwrap_or_else(|| std::ffi::OsString::from(r"C:\Windows"));
    let exe = PathBuf::from(root).join("System32").join("schtasks.exe");
    for name in names {
        if !is_safe_task_name(product, name) {
            tracing::warn!("skip unsafe scheduled task name: {name}");
            continue;
        }
        match process::spawn(&exe, &["/Delete", "/TN", name.as_str(), "/F"], true) {
            Ok(child) => match child.wait().await {
                Ok(0) => {}
                Ok(code) => tracing::warn!("schtasks delete {name} exited with {code}"),
                Err(err) => tracing::warn!("schtasks delete {name} failed: {err}"),
            },
            Err(err) => tracing::warn!("schtasks spawn failed: {err}"),
        }
    }
}

/// 展开 `%VAR%` 形式的环境变量。
///
/// 注册表里的 `ProfileImagePath` 常写成 `%SystemDrive%\Users\xxx`（REG_EXPAND_SZ），
/// 配置里的用户数据目录也可能是 `%LOCALAPPDATA%\...`，不展开就不能当路径用。
/// 未知变量原样保留：宁可少删，也不要拼出半个路径去删。`%%` 是一个字面 `%`，
/// 末尾落单的 `%` 原样输出（`100% done` 不变）；不支持 Windows 的子串语法。
fn expand_env_vars(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    loop {
        let (head, tail) = match rest.split_once('%') {
            Some(v) => v,
            None => {
                out.push_str(rest);
                break;
            }
        };
        out.push_str(head);
        match tail.split_once('%') {
            None => {
                out.push('%');
                out.push_str(tail);
                break;
            }
            Some((name, after)) => {
                if name.is_empty() {
                    out.push('%');
                    rest = &tail[1..];
                    continue;
                }
                match std::env::var(name) {
                    Ok(value) => out.push_str(&value),
                    Err(_) => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = after;
            }
        }
    }
    out
}

/// 批量展开路径里的 `%VAR%`，顺手去空白与空项、大小写不敏感去重。
fn expand_path_list(paths: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(paths.len());
    for path in paths {
        let expanded = expand_env_vars(path.trim());
        if expanded.is_empty() {
            continue;
        }
        if !out.iter().any(|p| p.eq_ignore_ascii_case(&expanded)) {
            out.push(expanded);
        }
    }
    out
}

/// 允许做「多用户清理」的每用户容器（相对 `%USERPROFILE%` 的第一级目录）。
///
/// 只有落在这些容器下面的路径才会被映射到别的用户配置目录去删；安装目录、
/// `ProgramData` 这类与「哪个用户」无关的路径不参与重放。
const PER_USER_CLEANUP_ROOTS: &[&str] = &["AppData", "Documents", "Desktop"];

/// 跨用户重放时禁止命中的「Shell 容器」名（大写比较）。
///
/// 重放会把「相对用户目录的尾巴」拼到每一个用户目录上，尾巴本身一旦是容器而不是
/// 产品目录，后果就是**所有用户**的开始菜单 / 文档 / 桌面被整个端掉。产品自己的
/// 目录名与历史快捷方式名都不在这张表里，所以功能不受影响 —— 命中即拒绝 + 记日志。
const PER_USER_DENY_LEAVES: &[&str] = &[
    "APPDATA",
    "LOCAL",
    "LOCALLOW",
    "ROAMING",
    "MICROSOFT",
    "WINDOWS",
    "START MENU",
    "PROGRAMS",
    "STARTUP",
    "SYSTEM",
    "SYSTEM32",
    "TEMP",
    "TMP",
    "CACHE",
    "CLASSES",
    "SOFTWARE",
    "USERS",
    "PUBLIC",
    "PROFILE",
    "DESKTOP",
    "DOCUMENTS",
    "DOWNLOADS",
    "MUSIC",
    "PICTURES",
    "VIDEOS",
    "TEMPLATES",
    "FAVORITES",
    "CONTACTS",
    "LINKS",
    "SAVED GAMES",
    "SEARCHES",
    "3D OBJECTS",
    "ONEDRIVE",
];

/// 取路径相对当前用户配置目录（`%USERPROFILE%`）的尾部，并校验它确实落在允许清理的
/// 每用户容器里；不满足返回 `None`（说明这条路径与「哪个用户」无关）。额外限制：
/// 尾部至少两级、`Desktop` 下只放行 `.lnk`、叶子不能是 Shell 容器、`AppData` 下至少三级。
fn profile_relative_tail(path: &Path) -> Option<PathBuf> {
    let profile = std::env::var_os("USERPROFILE").map(PathBuf::from)?;
    if profile.as_os_str().is_empty() {
        return None;
    }
    let tail = path.strip_prefix(profile.as_path()).ok()?;
    if tail
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }
    let first = match tail.components().next() {
        Some(std::path::Component::Normal(s)) => s.to_str()?,
        _ => return None,
    };
    if !PER_USER_CLEANUP_ROOTS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(first))
    {
        return None;
    }
    if tail.components().count() < 2 {
        return None;
    }
    if first.eq_ignore_ascii_case("Desktop")
        && !tail
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".lnk")
    {
        return None;
    }
    let leaf = tail.components().next_back()?.as_os_str().to_str()?;
    if PER_USER_DENY_LEAVES
        .iter()
        .any(|d| d.eq_ignore_ascii_case(leaf))
    {
        return None;
    }
    if first.eq_ignore_ascii_case("AppData") && tail.components().count() < 3 {
        return None;
    }
    Some(tail.to_path_buf())
}

/// 枚举本机已加载的用户配置目录（`ProfileList\<SID>\ProfileImagePath`）。
///
/// 卸载器通常以管理员身份运行，`%LOCALAPPDATA%` 指向执行卸载的账户；当初使用本
/// 软件的可能是另一个账户。注册表清理已按 `HKEY_USERS` 补齐，文件这边靠这里补齐。
fn loaded_profile_roots() -> Vec<PathBuf> {
    const PROFILE_LIST: &str = r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList";
    let Ok(list) = windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .open(PROFILE_LIST)
    else {
        return Vec::new();
    };
    let Ok(sids) = list.keys() else {
        return Vec::new();
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    for sid in sids {
        // `*_Classes` 只是视图键；`.DEFAULT` / LocalSystem 不是普通登录用户
        if sid.ends_with("_Classes") || sid == ".DEFAULT" || sid == "S-1-5-18" {
            continue;
        }
        let Ok(key) = list.open(&sid) else {
            continue;
        };
        let Ok(raw) = key.get_string("ProfileImagePath") else {
            continue;
        };
        let expanded = expand_env_vars(raw.trim());
        if expanded.is_empty() {
            continue;
        }
        let root = PathBuf::from(expanded);
        if !root.is_absolute() {
            continue;
        }
        if !roots.iter().any(|r| path_eq(r, &root)) {
            roots.push(root);
        }
    }
    roots
}

/// 把「当前用户展开后的数据目录 / 快捷方式」映射到所有已加载用户配置目录下的同一
/// 相对位置，得到还需要补删的候选。每个候选都要过 `is_safe_delete_root`，且必须真实
/// 存在；文件只放行 `.lnk`，目录不限（数据目录里可能有 WebView2 缓存等任意内容）。
fn collect_all_users_cleanup_targets(paths: &[String]) -> Vec<PathBuf> {
    let mut tails: Vec<PathBuf> = Vec::new();
    for pathstr in paths {
        if let Some(tail) = profile_relative_tail(Path::new(pathstr)) {
            if !tails.contains(&tail) {
                tails.push(tail);
            }
        }
    }
    if tails.is_empty() {
        return Vec::new();
    }
    let mut out: Vec<PathBuf> = Vec::new();
    for root in loaded_profile_roots() {
        for tail in &tails {
            let candidate = root.join(tail);
            if !is_safe_delete_root(&candidate) {
                tracing::warn!("skip unsafe per-user path: {}", candidate.display());
                continue;
            }
            let is_lnk = tail
                .to_string_lossy()
                .to_ascii_lowercase()
                .ends_with(".lnk");
            if !candidate.is_dir() && !(is_lnk && candidate.is_file()) {
                continue;
            }
            if !out.iter().any(|o| path_eq(o, &candidate)) {
                out.push(candidate);
            }
        }
    }
    out
}

/// 多用户残留清理：尽力而为，删不掉只记日志，绝不让卸载失败。
async fn clean_per_user_leftovers(paths: &[String]) {
    for target in collect_all_users_cleanup_targets(paths) {
        let target_str = target.display().to_string();
        let res = if target.is_dir() {
            tokio::fs::remove_dir_all(&target).await
        } else {
            tokio::fs::remove_file(&target).await
        };
        match res {
            Ok(()) => tracing::info!("removed per-user leftover {target_str}"),
            Err(err) => tracing::warn!("per-user cleanup failed (ignored) {target_str}: {err}"),
        }
    }
}

/// `%TEMP%` 里属于本安装器的文件名白名单。只认固定形状，绝不按「含 kachina 就删」
/// 这种模糊规则来：
/// - `KachinaInstaller.log`：安装 / 卸载日志，一直在追加，从来没人删
/// - `kachina.MicrosoftEdgeWebview2Setup.exe`：WebView2 引导安装器
/// - `kachina-agreement.txt`：原生简化 UI 查看协议全文时写的临时文件
///
/// 运行时安装包与卸载器副本不在这里：新架构把它们放在 staging 根目录下，随
/// staging 一起回收（见 `fs/staging.rs` 的 `dl\` / `old\`）。
fn is_installer_temp_artifact(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower == "kachinainstaller.log"
        || lower == "kachina.microsoftedgewebview2setup.exe"
        || lower == "kachina-agreement.txt"
}

/// 清理 `%TEMP%` 里本安装器留下的东西。只删文件不删目录、不递归，失败只记日志
/// （正在被使用的文件本来也删不掉）。
async fn clean_installer_temp_files() {
    let temp = std::env::temp_dir();
    let mut entries = match tokio::fs::read_dir(&temp).await {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!("read temp dir failed (ignored) {}: {err}", temp.display());
            return;
        }
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if !is_installer_temp_artifact(name) {
            continue;
        }
        // 只删文件：同名目录不是本安装器造的
        match entry.file_type().await {
            Ok(ft) if ft.is_file() => {}
            _ => continue,
        }
        match tokio::fs::remove_file(&path).await {
            Ok(()) => tracing::info!("removed temp artifact {}", path.display()),
            Err(err) => tracing::warn!("remove temp artifact failed (ignored) {}: {err}", path.display()),
        }
    }
}

pub async fn run_uninstall(
    source: String,
    files: Vec<String>,
    user_data_path: Vec<String>,
    extra_uninstall_path: Vec<String>,
    extra_uninstall_shortcuts: Vec<String>,
    extra_uninstall_registry: Vec<RegistryCleanupItem>,
    extra_uninstall_scheduled_tasks: Vec<String>,
    reg_name: String,
    uninstall_name: String,
) -> TAResult<UninstallOutcome> {
    let exe_path = std::env::current_exe().context("GET_EXE_PATH_ERR")?;
    let self_moved_to = if exe_path.starts_with(&source) {
        Some(park_self(&exe_path, &source, &uninstall_name).await?)
    } else {
        None
    };

    // 配置与网络元数据都可能写错，而卸载器通常以管理员身份运行：
    // 清单里的相对路径必须真的落在安装目录内，声明的目录必须是产品目录本身。
    let files: Vec<String> = files
        .into_iter()
        .filter(|f| {
            let ok = crate::fs::staging::is_safe_rel(f);
            if !ok {
                tracing::warn!("skip unsafe uninstall file entry: {f}");
            }
            ok
        })
        .collect();
    // `%VAR%` 必须在「真正要删的那一刻、那个进程里」展开：提权后 `%LOCALAPPDATA%`
    // 指向的是执行卸载的账户，而不是发起卸载的那个。
    let keep_roots = |paths: Vec<String>, label: &str| -> Vec<String> {
        expand_path_list(&paths)
            .into_iter()
            .filter(|p| {
                let ok = is_safe_delete_root(Path::new(p));
                if !ok {
                    tracing::warn!("skip unsafe {label}: {p}");
                }
                ok
            })
            .collect()
    };
    let user_data_path = keep_roots(user_data_path, "userDataPath");
    let extra_uninstall_path = keep_roots(extra_uninstall_path, "extraUninstallPath");
    let extra_uninstall_shortcuts: Vec<String> = expand_path_list(&extra_uninstall_shortcuts)
        .into_iter()
        .filter(|p| {
            let ok = is_safe_shortcut_path(Path::new(p));
            if !ok {
                tracing::warn!("skip unsafe shortcut: {p}");
            }
            ok
        })
        .collect();
    // 跨用户重放吃的是同一份「展开后的数据目录 + 额外路径」：勾了「同时删除用户数据」
    // 才会带上 userDataPath，没勾就是空表，所以勾选语义自动跟随。
    let per_user_paths: Vec<String> = user_data_path
        .iter()
        .chain(extra_uninstall_path.iter())
        .cloned()
        .collect();

    let mut delete_list = files
        .iter()
        .map(|f| Path::new(source.as_str()).join(f))
        .filter(|f| f.exists() && *f != exe_path)
        .collect::<Vec<_>>();
    if !exe_path.starts_with(&source) {
        // external uninstaller
        delete_list.push(Path::new(source.as_str()).join(uninstall_name));
    }
    let mut errors = rm_list(delete_list).await;
    errors.extend(remove_paths(&[&user_data_path[..], &extra_uninstall_path[..]].concat()).await);
    // 其它登录账户下的同一份残留（数据目录、桌面死图标、开始菜单文件夹）。
    clean_per_user_leftovers(&per_user_paths).await;
    // 下面三项是宿主自己的登记残留：清不掉只记日志，不把卸载判为失败。
    for err in remove_paths(&extra_uninstall_shortcuts).await {
        tracing::warn!("{err}");
    }
    clean_extra_registry(&extra_uninstall_registry);
    clean_extra_scheduled_tasks(&reg_name, &extra_uninstall_scheduled_tasks).await;
    clean_installer_temp_files().await;
    if let Err(e) = clear_empty_dirs(source).await {
        errors.push(format!("{e:#}"));
    }

    Ok(UninstallOutcome {
        errors,
        self_moved_to,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::windows::fs::OpenOptionsExt;

    #[test]
    fn env_expansion_keeps_unknown_vars_and_percent_literals() {
        std::env::set_var("KACHINA_TEST_VAR", r"C:\kachina");
        assert_eq!(expand_env_vars(r"%KACHINA_TEST_VAR%\x"), r"C:\kachina\x");
        // Windows 的环境变量名大小写不敏感
        assert_eq!(expand_env_vars(r"%kachina_test_var%\x"), r"C:\kachina\x");
        // 未知变量原样保留：宁可少删，也不拼出半个路径去删
        assert_eq!(expand_env_vars("%NOPE_SURELY%\\x"), "%NOPE_SURELY%\\x");
        assert_eq!(expand_env_vars("100% done"), "100% done");
        assert_eq!(expand_env_vars("a%%b"), "a%b");
        // 展开后去空白、空项、大小写不敏感去重
        assert_eq!(
            expand_path_list(&[r"  C:\a\b  ".into(), r"c:\A\B".into(), "  ".into()]),
            vec![r"C:\a\b".to_string()]
        );
    }

    #[test]
    fn per_user_tail_only_accepts_product_shaped_paths() {
        let profile = std::env::temp_dir().join("kachina-profile");
        std::env::set_var("USERPROFILE", &profile);
        let joined = |rel: &str| {
            let mut path = profile.clone();
            for part in rel.split(['/', '\\']) {
                path.push(part);
            }
            path
        };
        let tail = |rel: &str| {
            profile_relative_tail(&joined(rel))
                .map(|t| t.to_string_lossy().replace('\\', "/"))
        };
        assert_eq!(tail("AppData/Local/App").as_deref(), Some("AppData/Local/App"));
        assert_eq!(
            tail("AppData/Roaming/App").as_deref(),
            Some("AppData/Roaming/App")
        );
        assert_eq!(
            tail("Documents/App").as_deref(),
            Some("Documents/App")
        );
        assert_eq!(
            tail("Desktop/App.lnk").as_deref(),
            Some("Desktop/App.lnk")
        );
        // 容器本身、别人的容器、桌面上的普通文件、非每用户目录一律拒绝
        assert!(tail("AppData/Local").is_none());
        assert!(tail("AppData/Roaming/Microsoft").is_none());
        assert!(tail("AppData/Roaming/Microsoft/Windows/Start Menu/Programs").is_none());
        assert!(tail("Desktop/notes.txt").is_none());
        assert!(tail("Desktop").is_none());
        assert!(tail("Downloads/App").is_none());
        assert!(tail("AppData/Local/App/../Other").is_none());
    }

    #[test]
    fn temp_artifact_whitelist_is_shape_based() {
        assert!(is_installer_temp_artifact("KachinaInstaller.log"));
        assert!(is_installer_temp_artifact(
            "kachina.MicrosoftEdgeWebview2Setup.exe"
        ));
        assert!(is_installer_temp_artifact("kachina-agreement.txt"));
        // 目录名、别人的文件、形状不符的都不动
        assert!(!is_installer_temp_artifact("kachina-staged"));
        assert!(!is_installer_temp_artifact("kachina-backup.exe"));
        assert!(!is_installer_temp_artifact("other.txt"));
    }

    #[tokio::test]
    async fn park_self_moves_running_image_into_staging_old() {
        let base =
            crate::fs::staging::scratch_file(&format!("kachina-uninst-{}", uuid::Uuid::new_v4()));
        let install = base.join("app");
        std::fs::create_dir_all(&install).unwrap();
        let exe = install.join("uninst.exe");
        std::fs::write(&exe, b"MZ").unwrap();
        // a running image: readable, renamable, not deletable
        let _running = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1 | 0x4)
            .open(&exe)
            .unwrap();
        let root = park_self(&exe, &install.to_string_lossy(), "uninst.exe")
            .await
            .unwrap();
        assert!(!exe.exists());
        let parked = Staging::at(&root).old_path("uninst.exe");
        assert_eq!(std::fs::read(&parked).unwrap(), b"MZ");
        assert_eq!(
            delete_on_exit_path(),
            None,
            "park does not schedule by itself"
        );
        schedule_delete_on_exit(&root);
        assert_eq!(delete_on_exit_path().as_deref(), Some(root.as_str()));
        clear_delete_on_exit();
        drop(_running);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[tokio::test]
    async fn rm_list_missing_is_ok_locked_is_err() {
        let dir = crate::fs::staging::scratch_file(&format!("kachina-rm-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("gone.txt");
        assert!(rm_list(vec![missing]).await.is_empty());

        let ok = dir.join("ok.txt");
        std::fs::write(&ok, b"x").unwrap();
        assert!(rm_list(vec![ok.clone()]).await.is_empty());
        assert!(!ok.exists());

        let locked = dir.join("locked.txt");
        std::fs::write(&locked, b"x").unwrap();
        let _hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&locked)
            .unwrap();
        let errs = rm_list(vec![locked.clone()]).await;
        assert!(!errs.is_empty(), "{errs:?}");
        assert!(locked.exists());
        drop(_hold);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn uninstall_continues_past_a_locked_file() {
        let base =
            crate::fs::staging::scratch_file(&format!("kachina-uninst-{}", uuid::Uuid::new_v4()));
        let install = base.join("app");
        let extra = base.join("extra");
        std::fs::create_dir_all(install.join("sub")).unwrap();
        std::fs::create_dir_all(&extra).unwrap();
        for name in ["locked.txt", "sub/a.txt", "b.txt"] {
            std::fs::write(install.join(name), b"x").unwrap();
        }
        std::fs::write(extra.join("c.txt"), b"x").unwrap();
        let _hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(install.join("locked.txt"))
            .unwrap();

        let outcome = run_uninstall(
            install.to_string_lossy().into_owned(),
            vec!["locked.txt".into(), "sub/a.txt".into(), "b.txt".into()],
            Vec::new(),
            vec![extra.to_string_lossy().into_owned()],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            "App".into(),
            "uninst.exe".into(),
        )
        .await
        .unwrap();

        assert_eq!(outcome.errors.len(), 1, "{:?}", outcome.errors);
        assert!(
            outcome.errors[0].contains("locked.txt"),
            "{:?}",
            outcome.errors
        );
        assert!(install.join("locked.txt").exists());
        assert!(!install.join("b.txt").exists());
        assert!(
            !install.join("sub").exists(),
            "emptied folders are still cleared"
        );
        assert!(!extra.exists(), "later paths are still removed");
        drop(_hold);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn scheduled_task_names_must_belong_to_this_product() {
        assert!(is_safe_task_name("App", "App"));
        assert!(is_safe_task_name("App", "App AutoStart"));
        assert!(is_safe_task_name("App", "App-AutoStart_2.1"));
        // 通配符、目录形式、别人的任务名、空名、超长名、命令行注入形态一律拒绝
        assert!(!is_safe_task_name("App", "*"));
        assert!(!is_safe_task_name("App", "\\App"));
        assert!(!is_safe_task_name("App", "Other App"));
        assert!(!is_safe_task_name("App", ""));
        assert!(!is_safe_task_name("", "App"));
        assert!(!is_safe_task_name("App", &"A".repeat(101)));
        assert!(!is_safe_task_name("App", "App & calc.exe"));
        assert!(!is_safe_task_name("App", "App\"; del /f /q C:\\"));
    }

    #[test]
    fn staging_cleanup_deletes_ampersand_path_and_spares_sibling() {
        let root =
            crate::fs::staging::scratch_file(&format!("kachina-clean-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let staged = root.join("应用 & Games.kachina-staged");
        std::fs::create_dir_all(staged.join("old")).unwrap();
        std::fs::write(staged.join("old").join("a.txt"), b"x").unwrap();
        let sentinel = root.join("keep.txt");
        std::fs::write(&sentinel, b"y").unwrap();

        let child = spawn_staging_cleanup(&staged.to_string_lossy()).unwrap();
        child.wait_blocking().unwrap();

        assert!(
            !staged.exists(),
            "staged dir should be gone: {}",
            staged.display()
        );
        assert!(sentinel.exists(), "sibling sentinel must stay");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_cleanup_retries_until_lock_released() {
        let root =
            crate::fs::staging::scratch_file(&format!("kachina-clean-{}", uuid::Uuid::new_v4()));
        let staged = root.join("Apps&Games.kachina-staged");
        std::fs::create_dir_all(&staged).unwrap();
        let locked = staged.join("held.txt");
        std::fs::write(&locked, b"x").unwrap();
        let hold = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0x1)
            .open(&locked)
            .unwrap();

        let child = spawn_staging_cleanup(&staged.to_string_lossy()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        assert!(staged.exists(), "rmdir must wait while the file is held");
        drop(hold);
        child.wait_blocking().unwrap();
        assert!(!staged.exists(), "dir goes away after the lock is dropped");
        let _ = std::fs::remove_dir_all(&root);
    }
}

pub fn delete_self_on_exit() {
    // 幂等：WebView 消息循环（WM_CLOSE / WM_QUIT）与 main 的最终收尾都会调用，
    // 只清理一次。
    let Some(path) = DELETE_SELF_ON_EXIT_PATH
        .write()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    else {
        return;
    };
    let _ = spawn_staging_cleanup(&path);
}

/// `ping -n 2` ≈ 1s; 20 tries cap the wait at about 20s. The path is only in
/// `%KACHINA_CLEANUP%`, so `&` in the directory name is not cmd syntax.
const CLEANUP_CMD: &str = r#"for /L %i in (1,1,20) do @if exist "%KACHINA_CLEANUP%" (rmdir /s /q "%KACHINA_CLEANUP%" & ping 127.0.0.1 -n 2 >nul)"#;

fn spawn_staging_cleanup(path: &str) -> io::Result<process::Child> {
    process::spawn_system_cmd(CLEANUP_CMD, &[("KACHINA_CLEANUP", path)], true)
}

/// Stage the installer image (uninstaller / updater) under `new\` as phase-one
/// products. `copy_from` names an existing file to duplicate (the updater the
/// metadata shipped); otherwise the running executable's `base + config` is
/// written and its packed index mark cleared.
#[derive(serde::Deserialize, serde::Serialize, Clone, Debug)]
pub struct StageSelfImageArgs {
    pub install_dir: String,
    pub new_dir: String,
    pub hash_algorithm: String,
    pub names: Vec<String>,
    pub copy_from: Option<String>,
}

#[derive(serde::Deserialize, serde::Serialize, Clone, Debug, PartialEq, Eq)]
pub struct StagedImage {
    pub rel: String,
    pub hash: String,
    /// Hash of the file currently in the install directory, if any.
    pub old: Option<String>,
    /// The install directory already holds identical bytes; the staged copy
    /// was removed and no unit is needed.
    pub unchanged: bool,
}

pub async fn stage_self_image(args: StageSelfImageArgs) -> TAResult<Vec<StagedImage>> {
    let new_dir = Path::new(&args.new_dir);
    let install = Path::new(&args.install_dir);
    let mut out = Vec::new();
    for name in &args.names {
        if !crate::fs::staging::is_safe_rel(name) {
            return Err(anyhow::Error::from(Coded::bare(FILE_IO_FAILED)).into());
        }
        let staged = crate::fs::staging::join_rel(new_dir, name);
        if let Some(parent) = staged.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("CREATE_DIR_ERR")?;
        }
        match &args.copy_from {
            Some(src) => {
                tokio::fs::copy(src, &staged)
                    .await
                    .context("CREATE_UPDATER_ERR")?;
            }
            None => {
                let mut image = crate::local::get_base_with_config().await?;
                let file = tokio::fs::File::create(&staged)
                    .await
                    .context("CREATE_UNINSTALLER_ERR")?;
                let mut writer = tokio::io::BufWriter::new(file);
                tokio::io::copy(&mut image, &mut writer)
                    .await
                    .context("CREATE_UNINSTALLER_ERR")?;
                writer.flush().await.context("CREATE_UNINSTALLER_ERR")?;
                drop(writer);
                clear_index_mark(&staged).await?;
            }
        }
        crate::fs::sync_staged_file(&staged).await?;
        let hash =
            crate::utils::hash::run_hash(&args.hash_algorithm, &staged.to_string_lossy()).await?;
        let existing = crate::fs::staging::join_rel(install, name);
        let old = if existing.is_file() {
            crate::utils::hash::run_hash(&args.hash_algorithm, &existing.to_string_lossy())
                .await
                .ok()
        } else {
            None
        };
        let unchanged = old.as_deref() == Some(hash.as_str());
        if unchanged {
            let _ = tokio::fs::remove_file(&staged).await;
        }
        out.push(StagedImage {
            rel: name.replace('\\', "/"),
            hash,
            old,
            unchanged,
        });
    }
    Ok(out)
}
pub async fn clear_index_mark(path: &PathBuf) -> anyhow::Result<()> {
    // open again with rw
    let mut output_file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .await
        .context("SELF_UPDATE_ERR")?;
    // read first 256 bytes to buffer
    let mut buffer = [0u8; 256];
    output_file
        .read_exact(&mut buffer)
        .await
        .context("SELF_UPDATE_ERR")?;

    // check ! and K
    let mark_pos = buffer.windows(2).position(|w| w == b"!K".as_ref());
    if let Some(mark_pos) = mark_pos {
        // check if equals !KachinaInstaller!
        let mark_str = "!KachinaInstaller!";
        let mark_real = String::from_utf8_lossy(&buffer[mark_pos..mark_pos + mark_str.len()]);
        if mark_real == mark_str {
            let index_start = mark_pos + mark_str.len();
            // PE header replaced with index. Remove it.
            // write 5*4 bytes of 0 after index_start
            output_file
                .seek(tokio::io::SeekFrom::Start(index_start as u64))
                .await
                .context("SELF_UPDATE_ERR")?;
            let zero = [0u8; 5 * 4];
            output_file
                .write_all(&zero)
                .await
                .context("SELF_UPDATE_ERR")?;
        }
    }
    // close file
    output_file.flush().await.context("SELF_UPDATE_ERR")?;
    output_file.sync_all().await.context("SELF_UPDATE_ERR")?;
    drop(output_file);
    Ok(())
}
