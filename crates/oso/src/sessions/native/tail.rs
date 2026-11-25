//! What stands after a native session's rows in its transcript, at its end (ADR 0182, "The
//! block"): the messages being sent (ADR 0165), the queue's rows, then the block of the request that waits for the person, each part
//! with runs of its own. What draws, counts, colours or finds what stands there goes through
//! here.
//!
//! Must not depend on: the editor or the UI; the host puts the text where this says.
//!
//! - Places are in bytes of the tail, whose first byte is the one after the rows' last: rows put
//!   in above move no part of it.
//! - A part is put whole and changes no other part's bytes.

use std::ops::Range;

use oso_core::syntax::Highlight;

/// A stretch of text and its highlight, as `Editor::replace_native_rows` takes them.
pub type Run = (usize, Option<Highlight>);

/// A part of the tail, in the order they stand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The messages oso sent that the CLI has not replayed (ADR 0165).
    Sending,
    /// The rows of the messages that wait (ADR 0165, step 2).
    Queue,
    /// The title of the block of the request that waits for the person, after the row that
    /// stands between it and what is above: drawn again for the window's width and the count.
    Title,
    /// What stands under that title, which does not change until it goes.
    Body,
}

impl Part {
    const ALL: [Self; 4] = [Self::Sending, Self::Queue, Self::Title, Self::Body];
}

/// The tail's parts.
#[derive(Debug, Default)]
pub struct Tail {
    parts: [Text; Part::ALL.len()],
}

/// A part's text and its runs, which cover it.
#[derive(Debug, Default)]
struct Text {
    text: String,
    runs: Vec<Run>,
}

impl Tail {
    /// How many bytes it takes.
    pub fn len(&self) -> usize {
        Part::ALL.iter().map(|&part| self.of(part).text.len()).sum()
    }

    /// Where `part` is in it.
    pub fn place(&self, part: Part) -> Range<usize> {
        let start = (Part::ALL.iter())
            .take_while(|&&before| before != part)
            .map(|&before| self.of(before).text.len())
            .sum();
        start..start + self.of(part).text.len()
    }

    /// Puts `text`, in `runs`, in place of `part`'s. The bytes it takes the place of, or `None`
    /// if it is the same.
    pub fn put(&mut self, part: Part, text: String, runs: Vec<Run>) -> Option<Range<usize>> {
        debug_assert_eq!(runs.iter().map(|run| run.0).sum::<usize>(), text.len());
        let mut runs = runs;
        runs.retain(|run| run.0 > 0);
        let place = self.place(part);
        let held = &mut self.parts[part as usize];
        if held.text == text && held.runs == runs {
            return None;
        }
        *held = Text { text, runs };
        Some(place)
    }

    /// What `part` holds, and its runs.
    pub fn text(&self, part: Part) -> (&str, &[Run]) {
        let held = self.of(part);
        (&held.text, &held.runs)
    }

    /// Its runs, which cover it.
    pub fn runs(&self) -> impl Iterator<Item = Run> {
        (Part::ALL.iter()).flat_map(|&part| self.of(part).runs.iter().copied())
    }

    fn of(&self, part: Part) -> &Text {
        &self.parts[part as usize]
    }
}

#[cfg(test)]
mod tests {
    use oso_core::editor::Editor;
    use oso_core::syntax::Highlight;
    use proptest::prelude::*;

    use super::{Part, Run, Tail};

    /// The part that holds byte `at` of `tail`, if one does, and the byte of that part.
    fn at(tail: &Tail, at: usize) -> Option<(Part, usize)> {
        (Part::ALL.iter()).find_map(|&part| {
            let place = tail.place(part);
            place.contains(&at).then(|| (part, at - place.start))
        })
    }

    /// The parts, each with a highlight of its own.
    const PARTS: [(Part, Highlight); 4] = [
        (Part::Sending, Highlight::SessionPrompt),
        (Part::Queue, Highlight::SessionQuiet),
        (Part::Title, Highlight::CardTitle),
        (Part::Body, Highlight::CardKey),
    ];

    fn runs(text: &str, highlight: Highlight) -> Vec<Run> {
        match text.len() {
            0 => Vec::new(),
            len => vec![(len, Some(highlight))],
        }
    }

    #[test]
    fn what_is_sent_then_the_queue_stand_before_the_block() {
        let mut tail = Tail::default();
        // Nothing for nothing is no change.
        let nothing = vec![(0, Some(Highlight::SessionQuiet))];
        assert_eq!(tail.put(Part::Queue, String::new(), nothing), None);
        assert_eq!((tail.len(), at(&tail, 0)), (0, None));
        let put = |tail: &mut Tail, part: usize, text: &str| {
            let (part, highlight) = PARTS[part];
            tail.put(part, text.to_owned(), runs(text, highlight))
        };
        assert_eq!(put(&mut tail, 3, "  ls\n"), Some(0..0));
        assert_eq!(put(&mut tail, 2, "\n⚠ asks\n"), Some(0..0));
        assert_eq!(put(&mut tail, 1, "Queued (1)\n"), Some(0..0));
        assert_eq!(put(&mut tail, 1, "Queued (1)\n"), None);
        assert_eq!(put(&mut tail, 0, "…  a\n"), Some(0..0));
        let places = PARTS.map(|(part, _)| tail.place(part));
        assert_eq!(places, [0..7, 7..18, 18..28, 28..33]);
        assert_eq!(at(&tail, 17), Some((Part::Queue, 10)));
        assert_eq!(at(&tail, 18), Some((Part::Title, 0)));
        assert_eq!(at(&tail, 28), Some((Part::Body, 0)));
        assert_eq!((at(&tail, 33), tail.len()), (None, 33));
        let all: Vec<Run> = tail.runs().collect();
        let wanted = [(7, 0), (11, 1), (10, 2), (5, 3)].map(|(len, at)| (len, Some(PARTS[at].1)));
        assert_eq!(all, wanted);
        assert_eq!(put(&mut tail, 1, ""), Some(7..18));
        assert_eq!(put(&mut tail, 0, ""), Some(0..7));
        assert_eq!(at(&tail, 0), Some((Part::Title, 0)));
    }

