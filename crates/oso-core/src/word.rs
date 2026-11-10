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
//!   `日本語の|文章で` splits as shown, and `ー` stays with hiragana before it, so
//!   `すごーい` is `すごー|い`;
//! - at camelCase boundaries, as `subword-mode` does going forward: `fooBar` is
//!   `foo|Bar`, and `HTTPServer` is `HTTP|Server`.
//!
//! A mark is part of what it follows, whether a word or not, so a word breaks
//! between what comes before a mark and what comes after it, as if it were not there.
//!
//! Whether a word breaks between two clusters depends on them and the one after, so
//! a word is found by reading from the cursor, and forward and backward motion
//! agree. Motion stops after what a stop may read in any case (`motion`,
//! `WalkBudget`), as a long run with no boundary must not stall a key.

use crate::display_map::cell::{is_mark, takes_no_cells};
use crate::display_map::{CellKind, DisplaySnapshot, Piece, PiecesBefore};
use crate::motion::{SEEK_COST, StopWalk, WalkBudget};

/// What a cluster is to word motion. Everything but [`Class::NotWord`] is part of a
/// word, and a word breaks where its class changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    /// White space, punctuation, symbols, controls and invalid bytes: what motion
    /// passes on its way to a word.
    NotWord,
    /// A mark with no base of its own, such as a boxed one past a cluster's limits
    /// (ADR 0054). It belongs to what comes before it, so no word starts at it.
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
    /// takes them.
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
/// dependency (ADR 0030, ADR 0058). Other letters need only
/// `char::is_alphanumeric`; the common ones and common punctuation are here because
/// the table answers faster. Below U+1F000 emoji are those Unicode shows as emoji by
/// default (`Emoji_Presentation`), so that `★` and `♪` stay symbols.
const SCRIPTS: &[ScriptRange] = &[
    (0x00C0, 0x00D6, Class::Latin),
    (0x00D8, 0x00F6, Class::Latin),
    (0x00F8, 0x024F, Class::Latin),
    (0x0391, 0x03C9, Class::Other),
    (0x0400, 0x0482, Class::Other),
    (0x048A, 0x052F, Class::Other),
    (0x1E00, 0x1EFF, Class::Latin),
    (0x2010, 0x2027, Class::NotWord),
    (0x2030, 0x205E, Class::NotWord),
    (0x231A, 0x231B, Class::Emoji),
    (0x23E9, 0x23EC, Class::Emoji),
    (0x23F0, 0x23F0, Class::Emoji),
    (0x23F3, 0x23F3, Class::Emoji),
    (0x25FD, 0x25FE, Class::Emoji),
    (0x2614, 0x2615, Class::Emoji),
    (0x2648, 0x2653, Class::Emoji),
    (0x267F, 0x267F, Class::Emoji),
    (0x2693, 0x2693, Class::Emoji),
    (0x26A1, 0x26A1, Class::Emoji),
    (0x26AA, 0x26AB, Class::Emoji),
    (0x26BD, 0x26BE, Class::Emoji),
    (0x26C4, 0x26C5, Class::Emoji),
    (0x26CE, 0x26CE, Class::Emoji),
    (0x26D4, 0x26D4, Class::Emoji),
    (0x26EA, 0x26EA, Class::Emoji),
    (0x26F2, 0x26F3, Class::Emoji),
    (0x26F5, 0x26F5, Class::Emoji),
    (0x26FA, 0x26FA, Class::Emoji),
    (0x26FD, 0x26FD, Class::Emoji),
    (0x2705, 0x2705, Class::Emoji),
    (0x270A, 0x270B, Class::Emoji),
    (0x2728, 0x2728, Class::Emoji),
    (0x274C, 0x274C, Class::Emoji),
    (0x274E, 0x274E, Class::Emoji),
    (0x2753, 0x2755, Class::Emoji),
    (0x2757, 0x2757, Class::Emoji),
    (0x2795, 0x2797, Class::Emoji),
    (0x27B0, 0x27B0, Class::Emoji),
    (0x27BF, 0x27BF, Class::Emoji),
    (0x2B1B, 0x2B1C, Class::Emoji),
    (0x2B50, 0x2B50, Class::Emoji),
    (0x2B55, 0x2B55, Class::Emoji),
    (0x2C60, 0x2C7F, Class::Latin),
    (0x2E80, 0x2FD5, Class::Han),
    // Ideographic space and punctuation.
    (0x3000, 0x3004, Class::NotWord),
    // 々, 〆 and 〇.
    (0x3005, 0x3007, Class::Han),
    // Brackets and marks.
    (0x3008, 0x3020, Class::NotWord),
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
    (0xAC00, 0xD7A3, Class::Other),
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
        _ if c.is_alphanumeric() && !takes_no_cells(c) => Class::Other,
        _ if is_mark(c) => Class::Mark,
        _ => Class::NotWord,
    }
}

