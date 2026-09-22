use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use windows::Win32::System::Threading::CREATE_NO_WINDOW;

use crate::fs::{
    create_http_stream, create_local_stream, create_staged_file, progressed_copy,
};
use crate::ipc_v2::{Progress, ProgressNotify};
use crate::utils::process;
use crate::utils::secure_temp;

const DOTNET_INSTALLED_VERSIONS: &str =
    r"SOFTWARE\WOW6432Node\dotnet\Setup\InstalledVersions\x64";

fn dotnet_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(root) = std::env::var_os("DOTNET_ROOT") {
        roots.push(PathBuf::from(root));
    }
    if let Ok(key) = windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .open(DOTNET_INSTALLED_VERSIONS)
    {
        if let Ok(location) = key.get_string("InstallLocation") {
            roots.push(PathBuf::from(location));
        }
    }
    if let Some(program_files) = std::env::var_os("ProgramFiles") {
        roots.push(PathBuf::from(program_files).join("dotnet"));
    }
    roots
}

pub fn dotnet_runtime_installed(framework: &str, major: &str) -> bool {
    let prefix = format!("{major}.");
    let matches = |name: &str| name.starts_with(&prefix);
    for root in dotnet_roots() {
        let Ok(entries) = std::fs::read_dir(root.join("shared").join(framework)) else {
            continue;
        };
        if entries.flatten().any(|entry| {
            entry.file_type().is_ok_and(|kind| kind.is_dir())
                && entry.file_name().to_str().is_some_and(matches)
        }) {
            return true;
        }
    }
    if let Ok(key) = windows_registry::LOCAL_MACHINE
        .options()
        .read()
        .open(format!(
            r"{DOTNET_INSTALLED_VERSIONS}\sharedfx\{framework}"
        ))
    {
        if let Ok(mut values) = key.values() {
            if values.any(|(name, _)| matches(&name)) {
                return true;
            }
        }
    }
    false
}

/// 新会话层的运行时安装：安装包落在 staging 的 `dl\`，随 staging 一起清理。
pub async fn install_runtime_v2(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    dl_dir: String,
    notify: ProgressNotify,
) -> Result<String> {
    let dl_dir = PathBuf::from(dl_dir);
    if tag.starts_with("Microsoft.DotNet") {
        return install_dotnet_v2(tag, offset, size, &dl_dir, notify).await;
    }
    if tag.starts_with("Microsoft.VCRedist") {
        return install_vcredist_v2(tag, offset, size, &dl_dir, notify).await;
    }
    Err(anyhow::anyhow!("UNSUPPORTED_RUNTIME"))
}

