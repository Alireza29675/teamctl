//! Adapter credential references participate in the operator's doctor command.
use std::fs;
use std::process::Command;

#[test]
fn doctor_rejects_missing_process_adapter_credential_without_echoing_values() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("projects")).unwrap();
    fs::write(
        root.path().join("projects/p.yaml"),
        "version: 2\nproject: {id: p, name: Fixture, cwd: .}\nmanagers:\n  a: {runtime: codex}\n",
    )
    .unwrap();
    fs::write(root.path().join("team-compose.yaml"), "version: 2.0.0\nprojects: [{file: projects/p.yaml}]\ninterfaces:\n  - name: fixture\n    type: process\n    config:\n      agent: p:a\n      command: /not/executed\n      env: {PROVIDER_TOKEN: TEAMCTL_TEST_ADAPTER_CREDENTIAL}\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .args(["--root", root.path().to_str().unwrap(), "env", "--doctor"])
        .env_remove("TEAMCTL_TEST_ADAPTER_CREDENTIAL")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("TEAMCTL_TEST_ADAPTER_CREDENTIAL"));
    assert!(stdout.contains("UNSET"));
    assert!(stdout.contains("interfaces[fixture].config.env.PROVIDER_TOKEN"));
}
