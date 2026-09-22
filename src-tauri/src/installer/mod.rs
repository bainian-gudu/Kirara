use windows::Win32::{
    Foundation::{CloseHandle, WAIT_FAILED, WAIT_TIMEOUT},
    System::Diagnostics::ToolHelp::PROCESSENTRY32W,
};

use crate::cli::arg::InstallArgs;
use crate::utils::{
    dir::in_private_folder,
    error::{IntoTAResult, TAResult},
};
use anyhow::{Context, Result};

pub mod config;
pub mod lnk;
pub mod registry;
pub mod runtimes;
pub mod uninstall;

pub async fn launch(path: String) {
    let _ = open::that(path);
}

pub async fn launch_and_exit(path: String) {
    let _ = open::that(path);
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DirState {
    Unwritable,
    Writable,
    Private,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SelectDirRes {
    pub path: String,
    pub state: DirState,
    pub empty: bool,
    pub upgrade: bool,
}

/// 安装器会话对目标目录的全部认知。GUI / 原生 `UiState.path`、`Settings.elevate`、
/// `Settings.is_update` 与目录选择器共用同一份判定；路径是已存在的文件时返回 `None`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirProbe {
    pub exists: bool,
    pub empty: bool,
    /// 项目 exe 已经在该目录里。
    pub upgrade: bool,
    pub writable: bool,
    pub private: bool,
}

impl DirProbe {
    pub fn state(&self) -> DirState {
        if !self.writable {
            DirState::Unwritable
        } else if self.private {
            DirState::Private
        } else {
            DirState::Writable
        }
    }
}

/// `C:\` / `D:/` 这类盘根：盘根永远不能作为安装目录——它无法被整体改名或删除，
/// 卸载它会扫掉整个卷。
pub fn is_drive_root(path: &str) -> bool {
    let n = path.replace('\\', "/");
    let n = n.trim_end_matches('/');
    n.len() == 2 && n.as_bytes()[1] == b':' && n.as_bytes()[0].is_ascii_alphabetic()
}

/// 可写性用「在目录里建一个探针文件再删掉」判定；目录还不存在时用最近的存在祖先。
/// 已存在的文件、盘根、自身是重解析点（junction / symlink）的路径都返回 `None`：
/// 这些路径在任何把探测结果喂给 `Settings` 的地方都必须被拒绝。
pub fn probe_dir(path: &std::path::Path, exe_name: &str) -> Option<DirProbe> {
    if path.is_file() || is_drive_root(&path.to_string_lossy()) {
        return None;
    }
    if let Ok(meta) = std::fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() || crate::fs::commit::is_reparse(&meta) {
            return None;
        }
    }
    let exists = path.is_dir();
    let (empty, upgrade) = if exists {
        let upgrade = !exe_name.is_empty() && path.join(exe_name).is_file();
        let empty = !upgrade
            && std::fs::read_dir(path)
                .map(|mut it| it.next().is_none())
                .unwrap_or(true);
        (empty, upgrade)
    } else {
        (true, false)
    };
    let writable = path
        .ancestors()
        .find(|p| p.is_dir())
        .is_some_and(can_create_probe_file);
    Some(DirProbe {
        exists,
        empty,
        upgrade,
        writable,
        private: in_private_folder(path),
    })
}

