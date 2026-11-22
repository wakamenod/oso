//! A native session's replies highlighted once whole: what is opened, which reports count,
//! where their runs go, and what font-lock, a failure and a session gone do.
//!
//! Must not depend on: the syntax threads or a process: a stand-in takes the opens, and
//! reports are made by hand.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use oso_core::buffer::{Buffer, BufferId, Verdict};
use oso_core::editor::Editor;
use oso_core::rope::Rope;
use oso_core::syntax::{Highlight, Highlights};
use oso_syntax::{Failure, Outcome, Refused, Report};
use serde_json::json;

use super::super::composer::Composing;
use super::super::interrupting::SIGNALS;
use super::super::note::NativeState;
use super::super::{Native, NativeSessions, draw};
use super::{MAX_OPEN, MAX_WAITING, Marks, Opener, reported, sync};
use crate::app::freer::Freer;
use crate::sessions::native::interrupt::Interrupt;
use crate::sessions::native::placed::Placed;
use crate::sessions::native::render::Face;
use crate::sessions::native::rows::{Changes, Rows};
use crate::sessions::native::slash::SlashCommands;
use crate::sessions::native::wire::parse;

/// What the stand-in was told: opens with their text, and closes.
#[derive(Debug, Default)]
struct Told {
    opened: Vec<(u64, Verdict, String)>,
    shown: Vec<bool>,
    closed: Vec<u64>,
    /// Refuse every open so.
    refuse: Option<Refused>,
}

#[derive(Clone, Default)]
struct Stand(Rc<RefCell<Told>>);

impl Opener for Stand {
    fn open(&self, key: u64, verdict: Verdict, text: Rope, shown: bool) -> Result<(), Refused> {
        let mut told = self.0.borrow_mut();
        if let Some(refused) = told.refuse {
            return Err(refused);
        }
        told.opened.push((key, verdict, text.to_string()));
        told.shown.push(shown);
        Ok(())
    }

    fn close(&self, key: u64) {
        self.0.borrow_mut().closed.push(key);
    }
}

struct Setup {
    editor: Editor,
    natives: NativeSessions,
    freer: Freer,
    stand: Stand,
    buffer: BufferId,
}

const SESSION: u64 = 7;

/// A running native session over `buffer` with no rows yet.
fn native(buffer: BufferId) -> Native {
    Native {
        buffer,
        rows: Rows::new(PathBuf::from("/w")),
        placed: Placed::default(),
        stream: None,
        first: false,
        spoke: true,
        ended: None,
        result: None,
        result_of: None,
        calls: HashMap::new(),
        composing: Composing::starting(false),
        state: NativeState::Running,
        control: None,
        interrupt: Interrupt::new(SIGNALS),
        commands: SlashCommands::default(),
        slash_rows: None,
        cli_id: None,
        moved: false,
        markdown: Marks::default(),
    }
}

fn setup() -> Setup {
    let mut editor = Editor::with_buffer(Buffer::from_text("*scratch*", ""));
    let buffer = editor.add_native_session_buffer("*claude: a*");
    editor.show(buffer);
    let native = native(buffer);
    let mut natives = NativeSessions::default();
    natives.map.insert(SESSION, native);
    let stand = Stand::default();
    natives.replies.opener = Some(Box::new(stand.clone()));
    Setup {
        editor,
        natives,
        freer: Freer::start(),
        stand,
        buffer,
    }
}

impl Setup {
    /// Reads `lines` into the session's rows and draws what changed, as a batch is.
    fn feed(&mut self, lines: &[String]) {
        let native = self.natives.map.get_mut(&SESSION).expect("session");
        let mut changes = Changes::default();
        for line in lines {
            for change in native.rows.apply(parse(line.as_bytes())) {
                changes.add(change);
            }
        }
        draw(&mut self.editor, native, changes);
    }

