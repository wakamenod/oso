//! `*Sessions*`: its groups and their order, pins, nesting, folds, one line a session at any
//! width with a reason line only where the person has something to do, the line map, and its
//! cost.
//!
//! Must not depend on: `app` or `dispatch`; these tests drive the sessions' own
//! parts, with no window.

use std::time::{Duration, Instant};

use super::{Entry, Group, Key, Line, List, age, cells, cut, cut_middle};
use crate::sessions::state::{Reason, State};
use oso_core::syntax::Highlight;

const ASK: State = State::NeedsInput(Reason::Permission);

fn entry(key: u64, name: &str, state: State, since: Instant) -> Entry {
    Entry {
        key: Key::Hosted(key),
        name: name.to_owned(),
        space: "oso".to_owned(),
        branch: Some("main".to_owned()),
        state,
        since,
        recap: format!("{name} recap"),
        pinned: false,
        archived: false,
        parent: None,
    }
}

fn headers(lines: &[Line]) -> Vec<Group> {
    lines
        .iter()
        .filter_map(|line| match line {
            Line::Header(group) => Some(group.clone()),
            Line::Session(_) => None,
        })
        .collect()
}

/// The sessions in the order shown, once each.
fn order(lines: &[Line]) -> Vec<Key> {
    let mut keys: Vec<Key> = Vec::new();
    for line in lines {
        if let Line::Session(key) = line
            && keys.last() != Some(key)
        {
            keys.push(*key);
        }
    }
    keys
}

#[test]
fn groups_follow_the_taxonomy_and_pins_and_waits_order_a_group() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "idle", State::Idle, t),
        entry(2, "late-ask", ASK, t + Duration::from_secs(5)),
        entry(3, "work", State::Working, t),
        entry(4, "early-ask", ASK, t),
        entry(5, "pinned-ask", ASK, t + Duration::from_secs(9)),
        entry(6, "failed", State::Failed, t),
    ];
    entries[4].pinned = true;
    let list = List::default();
    let shown = list.render(&entries, 32, t + Duration::from_secs(60));
    assert_eq!(
        headers(&shown.lines),
        [
            Group::State(0),
            Group::State(1),
            Group::State(2),
            Group::State(5)
        ]
    );
    assert_eq!(
        order(&shown.lines),
        [5, 4, 2, 6, 3, 1].map(Key::Hosted),
        "pinned first, then the longest waiting"
    );
    assert!(shown.text.starts_with("▾ Needs input 3\n⚠ pinned-ask"));
}

#[test]
fn by_space_groups_by_name_and_archived_come_last_folded() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "a", State::Idle, t),
        entry(2, "b", ASK, t),
        entry(3, "old", State::Stopped, t),
    ];
    entries[0].space = "web".to_owned();
    entries[2].archived = true;
    let mut list = List::default();
    list.regroup();
    let shown = list.render(&entries, 80, t);
    assert_eq!(
        headers(&shown.lines),
        [
            Group::Space("oso".to_owned()),
            Group::Space("web".to_owned()),
            Group::Archived
        ]
    );
    assert_eq!(
        order(&shown.lines),
        [2, 1].map(Key::Hosted),
        "archived is folded"
    );
    assert!(shown.text.ends_with("▸ Archived 1\n"));
    list.fold(&Line::Header(Group::Archived));
    let opened = list.render(&entries, 80, t);
    assert_eq!(order(&opened.lines), [2, 1, 3].map(Key::Hosted));
    list.fold(&Line::Header(Group::Space("oso".to_owned())));
    assert_eq!(
        order(&list.render(&entries, 80, t).lines),
        [1, 3].map(Key::Hosted)
    );
}

#[test]
fn sessions_nest_under_their_parent_which_counts_those_that_need_input() {
    let t = Instant::now();
    let mut entries = vec![
        entry(1, "lead", State::Working, t),
        entry(2, "child-a", ASK, t),
        entry(3, "child-b", ASK, t),
        entry(4, "grandchild", State::Done, t),
    ];
    for (child, parent) in [(1, 1), (2, 1), (3, 3)] {
        entries[child].parent = Some(Key::Hosted(parent));
    }
    let mut list = List::default();
    list.regroup();
    let shown = list.render(&entries, 32, t);
    assert_eq!(order(&shown.lines), [1, 2, 3, 4].map(Key::Hosted));
    let text: Vec<&str> = shown.text.lines().collect();
    // The lead works, so only its nested sessions give it a reason line.
    assert_eq!(text[2], "  2 nested need input", "{text:?}");
    assert!(text[3].starts_with("  ⚠ child-a"), "indented: {text:?}");
    list.fold(&Line::Session(Key::Hosted(1)));
    assert_eq!(order(&list.render(&entries, 32, t).lines), [Key::Hosted(1)]);
    // By state the children wait in their own group, at the top level.
    list.regroup();
    let by_state = list.render(&entries, 32, t);
    assert_eq!(order(&by_state.lines), [2, 3, 1, 4].map(Key::Hosted));
}

