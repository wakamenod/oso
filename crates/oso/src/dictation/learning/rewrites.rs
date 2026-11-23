//! The person's own rewrites of dictated text (ADR 0175, step 3): the small replacements
//! between the text oso left and the text a watch ended on, by a character diff.
//!
//! Must not depend on: files, threads or the app; it sees strings, and runs on the learning's
//! thread.
//!
//! The diff is oso's own, not `similar`'s, for a bound fixed by the edit distance rather than
//! by a time limit: it stops at [`MAX_DIFF`], as nothing past that is a small rewrite.
//!
//! - Each side is at most [`MAX_SIDE`] bytes, and the diff gives up past [`MAX_DIFF`]
//!   characters inserted or deleted: O((n + m) × [`MAX_DIFF`]) at the most (Myers).
//! - A change that touches a word of ASCII letters, digits or `_` takes in the whole word on
//!   both sides, so that a letter added or dropped in a term is the term's replacement.
//! - Kept: replacements only, each `wrong` at most [`MAX_WRONG`] characters and `right` at
//!   most [`MAX_RIGHT`], on one line, at most [`MAX_REWRITES`] changes, and at most half the
//!   text changed unless it is one change. Pure insertions and deletions are not mishearings.

use std::ops::Range;

use crate::dictation::fix;

/// The most bytes of each side diffed.
pub const MAX_SIDE: usize = 4 * 1024;
/// The most characters inserted or deleted that the diff follows.
pub const MAX_DIFF: usize = 64;
/// The most changes a rewrite has that is learned from.
pub const MAX_REWRITES: usize = 5;
/// The most characters of a rewrite's `wrong`.
pub const MAX_WRONG: usize = 20;
/// The most characters of a rewrite's `right`.
pub const MAX_RIGHT: usize = 40;

/// One change: what of the old text became what of the new.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Hunk {
    old: Range<usize>,
    new: Range<usize>,
}

/// The small replacements that make `old` into `new`, each `(wrong, right)`, in order: none
/// if it is a large rewrite, or either side is too long.
pub fn rewrites(old: &str, new: &str) -> Vec<(String, String)> {
    if old.len() > MAX_SIDE || new.len() > MAX_SIDE || old == new {
        return Vec::new();
    }
    let a: Vec<char> = old.chars().collect();
    let b: Vec<char> = new.chars().collect();
    let Some(hunks) = hunks(&a, &b) else {
        return Vec::new();
    };
    let hunks = widened(&a, &b, hunks);
    if hunks.len() > MAX_REWRITES {
        return Vec::new();
    }
    let changed: usize = hunks.iter().map(|hunk| hunk.old.len()).sum();
    if hunks.len() > 1 && changed * 2 > a.len() {
        return Vec::new();
    }
    hunks
        .iter()
        .filter_map(|hunk| {
            let wrong: String = a[hunk.old.clone()].iter().collect();
            let right: String = b[hunk.new.clone()].iter().collect();
            let kept = !wrong.trim().is_empty()
                && !right.trim().is_empty()
                && wrong.trim() != right.trim()
                && hunk.old.len() <= MAX_WRONG
                && hunk.new.len() <= MAX_RIGHT
                && !wrong.contains('\n')
                && !right.contains('\n')
                && right.chars().all(fix::shown_as_text);
            kept.then_some((wrong, right))
        })
        .collect()
}

/// The changes between `a` and `b`, ascending and apart: their common start and end taken
/// off, then Myers's diff of the rest, `None` past [`MAX_DIFF`].
fn hunks(a: &[char], b: &[char]) -> Option<Vec<Hunk>> {
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (a_mid, b_mid) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);
    let ops = myers(a_mid, b_mid)?;
    let mut hunks: Vec<Hunk> = Vec::new();
    let (mut x, mut y) = (prefix, prefix);
    for op in ops {
        let (dx, dy) = match op {
            Op::Same => {
                x += 1;
                y += 1;
                continue;
            }
            Op::Delete => (1, 0),
            Op::Insert => (0, 1),
        };
        match hunks.last_mut() {
            Some(last) if last.old.end == x && last.new.end == y => {
                last.old.end += dx;
                last.new.end += dy;
            }
            _ => hunks.push(Hunk {
                old: x..x + dx,
                new: y..y + dy,
            }),
        }
        x += dx;
        y += dy;
    }
    Some(hunks)
}

