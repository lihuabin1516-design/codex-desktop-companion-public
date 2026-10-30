#![cfg(windows)]

use std::fs;
use std::process::Command;
use std::thread;
use std::time::Duration;

#[test]
fn start_command_sanitizes_the_real_child_process_boundary() {
    let root = tempfile::tempdir().expect("tempdir");
    let executable = env!("CARGO_BIN_EXE_codex-ccpanes");
    let output = Command::new(executable)
        .args([
            "start",
            "--ccpanes-bin",
            r"C:\Windows\System32\cmd.exe",
            "--wait",
            "--",
            "/d",
            "/c",
            "set",
        ])
        .env("CODEX_CCPANES_HOME", root.path().join("home"))
        .env("CODEX_INTERNAL_BOUNDARY_TEST", "PRIVATE_VALUE")
        .env("CODEX_THREAD_ID", "PRIVATE_THREAD")
        .env("CODEX_HOME", "PRESERVED_HOME")
        .output()
        .expect("run isolated child");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let child_environment = String::from_utf8_lossy(&output.stdout);
    assert!(!child_environment.contains("CODEX_INTERNAL_BOUNDARY_TEST="));
    assert!(!child_environment.contains("CODEX_THREAD_ID="));
    assert!(child_environment.contains("CODEX_HOME=PRESERVED_HOME"));
}

#[test]
fn gui_shortcut_launcher_sanitizes_the_detached_child_boundary() {
    let root = tempfile::tempdir().expect("tempdir");
    let output_path = root.path().join("gui-child-environment.txt");
    let script_path = root.path().join("write-environment.cmd");
    let executable = env!("CARGO_BIN_EXE_CodexCCPanes");
    fs::write(
        &script_path,
        "@echo off\r\n\
         > \"%CODEX_CCPANES_GUI_TEST_OUTPUT%\" echo %CODEX_INTERNAL_BOUNDARY_TEST%^|%CODEX_HOME%\r\n",
    )
    .expect("write cmd fixture");
    let status = Command::new(executable)
        .args(["/d", "/c"])
        .arg(&script_path)
        .env("CODEX_CCPANES_EXE", r"C:\Windows\System32\cmd.exe")
        .env("CODEX_CCPANES_HOME", root.path().join("home"))
        .env("CODEX_CCPANES_GUI_TEST_OUTPUT", &output_path)
        .env("CODEX_INTERNAL_BOUNDARY_TEST", "PRIVATE_VALUE")
        .env("CODEX_THREAD_ID", "PRIVATE_THREAD")
        .env("CODEX_HOME", "PRESERVED_HOME")
        .status()
        .expect("run GUI launcher");
    assert!(status.success());

    for _ in 0..100 {
        if output_path.is_file() {
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let child_environment =
        std::fs::read_to_string(output_path).expect("read GUI child environment fixture");
    assert_eq!(child_environment.trim(), "|PRESERVED_HOME");
}