async fn install_dotnet_v2(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    dl_dir: &Path,
    notify: ProgressNotify,
) -> Result<String> {
    let tag_without_version = tag.split('.').take(3).collect::<Vec<&str>>().join(".");
    let runtime = match tag_without_version.as_str() {
        "Microsoft.DotNet.DesktopRuntime" => (
            "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/$/latest.version",
            "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/$/windowsdesktop-runtime-$-win-x64.exe",
            "Microsoft.WindowsDesktop.App",
        ),
        "Microsoft.DotNet.Runtime" => (
            "https://builds.dotnet.microsoft.com/dotnet/Runtime/$/latest.version",
            "https://builds.dotnet.microsoft.com/dotnet/Runtime/$/dotnet-runtime-$-win-x64.exe",
            "Microsoft.NETCore.App",
        ),
        _ => return Err(anyhow::anyhow!("UNSUPPORTED_DOTNET_RUNTIME")),
    };
    let version_primary = tag
        .split('.')
        .nth(3)
        .ok_or_else(|| anyhow::anyhow!("INVALID_DOTNET_VERSION"))?;
    if dotnet_runtime_installed(runtime.2, version_primary) {
        return Ok("ALREADY_INSTALLED".to_string());
    }

    let installer_path = dl_dir.join(format!("Kachina.RuntimePackage.{tag}.exe"));
    let installer_str = installer_path.to_string_lossy();
    let mut target = create_staged_file(&installer_str)
        .await
        .context("CREATE_TARGET_FILE_ERR")?;
    let (mut stream, len) = if offset.is_some() || size.is_some() {
        let stream = create_local_stream(offset.unwrap(), size.unwrap(), true)
            .await
            .context("RUNTIME_EXTRACT_ERR")?;
        (stream, size.unwrap())
    } else {
        let mut version = tag.split('.').skip(3).collect::<Vec<&str>>().join(".");
        if version.len() == 1 || version.len() == 2 {
            let release = if version.len() == 1 {
                format!("{version}.0")
            } else {
                version.clone()
            };
            let url = runtime.0.replace('$', &release);
            let response = reqwest::get(&url)
                .await
                .context("RUNTIME_VERSION_FETCH_ERR")?;
            if !response.status().is_success() {
                return Err(anyhow::anyhow!("RUNTIME_VERSION_API_ERR"));
            }
            version = response
                .text()
                .await
                .context("RUNTIME_VERSION_READ_ERR")?
                .trim()
                .to_string();
        }
        let url = runtime.1.replace('$', &version);
        let (stream, len, _insight) = create_http_stream(&url, 0, 0, true)
            .await
            .context("RUNTIME_DOWNLOAD_ERR")?;
        (stream, len.try_into().unwrap_or(0))
    };
    let progress_notify = move |downloaded: usize| {
        notify(Progress::BytesOf {
            done: downloaded as u64,
            total: len as u64,
        });
    };
    progressed_copy(stream.as_mut(), &mut target, &progress_notify).await?;
    drop(stream);
    drop(target);
    let child = process::spawn(&installer_path, &["/passive", "/norestart"], true)
        .context("RUNTIME_INSTALL_START_ERR")?;
    if child.wait().await.context("RUNTIME_INSTALL_WAIT_ERR")? != 0 {
        return Err(anyhow::anyhow!("RUNTIME_INSTALL_FAILED"));
    }
    let _ = tokio::fs::remove_file(&installer_path).await;
    Ok("NEWLY_INSTALLED".to_string())
}

async fn install_vcredist_v2(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    dl_dir: &Path,
    notify: ProgressNotify,
) -> Result<String> {
    let x64_prefix = "SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\";
    let x86_prefix = "SOFTWARE\\Wow6432Node\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\";
    let (url, reg) = match tag.as_str() {
        "Microsoft.VCRedist.2015+.x64" => (
            "https://aka.ms/vs/17/release/vc_redist.x64.exe",
            format!("{x64_prefix}x64"),
        ),
        "Microsoft.VCRedist.2015+.x86" => (
            "https://aka.ms/vs/17/release/vc_redist.x86.exe",
            format!("{x86_prefix}x86"),
        ),
        _ => return Err(anyhow::anyhow!("UNSUPPORTED_TAG")),
    };
    if check_vcredist(&reg) {
        return Ok("ALREADY_INSTALLED".to_string());
    }

    let installer_path = dl_dir.join(format!("Kachina.RuntimePackage.{tag}.exe"));
    let installer_str = installer_path.to_string_lossy();
    let (mut stream, len) = if offset.is_some() || size.is_some() {
        let stream = create_local_stream(offset.unwrap(), size.unwrap(), true)
            .await
            .context("RUNTIME_EXTRACT_ERR")?;
        (stream, size.unwrap())
    } else {
        let (stream, len, _insight) = create_http_stream(url, 0, 0, true)
            .await
            .context("RUNTIME_DOWNLOAD_ERR")?;
        (stream, len.try_into().unwrap_or(0))
    };
    let mut target = create_staged_file(&installer_str)
        .await
        .context("CREATE_TARGET_FILE_ERR")?;
    let progress_notify = move |downloaded: usize| {
        notify(Progress::BytesOf {
            done: downloaded as u64,
            total: len as u64,
        });
    };
    progressed_copy(stream.as_mut(), &mut target, &progress_notify).await?;
    drop(stream);
    drop(target);
    let child = process::spawn(
        &installer_path,
        &["/install", "/quiet", "/norestart"],
        true,
    )
    .context("RUNTIME_INSTALL_START_ERR")?;
    if child.wait().await.context("RUNTIME_INSTALL_WAIT_ERR")? != 0 {
        return Err(anyhow::anyhow!("RUNTIME_INSTALL_FAILED"));
    }
    let _ = tokio::fs::remove_file(&installer_path).await;
    Ok("NEWLY_INSTALLED".to_string())
}