/// A step of the diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Op {
    Same,
    Delete,
    Insert,
}

/// The shortest edit script from `a` to `b`, if it inserts and deletes at most [`MAX_DIFF`]
/// characters: Myers's greedy diff, which keeps one row of furthest points a step.
fn myers(a: &[char], b: &[char]) -> Option<Vec<Op>> {
    let (n, m) = (a.len(), b.len());
    if n + m > 2 * MAX_SIDE {
        return None;
    }
    let offset = MAX_DIFF + 1;
    let width = 2 * MAX_DIFF + 3;
    // The furthest x on each diagonal k, at `offset + k`.
    let mut v = vec![0usize; width];
    let mut trace: Vec<Vec<usize>> = Vec::new();
    for d in 0..=MAX_DIFF {
        trace.push(v.clone());
        let d_i = d.cast_signed();
        let mut k = -d_i;
        while k <= d_i {
            let at = (offset.cast_signed() + k).cast_unsigned();
            let down = k == -d_i || (k != d_i && v[at - 1] < v[at + 1]);
            let mut x = if down { v[at + 1] } else { v[at - 1] + 1 };
            let mut y = (x.cast_signed() - k).cast_unsigned();
            while x < n && y < m && a[x] == b[y] {
                x += 1;
                y += 1;
            }
            v[at] = x;
            if x >= n && y >= m {
                return Some(back(&trace, n, m, d, offset));
            }
            k += 2;
        }
    }
    None
}

/// The script Myers's diff found in `d` steps, read back through `trace`.
fn back(trace: &[Vec<usize>], n: usize, m: usize, d: usize, offset: usize) -> Vec<Op> {
    let mut ops = Vec::with_capacity(n + m);
    let (mut x, mut y) = (n, m);
    for step in (0..=d).rev() {
        let v = &trace[step];
        let k = x.cast_signed() - y.cast_signed();
        let at = (offset.cast_signed() + k).cast_unsigned();
        let step_i = step.cast_signed();
        let down = k == -step_i || (k != step_i && v[at - 1] < v[at + 1]);
        let prev_k = if down { k + 1 } else { k - 1 };
        let prev_x = v[(offset.cast_signed() + prev_k).cast_unsigned()];
        let prev_y = (prev_x.cast_signed() - prev_k).cast_unsigned();
        while x > prev_x && y > prev_y {
            ops.push(Op::Same);
            x -= 1;
            y -= 1;
        }
        if step > 0 {
            ops.push(if x == prev_x { Op::Insert } else { Op::Delete });
        }
        x = prev_x;
        y = prev_y;
    }
    ops.reverse();
    ops
}

