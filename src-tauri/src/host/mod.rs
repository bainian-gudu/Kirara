pub(crate) mod assets;
mod bridge;
mod webview;
mod window;

use std::sync::{mpsc, Arc, Mutex};

use anyhow::Context;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde_json::Value;
use tokio::sync::oneshot;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsWindow, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
    WM_QUIT, WM_SIZE,
};

use crate::cli::arg::InstallArgs;
use crate::installer::uninstall::delete_self_on_exit;
use crate::ipc_v2::manager::ManagedElevate;
use crate::session::commands::{GuiRuntime, SessionState};
use crate::session::types::SessionInput;
use crate::APP_BOOT_SIGNAL;

pub use window::HwndParent;

const UI_HOST: &str = "https://app.localhost";

pub enum UiAction {
    Emit { event: String, payload: Value },
    Reply { id: u64, ok: bool, data: Value },
    Close,
    Show,
    Minimize,
    SetTitle(String),
    SetDecorations(bool),
    SetBackground { dark: bool },
}

#[derive(Clone)]
pub struct HostHandle {
    tx: mpsc::Sender<UiAction>,
    thread_id: u32,
    hwnd: isize,
}

/// `rfd` 需要原始窗口句柄来把对话框挂到安装器窗口上。
///
/// 只保存 `HWND`，避免把宿主生命周期或 Tauri 类型带进安装逻辑。
#[derive(Clone, Copy)]
pub struct DialogParent {
    hwnd: isize,
}

impl HostHandle {
    pub fn dialog_parent(&self) -> DialogParent {
        DialogParent { hwnd: self.hwnd }
    }
}

impl HasWindowHandle for DialogParent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = std::num::NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?;
        let raw = Win32WindowHandle::new(hwnd).into();
        // SAFETY: `DialogParent` is created from a live native host window and
        // the returned handle borrows this wrapper for the same lifetime.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

impl HasDisplayHandle for DialogParent {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}

impl HostHandle {
    pub fn hwnd(&self) -> HWND {
        HWND(self.hwnd as *mut _)
    }

    pub fn parent(&self) -> HwndParent {
        HwndParent::from_hwnd(self.hwnd())
    }

    pub fn close(&self) {
        self.send(UiAction::Close);
    }

    pub fn emit(&self, event: &str, payload: impl serde::Serialize) {
        let payload = serde_json::to_value(payload).unwrap_or(Value::Null);
        self.send(UiAction::Emit {
            event: event.to_string(),
            payload,
        });
    }

    pub(crate) fn send(&self, action: UiAction) {
        let _ = self.tx.send(action);
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_APP, WPARAM(0), LPARAM(0));
        }
    }
}

pub struct HostCtx {
    pub args: InstallArgs,
    /// 新会话层使用的 typed/postcard IPC。
    pub elevate: ManagedElevate,
    pub session: SessionState,
    pub ui: HostHandle,
    pub plugin_runtime: bool,
    pub plugin_ready: Mutex<Option<oneshot::Sender<()>>>,
    pub preset: Option<SessionInput>,
    pub gui: Mutex<Option<Arc<GuiRuntime>>>,
}

pub struct PluginRuntime {
    handle: HostHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

impl PluginRuntime {
    pub fn handle(&self) -> &HostHandle {
        &self.handle
    }

    pub fn close(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.handle.close();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PluginRuntime {
    fn drop(&mut self) {
        if self.join.is_some() {
            self.shutdown();
        }
    }
}

pub fn webview_version() -> anyhow::Result<String> {
    webview::available_version()
}

pub fn run(args: InstallArgs) -> anyhow::Result<()> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    window::enable_dpi_awareness();

    let scale = crate::windows_text_scale_factor() * window::dpi_scale();
    let width = (520.0 * scale).round() as i32;
    let height = (250.0 * scale).round() as i32;
    let hwnd = window::create(width, height).context("create native host window")?;
    tracing::info!("native host: window created ({width}x{height} client)");

    let (tx, rx) = mpsc::channel();
    let handle = HostHandle {
        tx,
        thread_id: unsafe { GetCurrentThreadId() },
        hwnd: hwnd.0 as isize,
    };
    // 无人值守运行（`-S` / `-I`）里没人能点模态框：CI、控制面板、自动化调用
    // 一旦走到弹框就会永久挂住。看门狗只在交互运行里弹框，其余只写日志退出。
    let unattended = args.silent || args.non_interactive;
    let ctx = Arc::new(HostCtx {
        args,
        elevate: ManagedElevate::new(),
        session: SessionState::default(),
        ui: handle.clone(),
        plugin_runtime: false,
        plugin_ready: Mutex::new(None),
        preset: None,
        gui: Mutex::new(None),
    });

    let watchdog_handle = handle.clone();
    tokio::spawn({
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if APP_BOOT_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("Native WebView2 frontend is ready");
                return;
            }
            tracing::error!(
                "WebView2 frontend failed to become ready within 30s (unattended={unattended})"
            );
            if unattended {
                // 先记日志再退出：日志是无人值守场景唯一的诊断出口，弹框只会
                // 把失败伪装成超时（CI 上曾因此每个安装用例都卡满 3 分钟）。
                std::process::exit(1);
            }
            let parent = watchdog_handle.dialog_parent();
            rfd::MessageDialog::new()
                .set_title("Kachina Installer")
                .set_description("Initialization failed due to webview2 fault")
                .set_level(rfd::MessageLevel::Error)
                .set_parent(&parent)
                .show();
            std::process::exit(1);
        }
    });

