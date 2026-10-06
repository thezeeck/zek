#![cfg(unix)]

use std::process::{Command, Stdio};

#[test]
fn noninteractive_confirmation_does_not_execute_step() {
    let tmp = tempfile::tempdir().unwrap();
    let workdir = tmp.path().join("workspace");
    let config_dir = tmp.path().join("config");
    std::fs::create_dir_all(workdir.join("commands")).unwrap();
    std::fs::create_dir_all(workdir.join("flows")).unwrap();
    std::fs::create_dir_all(config_dir.join("zek")).unwrap();
    let marker = workdir.join("should-not-exist");
    std::fs::write(
        config_dir.join("zek/config.yaml"),
        serde_json::json!({"workdir": workdir}).to_string(),
    )
    .unwrap();
    std::fs::write(
        workdir.join("flows/confirm.yaml"),
        "name: confirm\nsteps:\n  - name: guarded\n    type: command\n    command: echo ran > should-not-exist\n    cwd: .\n    confirm: true\n",
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_zek"))
        .current_dir(&workdir)
        .env("XDG_CONFIG_HOME", &config_dir)
        .env("APPDATA", &config_dir)
        .args(["--report", "json", "confirm"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["skipped_steps"], serde_json::json!(["guarded"]));
    assert!(!marker.exists());
}