#[test]
fn a_cycle_of_parents_ends() {
    let t = Instant::now();
    let mut entries = vec![entry(1, "a", ASK, t), entry(2, "b", ASK, t)];
    entries[0].parent = Some(Key::Hosted(2));
    entries[1].parent = Some(Key::Hosted(1));
    let shown = List::default().render(&entries, 32, t);
    assert_eq!(
        order(&shown.lines),
        [1, 2].map(Key::Hosted),
        "both at the top level"
    );
}

#[test]
fn a_session_takes_one_line_and_a_reason_only_when_it_needs_the_person() {
    let t = Instant::now();
    let mut long = entry(1, &format!("chore-{}-hook", "n".repeat(40)), ASK, t);
    long.recap = "Do you want to proceed with this rather long command?".to_owned();
    long.branch = Some("feature/界面".to_owned());
    let mut quiet = entry(2, "quiet", State::Working, t);
    quiet.recap = "Design the pane".to_owned();
    let shown = List::default().render(&[long, quiet], 32, t + Duration::from_secs(720));
    let text: Vec<&str> = shown.text.lines().collect();
    assert_eq!(text.len(), 5, "{text:?}");
    // The name is cut in the middle, so its end shows; the age is at the right edge.
    assert_eq!(text[1], format!("⚠ chore-{}…nnn-hook 12m", "n".repeat(11)));
    assert_eq!(text[2], format!("  {}…", &"Do you want to proceed with t"));
    // A working session shows no recap; its place fits after its name.
    assert_eq!(text[4], "▶ quiet  oso · main          12m");
    for line in &text {
        assert!(cells(line) <= 32, "{line:?}");
    }
    assert_eq!(shown.lines.len(), 5);
    assert_eq!(shown.lines[2], Line::Session(Key::Hosted(1)));
    // As wide as it may be, a session keeps to one line.
    let wide = List::default().render(&[entry(3, "a", State::Idle, t)], 120, t);
    assert_eq!(wide.text.lines().count(), 2);
}

#[test]
fn an_external_row_is_quiet_throughout_and_never_spins() {
    let t = Instant::now();
    let mut external = entry(9, "other", State::Working, t);
    external.key = Key::External(4242);
    external.space = "a-space-name-long-enough-to-be-cut".to_owned();
    external.branch = None;
    let mut list = List::default();
    list.set_spin(Some(3));
    let shown = list.render(&[external], 60, t);
    let row = shown.text.lines().nth(1).unwrap().to_owned();
    assert!(row.starts_with("▶ other  a-space"), "{row}");
    assert!(!row.contains("external"), "{row}");
    let start = shown.text.find('▶').unwrap();
    assert_eq!(
        shown.highlights,
        [(start..start + row.len(), Highlight::SessionQuiet)]
    );
    assert!(shown.spinning.is_empty());
}

#[test]
fn marks_take_their_states_face_and_a_working_one_spins() {
    let t = Instant::now();
    let mut asking = entry(1, "ask", ASK, t);
    asking.recap = "Proceed?".to_owned();
    let entries = [
        asking,
        entry(2, "work", State::Working, t),
        entry(3, "done", State::Done, t),
    ];
    let mut list = List::default();
    let still = list.render(&entries, 32, t);
    assert!(still.spinning.is_empty());
    assert!(still.text.contains("▶ work"), "{}", still.text);
    list.set_spin(Some(12));
    let shown = list.render(&entries, 32, t);
    let text: Vec<&str> = shown.text.lines().collect();
    // Frame 12 is the third of ten.
    assert!(text[4].starts_with("⠹ work"), "{text:?}");
    assert_eq!(shown.spinning, [4]);
    let face_at = |needle: &str| {
        let at = shown.text.find(needle).unwrap();
        shown
            .highlights
            .iter()
            .find(|(range, _)| range.start == at)
            .map(|(_, highlight)| *highlight)
    };
    assert_eq!(face_at("⚠ ask"), Some(Highlight::SessionAlert));
    assert_eq!(face_at("⠹ work"), Some(Highlight::SessionWorking));
    assert_eq!(face_at("● done"), Some(Highlight::SessionDone));
    assert_eq!(face_at("Proceed?"), Some(Highlight::SessionQuiet));
    for (range, _) in &shown.highlights {
        assert!(shown.text.is_char_boundary(range.start) && shown.text.is_char_boundary(range.end));
        assert!(range.end <= shown.text.len());
    }
}

