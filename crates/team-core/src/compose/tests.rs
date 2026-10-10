use super::*;

#[test]
fn channel_members_all_expands() {
    let all = ChannelMembers::All("*".into());
    assert!(all.includes("dev1", &["dev1", "dev2"]));
    assert!(!all.includes("ghost", &["dev1", "dev2"]));
}

#[test]
fn channel_members_explicit_checks_list() {
    let exp = ChannelMembers::Explicit(vec!["dev1".into(), "critic".into()]);
    assert!(exp.includes("dev1", &[]));
    assert!(!exp.includes("dev2", &[]));
}

#[test]
fn agent_defaults_are_stable() {
    let a: Agent = serde_yaml::from_str("model: claude-opus-4-8\n").unwrap();
    assert_eq!(a.runtime, "claude-code");
    assert_eq!(a.autonomy, "low_risk_only");
    assert!(a.interfaces.is_none());
    assert!(a.telegram().is_none());
    assert!(a.effort.is_none());
}

#[test]
fn agent_telegram_block_parses_under_interfaces() {
    let yaml = "interfaces:\n  telegram:\n    bot_token_env: T\n    chat_ids_env: C\n";
    let a: Agent = serde_yaml::from_str(yaml).unwrap();
    let tg = a.telegram().expect("telegram parsed");
    assert_eq!(tg.bot_token_env, "T");
    assert_eq!(tg.chat_ids_env, "C");
    assert!(tg.speech_to_text.is_none());
}

#[test]
fn agent_telegram_block_parses_speech_to_text() {
    let yaml = "\
interfaces:
  telegram:
    bot_token_env: T
    chat_ids_env: C
    speech_to_text:
      provider: groq
      api_key_env: GROQ_API_KEY
      model: whisper-large-v3
      language: en
";
    let a: Agent = serde_yaml::from_str(yaml).unwrap();
    let stt = a
        .telegram()
        .and_then(|t| t.speech_to_text.as_ref())
        .expect("speech_to_text parsed");
    assert_eq!(stt.provider, "groq");
    assert_eq!(stt.api_key_env, "GROQ_API_KEY");
    assert_eq!(stt.model, "whisper-large-v3");
    assert_eq!(stt.language.as_deref(), Some("en"));
}

#[test]
fn agent_telegram_block_parses_speech_to_text_without_language() {
    let yaml = "\
interfaces:
  telegram:
    bot_token_env: T
    chat_ids_env: C
    speech_to_text:
      provider: groq
      api_key_env: K
      model: whisper-large-v3
";
    let a: Agent = serde_yaml::from_str(yaml).unwrap();
    let stt = a
        .telegram()
        .and_then(|t| t.speech_to_text.as_ref())
        .expect("speech_to_text parsed");
    assert!(stt.language.is_none());
}

#[test]
fn effort_parses_all_five_levels() {
    for (yaml, expected) in [
        ("effort: low\n", EffortLevel::Low),
        ("effort: medium\n", EffortLevel::Medium),
        ("effort: high\n", EffortLevel::High),
        ("effort: xhigh\n", EffortLevel::Xhigh),
        ("effort: max\n", EffortLevel::Max),
    ] {
        let a: Agent = serde_yaml::from_str(yaml).expect(yaml);
        assert_eq!(a.effort, Some(expected), "yaml: {yaml}");
    }
}

#[test]
fn ultracode_parses_from_yaml_and_defaults_false() {
    // #461: the opt-in must round-trip through the YAML parse path (the
    // render tests set the field by struct mutation, so this pins the
    // serde wiring the operator actually hits).
    let on: Agent = serde_yaml::from_str("ultracode: true\n").expect("ultracode: true");
    assert!(on.ultracode, "ultracode: true must parse to true");
    let off: Agent = serde_yaml::from_str("ultracode: false\n").expect("ultracode: false");
    assert!(!off.ultracode, "ultracode: false must parse to false");
    // Absent key: #[serde(default)] on a bool defaults to false.
    let absent: Agent = serde_yaml::from_str("effort: low\n").expect("no ultracode key");
    assert!(!absent.ultracode, "omitted ultracode must default to false");
}