    /// What follows the event: highlighting `on`, as `global-font-lock-mode` sets it, then
    /// opening what waits.
    fn sync(&mut self, on: bool) -> bool {
        self.editor.set_font_lock(on);
        let redraw = sync(&mut self.natives, &mut self.editor, &self.freer, on);
        let natives = &mut self.natives;
        natives.replies.fill(&mut natives.map, &self.editor);
        redraw
    }

    fn report(&mut self, key: u64, outcome: Outcome, done: bool) -> bool {
        let report = Report {
            buffer: key,
            epoch: 0,
            stamp: 1,
            revision: 0,
            outcome,
            done,
        };
        reported(&mut self.natives, &mut self.editor, &self.freer, report)
    }

    fn opened(&self) -> Vec<(u64, Verdict, String)> {
        self.stand.0.borrow().opened.clone()
    }

    fn at(&self, offset: usize) -> Option<Highlight> {
        let buffer = self.editor.buffer(self.buffer).expect("buffer");
        buffer
            .highlights()
            .and_then(|highlights| highlights.at(offset))
    }

    fn text(&self) -> String {
        self.editor
            .buffer(self.buffer)
            .expect("buffer")
            .text()
            .to_string()
    }
}

fn start(message: &str) -> String {
    let block = json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}});
    json!({"type": "stream_event", "api_message_id": message, "event": block}).to_string()
}

fn delta(message: &str, text: &str) -> String {
    let delta = json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": text}});
    json!({"type": "stream_event", "api_message_id": message, "event": delta}).to_string()
}

fn whole(message: &str, text: &str) -> String {
    json!({"type": "assistant", "message": {"id": message, "content": [{"type": "text", "text": text}]}})
        .to_string()
}

/// `text`'s highlights with `Heading` over `range`.
fn heading(text: &str, range: std::ops::Range<usize>) -> Outcome {
    let mut highlights = Highlights::new(text.len());
    highlights.replace(range.clone(), [(range.len(), Some(Highlight::Heading))]);
    Outcome::Highlights(highlights)
}

const REPLY: &str = "# Hi\nsome `x`";

#[test]
fn a_reply_is_opened_once_whole_and_its_runs_go_past_its_marks() {
    let mut setup = setup();
    setup.feed(&[start("m"), delta("m", "# Hi\nso")]);
    setup.sync(true);
    setup.feed(&[delta("m", "me `x`")]);
    setup.sync(true);
    assert!(setup.opened().is_empty(), "plain while it streams");
    // The whole message is what streamed: no row changes, but the reply is whole now.
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let opened = setup.opened();
    assert_eq!(opened.len(), 1);
    let (key, verdict, text) = opened[0].clone();
    assert_eq!((verdict, text.as_str()), (Verdict::Ordinary, REPLY));
    setup.sync(true);
    assert_eq!(setup.opened().len(), 1, "once");

    assert_eq!(setup.text(), "● # Hi\n  some `x`\n");
    assert!(setup.report(key, heading(REPLY, 0..4), true));
    // `● ` is four bytes, and keeps its face; `# Hi` takes the report's.
    assert_eq!(setup.at(0), Some(Highlight::SessionQuiet));
    assert_eq!(setup.at(4), Some(Highlight::Heading));
    assert_eq!(setup.at(7), Some(Highlight::Heading));
    assert_eq!(setup.at(8), None, "a break has none");
    assert_eq!(
        setup.at(9),
        Some(Highlight::SessionQuiet),
        "the next line's mark"
    );
    assert_eq!(setup.stand.0.borrow().closed, vec![key], "closed once done");
    assert!(setup.natives.replies.open.is_empty());
}

#[test]
fn only_the_done_report_of_the_key_it_waits_under_counts() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    assert!(!setup.report(key, heading(REPLY, 0..4), false), "not done");
    assert_eq!(setup.at(4), None);
    // The row is drawn again, with another text: what was asked for it is stale.
    let other = "# Ho\nmore";
    setup.feed(&[whole("m2", other)]);
    let native = setup.natives.map.get_mut(&SESSION).expect("session");
    native.markdown.drawn(0);
    setup.sync(true);
    assert!(!setup.report(key, heading(REPLY, 0..4), true), "stale");
    assert_eq!(setup.at(4), None);
    let keys: Vec<u64> = setup.opened().iter().map(|(key, ..)| *key).collect();
    assert_eq!(keys.len(), 3, "row 0 again, and row 1: {keys:?}");
    assert!(!keys[1..].contains(&key));
}

