use super::*;

#[test]
fn unicode_insert_motion_backspace_and_line_split_are_character_safe() {
    let mut editor = Editor::default();
    for character in "éسلام🙂".chars() {
        editor.apply_key(k(KeyCode::Char(character)));
    }
    assert_eq!(editor.cursor_col, "éسلام🙂".len());
    editor.apply_key(k(KeyCode::Left));
    assert_eq!(editor.cursor_col, "éسلام".len());
    editor.apply_key(k(KeyCode::Backspace));
    assert_eq!(editor.body(), "éسلا🙂");
    editor.apply_key(k(KeyCode::Enter));
    assert_eq!(editor.lines, ["éسلا", "🙂"]);
    editor.apply_key(k(KeyCode::Backspace));
    assert_eq!(editor.lines, ["éسلا🙂"]);
    assert!(editor.lines[0].is_char_boundary(editor.cursor_col));
}

#[test]
fn unicode_vertical_and_word_motions_never_leave_byte_boundaries() {
    let mut editor = Editor {
        lines: vec!["abc".into(), "🙂你好 world".into()],
        cursor_col: 3,
        ..Default::default()
    };
    editor.apply_key(k(KeyCode::Down));
    assert_eq!(editor.cursor_col, 0, "three bytes would split the emoji");
    editor.apply_key(k(KeyCode::Esc));
    for motion in ['w', 'e', 'b', 'l', 'h', '$', 'b', 'w'] {
        editor.apply_key(k(KeyCode::Char(motion)));
        assert!(editor.lines[editor.cursor_row].is_char_boundary(editor.cursor_col));
    }
    editor.cursor_col = "🙂".len();
    editor.apply_key(k(KeyCode::Char('e')));
    assert_eq!(editor.cursor_col, "🙂你".len());
    editor.apply_key(k(KeyCode::Char('w')));
    assert_eq!(editor.cursor_col, "🙂你好 ".len());
}

#[test]
fn viewport_tracks_long_drafts_using_display_width() {
    let editor = Editor {
        lines: vec!["你好🙂abc".into(); 30],
        cursor_row: 29,
        cursor_col: "你好🙂abc".len(),
        ..Default::default()
    };
    assert_eq!(editor.viewport(5, 10), (20, 5));
    assert_eq!(editor.viewport(0, 0), (29, 9));
}

#[test]
fn renderer_clamps_an_external_non_boundary_cursor() {
    let editor = Editor {
        lines: vec!["🙂é".into()],
        cursor_col: 2,
        ..Default::default()
    };
    assert_eq!(editor.cursor_byte_col(), 0);
}

#[test]
fn control_letters_do_not_inject_text_in_insert_mode() {
    let mut editor = Editor::default();
    editor.apply_key(k_ctrl(KeyCode::Char('c')));
    assert!(editor.body().is_empty());
}

#[test]
fn broadcast_adapter_retains_project_qualifier_and_literal_body() {
    let command = message_command(
        Path::new("/tmp/team root"),
        "broadcast",
        "project-two:all",
        "hello\n$(literal)",
    );
    let args: Vec<_> = command
        .get_args()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        args,
        [
            "--root",
            "/tmp/team root",
            "broadcast",
            "--",
            "project-two:all",
            "hello\n$(literal)"
        ]
    );
}

#[test]
fn message_adapters_preserve_flag_shaped_bodies() {
    for verb in ["send", "broadcast"] {
        let command = message_command(Path::new("/tmp/team"), verb, "one:all", "--help");
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(&args[3..], &["--", "one:all", "--help"]);
    }
}
