use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const STAGING_SUFFIX: &str = ".kachina-staged";
pub const NEW_DIR: &str = "new";
pub const OLD_DIR: &str = "old";
pub const DL_DIR: &str = "dl";
pub const JOURNAL_FILE: &str = "journal";
pub const LOCK_FILE: &str = "lock";
pub const JOURNAL_VERSION: &str = "kachina-journal 1";

#[derive(serde::Serialize, Debug, Clone)]
pub struct StagingOpenResult {
    pub staging_root: String,
    pub journal: Option<String>,
}

/// 安装路径的稳定指纹。保留给 journal 路径诊断与后续跨会话定位使用。
pub fn path_hash(path: &str) -> String {
    let normalized = path
        .replace('\\', "/")
        .to_ascii_lowercase()
        .trim_end_matches('/')
        .to_string();
    let mut hasher = Sha256::new();
    hasher.update(normalized.as_bytes());
    hex::encode(&hasher.finalize()[..8])
}

/// 暂存目录与安装目录同级，保证所有提交 rename 都在同一卷内完成。
pub fn staging_root(install_dir: &str) -> Result<PathBuf> {
    let install = Path::new(install_dir);
    if !install.is_absolute() {
        return Err(anyhow::anyhow!("Install dir must be absolute").context("STAGING_PATH_INVALID"));
    }
    let parent = install.parent().context("STAGING_PATH_INVALID")?;
    let name = install
        .file_name()
        .and_then(|v| v.to_str())
        .context("STAGING_PATH_INVALID")?;
    Ok(parent.join(format!("{name}{STAGING_SUFFIX}")))
}

fn read_lock_pid(lock: &Path) -> Option<u32> {
    std::fs::read_to_string(lock)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}

fn pid_alive(pid: u32) -> bool {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
        use windows::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        unsafe {
            let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
                return false;
            };
            let mut code = 0u32;
            let alive =
                GetExitCodeProcess(handle, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
            let _ = CloseHandle(handle);
            alive
        }
    }
    #[cfg(not(windows))]
    {
        let _ = pid;
        false
    }
}

fn claim_lock(path: &Path) -> Result<(), anyhow::Error> {
    let pid = std::process::id();
    let create = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?;
        std::io::Write::write_all(&mut file, pid.to_string().as_bytes())?;
        Ok(())
    };
    match create() {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if let Some(holder) = read_lock_pid(path) {
                if holder != pid && pid_alive(holder) {
                    return Err(
                        anyhow::anyhow!("另一个安装程序正在处理此目录（pid {holder}）")
                            .context("STAGING_IN_USE"),
                    );
                }
            }
            let _ = std::fs::remove_file(path);
            match create() {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    Err(anyhow::anyhow!("另一个安装程序正在处理此目录").context("STAGING_IN_USE"))
                }
                Err(e) => Err(e).context("STAGING_LOCK_ERR"),
            }
        }
        Err(e) => Err(e).context("STAGING_LOCK_ERR"),
    }
}

/// 打开或创建暂存目录。若 journal 已存在，原样返回给调用方决定是否前滚。
pub fn open(install_dir: &str) -> Result<StagingOpenResult> {
    let root = staging_root(install_dir)?;
    let journal_path = root.join(JOURNAL_FILE);
    let journal = match std::fs::read_to_string(&journal_path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).context("STAGING_JOURNAL_READ_ERR"),
    };

    let lock = root.join(LOCK_FILE);
    if let Some(pid) = read_lock_pid(&lock) {
        if pid != std::process::id() {
            if pid_alive(pid) {
                return Err(anyhow::anyhow!("另一个安装程序正在处理此目录（pid {pid}）")
                    .context("STAGING_IN_USE"));
            }
            tracing::warn!("接管上次中断留下的暂存目录（原 pid {pid}）");
        }
    }

    // 没有 journal 的残留目录不可能恢复到可用状态，先清掉上一次阶段一写下的内容。
    // 有 journal 时保留 new/old/dl，交给调用方按 journal 决定前滚还是丢弃。
    if journal.is_none() {
        for dir in [NEW_DIR, OLD_DIR, DL_DIR] {
            let dir = root.join(dir);
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("STAGING_CREATE_ERR"),
            }
        }
    }
    std::fs::create_dir_all(root.join(NEW_DIR)).context("STAGING_CREATE_ERR")?;
    std::fs::create_dir_all(root.join(OLD_DIR)).context("STAGING_CREATE_ERR")?;
    std::fs::create_dir_all(root.join(DL_DIR)).context("STAGING_CREATE_ERR")?;
    claim_lock(&lock)?;

    Ok(StagingOpenResult {
        staging_root: root.to_string_lossy().to_string(),
        journal,
    })
}

pub fn discard(staging_root: &str) {
    let root = Path::new(staging_root);
    match std::fs::remove_dir_all(root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!("清理暂存目录失败 {}: {e}", root.display()),
    }
}

pub fn journal_path(staging_root: &Path) -> PathBuf {
    staging_root.join(JOURNAL_FILE)
}
