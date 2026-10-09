use anyhow::{Context, Result};
use std::path::Path;
use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        Storage::FileSystem::{GetDriveTypeW, QueryDosDeviceW},
        UI::Shell::{
            FOLDERID_Desktop, FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_LocalAppData,
            FOLDERID_LocalAppDataLow, FOLDERID_Profile, FOLDERID_RoamingAppData,
            SHGetKnownFolderPath, KF_FLAG_DEFAULT,
        },
    },
};

pub fn get_dir(dir: &GUID) -> Result<String> {
    let pwstr = unsafe {
        SHGetKnownFolderPath(dir, KF_FLAG_DEFAULT, None)
            .map(|pwstr| pwstr.to_string().context("INTERNAL_ERROR"))
            .context("GET_KNOWNFOLDER_ERR")??
    };
    Ok(pwstr)
}

/// 当前用户的 profile 目录。`GetUserProfileDirectoryW` 需要一个有效的用户令牌句柄，
/// 传默认 / 空句柄会以 `ERROR_INVALID_HANDLE` 失败并让私有目录判定失效；Shell 的
/// `FOLDERID_Profile` 对交互用户直接解析，不需要令牌句柄，提权后同样可用。
pub fn get_userprofile() -> Result<String> {
    get_dir(&FOLDERID_Profile)
}

/// `path` 等于 `parent` 或落在其下：统一分隔符方向、忽略大小写、按整段比较。
fn path_is_equal_or_child(path: &Path, parent: &str) -> bool {
    let path = path.to_string_lossy().replace('/', "\\");
    let parent = parent.trim_end_matches(['\\', '/']).replace('/', "\\");
    if parent.is_empty() {
        return false;
    }
    let path = path.to_ascii_lowercase();
    let parent = parent.to_ascii_lowercase();
    path == parent || path.starts_with(&(parent + "\\"))
}

/// Whether `path` sits on a drive letter that is a network mapping or a `subst`
/// alias. Both belong to the logon session that created them; with default
/// `EnableLinkedConnections` an elevated token does not see them.
pub fn on_session_drive(path: &str) -> bool {
    const DRIVE_REMOTE: u32 = 4;
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        return false;
    }
    let letter = &path[..2];
    let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(std::iter::once(0)).collect() };
    let root = wide(&format!("{letter}\\"));
    if unsafe { GetDriveTypeW(PCWSTR(root.as_ptr())) } == DRIVE_REMOTE {
        return true;
    }
    let device = wide(letter);
    let mut target = [0u16; 1024];
    let len = unsafe { QueryDosDeviceW(PCWSTR(device.as_ptr()), Some(&mut target)) } as usize;
    len > 0 && String::from_utf16_lossy(&target[..len]).starts_with(r"\??\")
}

pub fn in_private_folder(path: &Path) -> bool {
    let path_ids = vec![
        FOLDERID_LocalAppData,
        FOLDERID_LocalAppDataLow,
        FOLDERID_RoamingAppData,
        FOLDERID_Desktop,
        FOLDERID_Documents,
        FOLDERID_Downloads,
    ];
    // first check userprofile
    let userprofile = get_userprofile();
    if let Ok(userprofile) = userprofile {
        if path_is_equal_or_child(path, &userprofile) {
            return true;
        }
    }
    // then check known folders
    for id in path_ids {
        let known_folder = get_dir(&id);
        if let Ok(known_folder) = known_folder {
            if path_is_equal_or_child(path, &known_folder) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_and_unc_paths_are_not_session_drives() {
        let temp = std::env::temp_dir();
        assert!(!on_session_drive(&temp.to_string_lossy()));
        assert!(!on_session_drive(r"\\server\share\app"));
        assert!(!on_session_drive("relative"));
    }

    #[test]
    fn private_folder_matching_ignores_case_and_separators() {
        let profile = r"C:\Users\Alice";
        assert!(path_is_equal_or_child(Path::new(r"C:\Users\Alice"), profile));
        assert!(path_is_equal_or_child(
            Path::new(r"C:\Users\Alice\AppData\Local\App"),
            profile
        ));
        assert!(path_is_equal_or_child(
            Path::new("c:/users/alice/appdata/local/app"),
            profile
        ));
        assert!(path_is_equal_or_child(Path::new(r"C:\Users\Alice\"), profile));
        // 前缀相同但不是同一段，不能算在 profile 里。
        assert!(!path_is_equal_or_child(Path::new(r"C:\Users\Alice2"), profile));
        assert!(!path_is_equal_or_child(Path::new(r"C:\Users\Bob"), profile));
        assert!(!path_is_equal_or_child(Path::new(r"C:\Users"), profile));
    }
}
