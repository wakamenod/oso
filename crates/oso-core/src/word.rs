//! Where words start and end, for `M-f`, `M-b`, `M-d` and `M-DEL` (ADR 0030,
//! ADR 0058).
//!
//! Must not depend on: `window`, `editor` or `command`; these are pure functions of
//! offsets over a display snapshot.
//!
//! Text is display clusters, each with a class and a case that its first character
//! decides. A word is a run of clusters of a word class, and it breaks:
//!
//! - where the script changes, but kanji followed by kana stays one word, as
//!   Emacs's `word-combining-categories` does, so `書く` is one word and
//!   `日本語の|文章で` splits as shown;
//! - at camelCase boundaries, as `subword-mode` does: `fooBar` is `foo|Bar`, and
//!   `HTTPServer` is `HTTP|Server`.
//!
//! Whether a word breaks between two clusters depends on them and the one after, so
//! a word is found by reading from the cursor, and forward and backward motion
//! agree. Motion stops after what a stop may read in any case (`motion`,
//! `WalkBudget`), as a long run with no boundary must not stall a key.

use crate::display_map::cell::is_mark;
use crate::display_map::{CellKind, DisplayCluster, DisplayPoint, DisplaySnapshot};
use crate::motion::{self, SEEK_COST, WalkBudget};
use crate::sum_tree::Bias;

/// What a cluster is to word motion. Everything but [`Class::NotWord`] is part of a
/// word, and a word breaks where its class changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// White space, punctuation, symbols, controls and invalid bytes: what motion
    /// passes on its way to a word.
    NotWord,
    /// A mark with no base of its own, such as a boxed one past a cluster's limits
    /// (ADR 0054). No word breaks at it, as Emacs's `word-combining-categories` has
    /// it for combining characters.
    Mark,
    Han,
    Hiragana,
    /// Includes `ー`.
    Katakana,
    /// Latin letters and ASCII digits.
    Latin,
    /// Full-width letters and digits.
    FullWidth,
    /// Emoji make words of their own, so a motion stops around them and `M-d`
    /// takes them, in Emacs too, whose syntax table counts them as word characters.
    Emoji,
    /// Letters and digits of every other script. These are not told apart, as they
    /// rarely touch.
    Other,
}

/// The case of a cluster, for camelCase boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Case {
    /// Not cased.
    None,
    Upper,
    /// Lower case, or a digit: `subword-mode` keeps digits with what they follow.
    Lower,
}

/// A range of code points of one class.
type ScriptRange = (u32, u32, Class);

/// The non-ASCII ranges word motion tells apart, sorted. Written by hand, with no
/// dependency, as ADR 0030 allows: other letters need only `char::is_alphanumeric`.
const SCRIPTS: &[ScriptRange] = &[
    (0x00C0, 0x00D6, Class::Latin),
    (0x00D8, 0x00F6, Class::Latin),
    (0x00F8, 0x024F, Class::Latin),
    (0x1E00, 0x1EFF, Class::Latin),
    (0x231A, 0x231B, Class::Emoji),
    (0x23E9, 0x23F3, Class::Emoji),
    (0x2600, 0x27BF, Class::Emoji),
    (0x2B50, 0x2B50, Class::Emoji),
    (0x2B55, 0x2B55, Class::Emoji),
    (0x2C60, 0x2C7F, Class::Latin),
    (0x2E80, 0x2FD5, Class::Han),
    // 々, 〆 and 〇.
    (0x3005, 0x3007, Class::Han),
    (0x3021, 0x3029, Class::Han),
    (0x3038, 0x303B, Class::Han),
    (0x3041, 0x3096, Class::Hiragana),
    // Spacing voiced sound marks.
    (0x309B, 0x309C, Class::Mark),
    (0x309D, 0x309F, Class::Hiragana),
    // Not ゠ or ・, which are punctuation.
    (0x30A1, 0x30FA, Class::Katakana),
    // ー and the iteration marks.
    (0x30FC, 0x30FF, Class::Katakana),
    (0x31F0, 0x31FF, Class::Katakana),
    (0x3400, 0x4DBF, Class::Han),
    (0x4E00, 0x9FFF, Class::Han),
    (0xA720, 0xA7FF, Class::Latin),
    (0xAB30, 0xAB6F, Class::Latin),
    (0xF900, 0xFAFF, Class::Han),
    (0xFB00, 0xFB06, Class::Latin),
    (0xFF10, 0xFF19, Class::FullWidth),
    (0xFF21, 0xFF3A, Class::FullWidth),
    (0xFF41, 0xFF5A, Class::FullWidth),
    // Half-width katakana, after its punctuation.
    (0xFF66, 0xFF9F, Class::Katakana),
    (0x1F000, 0x1FAFF, Class::Emoji),
    (0x20000, 0x3FFFF, Class::Han),
];