#[test]
fn effort_unknown_value_is_rejected() {
    let err = serde_yaml::from_str::<Agent>("effort: hgih\n")
        .expect_err("typo'd effort value must fail to parse");
    let msg = err.to_string();
    assert!(
        msg.contains("low") && msg.contains("max"),
        "error should enumerate valid variants; got: {msg}"
    );
}

#[test]
fn effort_renders_to_lowercase_string() {
    assert_eq!(EffortLevel::Low.as_str(), "low");
    assert_eq!(EffortLevel::Xhigh.as_str(), "xhigh");
    assert_eq!(EffortLevel::Max.as_str(), "max");
}

#[test]
fn discover_prefers_dot_team() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = tmp.path();
    std::fs::create_dir_all(repo.join(".team")).unwrap();
    std::fs::write(repo.join(".team/team-compose.yaml"), "version: 2\n").unwrap();
    // a stray flat-layout file in the same dir should NOT be preferred.
    std::fs::write(repo.join("team-compose.yaml"), "version: 2\n").unwrap();

    // Walking up from a sub-dir should still find the .team/ root.
    let sub = repo.join("src/deep/nested");
    std::fs::create_dir_all(&sub).unwrap();
    let found = Compose::discover(&sub).unwrap();
    assert_eq!(found, repo.canonicalize().unwrap().join(".team"));
}

#[test]
fn discover_no_longer_falls_back_to_flat_layout() {
    // T-008: a flat `team-compose.yaml` at cwd (no `.team/` wrapper) is
    // not discoverable. The convention is `.team/`. Operators must
    // either `init` a `.team/` or pass `--root` explicitly.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("team-compose.yaml"), "version: 2\n").unwrap();
    let err = Compose::discover(tmp.path()).unwrap_err();
    assert!(err.to_string().contains("no `.team/team-compose.yaml`"));
}

#[test]
fn discover_returns_first_dot_team_walking_up() {
    // T-008 boundary: nested `.team/`s win over outer ones. We do NOT
    // keep walking past the first hit.
    let tmp = tempfile::tempdir().unwrap();
    let outer = tmp.path();
    let inner = outer.join("packages/inner");
    std::fs::create_dir_all(outer.join(".team")).unwrap();
    std::fs::write(outer.join(".team/team-compose.yaml"), "version: 2\n").unwrap();
    std::fs::create_dir_all(inner.join(".team")).unwrap();
    std::fs::write(inner.join(".team/team-compose.yaml"), "version: 2\n").unwrap();

    let from_inner = inner.join("src/deep");
    std::fs::create_dir_all(&from_inner).unwrap();
    let found = Compose::discover(&from_inner).unwrap();
    assert_eq!(found, inner.canonicalize().unwrap().join(".team"));
}

#[test]
fn discover_errors_when_nothing_found() {
    let tmp = tempfile::tempdir().unwrap();
    let err = Compose::discover(tmp.path()).unwrap_err();
    assert!(err.to_string().contains("no `.team/team-compose.yaml`"));
}

#[test]
fn role_prompt_parses_single_string_form() {
    let yaml = "role_prompt: roles/mgr.md\n";
    let agent: Agent = serde_yaml::from_str(&format!(
        "runtime: claude-code\nautonomy: low_risk_only\n{yaml}"
    ))
    .unwrap();
    match agent.role_prompt.unwrap() {
        RolePrompt::Single(p) => assert_eq!(p, PathBuf::from("roles/mgr.md")),
        other => panic!("expected Single, got {other:?}"),
    }
}

#[test]
fn role_prompt_parses_list_form() {
    let yaml = "role_prompt:\n  - roles/_base.md\n  - roles/mgr.md\n";
    let agent: Agent = serde_yaml::from_str(&format!(
        "runtime: claude-code\nautonomy: low_risk_only\n{yaml}"
    ))
    .unwrap();
    match agent.role_prompt.unwrap() {
        RolePrompt::Multiple(v) => assert_eq!(
            v,
            vec![
                PathBuf::from("roles/_base.md"),
                PathBuf::from("roles/mgr.md"),
            ]
        ),
        other => panic!("expected Multiple, got {other:?}"),
    }
}

