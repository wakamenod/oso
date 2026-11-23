//! Claude's text as a permission card's rows (ADR 0165, step 2e-2; ADR 0180): broken at the
//! card's width as the display measures it, so that a row of the card is a row on screen and
//! nothing runs past its edge.
//!
//! Must not depend on: the app or the UI; plain functions over text.
//!
//! - Each line feed starts a row of its own, and a row longer than the card goes on in the next,
//!   marked so. What runs, a command or a diff, is broken exactly at the width; what is read, a
//!   question or a plan, at a space or beside a wide character where there is one.
//! - A mark that would start a row goes on `◌`, as it would join the fence.
//! - A line's clusters are measured once, and each row once more as it is drawn, so that any
//!   text, crafted or not, costs its length.

use std::ops::Range;

use oso_core::display_map::text_clusters;
use oso_core::minibuffer::{cells_of, cut_to};

use crate::sessions::native::render::Face;

/// A row of a card between the frame's sides: what oso says before the text, the text, with no
/// line break, its cells, and whether it goes on with the row above.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Row {
    pub(super) lead: Vec<(String, Face)>,
    pub(super) text: String,
    pub(super) cells: usize,
    pub(super) on: bool,
}

impl Row {
    /// A row oso says among Claude's, as what it leaves out, cut to `room` cells.
    pub(super) fn said(text: &str, room: usize) -> Self {
        let (kept, cut) = cut_to(text, room);
        let text = format!("{kept}{cut}");
        let cells = cells_of(&text);
        Self {
            text,
            cells,
            ..Self::default()
        }
    }
}

/// Where a row longer than the card is broken.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Break {
    /// At the width.
    Exact,
    /// After the last space, or beside the last wide character, that fits; else at the width.
    Words,
}

/// Claude's `text` as rows of at most `room` cells: a row at each line feed, a tab as a space, and
/// a longer row broken as `breaks` says, measured as the display draws it (a mark past what one
/// cluster holds is drawn boxed, wider). At most `most` rows, and whether more were left out.
pub(super) fn rows_of(text: &str, room: usize, breaks: Break, most: usize) -> (Vec<Row>, bool) {
    let room = room.max(1);
    let mut rows = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line).replace('\t', " ");
        // Each display cluster's bytes and cells, measured once.
        let clusters: Vec<(Range<usize>, usize)> = text_clusters(&line, 0)
            .map(|cluster| {
                let bytes = cluster.offset..cluster.offset + cluster.text.len();
                (bytes, cluster.cells)
            })
            .collect();
        let mut from = 0;
        loop {
            if rows.len() >= most {
                return (rows, true);
            }
            let (row, next) = row_from(&line, &clusters, from, (room, breaks));
            rows.push(Row {
                on: from > 0,
                ..row
            });
            from = next;
            if from >= clusters.len() {
                break;
            }
        }
    }
    (rows, false)
}

/// The row that starts at cluster `from` of `line`, and the cluster the next one starts at.
fn row_from(
    line: &str,
    clusters: &[(Range<usize>, usize)],
    from: usize,
    (room, breaks): (usize, Break),
) -> (Row, usize) {
    let Some((first, _)) = clusters.get(from) else {
        return (Row::default(), from);
    };
    // A mark that starts a row would join the fence: it goes on a base of its own.
    let base = line[first.clone()].chars().next().is_some_and(joins);
    let mut used = usize::from(base);
    let mut end = from;
    while let Some((_, cells)) = clusters.get(end)
        && (used + cells <= room || end == from)
    {
        used += cells;
        end += 1;
    }
    let of = |at: usize| &line[clusters[at].0.clone()];
    if breaks == Break::Words && end < clusters.len() && of(end) != " " {
        let word = (from + 1..=end)
            .rev()
            .find(|&at| splits(of(at - 1), of(at)));
        end = word.unwrap_or(end);
    }
    let text_to = |end: usize| {
        let bytes = first.start..clusters[end - 1].0.end;
        let base = if base { BASE } else { "" };
        format!("{base}{}", &line[bytes])
    };
    let mut text = text_to(end);
    let mut cells = cells_of(&text);
    // On a base, or apart from the text before, a long run of marks may be boxed elsewhere, and
    // wider: what no longer fits goes on in the next row.
    while cells > room && end > from + 1 {
        end -= 1;
        text = text_to(end);
        cells = cells_of(&text);
    }
    // What is read starts no row with the space it broke at.
    let next = match clusters.get(end) {
        Some(_) if breaks == Break::Words && of(end) == " " => end + 1,
        _ => end,
    };
    let row = Row {
        text,
        cells,
        ..Row::default()
    };
    (row, next)
}

/// Whether what is read may break between the clusters `before` and `after`: after a space, and
/// beside a wide character, but not before a closing mark nor after an opening one.
fn splits(before: &str, after: &str) -> bool {
    let first = |text: &str| text.chars().next().unwrap_or(' ');
    let (before, after) = (first(before), first(after));
    if before == ' ' {
        return after != ' ';
    }
    (wide(before) || wide(after)) && !closes(after) && !opens(before)
}

/// Whether `c` is a kana, a Han character, a Hangul syllable or a full-width form.
fn wide(c: char) -> bool {
    matches!(
        c,
        '\u{3001}'..='\u{3029}'
            | '\u{3041}'..='\u{3096}'
            | '\u{30a0}'..='\u{30ff}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{ac00}'..='\u{d7a3}'
            | '\u{ff01}'..='\u{ff60}'
    )
}

