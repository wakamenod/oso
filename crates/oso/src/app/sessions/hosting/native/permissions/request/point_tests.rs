//! When point goes to a call's line by itself, and what a key typed as it went does.
//!
//! Must not depend on: the app or a clock; the tests say what time it is.

use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, Mode};
use oso_core::editor::{Editor, NativeAction, SessionRequest};
use oso_core::frame::{FrameSize, Way};
use oso_core::key::{KeyPress, KeySequence};
use oso_core::window::WindowId;

use super::super::Drawn;
use super::super::gate::{Pressed, waited};
use super::{
    ByItself, Came, Goes, Here, Person, Point, QUIET, Typing, Where, as_it_came, means_composer,
};
use crate::app::agent::Typed;
use crate::dispatch::Dispatcher;

/// The times of a test, in ms after its start.
struct Clock(Instant);

impl Clock {
    fn new() -> Self {
        Self(Instant::now())
    }

    fn at(&self, ms: u64) -> Instant {
        self.0 + Duration::from_millis(ms)
    }
}

fn key(name: &str) -> KeyPress {
    let keys: KeySequence = name.parse().unwrap();
    keys.0[0]
}

/// In the session's empty composer, in the middle of nothing, oso's window in front since 0 ms.
fn person(clock: &Clock) -> Person {
    Person {
        at: Where::Composer,
        middle: false,
        chosen: false,
        japanese: (false, None),
        seen: Some(clock.at(0)),
    }
}

/// Each thing that holds point where it is, at once and later.
fn held(clock: &Clock) -> Vec<(&'static str, Person)> {
    let person = person(clock);
    let elsewhere = Where::Elsewhere;
    vec![
        (
            "elsewhere, as with a draft",
            Person {
                at: elsewhere,
                ..person
            },
        ),
        (
            "in the middle of something",
            Person {
                middle: true,
                ..person
            },
        ),
        (
            "the composer chosen",
            Person {
                chosen: true,
                ..person
            },
        ),
        (
            "a Japanese source",
            Person {
                japanese: (true, Some(clock.at(0))),
                ..person
            },
        ),
        (
            "the window not in front",
            Person {
                seen: None,
                ..person
            },
        ),
    ]
}

#[test]
fn point_goes_at_once_from_the_empty_composer_and_the_transcripts_end() {
    let clock = Clock::new();
    for at in [Where::Composer, Where::Transcript] {
        let person = Person {
            at,
            ..person(&clock)
        };
        assert!(person.goes_at_once(clock.at(500)), "{at:?}");
        // The window just came in front: its first keys may be meant for anything, and the source is
        // told of only now.
        assert!(!person.goes_at_once(clock.at(499)), "{at:?}");
    }
    // The source just changed: what it is now may be on its way to another.
    let switched = Person {
        japanese: (false, Some(clock.at(1000))),
        ..person(&clock)
    };
    assert!(!switched.goes_at_once(clock.at(1499)));
    assert!(switched.goes_at_once(clock.at(1500)));
    for (why, person) in held(&clock) {
        assert!(!person.goes_at_once(clock.at(9000)), "{why}");
    }
}

#[test]
fn point_goes_later_once_nothing_holds_it_and_half_a_second_passed() {
    let clock = Clock::new();
    let shown = clock.at(1000);
    let goes_at = |person: Person, typed| person.goes_at(shown, typed);
    let free = person(&clock);
    assert_eq!(goes_at(free, None), Some(clock.at(1500)));
    assert_eq!(goes_at(free, Some(clock.at(200))), Some(clock.at(1500)));
    assert_eq!(goes_at(free, Some(clock.at(4000))), Some(clock.at(4500)));
    // Since the person's Japanese gave way to another source, and since oso came in front.
    let switched = Person {
        japanese: (false, Some(clock.at(5000))),
        ..free
    };
    assert_eq!(goes_at(switched, None), Some(clock.at(5500)));
    let back = Person {
        seen: Some(clock.at(6000)),
        ..free
    };
    assert_eq!(goes_at(back, None), Some(clock.at(6500)));
    for (why, person) in held(&clock) {
        assert_eq!(goes_at(person, None), None, "{why}");
    }
}