#[test]
fn role_prompt_paths_returns_declared_order() {
    let rp = RolePrompt::Multiple(vec![
        PathBuf::from("a.md"),
        PathBuf::from("b.md"),
        PathBuf::from("c.md"),
    ]);
    let got: Vec<&Path> = rp.paths();
    assert_eq!(
        got,
        vec![Path::new("a.md"), Path::new("b.md"), Path::new("c.md")]
    );
}

// T-265 PR-a: SchemaVersion Deserialize semantics. The owner-
// ratified contract is: accept YAML string verbatim; accept the
// single legacy integer `2` (coerce to "2.0.0", flag
// `from_legacy_int = true` so Compose::load knows to rewrite
// the file); reject anything else with a message naming the
// constraint.

#[test]
fn schema_version_accepts_semver_string() {
    let v: SchemaVersion = serde_yaml::from_str("\"2.0.0\"").unwrap();
    assert_eq!(v.value, "2.0.0");
    assert!(!v.from_legacy_int, "string form is NOT the legacy path");
}

#[test]
fn schema_version_accepts_arbitrary_semver_string_for_later_validation() {
    // Deserialize doesn't enforce the semver shape — validate
    // does. So `"abc"` parses fine here; the validate-time check
    // rejects it later. Test pins this contract — keeps the
    // deserialize impl narrow.
    let v: SchemaVersion = serde_yaml::from_str("\"abc\"").unwrap();
    assert_eq!(v.value, "abc");
}

#[test]
fn schema_version_coerces_legacy_integer_two() {
    let v: SchemaVersion = serde_yaml::from_str("2").unwrap();
    assert_eq!(v.value, "2.0.0");
    assert!(v.from_legacy_int, "integer-2 must be flagged for rewrite");
}

#[test]
fn schema_version_rejects_other_integers() {
    // The hardcoded-legacy-exception is EXACTLY `2`. Anything
    // else (`1`, `3`, `99`) hard-errors with a message naming
    // the constraint.
    for n in [0u64, 1, 3, 99] {
        let err = serde_yaml::from_str::<SchemaVersion>(&n.to_string())
            .expect_err("non-2 integer must fail");
        let msg = err.to_string();
        assert!(
            msg.contains("only legacy `2` is auto-coerced"),
            "error must name the constraint; got: {msg}"
        );
    }
}

#[test]
fn schema_version_rejects_non_string_non_int_shapes() {
    // Booleans, lists, mappings — none of them are a version.
    for yaml in ["true", "[1,2,3]", "{a: b}"] {
        let res = serde_yaml::from_str::<SchemaVersion>(yaml);
        assert!(res.is_err(), "yaml `{yaml}` must fail to deserialize");
    }
}

/// T-265 PR-a: Compose::load orchestration test — legacy `2`
/// file gets auto-rewritten to the semver string AND the
/// in-memory representation is `"2.0.0"` + `from_legacy_int =
/// true` (the flag the load logic reads to decide whether to
/// rewrite). The on-disk content after load must be `version:
/// "2.0.0"`, comments preserved.
#[test]
fn load_rewrites_legacy_version_two_in_file_and_in_memory() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".team");
    std::fs::create_dir_all(&root).unwrap();
    let yaml = "\
# T-265 fixture — legacy version
version: 2
broker:
  type: sqlite
  path: state/mailbox.db
";
    std::fs::write(root.join("team-compose.yaml"), yaml).unwrap();
    let compose = Compose::load(&root).expect("load succeeds on legacy file");
    // In-memory: normalized + flagged legacy.
    assert_eq!(compose.global.version.value, "2.0.0");
    // On-disk: rewritten to the semver string.
    let after = std::fs::read_to_string(root.join("team-compose.yaml")).unwrap();
    assert!(
        after.contains("version: \"2.0.0\""),
        "file must be rewritten;\n{after}"
    );
    assert!(
        !after.contains("\nversion: 2\n"),
        "no legacy literal must survive;\n{after}"
    );
    // Comment preserved.
    assert!(
        after.contains("# T-265 fixture"),
        "comment must survive the rewrite;\n{after}"
    );
    // broker block survives.
    assert!(after.contains("type: sqlite"));
}