/// The class of a character that starts a cluster of text.
fn class_of(c: char) -> Class {
    if c.is_ascii() {
        return if c.is_ascii_alphanumeric() {
            Class::Latin
        } else {
            Class::NotWord
        };
    }
    let code = u32::from(c);
    let index = SCRIPTS.partition_point(|&(_, last, _)| last < code);
    match SCRIPTS.get(index) {
        Some(&(first, _, class)) if first <= code => class,
        _ if is_mark(c) => Class::Mark,
        _ if c.is_alphanumeric() => Class::Other,
        _ => Class::NotWord,
    }
}

/// What word motion needs to know of a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kind {
    class: Class,
    case: Case,
}

impl Kind {
    /// What the end of a line, or of the buffer, reads as.
    const NOT_WORD: Self = Self {
        class: Class::NotWord,
        case: Case::None,
    };

    fn of(cluster: &DisplayCluster<'_>) -> Self {
        match cluster.kind {
            CellKind::Text => cluster
                .text
                .chars()
                .next()
                .map_or(Self::NOT_WORD, |c| Self {
                    class: class_of(c),
                    case: if c.is_uppercase() {
                        Case::Upper
                    } else if c.is_lowercase() || c.is_numeric() {
                        Case::Lower
                    } else {
                        Case::None
                    },
                }),
            CellKind::PastLimit(_) => Self {
                class: Class::Mark,
                case: Case::None,
            },
            CellKind::Tab
            | CellKind::Control(_)
            | CellKind::Invisible(_)
            | CellKind::InvalidByte(_)
            | CellKind::LineBreak => Self::NOT_WORD,
        }
    }

    fn is_word(self) -> bool {
        self.class != Class::NotWord
    }
}

/// Whether a word breaks between two neighboring word clusters, with `after` the
/// cluster that follows them.
fn splits(before: Kind, next: Kind, after: Kind) -> bool {
    match (before.class, next.class) {
        (Class::Mark, _) | (_, Class::Mark) => false,
        (Class::Han, Class::Hiragana | Class::Katakana) => false,
        (a, b) if a != b => true,
        _ => match (before.case, next.case) {
            (Case::Lower, Case::Upper) => true,
            // The last of a run of capitals starts the next word: `HTTP|Server`.
            (Case::Upper, Case::Upper) => after.class == next.class && after.case == Case::Lower,
            _ => false,
        },
    }
}

/// Whether a word starts at `next`, between `before` and `after`.
fn starts_word(before: Kind, next: Kind, after: Kind) -> bool {
    next.is_word() && (!before.is_word() || splits(before, next, after))
}

