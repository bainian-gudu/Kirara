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
    let keep_roots = |paths: Vec<String>, label: &str| -> Vec<String> {
        paths
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
    let extra_uninstall_shortcuts: Vec<String> = extra_uninstall_shortcuts
        .into_iter()
        .filter(|p| {
            let ok = is_safe_shortcut_path(Path::new(p));
            if !ok {
                tracing::warn!("skip unsafe shortcut: {p}");
            }
            ok
        })
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
    // 下面三项是宿主自己的登记残留：清不掉只记日志，不把卸载判为失败。
    for err in remove_paths(&extra_uninstall_shortcuts).await {
        tracing::warn!("{err}");
    }
    clean_extra_registry(&extra_uninstall_registry);
    clean_extra_scheduled_tasks(&reg_name, &extra_uninstall_scheduled_tasks).await;
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