    let start = if cfg!(debug_assertions) {
        "http://localhost:1420".to_string()
    } else {
        format!("{UI_HOST}/index.html")
    };
    tracing::info!("native host: attaching WebView2 at {start}");
    let webview =
        webview::attach(hwnd, handle.clone(), ctx, &start).context("attach WebView2 host")?;
    tracing::info!("native host: WebView2 attached");

    if cfg!(debug_assertions) {
        window::set_visible(hwnd, true);
        let _ = webview.open_devtools();
    } else {
        window::set_visible(hwnd, false);
    }

    let mut msg = MSG::default();
    loop {
        while let Ok(action) = rx.try_recv() {
            if !matches!(action, UiAction::Close) && !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                continue;
            }
            if let Err(err) = webview.apply(hwnd, action) {
                tracing::warn!("native host ui action failed: {err}");
            }
        }

        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match result.0 {
            -1 => return Err(anyhow::anyhow!("GetMessage failed")),
            0 => {
                delete_self_on_exit();
                return Ok(());
            }
            _ => {
                if msg.message == WM_QUIT {
                    delete_self_on_exit();
                    return Ok(());
                }
                if msg.message == window::WM_THEME_BACKGROUND {
                    let dark = crate::utils::gui::is_dark_mode().unwrap_or(false);
                    if let Err(err) = webview.apply(hwnd, UiAction::SetBackground { dark }) {
                        tracing::warn!("native host theme update failed: {err}");
                    }
                }
                if msg.message != WM_APP {
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
                if msg.message == WM_SIZE {
                    if let Err(err) = webview.resize(hwnd) {
                        tracing::warn!("native host resize failed: {err}");
                    }
                }
            }
        }
    }
}

pub async fn spawn_plugin_runtime(
    args: InstallArgs,
    session: SessionState,
) -> anyhow::Result<PluginRuntime> {
    let (started_tx, started_rx) = oneshot::channel();
    let (ready_tx, ready_rx) = oneshot::channel();
    let runtime = tokio::runtime::Handle::current();
    let join = std::thread::Builder::new()
        .name("kachina-plugin-host".into())
        .spawn(move || {
            let _enter = runtime.enter();
            plugin_runtime_thread(args, session, started_tx, ready_tx);
        })
        .context("spawn plugin host thread")?;

    let handle = started_rx
        .await
        .map_err(|_| anyhow::anyhow!("PLUGIN_HOST_FAILED"))?
        .map_err(|error| {
            tracing::error!("plugin host thread failed: {error:#}");
            error.context("PLUGIN_HOST_FAILED")
        })?;
    let plugin_runtime = PluginRuntime {
        handle,
        join: Some(join),
    };
    match tokio::time::timeout(std::time::Duration::from_secs(10), ready_rx).await {
        Ok(Ok(())) => Ok(plugin_runtime),
        _ => {
            plugin_runtime.close();
            Err(anyhow::anyhow!("PLUGIN_HOST_FAILED"))
        }
    }
}

fn plugin_runtime_thread(
    args: InstallArgs,
    session: SessionState,
    started_tx: oneshot::Sender<anyhow::Result<HostHandle>>,
    ready_tx: oneshot::Sender<()>,
) {
    match plugin_runtime_setup(args, session, ready_tx) {
        Ok((handle, rx, hwnd, webview)) => {
            let _ = started_tx.send(Ok(handle.clone()));
            plugin_runtime_loop(handle, rx, hwnd, webview);
        }
        Err(error) => {
            let _ = started_tx.send(Err(error));
        }
    }
}

fn plugin_runtime_setup(
    args: InstallArgs,
    session: SessionState,
    ready_tx: oneshot::Sender<()>,
) -> anyhow::Result<(
    HostHandle,
    mpsc::Receiver<UiAction>,
    HWND,
    webview::WebViewHost,
)> {
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
    }
    let hwnd = window::create_hidden().context("create hidden plugin window")?;
    let (tx, rx) = mpsc::channel();
    let handle = HostHandle {
        tx,
        thread_id: unsafe { GetCurrentThreadId() },
        hwnd: hwnd.0 as isize,
    };
    let ctx = Arc::new(HostCtx {
        args,
        elevate: ManagedElevate::new(),
        session,
        ui: handle.clone(),
        plugin_runtime: true,
        plugin_ready: Mutex::new(Some(ready_tx)),
        preset: None,
        gui: Mutex::new(None),
    });
    let start = format!("{UI_HOST}/index.html?pluginHost=1");
    let webview = webview::attach(hwnd, handle.clone(), ctx, &start)
        .context("attach hidden plugin webview")?;
    Ok((handle, rx, hwnd, webview))
}

fn plugin_runtime_loop(
    _handle: HostHandle,
    rx: mpsc::Receiver<UiAction>,
    hwnd: HWND,
    webview: webview::WebViewHost,
) {
    let mut msg = MSG::default();
    loop {
        while let Ok(action) = rx.try_recv() {
            if !matches!(action, UiAction::Close) && !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                continue;
            }
            if let Err(error) = webview.apply(hwnd, action) {
                tracing::warn!("plugin host ui action failed: {error}");
            }
        }
        let result = unsafe { GetMessageW(&mut msg, None, 0, 0) };
        match result.0 {
            -1 | 0 => break,
            _ => {
                if msg.message == WM_QUIT {
                    break;
                }
                if msg.message == window::WM_THEME_BACKGROUND {
                    let dark = crate::utils::gui::is_dark_mode().unwrap_or(false);
                    if let Err(error) = webview.apply(hwnd, UiAction::SetBackground { dark }) {
                        tracing::warn!("plugin host theme update failed: {error}");
                    }
                }
                if msg.message != WM_APP {
                    unsafe {
                        let _ = TranslateMessage(&msg);
                        DispatchMessageW(&msg);
                    }
                }
            }
        }
    }
}
