//! Windows WebView2 runtime detection and install prompt.
//! macOS uses WKWebView (built-in); Linux is out of scope for v1.
//! Dialog text is English-first (app default locale), with Chinese below.

#[cfg(windows)]
pub const WEBVIEW2_DOWNLOAD_URL: &str =
    "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Returns `true` when the app may continue (runtime present, or non-Windows).
pub fn ensure_or_prompt() -> bool {
    #[cfg(windows)]
    {
        if is_installed() {
            return true;
        }
        prompt_and_open_download();
        false
    }
    #[cfg(not(windows))]
    {
        true
    }
}

#[cfg(windows)]
fn is_installed() -> bool {
    const CLIENT_GUID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";
    let relative = format!(r"Microsoft\EdgeUpdate\Clients\{CLIENT_GUID}");
    let keys = [
        (
            winreg::enums::HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\WOW6432Node\{relative}"),
        ),
        (
            winreg::enums::HKEY_LOCAL_MACHINE,
            format!(r"SOFTWARE\{relative}"),
        ),
        (
            winreg::enums::HKEY_CURRENT_USER,
            format!(r"SOFTWARE\{relative}"),
        ),
    ];

    for (hive, path) in keys {
        let Ok(key) = winreg::RegKey::predef(hive).open_subkey(&path) else {
            continue;
        };
        let Ok(version) = key.get_value::<String, _>("pv") else {
            continue;
        };
        if !version.is_empty() && version != "0.0.0.0" {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn prompt_and_open_download() {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use windows::{
        Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
        },
        core::w,
    };

    let _ = Command::new("cmd")
        .args(["/C", "start", "", WEBVIEW2_DOWNLOAD_URL])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .spawn();

    unsafe {
        MessageBoxW(
            None,
            w!("ClipboardShare requires the Microsoft Edge WebView2 Runtime.\n\n\
The official download has been opened (Evergreen Bootstrapper):\n\
https://go.microsoft.com/fwlink/p/?LinkId=2124703\n\n\
Install it, then restart this app.\n\n\
——\n\n\
ClipboardShare 需要 Microsoft Edge WebView2 Runtime。\n\
已打开官方下载地址，请安装完成后重新启动本程序。\n\n\
More info: https://developer.microsoft.com/microsoft-edge/webview2/"),
            w!("ClipboardShare — Missing WebView2"),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

/// Fallback when Tauri/WebView creation fails even if registry looked fine.
#[cfg(windows)]
pub fn prompt_runtime_failure(error: &str) {
    use std::os::windows::process::CommandExt;
    use std::process::Command;
    use windows::{
        Win32::UI::WindowsAndMessaging::{
            MB_ICONERROR, MB_OK, MB_SETFOREGROUND, MB_TOPMOST, MessageBoxW,
        },
        core::{HSTRING, w},
    };

    let looks_like_webview = error.to_ascii_lowercase().contains("webview")
        || error.to_ascii_lowercase().contains("web view")
        || error.contains("0x80070002")
        || error.contains("0x8007139F");

    if !looks_like_webview && is_installed() {
        let text = HSTRING::from(format!(
            "ClipboardShare failed to start:\n\n{error}\n\n——\n\nClipboardShare 启动失败：\n{error}"
        ));
        unsafe {
            MessageBoxW(
                None,
                &text,
                w!("ClipboardShare"),
                MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
            );
        }
        return;
    }

    let _ = Command::new("cmd")
        .args(["/C", "start", "", WEBVIEW2_DOWNLOAD_URL])
        .creation_flags(0x0800_0000)
        .spawn();

    let text = HSTRING::from(format!(
        "ClipboardShare could not create a window. This is often caused by a missing or broken WebView2 Runtime.\n\n\
The download page has been opened:\n\
https://go.microsoft.com/fwlink/p/?LinkId=2124703\n\n\
Details:\n{error}\n\n——\n\n\
ClipboardShare 无法创建窗口，通常是缺少或损坏的 WebView2 Runtime。\n\
已打开官方下载页，请安装或修复后重试。"
    ));
    unsafe {
        MessageBoxW(
            None,
            &text,
            w!("ClipboardShare — Missing WebView2"),
            MB_OK | MB_ICONERROR | MB_TOPMOST | MB_SETFOREGROUND,
        );
    }
}

#[cfg(not(windows))]
pub fn prompt_runtime_failure(_error: &str) {}
