use std::process::Command;

#[test]
fn describe_contract_is_machine_readable() {
    let executable = env!("CARGO_BIN_EXE_codex-paneforge");
    let output = Command::new(executable)
        .args(["describe", "--json"])
        .output()
        .expect("run describe");
    assert!(output.status.success());
    let value: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("parse descriptor json");
    assert_eq!(value["schemaVersion"], 1);
    assert_eq!(value["liveagentRuntimeDependency"], false);
    assert_eq!(value["upstreamPolicy"], "official-external-binaries");
}

#[test]
fn help_lists_production_commands() {
    let executable = env!("CARGO_BIN_EXE_codex-paneforge");
    let output = Command::new(executable)
        .arg("--help")
        .output()
        .expect("run help");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "start",
        "doctor",
        "probe",
        "audit-builds",
        "migrate-config",
        "describe",
    ] {
        assert!(stdout.contains(command), "missing {command}");
    }
}
