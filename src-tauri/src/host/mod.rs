pub(crate) mod assets;
mod bridge;
mod webview;
mod window;

use std::sync::{mpsc, Arc};

use anyhow::Context;
use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, Win32WindowHandle, WindowHandle,
};
use serde_json::Value;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, IsWindow, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
    WM_QUIT, WM_SIZE,
};

use crate::cli::arg::InstallArgs;
use crate::installer::uninstall::delete_self_on_exit;
use crate::ipc::manager::ManagedElevate;
use crate::APP_BOOT_SIGNAL;

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
    pub elevate: ManagedElevate,
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

    let (tx, rx) = mpsc::channel();
    let handle = HostHandle {
        tx,
        thread_id: unsafe { GetCurrentThreadId() },
        hwnd: hwnd.0 as isize,
    };
    let ctx = Arc::new(HostCtx {
        args,
        elevate: ManagedElevate::new(),
    });

    let watchdog_handle = handle.clone();
    tokio::spawn({
        async move {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if APP_BOOT_SIGNAL.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::info!("Native WebView2 frontend is ready");
                return;
            }
            let parent = watchdog_handle.dialog_parent();
            rfd::MessageDialog::new()
                .set_title("Kachina Installer")
                .set_description("Initialization failed due to webview2 fault")
                .set_level(rfd::MessageLevel::Error)
                .set_parent(&parent)
                .show();
            tracing::error!("WebView2 frontend failed to become ready within 30s");
            std::process::exit(1);
        }
    });

    let start = if cfg!(debug_assertions) {
        "http://localhost:1420".to_string()
    } else {
        format!("{UI_HOST}/index.html")
    };
    let webview =
        webview::attach(hwnd, handle.clone(), ctx, &start).context("attach WebView2 host")?;

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