/// What word motion needs to know of a cluster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Kind {
    class: Class,
    case: Case,
    /// It is `ー`, which stays with hiragana before it.
    long_vowel: bool,
}

impl Kind {
    /// What the end of a line, or of the buffer, reads as.
    const NOT_WORD: Self = Self {
        class: Class::NotWord,
        case: Case::None,
        long_vowel: false,
    };

    /// The kind of a display cluster of `kind` that reads `text`.
    fn of(kind: CellKind, text: &str) -> Self {
        match kind {
            CellKind::Text => text.chars().next().map_or(Self::NOT_WORD, |c| Self {
                class: if asks_for_emoji(text, c) {
                    Class::Emoji
                } else {
                    class_of(c)
                },
                case: if c.is_uppercase() {
                    Case::Upper
                } else if c.is_lowercase() || c.is_numeric() {
                    Case::Lower
                } else {
                    Case::None
                },
                long_vowel: matches!(c, 'ー' | 'ｰ'),
            }),
            CellKind::PastLimit(_) => Self {
                class: Class::Mark,
                ..Self::NOT_WORD
            },
            CellKind::Tab
            | CellKind::Control(_)
            | CellKind::Invisible(_)
            | CellKind::InvalidByte(_)
            | CellKind::LineBreak => Self::NOT_WORD,
        }
    }

    fn is_word(self) -> bool {
        !matches!(self.class, Class::NotWord | Class::Mark)
    }

    fn is_mark(self) -> bool {
        self.class == Class::Mark
    }
}

/// Whether what follows the first character of a cluster asks for an emoji: a
/// variation selector, as in `☀️` and `1️⃣`, or a skin tone, as in `☝🏽`.
fn asks_for_emoji(text: &str, first: char) -> bool {
    text.len() > first.len_utf8()
        && text
            .chars()
            .any(|c| c == '\u{fe0f}' || ('\u{1f3fb}'..='\u{1f3ff}').contains(&c))
}