#[test]
fn a_reply_that_fails_stays_plain_and_says_nothing() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    assert!(!setup.report(key, Outcome::Failed(Failure::TooSlow), false));
    assert_eq!(setup.at(4), None);
    let messages = setup.editor.messages_buffer();
    let said = setup
        .editor
        .buffer(messages)
        .map(|buffer| buffer.text().len());
    assert_eq!(said, Some(0), "nothing said");
    assert_eq!(setup.stand.0.borrow().closed, vec![key]);
    setup.sync(true);
    assert_eq!(setup.opened().len(), 1, "not tried again");
}

#[test]
fn highlights_that_do_not_fit_the_row_leave_it_plain() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    // Highlights of another text.
    assert!(!setup.report(key, heading("# H", 0..3), true));
    assert_eq!(setup.at(4), None);
}

#[test]
fn font_lock_off_closes_what_is_open_and_on_lays_faces_and_highlights_again() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    setup.report(key, heading(REPLY, 0..4), true);
    assert_eq!(setup.at(4), Some(Highlight::Heading));
    // One more waits, and is open, when highlighting turns off.
    setup.feed(&[whole("m2", "two")]);
    setup.sync(true);
    let second = setup.opened()[1].0;

    assert!(!setup.sync(false), "the editor dropped them");
    assert_eq!(setup.at(4), None);
    assert!(setup.stand.0.borrow().closed.contains(&second));
    assert!(setup.natives.replies.opener.is_none(), "the handle goes");
    assert!(!setup.report(second, heading("two", 0..3), true), "closed");

    // On again: the faces are laid at once, as a window shows the session, then every whole
    // reply is asked for, the newest first, once a handle is back.
    assert!(setup.sync(true), "a frame is due");
    assert_eq!(setup.at(0), Some(Highlight::SessionQuiet), "its mark");
    assert_eq!(setup.at(4), None, "plain until reported");
    assert_eq!(setup.natives.replies.waiting.len(), 2);
    setup.natives.replies.opener = Some(Box::new(setup.stand.clone()));
    setup.sync(true);
    let texts: Vec<String> = setup.opened()[2..]
        .iter()
        .map(|(.., text)| text.clone())
        .collect();
    assert_eq!(texts, ["two", REPLY]);
    let key = setup.opened()[3].0;
    setup.report(key, heading(REPLY, 0..4), true);
    assert_eq!(setup.at(4), Some(Highlight::Heading));
}

#[test]
fn laid_faces_are_those_the_rows_were_drawn_with() {
    let mut setup = setup();
    let tool = json!({"type": "assistant", "message": {"id": "t", "content": [
        {"type": "tool_use", "id": "u1", "name": "Bash", "input": {"command": "ls"}}]}});
    setup.feed(&[
        whole("m", REPLY),
        tool.to_string(),
        whole("m2", "two\nlines"),
    ]);
    // A queued message's grey row at the end, as `draw_queue` puts it.
    let native = setup.natives.map.get_mut(&SESSION).expect("session");
    let (end, queued) = (native.placed.end(), "  queued\n".to_string());
    let runs = [(queued.len(), Some(Face::Dim.highlight()))];
    (setup.editor).replace_native_rows(setup.buffer, end..end, &queued, &runs);
    native.composing.rows = queued;
    // Its replies plain: none is reported.
    setup.natives.replies.opener = None;
    setup.sync(true);
    let drawn = setup
        .editor
        .buffer(setup.buffer)
        .and_then(Buffer::highlights)
        .cloned();
    setup.sync(false);
    setup.sync(true);
    let laid = setup
        .editor
        .buffer(setup.buffer)
        .and_then(Buffer::highlights)
        .cloned();
    let runs = |highlights: Option<Highlights>| -> Vec<Option<Highlight>> {
        let highlights = highlights.expect("highlights");
        (0..highlights.len()).map(|at| highlights.at(at)).collect()
    };
    assert_eq!(runs(laid), runs(drawn), "{}", setup.text());
}