/// The keys typed at the moment of the move out of the empty composer, and later: what is text goes to
/// the composer, and nothing of it answers.
#[test]
fn what_is_typed_as_point_comes_is_text_or_does_nothing() {
    let clock = Clock::new();
    let came = clock.at(0);
    let of = |typed, ms, middle| as_it_came(typed, true, Some(came), clock.at(ms), middle);
    for name in ["y", "n", "N", "a", "Y", "1", "SPC", "i", "é"] {
        let typed = Typing::Key(key(name));
        assert_eq!(of(typed, 0, false), Came::Draft, "{name}");
        assert_eq!(of(typed, 499, false), Came::Draft, "{name}");
        assert_eq!(of(typed, 500, false), Came::Line, "{name}");
        // After a prefix key or an argument, in a macro, behind what is held.
        assert_eq!(of(typed, 100, true), Came::Line, "{name}");
    }
    assert_eq!(of(Typing::Text, 100, false), Came::Draft);
    assert_eq!(of(Typing::Text, 100, true), Came::Line);
    for name in ["RET", "S-RET"] {
        assert_eq!(
            of(Typing::Key(key(name)), 100, false),
            Came::Dropped,
            "{name}"
        );
        assert_eq!(of(Typing::Key(key(name)), 500, false), Came::Line, "{name}");
    }
    for name in [
        "C-c", "C-u", "C-g", "C-v", "M-v", "C-y", "DEL", "TAB", "<down>", "C-x",
    ] {
        assert_eq!(of(Typing::Key(key(name)), 100, false), Came::Line, "{name}");
    }
    assert_eq!(of(Typing::Act, 100, false), Came::Line);
    // No frame showed point on the line yet, or oso's window is not in front: whatever the time is.
    let unseen = |typed| as_it_came(typed, true, None, clock.at(9000), false);
    assert_eq!(unseen(Typing::Key(key("y"))), Came::Draft);
    assert_eq!(unseen(Typing::Key(key("RET"))), Came::Dropped);
    // From the transcript it is no text.
    let from = |name| {
        as_it_came(
            Typing::Key(key(name)),
            false,
            Some(came),
            clock.at(100),
            false,
        )
    };
    assert_eq!((from("y"), from("RET")), (Came::Line, Came::Dropped));
}

/// Text stops being the composer's at the moment the line may take a key, and no later: no key is both,
/// and none typed as point came is taken by the line.
#[test]
fn no_key_is_both_text_and_an_answer() {
    let clock = Clock::new();
    for ms in [0, 250, 499, 500, 501, 2000] {
        let pressed = Pressed {
            at: clock.at(ms),
            before: None,
        };
        let draft = as_it_came(
            Typing::Key(key("y")),
            true,
            Some(clock.at(0)),
            clock.at(ms),
            false,
        );
        let answers = waited(clock.at(0), pressed);
        assert_eq!(draft == Came::Draft, !answers, "{ms}");
    }
    assert_eq!(QUIET, Duration::from_millis(500));
}

/// The line as the buffer selected, in a dispatcher.
fn line() -> (Dispatcher, Editor) {
    let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", ""));
    let line = editor.add_internal_buffer("*request: s*", Mode::Request);
    editor.show(line);
    (Dispatcher::new(), editor)
}

fn asked(editor: &mut Editor) -> Vec<NativeAction> {
    (editor.take_session_requests().into_iter())
        .filter_map(|request| match request {
            SessionRequest::Native { action, .. } => Some(action),
            _ => None,
        })
        .collect()
}

fn answers(action: &NativeAction) -> bool {
    matches!(
        action,
        NativeAction::Allow | NativeAction::Unread | NativeAction::Deny | NativeAction::Note
    )
}

/// What reaches the line within 500 ms of point coming to it, by any way, asks for an answer only
/// under the quiet wait, measured to its first key, or not at all.
#[test]
fn nothing_typed_as_point_comes_answers_on_the_line() {
    let clock = Clock::new();
    // The keys, and when each is pressed.
    let keys: [(&str, &[u64]); 9] = [
        ("y", &[100]),
        ("n", &[499]),
        ("N", &[10]),
        ("RET", &[100]),
        ("C-u y", &[100, 900]),
        ("C-u y", &[300, 5000]),
        ("C-c C-k", &[400, 1200]),
        ("SPC y", &[100, 590]),
        ("C-v n", &[200, 650]),
    ];
    for (names, pressed) in keys {
        let (mut dispatcher, mut editor) = line();
        let names: KeySequence = names.parse().unwrap();
        let mut typed = Typed::default();
        for (key, ms) in names.0.iter().zip(pressed) {
            typed.key(clock.at(*ms), dispatcher.keys_in_progress());
            dispatcher.handle_key(&mut editor, *key);
            for action in asked(&mut editor) {
                let pressed = Pressed::last(typed, clock.at(*ms));
                assert!(
                    !answers(&action) || !waited(clock.at(0), pressed),
                    "{names} {ms}"
                );
            }
        }
    }
    // A held key's repeats, an input method's text and a paste ask for none.
    let (mut dispatcher, mut editor) = line();
    dispatcher.set_key_repeats(true);
    for name in ["y", "n", "N"] {
        dispatcher.handle_key(&mut editor, key(name));
    }
    dispatcher.set_key_repeats(false);
    dispatcher.commit_text(&mut editor, "y");
    dispatcher.insert_text(&mut editor, "yes\nn");
    assert!(!asked(&mut editor).iter().any(answers));
}

/// The `n`th window of an editor.
fn window(n: usize) -> WindowId {
    let mut editor = Editor::default();
    editor.set_frame_size(FrameSize {
        rows: 60,
        columns: 60,
    });
    for _ in 0..4 {
        editor.split_window(Way::Down, false).unwrap();
    }
    let ids: Vec<WindowId> = editor.windows().map(|window| window.id()).collect();
    ids[n]
}

const ONE: u64 = 1;
const TWO: u64 = 2;