    /// A title that goes on from all it held, of a block whose first byte under it the person is at.
    #[test]
    fn a_title_that_goes_on_moves_no_place_under_it() {
        let mut editor = Editor::default();
        let id = editor.add_native_session_buffer("*claude: s*");
        editor.show(id);
        let mut tail = Tail::default();
        for (at, text) in [(2, "\n"), (3, "\n"), (2, "\n\n")] {
            let (part, highlight) = PARTS[at];
            if text == "\n\n" {
                editor.current_mut().0.selections.move_primary(1);
            }
            let its = runs(text, highlight);
            let place = tail.put(part, text.to_owned(), its.clone()).unwrap();
            editor.replace_native_rows(id, place, text, &its);
        }
        let now = editor.window().selections.primary().cursor;
        assert_eq!(at(&tail, now), Some((Part::Body, 0)));
    }

    /// One change of a transcript: rows put in after the last, the last ones drawn again, or a
    /// part put.
    #[derive(Clone, Debug)]
    enum Change {
        Rows(String),
        Again(String),
        Put(usize, String),
    }

    fn change() -> impl Strategy<Value = Change> {
        let lines = || "([a-z〉あ]{0,6}\n){0,3}";
        prop_oneof![
            lines().prop_map(Change::Rows),
            lines().prop_map(Change::Again),
            (0..PARTS.len(), lines()).prop_map(|(at, text)| Change::Put(at, text)),
        ]
    }

    proptest! {
        /// Whatever rows go in above it and whichever of its parts changes, the transcript ends
        /// with the tail, each part's text in its own highlight and nothing else in it; and a
        /// place under the title stays on its byte until that text changes.
        #[test]
        fn the_parts_stay_apart_whatever_is_put_in_above(
            changes in prop::collection::vec(change(), 1..16),
            row in 0_usize..3,
        ) {
            let mut editor = Editor::default();
            let id = editor.add_native_session_buffer("*claude: s*");
            editor.show(id);
            let mut tail = Tail::default();
            let (mut rows, mut last) = (String::new(), 0);
            let mut texts = [const { String::new() }; PARTS.len()];
            for change in changes {
                // The person goes to a row under the title; the end follows, which is no place.
                let body = tail.place(Part::Body);
                let its = texts[3].split_inclusive('\n').take(row).map(str::len).sum::<usize>();
                let held = (its < body.len() && body.start > tail.place(Part::Title).start).then_some(its);
                let goes = held.map_or(rows.len() + tail.len(), |at| rows.len() + body.start + at);
                editor.current_mut().0.selections.move_primary(goes);
                let mut body_changed = false;
                match change {
                    Change::Rows(text) => {
                        let at = rows.len();
                        editor.replace_native_rows(id, at..at, &text, &[(text.len(), None)]);
                        (last, rows) = (at, rows + &text);
                    }
                    Change::Again(text) => {
                        let range = last..rows.len();
                        editor.replace_native_rows(id, range, &text, &[(text.len(), None)]);
                        rows.truncate(last);
                        rows.push_str(&text);
                    }
                    Change::Put(at, text) => {
                        let (part, highlight) = PARTS[at];
                        body_changed = part == Part::Body;
                        let its = runs(&text, highlight);
                        if let Some(place) = tail.put(part, text.clone(), its.clone()) {
                            let range = rows.len() + place.start..rows.len() + place.end;
                            editor.replace_native_rows(id, range, &text, &its);
                        }
                        texts[at] = text;
                    }
                }
                let buffer = editor.buffer(id).unwrap();
                let wanted = format!("{rows}{}", texts.concat());
                prop_assert_eq!(buffer.text().to_string(), wanted);
                prop_assert_eq!(buffer.text().len(), rows.len() + tail.len());
                // None before anything was put.
                let all = |range, wanted| {
                    buffer.highlights().is_none_or(|highlights| {
                        highlights.runs(range).all(|(_, highlight)| highlight == wanted)
                    })
                };
                prop_assert!(all(0..rows.len(), None));
                for (part, highlight) in PARTS {
                    let place = tail.place(part);
                    let range = rows.len() + place.start..rows.len() + place.end;
                    prop_assert!(all(range, Some(highlight)), "{:?}", part);
                }
                let now = editor.window().selections.primary().cursor;
                if let (Some(at), false) = (held, body_changed) {
                    prop_assert_eq!(self::at(&tail, now - rows.len()), Some((Part::Body, at)));
                }
            }
        }
    }
}