/// Where the first word that ends after `offset` ends: past what is not a word, and
/// on to the word's end. The end of the buffer if there is none. It stops short if
/// the stop or the command has read all it may.
pub fn forward_word(display: &DisplaySnapshot, offset: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    if !walk.spend(SEEK_COST) {
        return offset;
    }
    let mut point = display.offset_to_display_point(offset);
    let mut end = offset;
    let mut word: Option<Kind> = None;
    loop {
        let mut clusters = display.clusters(point).peekable();
        while let Some(cluster) = clusters.next() {
            if !walk.spend(1) {
                return end;
            }
            let kind = Kind::of(&cluster);
            let after = clusters.peek().map_or(Kind::NOT_WORD, Kind::of);
            match word {
                Some(before) if !kind.is_word() || splits(before, kind, after) => {
                    return cluster.offset;
                }
                _ => word = kind.is_word().then_some(kind),
            }
            end = cluster.offset + cluster.text.len();
        }
        if word.is_some() || point.row + 1 >= display.row_count() || !walk.spend(SEEK_COST) {
            return end;
        }
        point = DisplayPoint::new(point.row + 1, 0);
        end = motion::line_start(display, point.row);
    }
}

/// The first cluster of a window that has been read, for the window before it.
#[derive(Clone, Copy)]
struct Read {
    offset: usize,
    column: usize,
    kind: Kind,
}

/// Where the last word that starts before `offset` starts: past what is not a word,
/// and back to the word's start. The start of the buffer if there is none. It stops
/// short if the stop or the command has read all it may.
pub fn backward_word(display: &DisplaySnapshot, offset: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    let mut point = display.offset_to_display_point(offset);
    let mut earliest = display.display_point_to_offset(point, Bias::Left);
    // What follows the window: its first cluster's next neighbor decides the last
    // cluster's start, and the first cluster of the window after waits for what
    // comes before it.
    let mut after = display
        .clusters(point)
        .next()
        .map_or(Kind::NOT_WORD, |cluster| Kind::of(&cluster));
    let mut waiting: Option<Read> = None;
    loop {
        let from_row_start = point.column <= motion::BACK_WINDOW;
        let mut before = from_row_start.then_some(Kind::NOT_WORD);
        let mut found = None;
        // The first cluster of the window, and the kind of its neighbor.
        let mut first: Option<Read> = None;
        let mut second = None;
        let mut current: Option<Read> = None;
        let mut read = 0;
        let window = motion::clusters_before(display, point, earliest).map(|cluster| Read {
            offset: cluster.offset,
            column: cluster.column,
            kind: Kind::of(&cluster),
        });
        for next in window.chain(waiting) {
            read += 1;
            if first.is_some() && second.is_none() {
                second = Some(next.kind);
            }
            first.get_or_insert(next);
            if let Some(cluster) = current {
                if before.is_some_and(|before| starts_word(before, cluster.kind, next.kind)) {
                    found = Some(cluster.offset);
                }
                before = Some(cluster.kind);
            }
            current = Some(next);
        }
        if let (Some(cluster), Some(before)) = (current, before)
            && starts_word(before, cluster.kind, after)
        {
            found = Some(cluster.offset);
        }
        let spent = walk.spend(read + SEEK_COST);
        if let Some(found) = found {
            return found;
        }
        earliest = first.map_or(earliest, |first| first.offset);
        if !spent {
            return earliest;
        }
        if from_row_start {
            if point.row == 0 {
                return 0;
            }
            let end = motion::line_end(display, point.row - 1);
            earliest = end;
            point = display.offset_to_display_point(end);
            after = Kind::NOT_WORD;
            waiting = None;
        } else {
            // Past the row's start is handled above, so a window always holds a
            // cluster here.
            let Some(first) = first else {
                return earliest;
            };
            after = second.unwrap_or(after);
            waiting = Some(first);
            point = DisplayPoint::new(point.row, first.column);
        }
    }
}

