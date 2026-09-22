// 发布版 Windows 下禁止额外控制台窗口，禁止删除！！
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

pub mod capabilities;
pub mod cli;
pub mod dfs;
pub mod fs;
pub mod host;
pub mod installer;
pub mod ipc;
pub mod local;
pub mod module;
pub mod thirdparty;
pub mod utils;
use clap::Parser;
use cli::arg::{Command, InstallArgs};
use std::{sync::atomic::AtomicBool, time::Duration};
use tracing_subscriber::prelude::*;

fn windows_text_scale_factor() -> f64 {
    use windows::UI::ViewManagement::UISettings;

    let scale = UISettings::new()
        .and_then(|settings| settings.TextScaleFactor())
        .unwrap_or(1.0);

    if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    }
}

lazy_static::lazy_static! {
    /// 不带中间件的原始 HTTP 客户端，供内部使用。
    pub(crate) static ref RAW_CLIENT: reqwest::Client = {
        reqwest::Client::builder()
            .user_agent(capabilities::ua_string()) // 由 DynamicUaMiddleware 按请求覆盖
            .gzip(true)
            .zstd(true)
            .read_timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(5))
            .build()
            .unwrap()
    };

    /// 用于 API 调用的 HTTP 客户端，携带实时动态 UA。
    pub static ref API_CLIENT: reqwest_middleware::ClientWithMiddleware = {
        reqwest_middleware::ClientBuilder::new(RAW_CLIENT.clone())
            .with(capabilities::DynamicUaMiddleware::new())
            .build()
    };

    /// 用于下载的 HTTP 客户端，通过中间件支持 H3/QUIC。
    pub static ref DOWNLOAD_CLIENT: reqwest_middleware::ClientWithMiddleware = {
        let h3_ok = capabilities::is_h3_available();

        let mut builder = reqwest_middleware::ClientBuilder::new(RAW_CLIENT.clone())
            .with(capabilities::DynamicUaMiddleware::new());

        if h3_ok {
            match capabilities::H3FallbackMiddleware::new(Duration::from_secs(60)) {
                Ok(h3mw) => {
                    builder = builder.with(h3mw);
                    tracing::info!("[H3] H3FallbackMiddleware enabled");
                }
                Err(e) => {
                    tracing::warn!("[H3] Middleware init failed: {:#}, disabling", e);
                    capabilities::disable_h3();
                }
            }
        }

        // SSH 隧道和 SFTP 中间件共用同一个 SSH 连接池
        let ssh_pool = std::sync::Arc::new(
            capabilities::ssh::SshPoolInner::new(Duration::from_secs(300)),
        );

        // SSH 隧道中间件：通过 SSH direct-tcpip 通道处理 ssh+http:// URL
        builder = builder.with(capabilities::ssh::SshMiddleware::with_pool(ssh_pool.clone()));
        tracing::info!("[SSH] SshMiddleware enabled");

        // SFTP 下载中间件：通过 SSH SFTP 子系统处理 sftp:// URL
        builder = builder.with(capabilities::sftp::SftpMiddleware::new(ssh_pool));
        tracing::info!("[SFTP] SftpMiddleware enabled");

        builder.build()
    };

    /// 旧版别名，迁移完成后移除。
    pub static ref REQUEST_CLIENT: &'static reqwest_middleware::ClientWithMiddleware = &*API_CLIENT;
    pub static ref APP_BOOT_SIGNAL: AtomicBool = AtomicBool::new(false);
}

fn main() {
    use windows::Win32::System::Console::{AttachConsole, ATTACH_PARENT_PROCESS};
    let _ = unsafe { AttachConsole(ATTACH_PARENT_PROCESS) };

    let cli = cli::Cli::parse();
    let mut command = cli.command();
    let wv2ver = host::webview_version();
    if wv2ver.is_err() {
        command = Command::InstallWebview2;
    }
    // 本项目已移除上游的遥测（见 ../../LOCAL_PATCHES.md 第 7 节）：不初始化
    // 上报 客户端、不挂 sentry-tracing layer。日志只进本地控制台与
    // %TEMP%\KachinaInstaller.日志，一个字节都不外发。
    let info_filter = utils::InfoFilter {};

    // 在临时目录创建日志文件，失败时忽略
    let temp_dir = std::env::temp_dir();
    let log_file = temp_dir.join("KachinaInstaller.log");

    let console_layer = tracing_subscriber::fmt::layer().with_filter(utils::InfoFilter {});

    let registry = tracing_subscriber::registry().with(console_layer);

    // %TEMP% 对同会话的普通权限进程可写，而这个文件名是**固定**的：先在那儿放一个指向
    // `C:\Windows\...` 的符号链接，提权子进程（headless-uac 也走这段初始化）就会把日志
    // 追加进系统文件。路径本身或任一父级是重解析点时，只留控制台日志、不写文件。
    let log_target = if installer::uninstall::has_reparse_point(&log_file) {
        eprintln!(
            "日志路径上是符号链接/junction，跳过文件日志: {}",
            log_file.display()
        );
        None
    } else {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)
            .ok()
    };
    if let Some(file) = log_target {
        let file_layer = tracing_subscriber::fmt::layer()
            .with_writer(file)
            .with_ansi(false)
            .with_filter(info_filter);
        registry.with(file_layer).init();
    } else {
        registry.init();
    }

    // 在创建任何客户端前尽早初始化 H3/QUIC 探测
    capabilities::init();

    // 命令不是 Command::Install，可以是其他任意命令
    match command {
        Command::HeadlessUac(args) => {
            tracing::info!("KachinaInstaller started as UAC Thread");
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(ipc::manager::uac_ipc_main(args));
        }
        Command::InstallWebview2 => {
            tracing::info!("KachinaInstaller started as Webview2 Installer");
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(module::wv2::install_webview2());
        }
        Command::Install(install) => {
            tracing::info!("KachinaInstaller started");
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(native_main(install));
        }
        Command::Other(_str) => {
            tracing::info!("KachinaInstaller started");
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(native_main(InstallArgs {
                    target: None,
                    non_interactive: false,
                    silent: false,
                    online: false,
                    uninstall: false,
                    source: None,
                    dfs_extras: None,
                    mirrorc_cdk: None,
                }));
        }
    }
}

async fn native_main(args: InstallArgs) {
    let temp_dir = std::env::temp_dir();
    let res = std::env::set_current_dir(&temp_dir);
    if res.is_err() {
        rfd::MessageDialog::new()
            .set_title("错误")
            .set_description("无法访问临时文件夹")
            .show();
        return;
    }
    if let Err(error) = host::run(args) {
        tracing::error!("native host failed: {error:#}");
        rfd::MessageDialog::new()
            .set_title("Kachina Installer")
            .set_description(format!("Native host failed: {error:#}"))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
