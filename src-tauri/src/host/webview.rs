use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{mpsc, Arc};

use anyhow::Context;
use serde_json::Value;
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::Foundation::{E_POINTER, HWND, RECT};
use windows::Win32::System::Com::IStream;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, VK_CONTROL, VK_F12, VK_I, VK_SHIFT,
};
use windows::Win32::UI::Shell::SHCreateMemStream;

use super::assets;
use super::bridge;
use super::window;
use super::{HostCtx, HostHandle, UiAction, UI_HOST};
use crate::utils::gui::is_dark_mode;

struct PostGate {
    ready: bool,
    pending: Vec<Value>,
}

pub struct WebViewHost {
    controller: ICoreWebView2Controller,
    webview: ICoreWebView2,
    mica: bool,
    posts: Rc<RefCell<PostGate>>,
}

impl WebViewHost {
    pub fn open_devtools(&self) -> anyhow::Result<()> {
        unsafe { self.webview.OpenDevToolsWindow() }.context("OpenDevToolsWindow")?;
        Ok(())
    }

    pub fn resize(&self, hwnd: HWND) -> anyhow::Result<()> {
        resize_controller(&self.controller, hwnd)
    }

    pub fn apply(&self, hwnd: HWND, action: UiAction) -> anyhow::Result<()> {
        match action {
            UiAction::Emit { event, payload } => {
                enqueue_or_post(
                    &self.webview,
                    &self.posts,
                    &serde_json::json!({
                        "kind": "event",
                        "event": event,
                        "payload": payload,
                    }),
                )?;
            }
            UiAction::Reply { id, ok, data } => {
                let message = if ok {
                    serde_json::json!({
                        "kind": "reply",
                        "id": id,
                        "ok": true,
                        "data": data,
                    })
                } else {
                    serde_json::json!({
                        "kind": "reply",
                        "id": id,
                        "ok": false,
                        "error": data,
                    })
                };
                enqueue_or_post(&self.webview, &self.posts, &message)?;
            }
            UiAction::Close => unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(hwnd),
                    windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                    windows::Win32::Foundation::WPARAM(0),
                    windows::Win32::Foundation::LPARAM(0),
                );
            },
            UiAction::Show => window::set_visible(hwnd, true),
            UiAction::Minimize => window::minimize(hwnd),
            UiAction::SetTitle(title) => window::set_title(hwnd, &title),
            UiAction::SetDecorations(decorated) => {
                window::set_decorations(hwnd, decorated);
                self.resize(hwnd)?;
            }
            UiAction::SetBackground { dark } => {
                set_background(&self.controller, self.mica, dark)?;
            }
        }
        Ok(())
    }
}

pub fn available_version() -> anyhow::Result<String> {
    let mut version = PWSTR::null();
    unsafe { GetAvailableCoreWebView2BrowserVersionString(PCWSTR::null(), &mut version) }
        .map_err(|e| anyhow::anyhow!(e))?;
    if version.is_null() {
        anyhow::bail!("webview2 missing");
    }
    let text = CoTaskMemPWSTR::from(version).to_string();
    if text.is_empty() {
        anyhow::bail!("webview2 missing");
    }
    Ok(text)
}

