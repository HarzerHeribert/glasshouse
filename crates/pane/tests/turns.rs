//! Acceptance tests for GH-PANE-61C-LOOP: running a task in turns against
//! the Anthropic Messages protocol (map line 2444) and routing through
//! Glasshouse's gateway without changing a byte of the request (map line
//! 2445). No test here makes a network call, per the packet's REQUIRED
//! BEHAVIOR #4.

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use pane::contract::{Conversation, Message, Role, SessionId};
use pane::rollout::{self, Rollout};
use pane::wire;

/// A fresh directory unique to the calling test. Parallel test threads can
/// land in the same millisecond, so a monotonic counter breaks the tie that
/// a pid + timestamp pair alone would not.
fn tempdir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();

    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "pane-turns-test-{}-{millis}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn sample_conversation() -> Conversation {
    Conversation {
        system: "You act by writing TypeScript.".to_string(),
        messages: vec![
            Message::text(Role::User, "How many files name IntegrationId?"),
            Message::text(Role::Assistant, "```pane\nreturn 1;\n```"),
        ],
    }
}

/// This is 2445's whole contract: the request body pane builds does not
/// depend on which base URL it will be sent to, so a gateway hop cannot
/// change a byte of it. `ANTHROPIC_BASE_URL` is exercised directly rather
/// than through a bound listener, per the packet's "assert on the
/// serialised body without sending it" option -- `wire::request_body`
/// takes no base-url parameter at all, so this also proves that by
/// construction rather than by care.
#[test]
fn the_gateway_hop_changes_no_byte() {
    let conversation = sample_conversation();

    // SAFETY: no other test reads or writes `ANTHROPIC_BASE_URL`; the
    // verification commands also run with it unset in the parent process.
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }
    assert_eq!(wire::base_url(), wire::DEFAULT_BASE_URL);
    let body_without_gateway = wire::request_body(&conversation);

    unsafe {
        std::env::set_var("ANTHROPIC_BASE_URL", "http://127.0.0.1:9");
    }
    assert_eq!(wire::base_url(), "http://127.0.0.1:9");
    let body_with_gateway = wire::request_body(&conversation);
    unsafe {
        std::env::remove_var("ANTHROPIC_BASE_URL");
    }

    assert_eq!(body_without_gateway, body_with_gateway);
}

#[test]
fn a_turn_is_appended_and_the_file_is_never_rewritten() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();

    rollout.record_turn(Role::User, "first").unwrap();
    let first_line_after_one = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    rollout.record_turn(Role::Assistant, "second").unwrap();
    let first_line_after_two = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .next()
        .unwrap()
        .to_string();

    assert_eq!(first_line_after_one, first_line_after_two);
}

#[test]
fn resume_rebuilds_the_conversation_from_the_file_alone() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    rollout.record_turn(Role::User, "hello").unwrap();
    rollout.record_turn(Role::Assistant, "hi there").unwrap();
    drop(rollout);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(conversation.system, "system prompt");
    assert_eq!(
        conversation.messages,
        vec![
            Message::text(Role::User, "hello"),
            Message::text(Role::Assistant, "hi there"),
        ]
    );
}

#[test]
fn resume_skips_a_truncated_final_line_and_keeps_the_rest() {
    let path = tempdir().join("session.jsonl");
    {
        let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
        rollout.record_turn(Role::User, "hello").unwrap();
    }
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    // No trailing newline: this is what a process dying mid-append leaves.
    write!(file, r#"{{"kind":"turn","session_id":"s1","turn":1,"rol"#).unwrap();
    drop(file);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(conversation.system, "system prompt");
    assert_eq!(
        conversation.messages,
        vec![Message::text(Role::User, "hello")]
    );
}

#[test]
fn resume_skips_a_rollout_kind_it_does_not_know() {
    let path = tempdir().join("session.jsonl");
    {
        let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
        rollout.record_turn(Role::User, "hello").unwrap();
    }
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    writeln!(
        file,
        r#"{{"kind":"cell","session_id":"s1","index":0,"code":"1+1"}}"#
    )
    .unwrap();
    drop(file);

    let conversation = rollout::resume(&path).unwrap();

    assert_eq!(
        conversation.messages,
        vec![Message::text(Role::User, "hello")]
    );
}

#[test]
fn the_system_prompt_is_recorded_once_not_per_turn() {
    let path = tempdir().join("session.jsonl");
    let mut rollout = Rollout::create(&path, SessionId::new("s1"), "system prompt").unwrap();
    rollout.record_turn(Role::User, "one").unwrap();
    rollout.record_turn(Role::Assistant, "two").unwrap();
    rollout.record_turn(Role::User, "three").unwrap();

    let system_lines = std::fs::read_to_string(&path)
        .unwrap()
        .lines()
        .filter(|line| line.contains(r#""kind":"system""#))
        .count();

    assert_eq!(system_lines, 1);
}