fn waits(key: u64) -> Drawn {
    Drawn {
        key,
        title: ((1, 1), 80),
        line: None,
        stale: false,
        goes: Goes::default(),
        heads: super::super::head::Heads::default(),
    }
}

/// Point as it went by itself from the composer of session `ONE` to its line, in window 1.
fn went() -> Point {
    let mut point = Point::default();
    point.went(ByItself {
        key: ONE,
        composer: true,
    });
    point.note((window(1), Here::Line(ONE)), std::iter::empty());
    point
}

/// The keyboard's source is told to be Japanese only after point went: as long as the person did
/// nothing, point is taken back, however late.
#[test]
fn a_move_by_itself_can_be_taken_back_until_the_person_does_something() {
    let back = |point: &mut Point| point.take(Here::Line(ONE)).map(|by| (by.key, by.composer));
    let mut point = went();
    // Frames, and events that are none of the person's.
    assert_eq!(point.seen(), Some(ONE));
    for _ in 0..3 {
        point.note((window(1), Here::Line(ONE)), std::iter::empty());
    }
    assert_eq!(back(&mut point), Some((ONE, true)));
    assert_eq!(back(&mut point), None, "once");
    // Another place selected, as before the event ends.
    for here in [Here::Composer(ONE), Here::Elsewhere, Here::Line(TWO)] {
        assert!(went().take(here).is_none(), "{here:?}");
    }

    // Taken by what the person did, there is none.
    let mut point = went();
    assert!(point.take(Here::Line(ONE)).is_some(), "a key");
    assert_eq!(back(&mut point), None);
    // Nor once point left, and came back by the person's doing.
    for here in [Here::Composer(ONE), Here::Elsewhere, Here::Line(TWO)] {
        let mut point = went();
        point.note((window(2), here), std::iter::empty());
        point.note((window(1), Here::Line(ONE)), std::iter::empty());
        assert_eq!(back(&mut point), None, "{here:?}");
        assert!(!point.unseen(ONE), "{here:?}");
    }
}

#[test]
fn a_line_point_came_to_by_itself_takes_no_key_before_a_frame_showed_it() {
    let mut point = went();
    assert!(point.unseen(ONE) && !point.unseen(TWO));
    // A key that is none of the line's answers shows nothing.
    let _ = point.take(Here::Line(ONE));
    assert!(point.unseen(ONE));
    assert_eq!(point.seen(), Some(ONE));
    assert!(!point.unseen(ONE));
    assert_eq!(point.seen(), None, "the first");
}

/// Two sessions wait, each in a window of its own.
#[test]
fn a_composer_chosen_and_a_line_gone_to_are_one_sessions() {
    let mut calls = vec![waits(ONE), waits(TWO)];
    let mut point = Point::default();
    let state = |calls: &[Drawn]| -> Vec<(bool, bool)> {
        calls
            .iter()
            .map(|call| (call.goes.pending, call.goes.chosen))
            .collect()
    };
    // In the composer already as the calls are shown.
    point.window = Some(window(1));
    point.note((window(1), Here::Composer(ONE)), calls.iter_mut());
    assert_eq!(state(&calls), [(true, false), (true, false)]);
    // To the other session's line, and to this one's composer.
    point.note((window(2), Here::Line(TWO)), calls.iter_mut());
    assert_eq!(state(&calls), [(true, false), (false, false)]);
    point.note((window(1), Here::Composer(ONE)), calls.iter_mut());
    assert_eq!(state(&calls), [(true, true), (false, false)]);
    // Elsewhere, and back to what was chosen: it stays so.
    point.note((window(3), Here::Elsewhere), calls.iter_mut());
    point.note((window(1), Here::Composer(ONE)), calls.iter_mut());
    assert_eq!(state(&calls), [(true, true), (false, false)]);
}

/// A press of the mouse on the line as it just took the composer's place, as the person pressed where
/// the composer was.
#[test]
fn a_press_on_the_line_that_just_took_the_composers_place_is_the_composers() {
    let clock = Clock::new();
    let since = Some(clock.at(0));
    for ms in [0, 499] {
        assert!(means_composer(true, true, since, clock.at(ms)), "{ms}");
        assert!(
            !means_composer(false, true, since, clock.at(ms)),
            "elsewhere"
        );
        assert!(
            !means_composer(true, false, since, clock.at(ms)),
            "above it"
        );
    }
    assert!(!means_composer(true, true, since, clock.at(500)));
    assert!(
        means_composer(true, true, None, clock.at(9000)),
        "no frame yet"
    );
}

/// A key too soon after another that did nothing is neither text nor an answer.
#[test]
fn a_key_soon_after_one_that_was_dropped_does_nothing() {
    let clock = Clock::new();
    let of = |name, ms| {
        as_it_came(
            Typing::Key(key(name)),
            true,
            Some(clock.at(0)),
            clock.at(ms),
            false,
        )
    };
    assert_eq!(of("RET", 400), Came::Dropped);
    assert_eq!(of("y", 600), Came::Line);
    let pressed = Pressed {
        at: clock.at(600),
        before: Some(clock.at(400)),
    };
    assert!(!waited(clock.at(0), pressed));
}