pub fn attach(
    hwnd: HWND,
    handle: HostHandle,
    ctx: Arc<HostCtx>,
    start: &str,
) -> anyhow::Result<WebViewHost> {
    let user_data = std::env::temp_dir().join("KachinaInstaller");
    let _ = std::fs::create_dir_all(&user_data);
    let user_data_w = wide(user_data.to_string_lossy().as_ref());

    let environment = {
        let (tx, rx) = mpsc::channel();
        CreateCoreWebView2EnvironmentCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                CreateCoreWebView2EnvironmentWithOptions(
                    PCWSTR::null(),
                    PCWSTR(user_data_w.as_ptr()),
                    None,
                    &handler,
                )
                .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(move |error_code, environment| {
                error_code?;
                tx.send(environment.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                    .expect("send env");
                Ok(())
            }),
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        rx.recv()
            .map_err(|_| anyhow::anyhow!("webview2 environment"))??
    };

    let controller = {
        let (tx, rx) = mpsc::channel();
        let environment = environment.clone();
        CreateCoreWebView2ControllerCompletedHandler::wait_for_async_operation(
            Box::new(move |handler| unsafe {
                environment
                    .CreateCoreWebView2Controller(hwnd, &handler)
                    .map_err(webview2_com::Error::WindowsError)
            }),
            Box::new(move |error_code, controller| {
                error_code?;
                tx.send(controller.ok_or_else(|| windows::core::Error::from(E_POINTER)))
                    .expect("send controller");
                Ok(())
            }),
        )
        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        rx.recv()
            .map_err(|_| anyhow::anyhow!("webview2 controller"))??
    };

    resize_controller(&controller, hwnd)?;
    unsafe { controller.SetIsVisible(true) }?;
    set_background(
        &controller,
        window::is_win11(),
        is_dark_mode().unwrap_or(false),
    )?;

    let webview = unsafe { controller.CoreWebView2() }?;
    unsafe {
        let settings = webview.Settings()?;
        settings.SetAreDefaultContextMenusEnabled(cfg!(debug_assertions))?;
        settings.SetAreDevToolsEnabled(cfg!(debug_assertions))?;
        settings.SetIsStatusBarEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        settings.SetIsWebMessageEnabled(true)?;
    }
    if cfg!(debug_assertions) {
        bind_devtools_shortcut(&controller, &webview)?;
    }
    inject_error_hook(&webview)?;

    let filter = wide(&format!("{UI_HOST}/*"));
    unsafe {
        webview.AddWebResourceRequestedFilter(
            PCWSTR(filter.as_ptr()),
            COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
        )?;
        let environment = environment.clone();
        let mut token = 0;
        webview.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    handle_resource(&environment, &args);
                }
                Ok(())
            })),
            &mut token,
        )?;
    }

    unsafe {
        let mut token = 0;
        let handle_for_message = handle.clone();
        let ctx_for_message = ctx.clone();
        webview.add_WebMessageReceived(
            &WebMessageReceivedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    let mut message = PWSTR::null();
                    if args.WebMessageAsJson(&mut message).is_ok() {
                        let json = CoTaskMemPWSTR::from(message).to_string();
                        bridge::on_message(&ctx_for_message, &handle_for_message, &json);
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }

    let posts = Rc::new(RefCell::new(PostGate {
        ready: false,
        pending: Vec::new(),
    }));
    unsafe {
        let mut token = 0;
        let posts_for_navigation = posts.clone();
        let webview_for_navigation = webview.clone();
        webview.add_NavigationCompleted(
            &NavigationCompletedEventHandler::create(Box::new(move |_sender, args| {
                let mut success = windows::core::BOOL::default();
                if let Some(args) = args {
                    let _ = args.IsSuccess(&mut success);
                }
                if !success.as_bool() {
                    return Ok(());
                }
                let pending = {
                    let mut gate = posts_for_navigation.borrow_mut();
                    gate.ready = true;
                    std::mem::take(&mut gate.pending)
                };
                for message in &pending {
                    if let Err(err) = post_json(&webview_for_navigation, message) {
                        tracing::warn!("flush web message failed: {err}");
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }

    let start_w = wide(start);
    unsafe { webview.Navigate(PCWSTR(start_w.as_ptr())) }?;

    Ok(WebViewHost {
        controller,
        webview,
        mica: window::is_win11(),
        posts,
    })
}

fn bind_devtools_shortcut(
    controller: &ICoreWebView2Controller,
    webview: &ICoreWebView2,
) -> anyhow::Result<()> {
    let webview = webview.clone();
    let mut token = 0i64;
    unsafe {
        controller.add_AcceleratorKeyPressed(
            &AcceleratorKeyPressedEventHandler::create(Box::new(move |_sender, args| {
                if let Some(args) = args {
                    if is_devtools_hotkey(&args) {
                        let _ = args.SetHandled(true);
                        let _ = webview.OpenDevToolsWindow();
                    }
                }
                Ok(())
            })),
            &mut token,
        )?;
    }
    Ok(())
}

fn is_devtools_hotkey(args: &ICoreWebView2AcceleratorKeyPressedEventArgs) -> bool {
    let mut kind = COREWEBVIEW2_KEY_EVENT_KIND::default();
    if unsafe { args.KeyEventKind(&mut kind) }.is_err() {
        return false;
    }
    if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
        && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
    {
        return false;
    }
    let mut virtual_key = 0u32;
    if unsafe { args.VirtualKey(&mut virtual_key) }.is_err() {
        return false;
    }
    if virtual_key == VK_F12.0 as u32 {
        return true;
    }
    virtual_key == VK_I.0 as u32 && key_down(VK_CONTROL) && key_down(VK_SHIFT)
}

fn key_down(vk: windows::Win32::UI::Input::KeyboardAndMouse::VIRTUAL_KEY) -> bool {
    unsafe { GetKeyState(i32::from(vk.0)) < 0 }
}

const ERROR_HOOK: &str = r#"
window.addEventListener("error", function (event) {
  try {
    chrome.webview.postMessage({
      id: 0,
      kind: "invoke",
      cmd: "error",
      args: {
        data: "webview uncaught " + (event.message || "") + " " + (event.filename || "") + ":" + (event.lineno || 0)
      }
    });
  } catch (_) {}
});
window.addEventListener("unhandledrejection", function (event) {
  try {
    var reason = event.reason;
    chrome.webview.postMessage({
      id: 0,
      kind: "invoke",
      cmd: "error",
      args: {
        data: "webview unhandledrejection " + (reason && reason.stack ? reason.stack : String(reason))
      }
    });
  } catch (_) {}
});
"#;

fn inject_error_hook(webview: &ICoreWebView2) -> anyhow::Result<()> {
    let script = wide(ERROR_HOOK);
    unsafe {
        webview.AddScriptToExecuteOnDocumentCreated(
            PCWSTR(script.as_ptr()),
            &AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(
                |_error, _id| Ok(()),
            )),
        )?;
    }
    Ok(())
}

fn handle_resource(
    environment: &ICoreWebView2Environment,
    args: &ICoreWebView2WebResourceRequestedEventArgs,
) {
    let Ok(request) = (unsafe { args.Request() }) else {
        return;
    };
    let mut uri = PWSTR::null();
    if unsafe { request.Uri(&mut uri) }.is_err() {
        return;
    }
    let uri = CoTaskMemPWSTR::from(uri).to_string();
    let Some(path) = uri.strip_prefix(UI_HOST) else {
        return;
    };
    let path = path.split('?').next().unwrap_or(path);
    let Some((bytes, mime)) = assets::lookup(path) else {
        if let Ok(response) = make_response(environment, b"not found", 404, "text/plain") {
            let _ = unsafe { args.SetResponse(&response) };
        }
        return;
    };
    if let Ok(response) = make_response(environment, bytes, 200, mime) {
        let _ = unsafe { args.SetResponse(&response) };
    }
}

fn make_response(
    environment: &ICoreWebView2Environment,
    bytes: &[u8],
    status: i32,
    mime: &str,
) -> anyhow::Result<ICoreWebView2WebResourceResponse> {
    let stream = unsafe { SHCreateMemStream(Some(bytes)) }.context("SHCreateMemStream")?;
    let stream: IStream = stream;
    let headers = wide(&format!(
        "Content-Type: {mime}\nAccess-Control-Allow-Origin: *\nCache-Control: no-cache"
    ));
    let reason = wide(if status == 200 { "OK" } else { "Not Found" });
    let response = unsafe {
        environment.CreateWebResourceResponse(
            &stream,
            status,
            PCWSTR(reason.as_ptr()),
            PCWSTR(headers.as_ptr()),
        )
    }?;
    Ok(response)
}

fn enqueue_or_post(
    webview: &ICoreWebView2,
    posts: &RefCell<PostGate>,
    value: &Value,
) -> anyhow::Result<()> {
    let mut gate = posts.borrow_mut();
    if gate.ready {
        drop(gate);
        return post_json(webview, value);
    }
    gate.pending.push(value.clone());
    Ok(())
}

fn post_json(webview: &ICoreWebView2, value: &Value) -> anyhow::Result<()> {
    let text = value.to_string();
    let text_w = wide(&text);
    unsafe { webview.PostWebMessageAsJson(PCWSTR(text_w.as_ptr())) }?;
    Ok(())
}

fn resize_controller(controller: &ICoreWebView2Controller, hwnd: HWND) -> anyhow::Result<()> {
    let (width, height) = window::client_size(hwnd);
    unsafe {
        controller.SetBounds(RECT {
            left: 0,
            top: 0,
            right: width,
            bottom: height,
        })?;
    }
    Ok(())
}

fn set_background(
    controller: &ICoreWebView2Controller,
    mica: bool,
    dark: bool,
) -> anyhow::Result<()> {
    let color = if mica {
        COREWEBVIEW2_COLOR {
            A: 0,
            R: 0,
            G: 0,
            B: 0,
        }
    } else if dark {
        COREWEBVIEW2_COLOR {
            A: 255,
            R: 0,
            G: 0,
            B: 0,
        }
    } else {
        COREWEBVIEW2_COLOR {
            A: 255,
            R: 255,
            G: 255,
            B: 255,
        }
    };
    if let Ok(controller2) = controller.cast::<ICoreWebView2Controller2>() {
        unsafe { controller2.SetDefaultBackgroundColor(color) }?;
    }
    Ok(())
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
