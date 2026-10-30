use std::env;
use std::ffi::{OsStr, OsString};
use std::process::Command;

const EXACT_SANITIZED_KEYS: [&str; 4] = [
    "CODEX_THREAD_ID",
    "CODEX_PERMISSION_PROFILE",
    "WORKFLOW_FABRIC_CONTROLLED_CODEX_LAUNCHER_ERROR",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
];

pub fn key_is_sanitized(key: &OsStr) -> bool {
    let normalized = key.to_string_lossy().to_ascii_uppercase();
    normalized.starts_with("CODEX_INTERNAL_")
        || EXACT_SANITIZED_KEYS
            .iter()
            .any(|candidate| *candidate == normalized)
}

pub fn current_sanitized_keys() -> Vec<OsString> {
    let mut keys = env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| key_is_sanitized(key))
        .collect::<Vec<_>>();
    keys.sort_by_key(|key| key.to_string_lossy().to_ascii_uppercase());
    keys.dedup_by(|left, right| {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    });
    keys
}

pub fn sanitize_command(command: &mut Command, keys: &[OsString]) {
    for key in keys {
        command.env_remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_task_level_codex_context_is_sanitized() {
        for key in [
            "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
            "codex_internal_future_flag",
            "CODEX_THREAD_ID",
            "CODEX_PERMISSION_PROFILE",
            "WORKFLOW_FABRIC_CONTROLLED_CODEX_LAUNCHER_ERROR",
        ] {
            assert!(key_is_sanitized(OsStr::new(key)), "{key}");
        }
        for key in [
            "CODEX_HOME",
            "CODEX_API_KEY",
            "OPENAI_API_KEY",
            "PATH",
            "ANTHROPIC_API_KEY",
        ] {
            assert!(!key_is_sanitized(OsStr::new(key)), "{key}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn sanitized_command_does_not_inherit_private_values() {
        let private_key = OsString::from("CODEX_INTERNAL_COMPANION_TEST");
        let preserved_key = OsString::from("CODEX_HOME");
        let mut command = Command::new("cmd.exe");
        command
            .args(["/d", "/c", "set"])
            .env(&private_key, "PRIVATE_VALUE")
            .env(&preserved_key, "PRESERVED_VALUE");
        sanitize_command(&mut command, std::slice::from_ref(&private_key));
        let output = command.output().expect("run cmd set");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(!stdout.contains("CODEX_INTERNAL_COMPANION_TEST="));
        assert!(stdout.contains("CODEX_HOME=PRESERVED_VALUE"));
    }
}
