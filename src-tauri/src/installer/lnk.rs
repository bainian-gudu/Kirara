use anyhow::{anyhow, Context, Result};
use std::path::Path;
use windows::Win32::UI::Shell::{
    FOLDERID_CommonPrograms, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_PublicDesktop,
};

use crate::utils::{
    dir::get_dir,
    error::{IntoAnyhow, TAResult},
};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct CreateLnkArgs {
    pub target: String,
    pub lnk: String,
}
pub async fn create_lnk_with_args(args: CreateLnkArgs) -> Result<()> {
    create_lnk(args.target, args.lnk).await.into_anyhow()
}

pub async fn create_lnk(target: String, lnk: String) -> TAResult<()> {
    let target = Path::new(&target);
    let lnk = Path::new(&lnk);
    if !target.is_absolute()
        || !lnk.is_absolute()
        || target
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || lnk
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || !lnk
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("lnk"))
        || !crate::installer::uninstall::is_safe_delete_target(target)
    {
        return Err(anyhow!("Invalid shortcut path")
            .context("CREATE_LNK_ERR")
            .into());
    }
    // 命令可能在提权助手中运行。绝不创建或跟随
    // 过期快捷方式目录提供的 junction 或符号链接。
    if crate::installer::uninstall::has_reparse_point(lnk) {
        return Err(anyhow!("Shortcut path is a reparse point")
            .context("CREATE_LNK_ERR")
            .into());
    }
    let lnk_dir = lnk.parent();
    if lnk_dir.is_none() {
        return Err(anyhow!("Failed to get lnk parent dir")
            .context("CREATE_LNK_ERR")
            .into());
    }
    let lnk_dir = lnk_dir.unwrap();
    tokio::fs::create_dir_all(lnk_dir)
        .await
        .context("CREATE_LNK_ERR")?;
    write_shortcut(target, lnk)
        .await
        .context("CREATE_LNK_ERR")?;
    Ok(())
}

/// 用 Windows 自带的 Shell Link 组件写 `.lnk`。
///
/// 上游这里用的是 `mslnk 0.1`（2022 年后再没更新），它自带约 1300 行手写的 .lnk
/// 二进制序列化代码。这里改成系统 API：`IShellLinkW` + `IPersistFile::Save`，
/// IDList、相对路径、图标这些格式细节全部交给 Windows 自己处理。
///
/// COM 要求线程先初始化 apartment，而这条命令跑在 tokio 的工作线程上，所以整段丢进
/// `spawn_blocking`，在同一个线程里自己 `CoInitializeEx` / `CoUninitialize`。
async fn write_shortcut(target: &Path, lnk: &Path) -> Result<()> {
    let target = target.to_path_buf();
    let lnk = lnk.to_path_buf();
    tokio::task::spawn_blocking(move || write_shortcut_blocking(&target, &lnk))
        .await
        .context("CREATE_LNK_ERR")?
}

fn write_shortcut_blocking(target: &Path, lnk: &Path) -> Result<()> {
    // Interface 必须在作用域里，IShellLinkW → IPersistFile 的 cast() 是它的方法。
    use windows::core::{Interface, HSTRING};
    use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile, CLSCTX_INPROC_SERVER,
        COINIT_APARTMENTTHREADED,
    };
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};

    // RPC_E_CHANGED_MODE：这个线程已经被别的组件按另一种 apartment 初始化过。
    // 不是失败 —— Shell Link 对象两种 apartment 都能用，继续即可，只是这种情况下
    // 不能配对调用 CoUninitialize（谁初始化谁负责）。
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let initialized = hr.is_ok();
    if !initialized && hr != RPC_E_CHANGED_MODE {
        return Err(anyhow!("COM 初始化失败: {hr:?}").context("CREATE_LNK_ERR"));
    }

    let result = (|| -> Result<()> {
        let shell_link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }
                .context("CREATE_LNK_ERR")?;
        let target_wide = HSTRING::from(target.as_os_str());
        let lnk_wide = HSTRING::from(lnk.as_os_str());

        // SAFETY: 两个 HSTRING 在本作用域内一直有效，接口指针来自刚创建的对象。
        unsafe {
            shell_link.SetPath(&target_wide).context("CREATE_LNK_ERR")?;
            // 起始位置与目标同目录（mslnk 那份实现填的是同一个值）。
            if let Some(dir) = target.parent() {
                shell_link
                    .SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))
                    .context("CREATE_LNK_ERR")?;
            }
            let persist: IPersistFile = shell_link.cast().context("CREATE_LNK_ERR")?;
            persist.Save(&lnk_wide, true).context("CREATE_LNK_ERR")?;
        }
        Ok(())
    })();

    if initialized {
        // SAFETY: 上面确认过这次调用成功初始化了 apartment。
        unsafe { CoUninitialize() };
    }
    result
}

pub async fn get_dirs(elevated: bool) -> TAResult<(String, String)> {
    if elevated {
        Ok((
            get_dir(&FOLDERID_CommonPrograms)?,
            get_dir(&FOLDERID_PublicDesktop)?,
        ))
    } else {
        Ok((get_dir(&FOLDERID_Programs)?, get_dir(&FOLDERID_Desktop)?))
    }
}