/// Whether `c` is of an ASCII word.
fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// `hunks` with each change that touches an ASCII word taking in the whole word on both
/// sides, changes that then meet joined.
fn widened(a: &[char], b: &[char], hunks: Vec<Hunk>) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::with_capacity(hunks.len());
    let ceilings: Vec<usize> = (hunks.iter().skip(1).map(|next| next.old.start))
        .chain([a.len()])
        .collect();
    for (mut hunk, ceiling) in hunks.into_iter().zip(ceilings) {
        let first = |hunk: &Hunk| {
            a.get(hunk.old.start)
                .filter(|_| !hunk.old.is_empty())
                .is_some_and(|&c| is_word(c))
                || b.get(hunk.new.start)
                    .filter(|_| !hunk.new.is_empty())
                    .is_some_and(|&c| is_word(c))
        };
        if first(&hunk) {
            let floor = out.last().map_or(0, |last| last.old.end);
            while hunk.old.start > floor && is_word(a[hunk.old.start - 1]) {
                hunk.old.start -= 1;
                hunk.new.start -= 1;
            }
        }
        let last_word = (!hunk.old.is_empty() && is_word(a[hunk.old.end - 1]))
            || (!hunk.new.is_empty() && is_word(b[hunk.new.end - 1]));
        if last_word {
            // Up to the next change, which it then joins.
            while hunk.old.end < ceiling && is_word(a[hunk.old.end]) {
                hunk.old.end += 1;
                hunk.new.end += 1;
            }
        }
        match out.last_mut() {
            Some(last) if last.old.end >= hunk.old.start => {
                last.old.end = last.old.end.max(hunk.old.end);
                last.new.end = last.new.end.max(hunk.new.end);
            }
            _ => out.push(hunk),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(old: &str, new: &str) -> Vec<(String, String)> {
        rewrites(old, new)
    }

    fn pair(wrong: &str, right: &str) -> (String, String) {
        (wrong.to_owned(), right.to_owned())
    }

    #[test]
    fn a_misheard_word_rewritten_is_one_pair() {
        assert_eq!(
            pairs("ユーズエフェクトを使って", "useEffectを使って"),
            [pair("ユーズエフェクト", "useEffect")]
        );
        assert_eq!(
            pairs("ここでリアクトとビートを使う", "ここでReactとViteを使う"),
            [pair("リアクト", "React"), pair("ビート", "Vite")]
        );
    }

    #[test]
    fn a_letter_added_or_changed_in_a_term_takes_the_whole_term() {
        assert_eq!(
            pairs("call useEfect here", "call useEffect here"),
            [pair("useEfect", "useEffect")]
        );
        assert_eq!(
            pairs("open the dokcer file", "open the docker file"),
            [pair("dokcer", "docker")]
        );
    }

    #[test]
    fn a_word_widened_up_to_the_next_change_joins_it() {
        assert_eq!(pairs("hello from TW", "hello to TW"), [pair("from", "to")]);
        assert_eq!(pairs("an mxa here", "an max here"), [pair("mxa", "max")]);
    }

    #[test]
    fn pure_insertions_and_deletions_are_not_learned() {
        assert!(pairs("今日は晴れ", "今日はとても晴れ").is_empty());
        assert!(pairs("this is good", "this is really good").is_empty());
        assert!(pairs("今日はとても晴れ", "今日は晴れ").is_empty());
        assert!(pairs("same", "same").is_empty());
    }

    #[test]
    fn a_large_rewrite_teaches_nothing() {
        assert!(pairs("今日は晴れです", "明日は雨でした").is_empty());
        let many = "a1 b2 c3 d4 e5 f6 g7 h8 i9 j0 and some words to keep it long enough";
        let changed = "x1 y2 z3 w4 v5 u6 g7 h8 i9 j0 and some words to keep it long enough";
        assert!(pairs(many, changed).is_empty(), "six changes");
        assert!(pairs("ab", &"c".repeat(MAX_DIFF + 2)).is_empty());
    }

    #[test]
    fn long_sides_and_long_pairs_are_left_out() {
        let long = "a".repeat(MAX_SIDE + 1);
        assert!(pairs(&long, "b").is_empty());
        assert!(pairs("short", &long).is_empty());
        let wrong = "ア".repeat(MAX_WRONG + 1);
        assert!(pairs(&format!("x{wrong}y"), "xRighty").is_empty());
        let right = "b".repeat(MAX_RIGHT + 1);
        assert!(pairs("x あ y", &format!("x {right} y")).is_empty());
    }

    #[test]
    fn a_line_break_or_an_invisible_character_is_never_a_pair() {
        assert!(pairs("あいう", "あ\nう").is_empty());
        assert!(pairs("あいう", "あ\u{202e}う").is_empty());
    }

    #[test]
    fn the_diff_is_minimal_within_its_bound() {
        let a: Vec<char> = "abcabba".chars().collect();
        let b: Vec<char> = "cbabac".chars().collect();
        let ops = myers(&a, &b).unwrap();
        let changes = ops.iter().filter(|op| **op != Op::Same).count();
        assert_eq!(changes, 5, "Myers's own example takes five steps");
        let rebuilt: String = {
            let (mut x, mut y, mut out) = (0, 0, String::new());
            for op in ops {
                match op {
                    Op::Same => {
                        out.push(a[x]);
                        x += 1;
                        y += 1;
                    }
                    Op::Delete => x += 1,
                    Op::Insert => {
                        out.push(b[y]);
                        y += 1;
                    }
                }
            }
            out
        };
        assert_eq!(rebuilt, "cbabac");
    }
}