#[test]
fn load_leaves_canonical_semver_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".team");
    std::fs::create_dir_all(&root).unwrap();
    let yaml = "\
version: \"2.0.0\"
broker:
  type: sqlite
";
    std::fs::write(root.join("team-compose.yaml"), yaml).unwrap();
    let compose = Compose::load(&root).expect("load succeeds");
    assert_eq!(compose.global.version.value, "2.0.0");
    assert!(
        !compose.global.version.from_legacy_int,
        "canonical file must NOT be flagged for rewrite"
    );
    // File content byte-identical (no auto-rewrite when not legacy).
    let after = std::fs::read_to_string(root.join("team-compose.yaml")).unwrap();
    assert_eq!(after, yaml, "canonical file must NOT be mutated on load");
}

#[test]
fn read_only_load_normalizes_legacy_version_without_rewriting_any_compose_file() {
    let directory = tempfile::tempdir().unwrap();
    let manifest = "# preserve preview bytes\nversion: 2\nprojects:\n  - file: project.yaml\n";
    let project = "version: 2\nproject:\n  id: preview\n  name: Preview\n  cwd: .\nmanagers:\n  manager:\n    runtime: codex\n";
    std::fs::write(directory.path().join("team-compose.yaml"), manifest).unwrap();
    std::fs::write(directory.path().join("project.yaml"), project).unwrap();
    let compose = Compose::load_read_only(directory.path()).unwrap();
    assert_eq!(compose.global.version.value, "2.0.0");
    assert!(compose.global.version.from_legacy_int);
    assert_eq!(compose.agents().next().unwrap().id(), "preview:manager");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("team-compose.yaml")).unwrap(),
        manifest
    );
    assert_eq!(
        std::fs::read_to_string(directory.path().join("project.yaml")).unwrap(),
        project
    );
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 2);
}

#[test]
fn load_hard_errors_on_non_two_integer_version() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".team");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("team-compose.yaml"), "version: 3\n").unwrap();
    let err = Compose::load(&root).expect_err("must reject integer-3 at parse");
    assert!(
        err.to_string().contains("only legacy `2` is auto-coerced"),
        "error must name the constraint; got: {err}"
    );
}