/// Whether a word breaks between two word clusters with only marks between them, with
/// `after` the cluster that follows them.
fn splits(before: Kind, next: Kind, after: Kind) -> bool {
    match (before.class, next.class) {
        (Class::Han, Class::Hiragana | Class::Katakana) => false,
        (Class::Hiragana, Class::Katakana) if next.long_vowel => false,
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

/// The kind of `piece`.
fn kind_of(piece: &Piece<'_>) -> Kind {
    Kind::of(piece.kind, piece.text)
}

/// `piece` as the next cluster on its line: none for a line break, which ends the line.
fn on_line(piece: Option<Piece<'_>>) -> Option<Piece<'_>> {
    piece.filter(|piece| piece.kind != CellKind::LineBreak)
}

/// Takes from `walk` what reading `piece` costs: its bytes, and a seek for each stretch the
/// reader went back to since the last piece, as a window back cost one. The first stretch
/// is where the reading starts, an entry seek (ADR 0101).
fn read(
    walk: &mut StopWalk<'_>,
    piece: &Piece<'_>,
    pieces: &PiecesBefore<'_>,
    seeks: &mut usize,
) -> bool {
    let refills = pieces.refills();
    let mut new = refills - *seeks;
    let first = *seeks == 0 && new > 0;
    *seeks = refills;
    if first {
        if !walk.enter() {
            return false;
        }
        new -= 1;
    }
    walk.spend(piece.text.len() + new * SEEK_COST)
}

/// What the last cluster before `offset` on its line is, passing marks: what a mark at
/// `offset` belongs to. Not a word at the start of the line, or if the stop has read all it
/// may.
fn kind_before_marks(display: &DisplaySnapshot, offset: usize, walk: &mut StopWalk<'_>) -> Kind {
    let mut pieces = display.pieces_before(offset);
    let mut seeks = 0;
    while let Some(piece) = pieces.next() {
        if !read(walk, &piece, &pieces, &mut seeks) || piece.kind == CellKind::LineBreak {
            return Kind::NOT_WORD;
        }
        let kind = kind_of(&piece);
        if !kind.is_mark() {
            return kind;
        }
    }
    Kind::NOT_WORD
}

/// Whether `offset`, a cluster's start, starts its line.
fn starts_line(display: &DisplaySnapshot, offset: usize) -> bool {
    display
        .pieces_before(offset)
        .next()
        .is_none_or(|piece| piece.kind == CellKind::LineBreak)
}

/// Whether a word is at `offset`: the cluster there is a word's, as [`forward_word`]
/// reads it.
pub(crate) fn word_at(display: &DisplaySnapshot, offset: usize) -> bool {
    on_line(display.pieces_after(offset).next()).is_some_and(|piece| kind_of(&piece).is_word())
}

/// Whether a word starts at `offset`, as [`backward_word`] finds starts: the cluster there
/// is a word's and no mark, and what comes before it, passing marks, is no word or splits
/// from it. Reads only the clusters around `offset`, within `budget`, and says yes when it
/// cannot tell, for a caller that checks with [`backward_word`] after (ADR 0091).
pub(crate) fn may_start_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> bool {
    let mut walk = budget.stop();
    if !walk.enter() {
        return true;
    }
    let offset = display.cluster_at(offset).0;
    let mut pieces = display.pieces_after(offset);
    let Some(kind) = on_line(pieces.next()).map(|piece| kind_of(&piece)) else {
        return false;
    };
    if kind.is_mark() {
        return false;
    }
    let after = on_line(pieces.next()).map_or(Kind::NOT_WORD, |piece| kind_of(&piece));
    let before = kind_before_marks(display, offset, &mut walk);
    starts_word(before, kind, after)
}

/// Whether a word ends at `offset`, as [`forward_word`] finds ends: the last cluster before
/// it, passing marks, is a word's, and the cluster at `offset` is no mark and no word, or
/// splits from it, or the line ends there. Reads only the clusters around `offset`, within
/// `budget`: marks past it before `offset` count as no word (ADR 0091).
pub(crate) fn may_end_word(
    display: &DisplaySnapshot,
    offset: usize,
    budget: &mut WalkBudget,
) -> bool {
    let mut walk = budget.stop();
    if !walk.enter() {
        return true;
    }
    let offset = display.cluster_at(offset).0;
    if starts_line(display, offset) {
        return false;
    }
    let mut pieces = display.pieces_after(offset);
    let next = on_line(pieces.next()).map(|piece| kind_of(&piece));
    if next.is_some_and(Kind::is_mark) {
        return false;
    }
    let after = on_line(pieces.next()).map_or(Kind::NOT_WORD, |piece| kind_of(&piece));
    let before = kind_before_marks(display, offset, &mut walk);
    before.is_word() && next.is_none_or(|next| !next.is_word() || splits(before, next, after))
}

/// Where the first word that ends after `offset` ends: past what is not a word, and
/// on to the word's end. The end of the buffer if there is none. It stops short if
/// the stop or the command has read all it may.
pub fn forward_word(display: &DisplaySnapshot, offset: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    if !walk.enter() {
        return offset;
    }
    let mut end = offset;
    let mut pieces = display.pieces_after(offset).peekable();
    // The word's last cluster, passing marks. A cursor on a mark is in what it follows.
    let starts_on_mark =
        on_line(pieces.peek().copied()).is_some_and(|piece| kind_of(&piece).is_mark());
    let mut word = if starts_on_mark {
        let start = display.cluster_at(offset).0;
        Some(kind_before_marks(display, start, &mut walk)).filter(|kind| kind.is_word())
    } else {
        None
    };
    // The kind of the next cluster, found by looking ahead.
    let mut peeked = None;
    while let Some(piece) = pieces.next() {
        if piece.kind == CellKind::LineBreak {
            // A line break ends a word; past it, the next line is read for one.
            if word.is_some() || !walk.spend(SEEK_COST) {
                return end;
            }
            end = piece.offset + piece.text.len();
            peeked = None;
            continue;
        }
        if !walk.spend(piece.text.len()) {
            return end;
        }
        let kind = peeked.take().unwrap_or_else(|| kind_of(&piece));
        let after = on_line(pieces.peek().copied()).map_or(Kind::NOT_WORD, |piece| kind_of(&piece));
        peeked = Some(after);
        if !kind.is_mark() {
            match word {
                Some(before) if !kind.is_word() || splits(before, kind, after) => {
                    return piece.offset;
                }
                _ => word = kind.is_word().then_some(kind),
            }
        }
        end = piece.offset + piece.text.len();
    }
    end
}

/// Where the last word that starts before `offset` starts: past what is not a word,
/// and back to the word's start. The start of the buffer if there is none. It stops
/// short if the stop or the command has read all it may.
///
/// It reads back one cluster at a time. A cluster that is no mark waits for the one before
/// it that is no mark, or for the start of its line, which decide whether a word starts at
/// it, with the cluster right after it.
pub fn backward_word(display: &DisplaySnapshot, offset: usize, budget: &mut WalkBudget) -> usize {
    let mut walk = budget.stop();
    if !walk.enter() {
        return offset;
    }
    let start = display.cluster_at(offset).0;
    // What follows the cluster read last: a mark or not, and no word past a line's end.
    let mut after =
        on_line(display.pieces_after(start).next()).map_or(Kind::NOT_WORD, |piece| kind_of(&piece));
    let mut earliest = start;
    // A cluster that is no mark, its kind, and what follows it, waiting for what is before.
    let mut waiting: Option<(usize, Kind, Kind)> = None;
    let mut pieces = display.pieces_before(start);
    let mut seeks = 0;
    while let Some(piece) = pieces.next() {
        if !read(&mut walk, &piece, &pieces, &mut seeks) {
            return earliest;
        }
        earliest = piece.offset;
        if piece.kind == CellKind::LineBreak {
            // Going back past a line's start costs a seek, as going on past its end does.
            if !walk.spend(SEEK_COST) {
                return earliest;
            }
            if let Some((at, kind, next)) = waiting.take()
                && starts_word(Kind::NOT_WORD, kind, next)
            {
                return at;
            }
            after = Kind::NOT_WORD;
            continue;
        }
        let kind = kind_of(&piece);
        if !kind.is_mark() {
            if let Some((at, waited, next)) = waiting.take()
                && starts_word(kind, waited, next)
            {
                return at;
            }
            waiting = Some((piece.offset, kind, after));
        }
        after = kind;
    }
    match waiting {
        Some((at, kind, next)) if starts_word(Kind::NOT_WORD, kind, next) => at,
        _ => 0,
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
mod by_points;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display_map::DisplayPoint;
    use crate::display_map::{DisplayMap, TabWidth};
    use crate::motion;
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
        assert_eq!(forward_pieces("ａｂ語"), ["ａｂ", "語"]);
    }

    #[test]
    fn long_vowel_marks_count_as_katakana() {
        assert_eq!(forward_pieces("コーヒー"), ["コーヒー"]);
        assert_eq!(forward_pieces("ｺｰﾋｰの"), ["ｺｰﾋｰ", "の"]);
        assert_eq!(forward_pieces("語ー"), ["語ー"]);
        assert_eq!(forward_pieces("ラーメンを"), ["ラーメン", "を"]);
    }

    #[test]
    fn a_long_vowel_mark_stays_with_hiragana_before_it() {
        assert_eq!(forward_pieces("すごーい"), ["すごー", "い"]);
        assert_eq!(backward_pieces("すごーい"), ["すごー", "い"]);
        assert_eq!(forward_pieces("えーと"), ["えー", "と"]);
        assert_eq!(forward_pieces("らーめん"), ["らー", "めん"]);
    }

    #[test]
    fn a_mark_belongs_to_what_it_follows_and_words_break_around_it() {
        // U+309B stands alone as a cluster.
        assert_eq!(forward_pieces("か\u{309b}語"), ["か\u{309b}", "語"]);
        assert_eq!(backward_pieces("か\u{309b}語"), ["か\u{309b}", "語"]);
        let text = "fooか\u{309b}Bar";
        assert_eq!(forward_pieces(text), ["foo", "か\u{309b}", "Bar"]);
        assert_eq!(backward_pieces(text), ["foo", "か\u{309b}", "Bar"]);
        // After what is not a word, a lone mark is not a word either.
        assert_eq!(forward_pieces(" \u{309b} a"), [" \u{309b} a"]);
        assert_eq!(backward_pieces(" \u{309b} a"), [" \u{309b} ", "a"]);
        assert_eq!(forward_pieces("\u{309b}"), ["\u{309b}"]);
    }

    #[test]
    fn a_cursor_on_a_mark_is_in_the_word_it_follows() {
        let text = "か\u{309b}語";
        let d = display(text);
        let mut budget = WalkBudget::new();
        assert_eq!(
            forward_word(&d, "か".len(), &mut budget),
            "か\u{309b}".len()
        );
        // Past a window of marks, each way.
        let marks = "\u{309b}".repeat(motion::BACK_WINDOW * 3);
        let text = format!("か{marks}語");
        let d = display(&text);
        let mut budget = WalkBudget::new();
        let inside = "か".len() + "\u{309b}".len() * motion::BACK_WINDOW * 2;
        assert_eq!(
            forward_word(&d, inside, &mut budget),
            text.len() - "語".len()
        );
        assert_eq!(
            backward_word(&d, text.len(), &mut budget),
            text.len() - "語".len()
        );
        assert_eq!(backward_word(&d, text.len() - "語".len(), &mut budget), 0);
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
        // A variation selector makes a symbol an emoji; alone it is a symbol.
        assert_eq!(forward_pieces("☀\u{fe0f}a"), ["☀\u{fe0f}", "a"]);
        assert_eq!(forward_pieces("★a"), ["★a"]);
        // A skin tone does too, so that `M-d` stops after it.
        for emoji in ["☝\u{1f3fd}", "✌\u{1f3fb}", "⛹\u{1f3fd}"] {
            let text = format!("{emoji} next");
            assert_eq!(forward_pieces(&text), [emoji, " next"]);
        }
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
        // Less than it reads, as each stretch read back costs a seek as well (ADR 0101).
        assert!(back > 0 && read > 0, "{read}");
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
        let text = "語\t".repeat(motion::MAX_STOP_WALK / 2);
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
                .map(|cluster| {
                    let kind = Kind::of(cluster.kind, cluster.text);
                    (cluster.offset, cluster.text.len(), kind)
                })
                .collect();
            // What came before, passing marks.
            let mut before = Kind::NOT_WORD;
            for (index, &(offset, len, kind)) in row.iter().enumerate() {
                if kind.is_mark() {
                    if before.is_word()
                        && let Some(word) = words.last_mut()
                    {
                        word.1 = offset + len;
                    }
                    continue;
                }
                let after = row.get(index + 1).map_or(Kind::NOT_WORD, |next| next.2);
                if starts_word(before, kind, after) {
                    words.push((offset, offset + len));
                } else if kind.is_word()
                    && let Some(word) = words.last_mut()
                {
                    word.1 = offset + len;
                }
                before = kind;
            }
        }
        words
    }

    pub(super) fn word_text() -> impl Strategy<Value = String> {
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
            Just("\u{309b}"),
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
            ('★', Class::NotWord),
            ('♪', Class::NotWord),
            ('✓', Class::NotWord),
            ('…', Class::NotWord),
            ('α', Class::Other),
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
