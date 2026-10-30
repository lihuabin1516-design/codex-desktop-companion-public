#![cfg_attr(windows, windows_subsystem = "windows")]

use std::env;
use std::ffi::OsString;
use std::process;

use codex_ccpanes::launcher::{launch, LaunchRequest};

fn main() {
    let arguments = env::args_os().skip(1).collect::<Vec<OsString>>();
    if let Err(error) = launch(LaunchRequest {
        explicit_target: None,
        arguments,
        wait: false,
    }) {
        show_error(&format!("CC-Panes 安全启动失败：\n\n{error:#}"));
        process::exit(2);
    }
}

#[cfg(windows)]
fn show_error(message: &str) {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};

    let title = std::ffi::OsStr::new("Codex + CC-Panes")
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let body = std::ffi::OsStr::new(message)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    unsafe {
        MessageBoxW(
            std::ptr::null_mut(),
            body.as_ptr(),
            title.as_ptr(),
            MB_OK | MB_ICONERROR,
        );
    }
}

#[cfg(not(windows))]
fn show_error(message: &str) {
    eprintln!("{message}");
}
