use clap::Parser;

use super::*;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Chat(Args),
}

fn parse(args: &[&str]) -> Args {
    let Command::Chat(args) =
        Cli::try_parse_from(std::iter::once("teamctl").chain(args.iter().copied()))
            .unwrap()
            .command;
    args
}

#[test]
fn hybrid_parser_accepts_interactive_and_scriptable_commands() {
    for target in ["a", "p:a", "chat-deadbeef"] {
        let args = parse(&["chat", target]);
        assert_eq!(args.target.as_deref(), Some(target));
        assert!(args.action.is_none());
    }
    let args = parse(&[
        "chat",
        "send",
        "p:a",
        "سلام --help",
        "--request-id",
        "retry-1",
        "--json",
    ]);
    assert!(args.target.is_none());
    match args.action.unwrap() {
        Action::Send {
            target,
            text,
            request_id,
            json,
        } => {
            assert_eq!(target, "p:a");
            assert_eq!(text, "سلام --help");
            assert_eq!(request_id.as_deref(), Some("retry-1"));
            assert!(json);
        }
        _ => panic!("expected send"),
    }
    assert!(matches!(
        parse(&["chat", "history", "p:a", "--before", "9", "--limit", "2", "--json"]).action,
        Some(Action::History {
            before: Some(9),
            after: None,
            limit: 2,
            json: true,
            ..
        })
    ));
    assert!(matches!(
        parse(&["chat", "history", "p:a", "--after", "3"]).action,
        Some(Action::History {
            before: None,
            after: Some(3),
            limit: 100,
            ..
        })
    ));
    assert!(matches!(
        parse(&["chat", "follow", "p:a", "--json"]).action,
        Some(Action::Follow { json: true, .. })
    ));
    assert!(matches!(
        parse(&["chat", "list", "--json"]).action,
        Some(Action::List { json: true })
    ));
}

#[test]
fn parser_rejects_conflicting_history_cursors_and_preserves_hyphen_text() {
    assert!(Cli::try_parse_from([
        "teamctl", "chat", "history", "p:a", "--before", "2", "--after", "1"
    ])
    .is_err());
    assert!(
        matches!(parse(&["chat", "send", "p:a", "--literal"]).action,
        Some(Action::Send { text, .. }) if text == "--literal")
    );
    let args = parse(&["chat", "p:a", "list"]);
    assert!(args.target.is_some() && args.action.is_some());
    let error = run(Path::new("/nonexistent-chat-test-root"), args).unwrap_err();
    assert!(error.to_string().contains("choose interactive chat"));
}

fn database() -> rusqlite::Connection {
    let conn = chat::open(":memory:").unwrap();
    conn.execute_batch("INSERT INTO projects(id,name) VALUES('p','P'); INSERT INTO agents(id,project_id,role,runtime) VALUES('p:a','p','worker','fake');").unwrap();
    conn
}

#[test]
fn arrivals_and_own_send_are_read_in_order_without_cursor_skips() {
    let conn = database();
    let repo = Repository::new(&conn);
    let conversation = repo.open_local("p:a").unwrap();
    let first = repo.send_local(&conversation.id, "first", "first").unwrap();
    let mut transcript = transcript::Transcript::new(25, 100);
    assert_eq!(transcript.poll(&repo, &conversation.id).unwrap(), [first]);
    let reply = repo
        .reply(
            "p:a",
            Some(&conversation.id),
            None,
            "arrived while typing",
            "text",
            None,
            None,
        )
        .unwrap();
    let own = repo.send_local(&conversation.id, "next", "next").unwrap();
    assert_eq!(
        transcript.poll(&repo, &conversation.id).unwrap(),
        [reply.clone(), own.clone()]
    );
    assert!(transcript.poll(&repo, &conversation.id).unwrap().is_empty());
    assert!(message_text(&reply, &conversation).starts_with("[agent ·"));
    assert!(message_text(&own, &conversation).starts_with("[you ·"));
}

#[test]
fn follow_drains_large_backlogs_and_bounds_revision_cache() {
    let conn = database();
    let repo = Repository::new(&conn);
    let conversation = repo.open_local("p:a").unwrap();
    let mut transcript = transcript::Transcript::new(200, 200);
    assert!(transcript.poll(&repo, &conversation.id).unwrap().is_empty());
    let expected: Vec<_> = (0..525)
        .map(|index| {
            repo.send_local(&conversation.id, "message", &format!("request-{index}"))
                .unwrap()
                .id
        })
        .collect();
    let mut actual = Vec::new();
    for _ in 0..4 {
        actual.extend(
            transcript
                .poll(&repo, &conversation.id)
                .unwrap()
                .iter()
                .map(|row| row.id),
        );
        assert!(transcript.cached_revisions() <= 200);
    }
    assert_eq!(actual, expected);
    assert_eq!(transcript.cached_revisions(), 200);
    let last = expected.last().unwrap();
    conn.execute("UPDATE chat_messages SET status='failed',error='delivery offline',revision=revision+1 WHERE id=?1", [last]).unwrap();
    let revised = transcript.poll(&repo, &conversation.id).unwrap();
    assert_eq!(revised.len(), 1);
    assert_eq!(revised[0].id, *last);
    assert!(message_text(&revised[0], &conversation).contains("delivery: delivery offline"));
    assert!(transcript.poll(&repo, &conversation.id).unwrap().is_empty());
}

#[test]
fn startup_is_bounded_and_failed_reads_do_not_consume_arrivals() {
    let conn = database();
    let repo = Repository::new(&conn);
    let conversation = repo.open_local("p:a").unwrap();
    for index in 0..30 {
        repo.send_local(&conversation.id, "message", &index.to_string())
            .unwrap();
    }
    let mut transcript = transcript::Transcript::new(25, 100);
    let initial = transcript.poll(&repo, &conversation.id).unwrap();
    assert_eq!(initial.len(), 25);
    assert!(transcript.poll(&repo, "missing-conversation").is_err());
    let arrival = repo
        .reply(
            "p:a",
            Some(&conversation.id),
            None,
            "new",
            "text",
            None,
            None,
        )
        .unwrap();
    assert_eq!(transcript.poll(&repo, &conversation.id).unwrap(), [arrival]);
}

#[test]
fn terminal_text_removes_escape_sequences_and_request_ids_are_unique() {
    assert_eq!(safe_text("a\u{1b}[2J\rb\n\t\u{7}"), "a[2Jb\n\t");
    assert_ne!(request_id(), request_id());
}