/// Whether `c` starts no row of what is read: a stop, a closing mark or a small kana.
fn closes(c: char) -> bool {
    "。、，．：；！？・ー」』）］｝】〕〉》〟…‥,.:;!?)]}ぁぃぅぇぉっゃゅょゎァィゥェォッャュョヮ々"
        .contains(c)
}

/// Whether `c` ends no row of what is read: an opening mark.
fn opens(c: char) -> bool {
    "「『（［｛【〔〈《〝([{".contains(c)
}

/// What a mark that starts a row goes on, as a lone mark is shown.
const BASE: &str = "\u{25cc}";

/// Whether `c` joins the character before it, as a combining mark does.
fn joins(c: char) -> bool {
    let mut joined = String::from("a");
    joined.push(c);
    !c.is_ascii() && cells_of(&joined) <= 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(text: &str, room: usize, breaks: Break) -> Vec<String> {
        let (rows, _) = rows_of(text, room, breaks, 100);
        for row in &rows {
            assert_eq!(cells_of(&row.text), row.cells, "{row:?}");
            assert!(row.cells <= room, "{row:?}");
        }
        rows.into_iter().map(|row| row.text).collect()
    }

    #[test]
    fn what_is_read_breaks_at_spaces_and_wide_characters_and_what_runs_at_the_width() {
        let text = "one two three";
        assert_eq!(texts(text, 9, Break::Words), ["one two ", "three"]);
        assert_eq!(texts(text, 9, Break::Exact), ["one two t", "hree"]);
        assert_eq!(texts(text, 7, Break::Words), ["one two", "three"]);
        // A word longer than the row is broken at the width.
        assert_eq!(
            texts("ab cdefghij", 5, Break::Words),
            ["ab ", "cdefg", "hij"]
        );
        // Wide characters break anywhere, and beside a word.
        assert_eq!(texts("あいう", 4, Break::Words), ["あい", "う"]);
        assert_eq!(texts("あい word", 8, Break::Words), ["あい ", "word"]);
        assert_eq!(texts("ab あい", 5, Break::Words), ["ab あ", "い"]);
        // A stop starts no row, and an opening mark ends none.
        assert_eq!(texts("あいう。", 6, Break::Words), ["あい", "う。"]);
        assert_eq!(texts("あい（う", 6, Break::Words), ["あい", "（う"]);
        // Nothing is lost.
        let long = "The 再起動 of it, すべて, is_a_very_long_word_past_the_room end";
        for room in 4..40 {
            let shown = texts(long, room, Break::Words).concat().replace(' ', "");
            assert_eq!(shown, long.replace(' ', ""), "{room}");
        }
    }

    #[test]
    fn a_row_never_starts_with_a_mark_that_would_join_the_fence() {
        for text in ["ab \u{301}cdef", "\u{301}ab", "\u{20e3}", "x\n\u{336}y"] {
            for breaks in [Break::Words, Break::Exact] {
                for row in texts(text, 4, breaks) {
                    assert!(!row.chars().next().is_some_and(joins), "{text:?} {row:?}");
                }
            }
        }
    }

    /// The rows are what the display draws, whatever the text: none past the room, nothing lost.
    #[test]
    fn any_text_fits_its_rows_and_keeps_every_character() {
        let pieces = [
            "a",
            " ",
            "é",
            "e\u{301}",
            "\u{301}",
            "あ",
            "。",
            "한",
            "\u{1100}\u{1161}",
            "👩\u{200d}👩",
            "\u{200b}",
            "\u{1f1ef}",
            "α",
            "\u{202e}",
            "\u{1b}",
            "(",
            "\t",
        ];
        let mut seed = 7_u64;
        for _ in 0..400 {
            let mut text = String::new();
            for _ in 0..60 {
                seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
                text += pieces[usize::try_from(seed >> 33).unwrap() % pieces.len()];
            }
            for room in [16, 23, 76] {
                let shown = texts(&text, room, Break::Exact).concat();
                let plain = |text: &str| text.replace([' ', '\t'], "").replace(BASE, "");
                assert_eq!(plain(&shown), plain(&text));
                let shown = texts(&text, room, Break::Words).concat();
                assert_eq!(plain(&shown), plain(&text));
            }
        }
    }

    /// A text's cost is its length, whatever it holds: twice the text, about twice the time, where
    /// measuring a row again at each character took four times a row's length.
    #[test]
    #[ignore = "a timing to read, run by hand in release"]
    #[expect(clippy::disallowed_macros, reason = "the timing is printed to be read")]
    fn what_a_crafted_text_costs() {
        for piece in [
            "a\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}",
            "α\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}\u{301}",
            "\u{1f469}\u{200d}",
            "a\u{200b}",
            "\u{1100}",
            "привет ",
            "word ",
        ] {
            let text = piece.repeat((64 << 10) / piece.len());
            for room in [76, 196, 346] {
                let started = std::time::Instant::now();
                let (rows, _) = rows_of(&text, room, Break::Words, 2000);
                eprintln!(
                    "{piece:?} {room}: {} rows in {:?}",
                    rows.len(),
                    started.elapsed()
                );
            }
        }
    }
}
