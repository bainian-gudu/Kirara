//! 崩溃收尾：panic 时把诊断写进标准错误与日志文件，必要时拉起独立的
//! 崩溃提示进程（本体 `panic = "abort"` 后会消失，提示进程仍存活），随后终止。

fn hide_process_windows() {
    use windows::core::BOOL;
    use windows::Win32::Foundation::LPARAM;
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, ShowWindow, SW_HIDE,
    };

    unsafe extern "system" fn hide_window(
        hwnd: windows::Win32::Foundation::HWND,
        pid: LPARAM,
    ) -> BOOL {
        let mut owner = 0u32;
        let _ = GetWindowThreadProcessId(hwnd, Some(&mut owner));
        if owner == pid.0 as u32 {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
        BOOL(1)
    }

    unsafe {
        let _ = EnumWindows(Some(hide_window), LPARAM(GetCurrentProcessId() as isize));
    }
}

/// `show_dialog`：silent 等无人值守场景为 false。
pub fn install_panic_hook(show_dialog: bool) {
    std::panic::set_hook(Box::new(move |info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "Box<dyn Any>".to_string());
        let location = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".to_string());
        let diagnostic = format!("kachina-installer crashed at {location}: {message}");
        // Keep a direct fallback because the panic may happen before the logger is initialized.
        {
            use std::io::Write;
            let _ = writeln!(std::io::stderr(), "{diagnostic}");
            let log_path = super::log::path();
            if let Ok(mut file) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(log_path)
            {
                let _ = writeln!(file, "error panic: {diagnostic}");
            }
        }
        if show_dialog {
            hide_process_windows();
            if let Ok(exe) = std::env::current_exe() {
                let _ = super::process::spawn(&exe, &["crash-dialog"], false);
            }
        }
        // Panic recovery is not implemented yet; never leave the installer host alive
        // after an unrecoverable panic on a worker thread.
        std::process::exit(1);
    }));
}