#[test]
fn a_session_gone_closes_what_it_had_open() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    setup.natives.map.remove(&SESSION);
    setup.sync(true);
    assert_eq!(setup.stand.0.borrow().closed, vec![key]);
    assert!(setup.natives.replies.open.is_empty());
    assert!(!setup.report(key, heading(REPLY, 0..4), true));
}

#[test]
fn a_refused_reply_is_dropped_and_a_full_schedule_waits() {
    let mut setup = setup();
    setup.stand.0.borrow_mut().refuse = Some(Refused::MayHoldSecrets);
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    assert!(setup.natives.replies.waiting.is_empty());
    let native = &setup.natives.map[&SESSION];
    assert!(native.markdown.keys.is_empty(), "never asked again");

    setup.stand.0.borrow_mut().refuse = Some(Refused::Full);
    setup.feed(&[whole("m2", "two")]);
    setup.sync(true);
    assert_eq!(setup.natives.replies.waiting.len(), 1, "waits for room");
    setup.stand.0.borrow_mut().refuse = None;
    setup.sync(true);
    assert!(
        setup.opened().is_empty(),
        "not offered again within a second"
    );
    // A second passes.
    setup.natives.replies.full_until = Some(std::time::Instant::now());
    setup.sync(true);
    assert_eq!(setup.opened().len(), 1);
}

#[test]
fn a_reply_drawn_again_closes_what_was_open_for_it() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    let native = setup.natives.map.get_mut(&SESSION).expect("session");
    native.markdown.drawn(0);
    setup.sync(true);
    assert_eq!(setup.stand.0.borrow().closed, vec![key], "closed at once");
    let opened = setup.opened();
    assert_eq!(opened.len(), 2, "opened again");
    assert_eq!(setup.natives.replies.open.len(), 1);
}

#[test]
fn rows_drawn_while_font_lock_is_off_keep_nothing_and_a_reply_whole_then_is_highlighted_on() {
    let mut setup = setup();
    setup.sync(false);
    setup.feed(&[start("m"), delta("m", "# Hi")]);
    for _ in 0..100 {
        setup.feed(&[delta("m", "!")]);
        setup.sync(false);
    }
    let marks = &setup.natives.map[&SESSION].markdown;
    assert!(marks.drawn.is_empty(), "looked at while off");
    assert_eq!(marks.streaming, vec![0]);
    setup.sync(true);
    assert!(setup.opened().is_empty(), "it still streams");
    setup.feed(&[whole("m", &format!("# Hi{}", "!".repeat(100)))]);
    setup.sync(true);
    assert_eq!(setup.opened().len(), 1, "whole, once on");
}

#[test]
fn a_reply_ranks_as_shown_only_while_its_row_is_on_screen() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    assert_eq!(setup.stand.0.borrow().shown, vec![true]);
    // Another buffer in the window: nothing of the session shows.
    let scratch = setup
        .editor
        .buffers()
        .map(|(id, _)| id)
        .find(|id| *id != setup.buffer);
    setup.editor.show(scratch.expect("scratch"));
    setup.feed(&[whole("m2", "two")]);
    setup.sync(true);
    assert_eq!(setup.stand.0.borrow().shown, vec![true, false]);
}

