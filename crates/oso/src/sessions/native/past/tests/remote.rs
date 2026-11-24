//! Tests of a transcript's messages from claude.ai (ADR 0181): never the person's message at
//! oso, nor a brief, whatever they hold and wherever they are.
//!
//! Must not depend on: files, threads or the person's own transcripts.

use super::*;

const BRIEF: &str = "Session \"a\" started this session through oso, with this brief:\n\nGo";
const WORKTREE: &str = "You are in a git worktree of /r at /w, on branch b from main.\n\nGo";

/// A message from claude.ai.
fn remote(text: &str) -> Said {
    Said::Remote {
        text: text.to_owned(),
        uuid: Some(id(7)),
    }
}

fn prompt_of(text: &str) -> Said {
    Said::Prompt {
        text: text.to_owned(),
        uuid: None,
    }
}

/// The kinds of the rows `said` gives, read from the conversation's start in a session an agent
/// started.
fn kinds(said: Vec<Said>) -> Vec<Kind> {
    let mut rows = Rows::new(PathBuf::from("/w/proj"));
    rows.note_started_by(Some("session a".to_owned()));
    rows.past(said, true);
    rows.rows().iter().map(|row| row.kind.clone()).collect()
}

#[test]
fn a_message_from_claude_ai_is_a_row_that_says_so() {
    for text in [
        "hello",
        BRIEF,
        WORKTREE,
        // What a command, a hook or its output wrapped draws otherwise, or not at all.
        "<command-name>/model</command-name>",
        "<local-command-stdout>Compacted</local-command-stdout>",
        "<system-reminder>x</system-reminder>",
        // As a row of oso's says it.
        "Remote Control is off.",
    ] {
        let row = Kind::Note(format!("From claude.ai, not from oso: {text}"));
        assert_eq!(kinds(vec![remote(text)]), std::slice::from_ref(&row));
        assert_eq!(
            kinds(vec![prompt_of("first"), remote(text)]),
            [Kind::Prompt("first".into()), row]
        );
    }
}

#[test]
fn after_a_first_message_from_claude_ai_no_message_is_a_brief() {
    for text in [BRIEF, WORKTREE] {
        assert_eq!(
            kinds(vec![remote("hello"), prompt_of(text)])[1],
            Kind::Prompt(text.into())
        );
    }
}

#[test]
fn its_id_is_kept_for_the_next_read() {
    let mut rows = Rows::new(PathBuf::from("/w"));
    rows.past(vec![remote("hello")], true);
    assert_eq!(rows.kept_ids(), [id(7)]);
}

/// A line of the transcript with a message from claude.ai.
fn remote_line(n: usize, of: Option<usize>, text: &str) -> String {
    prompt(n, of, text).replace(
        r#""promptSource":"sdk","#,
        r#""promptSource":"sdk","origin":{"kind":"human"},"clientPlatform":"ios","#,
    )
}

#[test]
fn a_cut_starts_at_a_message_from_claude_ai_too_and_counts_its_turn() {
    let text = "x".repeat(60 << 10);
    let mut lines = vec![remote_line(1, None, "first")];
    for n in 1..200 {
        lines.push(match n % 2 {
            0 => remote_line(n + 1, Some(n), &text),
            _ => reply(n + 1, n, &text),
        });
    }
    let chosen = index(&lines).chosen(&[]);
    assert!(chosen.cut);
    assert_eq!(chosen.lines[0].shows, Shows::Remote);
    let shown = chosen
        .lines
        .iter()
        .filter(|line| line.shows == Shows::Remote);
    assert_eq!(shown.count() + chosen.prompts_left_out, 100);
}