/// #347 (regression guard, owner-ratified tg 3440): when the legacy-`2`
/// auto-rewrite cannot write the file (read-only filesystem: CI sandboxes,
/// immutable image mounts), `Compose::load` must NOT hard-error. It swallows
/// the rewrite failure, emits a single `tracing::warn`, and proceeds with
/// the in-memory normalized value. This pins the exact regression flagged in
/// the #346 review: a future refactor flipping the rewrite's `if let Err`
/// into `?` would silently turn the graceful degrade into a hard error, and
/// nothing else would catch it.
///
/// `#[cfg(unix)]` because the read-only condition is set via a unix `chmod`;
/// unix is where operators run teamctl. The write failure is simulated by
/// making the compose FILE read-only (`0o444`), which reproduces the
/// `std::fs::write` EACCES a read-only filesystem produces. The containing
/// dir is left writable so the tempdir teardown can still remove the file.
/// Caveat: skipped behavior is undefined under `sudo` (root bypasses the
/// file mode); CI runners are non-root.
#[cfg(unix)]
#[test]
fn load_degrades_to_in_memory_when_legacy_rewrite_cannot_write() {
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::{Arc, Mutex};

    // A `tracing` writer that captures formatted events into a buffer, so
    // the test can assert the degrade's warn fired (acceptance b).
    struct CaptureWriter(Arc<Mutex<Vec<u8>>>);
    impl Write for CaptureWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join(".team");
    std::fs::create_dir_all(&root).unwrap();
    let compose_path = root.join("team-compose.yaml");
    std::fs::write(
        &compose_path,
        "version: 2\nbroker:\n  type: sqlite\n  path: state/mailbox.db\n",
    )
    .unwrap();

    // Read-only file: the load can still read it, but the legacy-`2`
    // in-place rewrite (`std::fs::write`) fails with EACCES.
    std::fs::set_permissions(&compose_path, std::fs::Permissions::from_mode(0o444)).unwrap();

    // Capture WARN-and-above tracing for the duration of the load only
    // (a thread-scoped default subscriber, never global).
    let logs = Arc::new(Mutex::new(Vec::<u8>::new()));
    let sink = logs.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || CaptureWriter(sink.clone()))
        .finish();
    let result = tracing::subscriber::with_default(subscriber, || Compose::load(&root));

    // Restore write perms before any assertion can unwind, independent of
    // the tempdir teardown, so a failing assert never leaves a stuck file.
    std::fs::set_permissions(&compose_path, std::fs::Permissions::from_mode(0o644)).unwrap();

    // (a) No error: the rewrite failure degrades, it does not bubble up.
    let compose = result.expect("a read-only compose file must degrade, not error");
    // (c) In-memory state is normalized despite the un-rewritable file.
    assert_eq!(compose.global.version.value, "2.0.0");
    assert!(
        compose.global.version.from_legacy_int,
        "legacy-int normalization must still be flagged in memory"
    );
    // (b) The degrade is observable: the warn fired, not a silent swallow.
    let captured = String::from_utf8(logs.lock().unwrap().clone()).unwrap();
    assert!(
        captured.contains("could not rewrite legacy"),
        "the read-only degrade must emit its tracing::warn; captured:\n{captured}"
    );
    // And the rewrite genuinely did not happen (degrade is real, not a
    // silently-succeeded write): the legacy literal still on disk.
    let after = std::fs::read_to_string(&compose_path).unwrap();
    assert!(
        after.contains("version: 2"),
        "the file must be left un-rewritten on the degrade path;\n{after}"
    );
}

// ── #132 PR-1: Project.interfaces.telegram schema ──────────────

/// Minimal Project YAML head for the new-schema tests. Each test
/// appends its own `interfaces:` block (or omits it for the
/// zero-touch baseline). Mirrors the per-agent test fixture
/// pattern but at one level up.
const PROJECT_YAML_HEAD: &str = "\
version: 2
project:
  id: p
  name: P
  cwd: .
";

#[test]
fn project_without_interfaces_block_parses_unchanged() {
    // Zero-touch baseline: existing project YAMLs (which today have
    // no `interfaces:` block) keep parsing exactly as before.
    let p: Project = serde_yaml::from_str(PROJECT_YAML_HEAD).unwrap();
    assert!(p.interfaces.is_none());
    assert!(p.telegram().is_none());
}

#[test]
fn project_telegram_block_parses_under_interfaces() {
    // Mirror precedent: `agent_telegram_block_parses_under_interfaces`
    // at compose.rs:691. Both `manager_bot` and `profile_picture`
    // present, exercises the full top-level accessor path.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    manager_bot:
      token_env: TEAMCTL_TG_MANAGER_TOKEN
    profile_picture:
      image_model:
        provider: openai
        api_key_env: OPENAI_API_KEY
        model: gpt-image-2
      fallback: initials
"
    );
    let p: Project = serde_yaml::from_str(&yaml).unwrap();
    let tg = p.telegram().expect("project telegram parsed");
    let mb = tg.manager_bot.as_ref().expect("manager_bot parsed");
    assert_eq!(mb.token_env, "TEAMCTL_TG_MANAGER_TOKEN");
    let pp = tg.profile_picture.as_ref().expect("profile_picture parsed");
    let im = pp.image_model.as_ref().expect("image_model parsed");
    assert_eq!(im.provider, "openai");
    assert_eq!(im.api_key_env, "OPENAI_API_KEY");
    assert_eq!(im.model, "gpt-image-2");
    assert_eq!(pp.fallback, ProfilePictureFallback::Initials);
}