#[test]
fn turning_font_lock_on_asks_for_the_newest_of_sessions_on_screen_first_and_at_most_the_bound() {
    let mut setup = setup();
    setup.natives.replies.opener = None;
    let lines: Vec<String> = (0..MAX_WAITING)
        .map(|at| whole(&format!("m{at}"), &format!("r{at}")))
        .collect();
    setup.feed(&lines);
    // A second session, not shown, with more replies, as old.
    let hidden = setup.editor.add_native_session_buffer("*claude: b*");
    let mut native = native(hidden);
    let mut changes = Changes::default();
    for line in &lines {
        for change in native.rows.apply(parse(line.as_bytes())) {
            changes.add(change);
        }
    }
    draw(&mut setup.editor, &mut native, changes);
    setup.natives.map.insert(SESSION + 1, native);
    setup.sync(false);
    setup.sync(true);
    let waiting = &setup.natives.replies.waiting;
    assert_eq!(waiting.len(), MAX_WAITING);
    assert!(
        waiting.iter().all(|(session, ..)| *session == SESSION),
        "shown first"
    );
    assert_eq!(
        waiting.front().map(|(_, row, _)| *row),
        Some(MAX_WAITING - 1)
    );
    let hidden = &setup.natives.map[&(SESSION + 1)].markdown;
    assert!(hidden.keys.is_empty(), "nothing kept for what is not asked");
}

#[test]
fn at_most_a_few_are_open_the_newest_first_and_the_oldest_past_the_bound_stay_plain() {
    let mut setup = setup();
    let lines: Vec<String> = (0..6)
        .map(|at| whole(&format!("m{at}"), &format!("r{at}")))
        .collect();
    setup.feed(&lines);
    setup.sync(true);
    let texts: Vec<String> = setup
        .opened()
        .iter()
        .map(|(.., text)| text.clone())
        .collect();
    assert_eq!(texts.len(), MAX_OPEN);
    assert_eq!(texts, ["r5", "r4", "r3", "r2"]);
    // One done makes room for the next.
    let key = setup.opened()[0].0;
    setup.report(key, heading("r5", 0..2), true);
    setup.sync(true);
    assert_eq!(
        setup.opened().last().map(|(.., text)| text.as_str()),
        Some("r1")
    );

    let mut setup = self::setup();
    setup.natives.replies.opener = None;
    let lines: Vec<String> = (0..MAX_WAITING + 10)
        .map(|at| whole(&format!("m{at}"), &format!("r{at}")))
        .collect();
    setup.feed(&lines);
    setup.sync(true);
    let waiting = &setup.natives.replies.waiting;
    assert_eq!(waiting.len(), MAX_WAITING);
    assert_eq!(
        waiting.front().map(|(_, row, _)| *row),
        Some(MAX_WAITING + 9)
    );
    assert_eq!(waiting.back().map(|(_, row, _)| *row), Some(10));
}

#[test]
fn a_session_not_shown_lays_its_faces_and_asks_once_shown() {
    let mut setup = setup();
    setup.feed(&[whole("m", REPLY)]);
    setup.sync(true);
    let key = setup.opened()[0].0;
    setup.report(key, heading(REPLY, 0..4), true);
    let scratch = (setup.editor.buffers())
        .map(|(id, _)| id)
        .find(|id| *id != setup.buffer);
    let scratch = scratch.expect("scratch");
    setup.editor.show(scratch);
    setup.sync(false);
    setup.natives.replies.opener = Some(Box::new(setup.stand.clone()));
    let opened = setup.opened().len();

    // On while hidden: nothing is laid or asked for, and a reply whole since waits too.
    assert!(!setup.sync(true));
    setup.feed(&[whole("m2", "two")]);
    setup.sync(true);
    assert_eq!(setup.opened().len(), opened, "nothing asked while hidden");
    assert_eq!(setup.at(0), None);

    setup.editor.show(setup.buffer);
    assert!(setup.sync(true), "laid once shown");
    assert_eq!(setup.at(0), Some(Highlight::SessionQuiet));
    let texts: Vec<String> = setup.opened()[opened..]
        .iter()
        .map(|(.., text)| text.clone())
        .collect();
    assert_eq!(texts, ["two", REPLY], "then asked, the newest first");
}