/// Where `n` words from `offset` ends, backward if `n` is negative. A motion that
/// reaches the buffer's edge stops there, as Emacs's `forward-word` does, and the
/// budget says whether it stopped short.
pub fn move_words(
    display: &DisplaySnapshot,
    mut offset: usize,
    n: isize,
    budget: &mut WalkBudget,
) -> usize {
    for _ in 0..n.unsigned_abs() {
        let next = if n < 0 {
            backward_word(display, offset, budget)
        } else {
            forward_word(display, offset, budget)
        };
        if next == offset || budget.is_spent() {
            return next;
        }
        offset = next;
    }
    offset
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::motion::next_cluster;
    use crate::rope::Rope;
    use proptest::prelude::*;

    fn display(text: &str) -> DisplaySnapshot {
        DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot()
    }

    /// The pieces forward motion from the start makes of `text`: what lies between
    /// its stops.
    fn forward_pieces(text: &str) -> Vec<&str> {
        let d = display(text);
        let mut budget = WalkBudget::new();
        let mut pieces = Vec::new();
        let mut at = 0;
        loop {
            let next = forward_word(&d, at, &mut budget);
            if next == at {
                return pieces;
            }
            pieces.push(&text[at..next]);
            at = next;
        }
    }

    /// The pieces backward motion from the end makes of `text`, in text order.
    fn backward_pieces(text: &str) -> Vec<&str> {
        let d = display(text);
        let mut budget = WalkBudget::new();
        let mut pieces = Vec::new();
        let mut at = text.len();
        loop {
            let next = backward_word(&d, at, &mut budget);
            if next == at {
                pieces.reverse();
                return pieces;
            }
            pieces.push(&text[next..at]);
            at = next;
        }
    }

    #[test]
    fn japanese_breaks_where_the_script_changes() {
        assert_eq!(forward_pieces("日本語の文章で"), ["日本語の", "文章で"]);
        assert_eq!(backward_pieces("日本語の文章で"), ["日本語の", "文章で"]);
        assert_eq!(forward_pieces("書く"), ["書く"]);
        assert_eq!(backward_pieces("書く"), ["書く"]);
        // Kanji with katakana stays one word; other pairs break.
        assert_eq!(
            forward_pieces("東京タワーのカタカナと"),
            ["東京タワー", "の", "カタカナ", "と"]
        );
        assert_eq!(forward_pieces("ひらがな漢字"), ["ひらがな", "漢字"]);
        assert_eq!(
            forward_pieces("カタカナ漢字ひらがな"),
            ["カタカナ", "漢字ひらがな"]
        );
    }

    #[test]
    fn latin_digits_and_full_width_are_their_own_scripts() {
        assert_eq!(
            forward_pieces("Emacsの2024年"),
            ["Emacs", "の", "2024", "年"]
        );
        assert_eq!(forward_pieces("ａｂｃ１２３abc"), ["ａｂｃ１２３", "abc"]);
        assert_eq!(forward_pieces("ＡＢＣ語"), ["ＡＢＣ", "語"]);
    }

    #[test]
    fn long_vowel_marks_count_as_katakana() {
        assert_eq!(forward_pieces("コーヒー"), ["コーヒー"]);
        assert_eq!(forward_pieces("ｺｰﾋｰの"), ["ｺｰﾋｰ", "の"]);
        assert_eq!(forward_pieces("語ー"), ["語ー"]);
    }

    #[test]
    fn punctuation_and_white_space_are_passed_on_the_way_to_a_word() {
        assert_eq!(
            forward_pieces("日本語、文章。\n次"),
            ["日本語", "、文章", "。\n次"]
        );
        assert_eq!(
            backward_pieces("日本語、文章。\n次"),
            ["日本語、", "文章。\n", "次"]
        );
        assert_eq!(forward_pieces("  foo, bar!"), ["  foo", ", bar", "!"]);
        assert_eq!(backward_pieces("  foo, bar!"), ["  ", "foo, ", "bar!"]);
        assert_eq!(forward_pieces("カタ・カナ"), ["カタ", "・カナ"]);
        assert_eq!(forward_pieces("foo_bar"), ["foo", "_bar"]);
        assert_eq!(forward_pieces(" \t\r\n"), [" \t\r\n"]);
        assert_eq!(backward_pieces(" \t\r\n"), [" \t\r\n"]);
        assert_eq!(forward_pieces(""), Vec::<&str>::new());
    }

    #[test]
    fn camel_case_breaks_as_subword_mode_does() {
        assert_eq!(forward_pieces("fooBarBaz"), ["foo", "Bar", "Baz"]);
        assert_eq!(backward_pieces("fooBarBaz"), ["foo", "Bar", "Baz"]);
        assert_eq!(forward_pieces("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(backward_pieces("HTTPServer"), ["HTTP", "Server"]);
        assert_eq!(forward_pieces("HTTP"), ["HTTP"]);
        assert_eq!(forward_pieces("parseHTTP"), ["parse", "HTTP"]);
        assert_eq!(forward_pieces("Foo"), ["Foo"]);
        // Digits stay with what they follow, until a capital starts a word.
        assert_eq!(forward_pieces("foo2Bar3"), ["foo2", "Bar3"]);
        assert_eq!(forward_pieces("utf8decode"), ["utf8decode"]);
        assert_eq!(forward_pieces("HT1"), ["H", "T1"]);
        // Any cased script.
        assert_eq!(forward_pieces("приветМир"), ["привет", "Мир"]);
        assert_eq!(forward_pieces("ａｂＣｄ"), ["ａｂ", "Ｃｄ"]);
        assert_eq!(forward_pieces("éaÉa"), ["éa", "Éa"]);
    }

    #[test]
    fn a_word_stop_is_a_display_cluster_with_its_marks_and_sequences() {
        // A combining mark stays on its base, and dakuten on its kana.
        assert_eq!(forward_pieces("e\u{301}a b"), ["e\u{301}a", " b"]);
        assert_eq!(forward_pieces("か\u{3099}か語"), ["か\u{3099}か", "語"]);
        // Emoji are words of their own, whole.
        let family = "👨\u{200d}👩\u{200d}👧";
        let text = format!("abc{family}🇯🇵 def");
        assert_eq!(
            forward_pieces(&text),
            ["abc", &format!("{family}🇯🇵"), " def"]
        );
        assert_eq!(
            backward_pieces(&text),
            ["abc", &format!("{family}🇯🇵 "), "def"]
        );
    }

    #[test]
    fn invisible_characters_invalid_bytes_and_controls_are_not_words() {
        assert_eq!(forward_pieces("foo\u{202e}bar"), ["foo", "\u{202e}bar"]);
        assert_eq!(forward_pieces("foo\u{1}bar"), ["foo", "\u{1}bar"]);
        let d = crate::buffer::Buffer::from_bytes("t", b"foo\xffbar").display();
        let mut budget = WalkBudget::new();
        assert_eq!(forward_word(&d, 0, &mut budget), 3);
        assert_eq!(forward_word(&d, 3, &mut budget), 9);
    }

    #[test]
    fn past_limit_boxes_are_stops_inside_a_word() {
        // A base and twelve marks: eight in its cluster, four in boxes of their own.
        let marks = "\u{301}".repeat(12);
        let text = format!("a{marks}b c");
        let d = display(&text);
        let mut budget = WalkBudget::new();
        assert_eq!(forward_word(&d, 0, &mut budget), text.len() - 2);
        assert_eq!(backward_word(&d, text.len() - 2, &mut budget), 0);
        // From a box, each way, to the word's edges.
        let boxed = 1 + 8 * 2 + 2;
        assert_eq!(next_cluster(&d, boxed), Some(boxed + 2), "a box of its own");
        assert_eq!(forward_word(&d, boxed, &mut budget), text.len() - 2);
        assert_eq!(backward_word(&d, boxed, &mut budget), 0);
    }

    #[test]
    fn counts_go_as_far_as_they_can() {
        let text = "one two three";
        let d = display(text);
        let mut budget = WalkBudget::new();
        assert_eq!(move_words(&d, 0, 2, &mut budget), 7);
        assert_eq!(move_words(&d, 7, -1, &mut budget), 4);
        assert_eq!(move_words(&d, 7, 0, &mut budget), 7);
        assert_eq!(move_words(&d, 0, isize::MAX, &mut budget), text.len());
        assert_eq!(move_words(&d, text.len(), isize::MIN, &mut budget), 0);
        assert!(!budget.is_spent());
    }

    #[test]
    fn a_run_with_no_boundary_stops_after_what_a_stop_reads() {
        let text = "x".repeat(motion::MAX_STOP_WALK * 3);
        let d = display(&text);
        let mut budget = WalkBudget::new();
        let first = forward_word(&d, 0, &mut budget);
        assert_eq!(first, motion::MAX_STOP_WALK - motion::SEEK_COST);
        assert_eq!(forward_word(&d, first, &mut budget), 2 * first);
        let back = backward_word(&d, text.len(), &mut budget);
        let read = text.len() - back;
        // Half of what it reads, each window costing a seek as well.
        assert!(
            read > motion::MAX_STOP_WALK / 2 - motion::BACK_WINDOW,
            "{read}"
        );
        assert!(read <= motion::MAX_STOP_WALK, "{read}");
        assert!(!budget.is_spent(), "the command can go on");
        // Spaces and line breaks count too.
        let text = " \n".repeat(motion::MAX_STOP_WALK / motion::SEEK_COST * 2);
        let d = display(&text);
        let mut budget = WalkBudget::new();
        let stop = forward_word(&d, 0, &mut budget);
        assert!(stop > 0 && stop < text.len(), "{stop}");
        let stop = backward_word(&d, text.len(), &mut budget);
        assert!(stop > 0 && stop < text.len(), "{stop}");
    }

    #[test]
    fn a_count_stops_when_the_command_has_read_all_it_may() {
        // Each word takes a line, so each costs a seek.
        let text = "a\n".repeat(1 << 15);
        let d = display(&text);
        let mut budget = WalkBudget::new();
        let stop = move_words(&d, 0, isize::MAX, &mut budget);
        assert!(budget.is_spent());
        assert!(stop > 0 && stop < text.len(), "{stop}");
        assert_eq!(forward_word(&d, stop, &mut budget), stop, "nothing is left");
    }

    #[test]
    fn a_stop_on_a_long_run_stays_on_a_display_cluster() {
        // Wide characters and tabs are read in windows of columns.
        let text = "語\t".repeat(motion::MAX_STOP_WALK);
        let d = display(&text);
        let stops = all_stops(&d);
        let mut budget = WalkBudget::new();
        let stop = backward_word(&d, text.len(), &mut budget);
        assert!(stops.contains(&stop));
    }

    /// Every cluster boundary of `d`, as cursor stops.
    fn all_stops(d: &DisplaySnapshot) -> Vec<usize> {
        let mut stops = vec![0];
        while let Some(next) = next_cluster(d, *stops.last().unwrap()) {
            stops.push(next);
        }
        stops
    }

    /// The words of `d`, as ranges, found by looking at every cluster.
    fn naive_words(d: &DisplaySnapshot) -> Vec<(usize, usize)> {
        let mut words: Vec<(usize, usize)> = Vec::new();
        for row in 0..d.row_count() {
            let row: Vec<_> = d
                .clusters(DisplayPoint::new(row, 0))
                .map(|cluster| (cluster.offset, cluster.text.len(), Kind::of(&cluster)))
                .collect();
            for (index, &(offset, len, kind)) in row.iter().enumerate() {
                if !kind.is_word() {
                    continue;
                }
                let before = index
                    .checked_sub(1)
                    .map_or(Kind::NOT_WORD, |index| row[index].2);
                let after = row.get(index + 1).map_or(Kind::NOT_WORD, |next| next.2);
                if starts_word(before, kind, after) {
                    words.push((offset, offset + len));
                } else if let Some(word) = words.last_mut() {
                    word.1 = offset + len;
                }
            }
        }
        words
    }

    fn word_text() -> impl Strategy<Value = String> {
        let piece = prop_oneof![
            Just("日本語"),
            Just("の"),
            Just("カタ"),
            Just("ー"),
            Just("foo"),
            Just("Bar"),
            Just("HTTP"),
            Just("42"),
            Just("Ａ１"),
            Just(" "),
            Just("\n"),
            Just("\r\n"),
            Just("\t"),
            Just("、"),
            Just(","),
            Just("e\u{301}"),
            Just("\u{301}"),
            Just("😀"),
            Just("🇯🇵"),
            Just("\u{202e}"),
            Just("\u{1}"),
            Just("привет"),
        ];
        prop::collection::vec(piece, 0..16).prop_map(|pieces| pieces.concat())
    }

    /// Checks that a motion from every stop of `text` ends on a stop.
    fn stops_are_clusters(text: &str) -> Result<(), TestCaseError> {
        let d = display(text);
        let stops = all_stops(&d);
        let mut budget = WalkBudget::new();
        for &at in &stops {
            prop_assert!(stops.contains(&forward_word(&d, at, &mut budget)));
            prop_assert!(stops.contains(&backward_word(&d, at, &mut budget)));
        }
        Ok(())
    }

    proptest! {
        /// From anywhere, forward motion ends the first word that ends after the
        /// cursor, and backward motion starts the last word that starts before it.
        /// So the two agree on what the words are.
        #[test]
        fn prop_motion_agrees_with_the_words_of_the_text(text in word_text()) {
            let d = display(&text);
            let words = naive_words(&d);
            let mut budget = WalkBudget::new();
            for at in all_stops(&d) {
                let forward = words
                    .iter()
                    .find(|word| word.1 > at)
                    .map_or(text.len(), |word| word.1);
                prop_assert_eq!(forward_word(&d, at, &mut budget), forward, "from {}", at);
                let backward = words
                    .iter()
                    .rfind(|word| word.0 < at)
                    .map_or(0, |word| word.0);
                prop_assert_eq!(backward_word(&d, at, &mut budget), backward, "from {}", at);
            }
        }

        #[test]
        fn prop_every_stop_is_a_display_cluster(text in crate::rope::tests::text_strategy(40)) {
            stops_are_clusters(&text)?;
        }

        #[test]
        fn prop_stops_are_clusters_past_a_clusters_limits(
            base in "[a語😀]", marks in 0usize..40, rest in "[ aB語]{0,4}",
        ) {
            stops_are_clusters(&format!("{base}{}{rest}", "\u{301}".repeat(marks)))?;
        }

        /// Moving by a count ends where as many single moves do.
        #[test]
        fn prop_a_count_is_as_many_moves(text in word_text(), n in -6isize..6) {
            let d = display(&text);
            let mut budget = WalkBudget::new();
            let start = if n < 0 { text.len() } else { 0 };
            let mut at = start;
            for _ in 0..n.unsigned_abs() {
                at = move_words(&d, at, n.signum(), &mut budget);
            }
            prop_assert_eq!(move_words(&d, start, n, &mut budget), at);
        }
    }

    #[test]
    fn the_script_table_is_sorted() {
        assert!(SCRIPTS.windows(2).all(|pair| pair[0].1 < pair[1].0));
        assert!(SCRIPTS.iter().all(|range| range.0 <= range.1));
    }

    #[test]
    fn classes_of_real_characters() {
        for (c, class) in [
            ('a', Class::Latin),
            ('9', Class::Latin),
            ('é', Class::Latin),
            (',', Class::NotWord),
            ('_', Class::NotWord),
            ('語', Class::Han),
            ('々', Class::Han),
            ('𠮷', Class::Han),
            ('あ', Class::Hiragana),
            ('ゝ', Class::Hiragana),
            ('ア', Class::Katakana),
            ('ー', Class::Katakana),
            ('ｱ', Class::Katakana),
            ('・', Class::NotWord),
            ('、', Class::NotWord),
            ('「', Class::NotWord),
            ('\u{3000}', Class::NotWord),
            ('Ａ', Class::FullWidth),
            ('１', Class::FullWidth),
            ('！', Class::NotWord),
            ('😀', Class::Emoji),
            ('✅', Class::Emoji),
            ('д', Class::Other),
            ('한', Class::Other),
            ('\u{301}', Class::Mark),
            ('\u{3099}', Class::Mark),
            ('€', Class::NotWord),
        ] {
            assert_eq!(class_of(c), class, "{c}");
        }
    }
}
