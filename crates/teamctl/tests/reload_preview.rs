use std::process::Command;

#[test]
fn dry_run_preserves_legacy_manifest_bytes_without_creating_state() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join(".team");
    let home = directory.path().join("home");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    let manifest = "# legacy compose preview\nversion: 2\nprojects: []\n";
    std::fs::write(root.join("team-compose.yaml"), manifest).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--root")
        .arg(&root)
        .args(["reload", "--dry-run"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("team-compose.yaml")).unwrap(),
        manifest
    );
    assert_eq!(
        std::fs::read_dir(&root).unwrap().count(),
        1,
        "preview must not create state"
    );
    assert_eq!(
        std::fs::read_dir(&home).unwrap().count(),
        0,
        "preview must not create identity locks"
    );
}

#[test]
fn preview_collision_discovery_preserves_another_roots_legacy_manifest() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("preview");
    let other = directory.path().join("other");
    let home = directory.path().join("home");
    for path in [&root, &other, &home] {
        std::fs::create_dir_all(path).unwrap();
    }
    std::fs::write(root.join("team-compose.yaml"), "version: 2\nprojects: []\n").unwrap();
    let manifest = "# other root stays untouched\nversion: 2\nprojects:\n  - file: project.yaml\n";
    std::fs::write(other.join("team-compose.yaml"), manifest).unwrap();
    std::fs::write(other.join("project.yaml"), "version: 2\nproject:\n  id: other\n  name: Other\n  cwd: .\nmanagers:\n  manager:\n    runtime: codex\n").unwrap();
    team_core::registry::upsert(
        &home.join(".config/teamctl"),
        team_core::registry::TeamEntry {
            project_id: "other".into(),
            root: other.clone(),
            tmux_prefix: "t-".into(),
            agents: vec!["manager".into()],
            ..Default::default()
        },
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_teamctl"))
        .arg("--root")
        .arg(&root)
        .args(["reload", "--dry-run"])
        .env("HOME", &home)
        .env("USERPROFILE", &home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(other.join("team-compose.yaml")).unwrap(),
        manifest
    );
    assert_eq!(std::fs::read_dir(&other).unwrap().count(), 2);
}