fn can_create_probe_file(dir: &std::path::Path) -> bool {
    let p = dir.join(format!(".kachina-write-probe-{}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&p)
    {
        Ok(f) => {
            drop(f);
            let _ = std::fs::remove_file(&p);
            true
        }
        Err(_) => false,
    }
}

pub async fn inspect_dir(pathstr: String, exe_name: String) -> Option<SelectDirRes> {
    let probe = probe_dir(std::path::Path::new(&pathstr), &exe_name)?;
    Some(SelectDirRes {
        path: pathstr,
        state: probe.state(),
        empty: probe.empty,
        upgrade: probe.upgrade,
    })
}

pub async fn select_dir(
    path: String,
    exe_name: String,
    legacy_exe_names: Vec<String>,
    silent: bool,
    parent: Option<&crate::host::DialogParent>,
) -> Option<SelectDirRes> {
    let pathstr = if silent {
        path.clone()
    } else {
        let mut dialog = rfd::AsyncFileDialog::new()
            .set_directory(path)
            .set_can_create_directories(true);
        if let Some(parent) = parent {
            dialog = dialog.set_parent(parent);
        }
        let res = dialog.pick_folder().await;
        res.as_ref()?;
        let res = res.unwrap();
        res.path().to_str().map(|s| s.to_string())?
    };
    let mut empty = true;
    let mut upgrade = false;
    let path = std::path::Path::new(&pathstr);
    let mut state = DirState::Writable;
    if path.is_file() {
        return None;
    }
    if path.exists() {
        // 使用写入/截断标志打开目录在以下系统上始终失败：
        // Windows。通过创建唯一名称的子文件来探测可写性，
        // 然后删除该文件；这样也不会触碰用户文件。
        if !probe_directory_writable(path).await {
            state = DirState::Unwritable;
        }
        let has_exe = path.join(&exe_name).exists()
            || legacy_exe_names
                .iter()
                .any(|legacy| path.join(legacy).exists());
        if has_exe {
            upgrade = true;
            empty = false;
        } else {
            let entries = tokio::fs::read_dir(path).await;
            if let Ok(mut entries) = entries {
                if let Ok(Some(_entry)) = entries.next_entry().await {
                    empty = false;
                }
            }
        }
    } else {
        // 获取父目录
        let parent = path.parent();
        parent?;
        let parent = parent.unwrap();
        if !probe_directory_writable(parent).await {
            state = DirState::Unwritable;
        }
    }
    if in_private_folder(path) {
        state = DirState::Private;
    }
    Some(SelectDirRes {
        path: pathstr,
        state,
        empty,
        upgrade,
    })
}

async fn probe_directory_writable(dir: &std::path::Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let probe = dir.join(format!(".kachina-write-test-{}", uuid::Uuid::new_v4()));
    let result = tokio::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
        .await;
    match result {
        Ok(file) => {
            drop(file);
            let _ = tokio::fs::remove_file(probe).await;
            true
        }
        Err(_) => false,
    }
}

pub async fn kill_process(pid: u32) -> Result<()> {
    let ret = tokio::task::spawn_blocking(move || {
        // 使用 windows crate
        let handle = unsafe {
            windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_TERMINATE
                    | windows::Win32::System::Threading::PROCESS_SYNCHRONIZE,
                false,
                pid,
            )
        }
        .context("OPEN_PROCESS_ERR")?;
        let ret = unsafe { windows::Win32::System::Threading::TerminateProcess(handle, 1) }
            .context("KILL_PROCESS_ERR");
        if ret.is_err() {
            let _ = unsafe { CloseHandle(handle) };
            return ret;
        }
        // 等待进程退出，超时 10 秒
        let ret = unsafe { windows::Win32::System::Threading::WaitForSingleObject(handle, 10000) };
        match ret {
            WAIT_FAILED => {
                let oserr = windows::core::Error::from_win32();
                return Err(anyhow::anyhow!(oserr).context("WAIT_PROCESS_ERR"));
            }
            WAIT_TIMEOUT => {
                return Err(
                    anyhow::anyhow!("Process did not exit in time").context("KILL_PROCESS_TIMEOUT")
                );
            }
            _ => {}
        };
        let _ = unsafe { CloseHandle(handle) };
        Ok(())
    })
    .await;
    if let Err(e) = ret {
        return Err(anyhow::Error::new(e).context("KILL_PROCESS_ERR"));
    }
    ret.unwrap()
}

fn get_process_path(pid: u32) -> Option<String> {
    // QueryFullProcessImageName
    let handle = unsafe {
        windows::Win32::System::Threading::OpenProcess(
            windows::Win32::System::Threading::PROCESS_QUERY_LIMITED_INFORMATION,
            false,
            pid,
        )
    };
    if handle.is_err() {
        return None;
    }
    let handle = handle.unwrap();
    let mut buffer = [0u16; 1024];
    let mut size = buffer.len() as u32;
    let ret = unsafe {
        windows::Win32::System::Threading::QueryFullProcessImageNameW(
            handle,
            windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(handle) };
    if ret.is_err() {
        return None;
    }
    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    Some(path)
}

pub async fn find_process_by_name(name: String) -> Result<Vec<(u32, String)>> {
    let mut processes = Vec::new();
    unsafe {
        let snapshot = windows::Win32::System::Diagnostics::ToolHelp::CreateToolhelp32Snapshot(
            windows::Win32::System::Diagnostics::ToolHelp::TH32CS_SNAPPROCESS,
            0,
        )
        .context("FIND_PROCESS_ERR")?;
        if snapshot.is_invalid() {
            return Err(anyhow::anyhow!("Failed to create snapshot: invalid handle")
                .context("FIND_PROCESS_ERR"));
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = size_of::<PROCESSENTRY32W>() as u32;

        if windows::Win32::System::Diagnostics::ToolHelp::Process32FirstW(snapshot, &mut entry)
            .is_ok()
        {
            loop {
                let current_name = String::from_utf16_lossy(&entry.szExeFile)
                    .trim_end_matches('\0')
                    .to_lowercase();
                if current_name == name.to_lowercase() {
                    if let Some(path) = get_process_path(entry.th32ProcessID) {
                        processes.push((entry.th32ProcessID, path));
                    } else {
                        processes.push((entry.th32ProcessID, "".to_string()));
                    }
                }

                if windows::Win32::System::Diagnostics::ToolHelp::Process32NextW(
                    snapshot, &mut entry,
                )
                .is_err()
                {
                    break;
                }
            }
        }
        let _ = CloseHandle(snapshot);
    }
    Ok(processes)
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub struct VersionInfo {
    /// 文件的备注。
    pub comments: String,
    /// 生成该文件的公司名称。
    pub company_name: String,
    /// 文件描述。
    pub file_description: String,
    /// 文件版本号。
    pub file_version: String,
    /// 文件的内部名称（如果存在）。
    pub internal_name: String,
    /// 适用于该文件的版权声明。
    pub legal_copyright: String,
    /// 适用于该文件的商标和注册商标。
    pub legal_trademarks: String,
    /// 文件创建时的名称。
    pub original_filename: String,
    /// 发布该文件的产品名称。
    pub product_name: String,
    /// 发布该文件的产品版本。
    pub product_version: String,
    /// 文件的私有构建信息。
    pub private_build: String,
    /// 文件的特殊构建信息。
    pub special_build: String,
}

pub async fn get_exe_version(exe_name: String) -> TAResult<VersionInfo> {
    let info = win32_version_info::VersionInfo::from_file(exe_name).into_ta_result()?;
    Ok(VersionInfo {
        comments: info.comments,
        company_name: info.company_name,
        file_description: info.file_description,
        file_version: info.file_version,
        internal_name: info.internal_name,
        legal_copyright: info.legal_copyright,
        legal_trademarks: info.legal_trademarks,
        original_filename: info.original_filename,
        product_name: info.product_name,
        product_version: info.product_version,
        private_build: info.private_build,
        special_build: info.special_build,
    })
}

pub async fn error_dialog(
    title: String,
    message: String,
    args: &InstallArgs,
    parent: Option<&crate::host::DialogParent>,
) -> Result<(), String> {
    if !should_show_dialog(args.silent, args.non_interactive) {
        tracing::error!("{}: {}", title, message);
        return Ok(());
    }
    let mut dialog = rfd::MessageDialog::new()
        .set_title(&title)
        .set_description(&message)
        .set_level(rfd::MessageLevel::Error);
    if let Some(parent) = parent {
        dialog = dialog.set_parent(parent);
    }
    dialog.show();
    Ok(())
}

pub async fn confirm_dialog(
    title: String,
    message: String,
    args: &InstallArgs,
    parent: Option<&crate::host::DialogParent>,
) -> Result<bool, String> {
    if !should_show_dialog(args.silent, args.non_interactive) {
        tracing::warn!("{}: {}（无人值守运行，默认取消）", title, message);
        return Ok(false);
    }
    let mut dialog = rfd::MessageDialog::new()
        .set_title(&title)
        .set_description(&message)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNo);
    if let Some(parent) = parent {
        dialog = dialog.set_parent(parent);
    }
    let ret = dialog.show();

    Ok(matches!(ret, rfd::MessageDialogResult::Yes))
}

/// 静默 / 非交互运行不能弹模态对话框：`rfd::MessageDialog::show()` 会一直等用户点击，
/// CI、静默安装和控制面板调用会因此永久挂住。判定单独成纯函数，便于跨平台断言。
pub fn should_show_dialog(silent: bool, non_interactive: bool) -> bool {
    !silent && !non_interactive
}

pub fn log(data: String) {
    tracing::info!("{}", data);
}

pub fn warn(data: String) {
    tracing::warn!("{}", data);
}

pub fn error(data: String) {
    tracing::error!("{}", data);
}
