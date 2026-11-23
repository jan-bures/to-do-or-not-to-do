//! Dropped mail is handled locally; these fixtures contain invented messages only.
#![allow(clippy::unwrap_used)]

use todo_client::intent::{TaskRoom, email};
use todo_core::ids::{ListId, TaskId};
use todo_core::model::{LinkKind, Priority};
use todo_core::time::{Date, Timestamp};
use todo_core::{Actor, Command, State, apply, decide};

const PLAIN: &[u8] = include_bytes!("fixtures/eml/plain.eml");
const NOW: &str = "2026-11-21T10:35:00Z";

fn parse(raw: &[u8]) -> email::DroppedEmail {
    email::parse(raw, NOW.parse().unwrap()).expect("valid fixture")
}

fn commands(raw: &[u8], room: TaskRoom) -> Vec<Command> {
    let mut next = 0;
    parse(raw).commands(
        &State::default(),
        room,
        Date::constant(2026, 11, 21),
        || {
            next += 1;
            uuid::Uuid::from_u128(next)
        },
    )
}

fn task(cmds: &[Command]) -> &todo_core::command::NewTask {
    cmds.iter()
        .find_map(|cmd| match cmd {
            Command::CreateTask(task) => Some(task.as_ref()),
            _ => None,
        })
        .expect("create task")
}

#[test]
fn plain_mail_maps_to_the_forwarding_email_link_and_literal_title() {
    let cmds = commands(PLAIN, TaskRoom::MyDay);
    let task = task(&cmds);
    assert_eq!(task.title, "Confirm the workshop tomorrow !!!");
    assert_eq!(
        task.notes,
        "From: Alice Example <alice@example.com>\n\nPlease confirm the workshop agenda.\nBring the printed handouts."
    );
    assert_eq!(task.my_day, Some(Date::constant(2026, 11, 21)));
    assert_eq!(task.due, None);
    assert_eq!(task.priority, Priority::None);
    let Some(Command::AddLink {
        task: parent,
        kind: LinkKind::Email(link),
        ..
    }) = cmds.last()
    else {
        panic!("no email link");
    };
    assert_eq!(*parent, task.id);
    assert_eq!(link.subject, task.title);
    assert_eq!(link.sender, "alice@example.com");
    assert_eq!(link.message_id, "<plain-message@example.com>");
    assert_eq!(
        link.received_at,
        "2026-11-21T09:35:00Z".parse::<Timestamp>().unwrap()
    );
    assert_eq!(link.web_link, None);
}

#[test]
fn chooses_plain_body_and_ignores_alternatives_and_attachments() {
    let cmds = commands(
        include_bytes!("fixtures/eml/alternative.eml"),
        TaskRoom::Other,
    );
    assert_eq!(task(&cmds).title, "Check the workshop slides");
    assert_eq!(
        task(&cmds).notes,
        "From: Alice Example <alice@example.com>\n\nKeep the plain version: žádost."
    );
}

#[test]
fn html_is_reduced_to_text_without_loading_or_reproducing_active_content() {
    let cmds = commands(
        include_bytes!("fixtures/eml/html-only.eml"),
        TaskRoom::Other,
    );
    let notes = &task(&cmds).notes;
    for text in ["Hello & welcome!", "Confirm with Alice.", "<literal>", "✓"] {
        assert!(notes.contains(text), "missing {text:?} in {notes:?}");
    }
    for text in ["Invisible", "INVISIBLE", "<img", "https://", "title="] {
        assert!(!notes.contains(text), "found {text:?} in {notes:?}");
    }
    assert_eq!(cmds.len(), 2, "a link in the body isn't a message link");
}

#[test]
fn decodes_folded_czech_subject_and_sender() {
    let cmds = commands(
        include_bytes!("fixtures/eml/czech-subject.eml"),
        TaskRoom::Important,
    );
    assert_eq!(task(&cmds).title, "Příliš žluťoučký kůň úkoly");
    assert!(
        task(&cmds)
            .notes
            .starts_with("From: Jiří Example <sender@example.com>")
    );
    assert_eq!(task(&cmds).priority, Priority::High);
}

#[test]
fn missing_subject_falls_back_and_invalid_id_is_not_linked() {
    let cmds = commands(
        include_bytes!("fixtures/eml/no-subject.eml"),
        TaskRoom::Other,
    );
    assert_eq!(cmds.len(), 2);
    assert_eq!(task(&cmds).title, "(no subject)");
    assert!(task(&cmds).notes.ends_with("This email has no subject."));
}

#[test]
fn rejects_oversized_files_and_malformed_messages_and_bounds_notes() {
    let at = NOW.parse().unwrap();
    let fixture = include_str!("fixtures/eml/oversized.eml");
    let (header, body) = fixture.split_once("\n\n").unwrap();
    let under_limit = format!("{header}\n\n{}", body.repeat(100));
    let cmds = commands(under_limit.as_bytes(), TaskRoom::Other);
    let notes = &task(&cmds).notes;
    assert!(notes.ends_with('…'));
    assert!(notes.split_once("\n\n").unwrap().1.len() <= 4096);
    let oversized = format!(
        "{header}\n\n{}",
        body.repeat(email::MAX_FILE_BYTES / body.len() + 1)
    );
    assert_eq!(
        email::parse(oversized.as_bytes(), at).unwrap_err(),
        email::ImportError::TooLarge
    );
    assert_eq!(
        email::parse(include_bytes!("fixtures/eml/invalid.eml"), at).unwrap_err(),
        email::ImportError::Invalid
    );
    assert_eq!(
        email::parse(b"", at).unwrap_err(),
        email::ImportError::Invalid
    );
}

#[test]
fn drops_target_the_list_you_dropped_into_or_the_shared_default() {
    let actor = Actor::owner("fixtures").unwrap();
    let mut state = State::default();
    let at = NOW.parse().unwrap();
    let first = ListId::from_uuid(uuid::Uuid::from_u128(100));
    let default = ListId::from_uuid(uuid::Uuid::from_u128(101));
    let collection = todo_core::ids::CollectionId::from_uuid(uuid::Uuid::from_u128(103));
    for create in [
        todo_client::intent::create_collection(collection, "Events"),
        todo_client::intent::create_list(first, Some(collection), "Workshop"),
        todo_client::intent::create_list(default, None, "Inbox"),
    ] {
        for event in decide(&state, &actor, &create, at).unwrap() {
            apply(&mut state, &event).unwrap();
        }
    }
    for (room, expected, label) in [
        (TaskRoom::List(first), first, "into Workshop"),
        (TaskRoom::Collection(collection), first, "into Workshop"),
        (TaskRoom::Planned, default, "into Inbox"),
    ] {
        assert_eq!(email::drop_destination(&state, room), label);
        let id = uuid::Uuid::from_u128(102);
        let cmds = parse(PLAIN).commands(&state, room, Date::constant(2026, 11, 21), || id);
        assert_eq!(task(&cmds).list, expected);
        assert_eq!(task(&cmds).id, TaskId::from_uuid(id));
        assert_eq!(cmds.len(), 2);
    }
    assert_eq!(
        email::drop_destination(&State::default(), TaskRoom::Other),
        "into a new Tasks list"
    );
}