#[test]
fn project_telegram_block_parses_manager_bot_only() {
    // Realistic case: operator opts into managed-bots flow without
    // configuring AI profile pictures (uses the initials fallback
    // by absence-of-image-model). Each sub-block is independently
    // optional.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    manager_bot:
      token_env: TEAMCTL_TG_MANAGER_TOKEN
"
    );
    let p: Project = serde_yaml::from_str(&yaml).unwrap();
    let tg = p.telegram().expect("project telegram parsed");
    assert_eq!(
        tg.manager_bot.as_ref().unwrap().token_env,
        "TEAMCTL_TG_MANAGER_TOKEN"
    );
    assert!(tg.profile_picture.is_none());
}

#[test]
fn profile_picture_fallback_defaults_to_initials_when_omitted() {
    // Q3 contract: omitting `fallback:` is operator-readable as
    // "use the v1 default", which is initials. Future variants
    // slot in without breaking this default.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    profile_picture:
      image_model:
        provider: openai
        api_key_env: OPENAI_API_KEY
        model: gpt-image-2
"
    );
    let p: Project = serde_yaml::from_str(&yaml).unwrap();
    let pp = p
        .telegram()
        .and_then(|t| t.profile_picture.as_ref())
        .expect("profile_picture parsed");
    assert_eq!(pp.fallback, ProfilePictureFallback::Initials);
}

#[test]
fn profile_picture_image_model_optional_with_initials_fallback() {
    // Initials-only path: no AI generation configured, the
    // fallback is the entire rendering. Pins the (b) Q3-ratified
    // shape end to end at the schema level.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    profile_picture:
      fallback: initials
"
    );
    let p: Project = serde_yaml::from_str(&yaml).unwrap();
    let pp = p
        .telegram()
        .and_then(|t| t.profile_picture.as_ref())
        .expect("profile_picture parsed");
    assert!(pp.image_model.is_none());
    assert_eq!(pp.fallback, ProfilePictureFallback::Initials);
}

#[test]
fn manager_bot_missing_token_env_rejected() {
    // `token_env` is required (no `#[serde(default)]`). A YAML
    // missing it must reject at parse with a clear error so a
    // malformed setup is caught before it reaches the wizard.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    manager_bot: {{}}
"
    );
    let err =
        serde_yaml::from_str::<Project>(&yaml).expect_err("malformed manager_bot must reject");
    assert!(
        err.to_string().contains("token_env"),
        "error must name the missing field: {err}"
    );
}

#[test]
fn image_model_missing_required_fields_rejected() {
    // All three fields (provider, api_key_env, model) are
    // required. A YAML missing any of them must reject — mirrors
    // SttConfig's required-fields contract.
    for (label, yaml_fragment) in [
        ("missing provider", "api_key_env: K\n        model: M"),
        ("missing api_key_env", "provider: openai\n        model: M"),
        ("missing model", "provider: openai\n        api_key_env: K"),
    ] {
        let yaml = format!(
            "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    profile_picture:
      image_model:
        {yaml_fragment}
"
        );
        let result = serde_yaml::from_str::<Project>(&yaml);
        assert!(
            result.is_err(),
            "malformed image_model ({label}) must reject, got: {result:?}"
        );
    }
}

#[test]
fn profile_picture_fallback_unknown_value_rejected() {
    // Mirror precedent: `effort_unknown_value_is_rejected` at
    // compose.rs. An unknown enum variant must reject at parse so
    // typos surface immediately rather than silently defaulting.
    let yaml = format!(
        "{PROJECT_YAML_HEAD}\
interfaces:
  telegram:
    profile_picture:
      fallback: emoji
"
    );
    let err =
        serde_yaml::from_str::<Project>(&yaml).expect_err("unknown fallback variant must reject");
    assert!(
        err.to_string().contains("emoji") || err.to_string().contains("variant"),
        "error must explain the unknown variant: {err}"
    );
}