#[test]
fn ages_and_cuts() {
    let t = Instant::now();
    let after = |s| age(t, t + Duration::from_secs(s));
    assert_eq!(
        [0, 59, 60, 3599, 3600, 86_400].map(after),
        ["now", "now", "1m", "59m", "1h", "1d"]
    );
    assert_eq!(
        cut_middle("chore-survey-review-lifecycle-hook", 30),
        "chore-survey-review-…ycle-hook"
    );
    assert_eq!(cut_middle("short", 30), "short");
    assert_eq!(cut_middle("abcdefgh", 4), "abc…");
    assert_eq!(cut_middle("界界界界界界界界", 9), "界界界…界");
    for width in 0..40 {
        let cut = cut_middle("chore-survey-review-lifecycle-hook", width);
        assert!(cells(&cut) <= width, "{width}: {cut}");
    }
    assert_eq!(cut("abc", 3), "abc");
    assert_eq!(cut("abcd", 3), "ab…");
    assert_eq!(cut("界界界", 4), "界…");
    assert_eq!(cut("a\nb", 9), "a");
    assert_eq!(cut("abc", 0), "");
    assert_eq!(cells("a\u{202E}"), 1 + cells("\u{202E}"));
}

#[test]
fn two_hundred_fifty_six_sessions_render_within_the_budget() {
    let t = Instant::now();
    let entries: Vec<Entry> = (0..256)
        .map(|n| {
            let state = [ASK, State::Working, State::Done, State::Idle][n % 4];
            let mut entry = entry(n as u64, &format!("session-{n}"), state, t);
            entry.parent = (n % 5 == 1).then(|| Key::Hosted(n as u64 - 1));
            entry.recap = "Do you want to make this edit to src/main.rs?".to_owned();
            entry
        })
        .collect();
    let list = List::default();
    let mut best = Duration::MAX;
    for _ in 0..10 {
        let start = Instant::now();
        let shown = list.render(&entries, 32, t);
        best = best.min(start.elapsed());
        assert!(shown.lines.len() > 256);
    }
    // A debug build is several times slower; the budget is a release build's (ADR 0121).
    assert!(cfg!(debug_assertions) || best.as_micros() < 500, "{best:?}");
}

#[test]
fn a_recap_or_space_that_begins_with_a_mark_never_reads_as_a_row() {
    let now = Instant::now();
    let mut idle = entry(1, "proj", State::Idle, now);
    idle.recap = "  ⚠ deploy-prod                2s".into();
    idle.space = "▸● repo".into();
    let shown = List::default().render(&[idle], 32, now);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(rows[1].starts_with("○ proj  repo"), "{}", shown.text);
    let mut asking = entry(2, "ask", ASK, now);
    asking.recap = "  ⚠ deploy-prod                2s".into();
    let shown = List::default().render(&[asking], 32, now);
    let rows: Vec<&str> = shown.text.lines().collect();
    assert!(
        rows[2].trim_start().starts_with("deploy-prod"),
        "{}",
        shown.text
    );
    let first = rows[2].chars().find(|c| *c != ' ').unwrap();
    assert!(!super::MARKS.contains(first), "{}", rows[2]);
}

#[test]
fn a_spinner_tick_with_twenty_spinning_rows_stays_within_its_budget() {
    use oso_core::buffer::Mode;
    use oso_core::editor::Editor;
    use oso_core::syntax::Highlights;
    let t = Instant::now();
    let entries: Vec<Entry> = (0..40)
        .map(|n| {
            let state = if n < 20 { State::Working } else { State::Idle };
            entry(n, &format!("m4-session-number-{n}"), state, t)
        })
        .collect();
    let mut editor = Editor::default();
    let id = editor.add_internal_buffer("*Sessions*", Mode::Sessions);
    let mut list = List::default();
    let mut best = Duration::MAX;
    for frame in 0..30 {
        // What a tick does on the UI thread: the list with the next frame, its text and its
        // highlights set whole (ADR 0131, step 1 as built).
        let start = Instant::now();
        list.set_spin(Some(frame));
        let shown = list.render(&entries, 44, t);
        let mut highlights = Highlights::new(shown.text.len());
        for (range, highlight) in &shown.highlights {
            highlights.replace(range.clone(), [(range.len(), Some(*highlight))]);
        }
        editor.set_internal_text(id, &shown.text);
        assert!(editor.set_internal_highlights(id, highlights).is_none());
        best = best.min(start.elapsed());
        assert_eq!(shown.spinning.len(), 20);
    }
    // A debug build is several times slower; the budget is a release build's (ADR 0131).
    assert!(cfg!(debug_assertions) || best.as_micros() < 200, "{best:?}");
}