pub async fn install_runtime(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> Result<String> {
    // 如果标签以 Microsoft.DotNet 开头，安装 .NET 运行时
    if tag.starts_with("Microsoft.DotNet") {
        return install_dotnet(tag, offset, size, notify).await;
    }
    if tag.starts_with("Microsoft.VCRedist") {
        return install_vcredist(tag, offset, size, notify).await;
    }
    // 否则不支持
    Err(anyhow::anyhow!("UNSUPPORTED_RUNTIME"))
}

/*
 * 安装 .NET 运行时包。
 * 支持的标签：
 * Microsoft.DotNet.DesktopRuntime.*
 * Microsoft.DotNet.Runtime.*
 * * 可以是数字“8”或“8.0.1”。
 */
pub async fn install_dotnet(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> Result<String> {
    let tag_without_version = tag.split('.').take(3).collect::<Vec<&str>>().join(".");
    let runtime = match tag_without_version.as_str() {
        "Microsoft.DotNet.DesktopRuntime" => (
            "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/$/latest.version",
            "https://builds.dotnet.microsoft.com/dotnet/WindowsDesktop/$/windowsdesktop-runtime-$-win-x64.exe",
            "Microsoft.WindowsDesktop.App",
        ),
        "Microsoft.DotNet.Runtime" => (
            "https://builds.dotnet.microsoft.com/dotnet/Runtime/$/latest.version",
            "https://builds.dotnet.microsoft.com/dotnet/Runtime/$/dotnet-runtime-$-win-x64.exe",
            "Microsoft.NETCore.App",
        ),
        _ => {
            return Err(anyhow::anyhow!("UNSUPPORTED_DOTNET_RUNTIME"));
        }
    };
    // 通过运行 dotnet --list-runtimes 检查运行时是否已安装
    let cmd = tokio::process::Command::new("dotnet")
        .arg("--list-runtimes")
        .creation_flags(CREATE_NO_WINDOW.0)
        .output()
        .await;
    // 已安装则继续；检查失败则返回错误
    if let Ok(output) = cmd {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let version_primary = tag
                .split('.')
                .nth(3)
                .ok_or_else(|| anyhow::anyhow!("INVALID_DOTNET_VERSION"))?;
            let query_name = format!("{} {}", runtime.2, version_primary);
            if stdout.contains(&query_name) {
                return Ok("ALREADY_INSTALLED".to_string());
            }
        }
    }
    // 落地目录 / 随机文件名 / 独占创建 / 执行前验签：全部见 utils/secure_temp.rs
    let installer_path = secure_temp::package_path(&format!("Kachina.RuntimePackage.{tag}"));
    let (mut stream, len) = if offset.is_some() || size.is_some() {
        // 运行时已打包，只需解压并运行
        let stream = create_local_stream(offset.unwrap(), size.unwrap(), true)
            .await
            .context("RUNTIME_EXTRACT_ERR")?;
        tracing::info!(
            "Extracted {} installer from local stream, offset: {}, size: {}",
            tag,
            offset.unwrap(),
            size.unwrap()
        );
        (stream, size.unwrap())
    } else {
        let mut vernum = tag.split('.').skip(3).collect::<Vec<&str>>().join(".");
        // 如果 vernum 是发布版本，获取实际版本
        if vernum.len() == 1 || vernum.len() == 2 {
            let relver = if vernum.len() == 1 {
                format!("{vernum}.0")
            } else {
                vernum.clone()
            };
            let url = runtime.0.replace("$", &relver);
            let resp = reqwest::get(&url)
                .await
                .context("RUNTIME_VERSION_FETCH_ERR")?;
            if !resp.status().is_success() {
                return Err(anyhow::anyhow!("RUNTIME_VERSION_API_ERR"));
            }
            let text = resp.text().await.context("RUNTIME_VERSION_READ_ERR")?;
            vernum = text.trim().to_string();
        }
        // 获取实际下载 URL
        let url = runtime.1.replace("$", &vernum);
        let (stream, len, _insight) = create_http_stream(&url, 0, 0, true)
            .await
            .context("RUNTIME_DOWNLOAD_ERR")?;
        (stream, len.try_into().unwrap_or(0))
    };
    let mut target = secure_temp::create_exclusive_file(&installer_path).await?;
    let progress_noti = move |downloaded: usize| {
        notify(serde_json::json!((downloaded, len)));
    };
    let copied = progressed_copy(&mut stream, &mut target, progress_noti).await;
    // 关闭流
    drop(stream);
    drop(target);
    if let Err(e) = copied {
        // 半个安装包留在管理员目录里没意义，删掉
        let _ = tokio::fs::remove_file(&installer_path).await;
        return Err(e);
    }
    secure_temp::verify_microsoft_signed(&installer_path).await?;
    // 使用 /passive /norestart 运行安装器
    let mut cmd = tokio::process::Command::new(&installer_path)
        .arg("/passive")
        .arg("/norestart")
        .spawn()
        .context("RUNTIME_INSTALL_START_ERR")?;
    let status = cmd.wait().await.context("RUNTIME_INSTALL_WAIT_ERR")?;
    if !status.success() {
        return Err(anyhow::anyhow!("RUNTIME_INSTALL_FAILED"));
    }
    // 删除安装器
    let _ = tokio::fs::remove_file(&installer_path).await;
    Ok("NEWLY_INSTALLED".to_string())
}

pub fn check_vcredist(reg: &str) -> bool {
    let key = windows_registry::LOCAL_MACHINE.options().read().open(reg);
    if let Ok(key) = key {
        let installed = key.get_u32("Installed");
        if let Ok(installed) = installed {
            if installed == 1 {
                return true;
            }
        }
    }
    false
}

pub async fn install_vcredist(
    tag: String,
    offset: Option<usize>,
    size: Option<usize>,
    notify: impl Fn(serde_json::Value) + std::marker::Send + 'static,
) -> Result<String> {
    let x64_prefix = "SOFTWARE\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\";
    let x86_prefix = "SOFTWARE\\Wow6432Node\\Microsoft\\VisualStudio\\14.0\\VC\\Runtimes\\";
    let (url, reg) = match tag.as_str() {
        "Microsoft.VCRedist.2015+.x64" => (
            "https://aka.ms/vs/17/release/vc_redist.x64.exe",
            format!("{}{}", x64_prefix, "x64"),
        ),
        "Microsoft.VCRedist.2015+.x86" => (
            "https://aka.ms/vs/17/release/vc_redist.x86.exe",
            format!("{}{}", x86_prefix, "x86"),
        ),
        _ => {
            return Err(anyhow::anyhow!("UNSUPPORTED_TAG"));
        }
    };
    // 检查注册表中是否已安装
    if check_vcredist(&reg) {
        return Ok("ALREADY_INSTALLED".to_string());
    }
    // 落地目录 / 随机文件名 / 独占创建 / 执行前验签：全部见 utils/secure_temp.rs
    let installer_path = secure_temp::package_path(&format!("Kachina.RuntimePackage.{tag}"));
    let (mut stream, len) = if offset.is_some() || size.is_some() {
        // 运行时已打包，只需解压并运行
        let stream = create_local_stream(offset.unwrap(), size.unwrap(), true)
            .await
            .context("RUNTIME_EXTRACT_ERR")?;
        tracing::info!(
            "Extracted {} installer from local stream, offset: {}, size: {}",
            tag,
            offset.unwrap(),
            size.unwrap()
        );
        (stream, size.unwrap())
    } else {
        let (stream, len, _insight) = create_http_stream(url, 0, 0, true)
            .await
            .context("RUNTIME_DOWNLOAD_ERR")?;
        (stream, len.try_into().unwrap_or(0))
    };
    let mut target = secure_temp::create_exclusive_file(&installer_path).await?;
    let progress_noti = move |downloaded: usize| {
        notify(serde_json::json!((downloaded, len)));
    };
    let copied = progressed_copy(&mut stream, &mut target, progress_noti).await;
    // 关闭流
    drop(stream);
    drop(target);
    if let Err(e) = copied {
        let _ = tokio::fs::remove_file(&installer_path).await;
        return Err(e);
    }
    secure_temp::verify_microsoft_signed(&installer_path).await?;
    let mut cmd = tokio::process::Command::new(&installer_path)
        .arg("/install")
        .arg("/quiet")
        .arg("/norestart")
        .spawn()
        .context("RUNTIME_INSTALL_START_ERR")?;
    let status = cmd.wait().await.context("RUNTIME_INSTALL_WAIT_ERR")?;
    if !status.success() {
        return Err(anyhow::anyhow!("RUNTIME_INSTALL_FAILED"));
    }
    let _ = tokio::fs::remove_file(installer_path).await;
    Ok("NEWLY_INSTALLED".to_string())
}
