//! The tests of a review: comments carried across readings, who may change them, and decisions
//! kept by content. Also the helpers the line map's tests share: a small longest common
//! subsequence and a diff with no context built on it.
//!
//! Must not depend on: `editor`, `buffer` or anything outside the review, or I/O.

use super::*;

/// The longest common subsequence of `a` and `b`, by its whole table: the matcher the tip
/// crate's diff stands in for. Of equal choices, it keeps `a`'s line first.
pub(super) fn lcs(a: &[&[u8]], b: &[&[u8]]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut table = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            table[i][j] = if a[i] == b[j] {
                table[i + 1][j + 1] + 1
            } else {
                table[i + 1][j].max(table[i][j + 1])
            };
        }
    }
    let (mut i, mut j, mut pairs) = (0, 0, Vec::new());
    while i < n && j < m {
        if a[i] == b[j] {
            pairs.push((i, j));
            i += 1;
            j += 1;
        } else if table[i + 1][j] >= table[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    pairs
}

pub(super) fn lines(texts: &[&str]) -> Vec<Box<[u8]>> {
    texts.iter().map(|text| text.as_bytes().into()).collect()
}

/// A hunk removing `removed` from old line `old` and adding `added` at new line `new`.
pub(super) fn hunk(old: u32, removed: &[&str], new: u32, added: &[&str]) -> Hunk {
    Hunk {
        old_start: old,
        new_start: new,
        removed: lines(removed),
        added: lines(added),
    }
}

pub(super) fn blob(id: &str) -> Base {
    Base::Blob(id.as_bytes().into())
}

/// The runs of `old` and `new` that [`lcs`] leaves unmatched, as (old start, old lines, new
/// start, new lines), numbered from 1.
fn changes(old: &[&str], new: &[&str]) -> Vec<(u32, Vec<String>, u32, Vec<String>)> {
    let a: Vec<&[u8]> = old.iter().map(|line| line.as_bytes()).collect();
    let b: Vec<&[u8]> = new.iter().map(|line| line.as_bytes()).collect();
    let mut pairs = lcs(&a, &b);
    pairs.push((old.len(), new.len()));
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    for (pi, pj) in pairs {
        if i < pi || j < pj {
            let removed = old[i..pi].iter().map(|line| (*line).to_owned()).collect();
            let added = new[j..pj].iter().map(|line| (*line).to_owned()).collect();
            out.push((i as u32 + 1, removed, j as u32 + 1, added));
        }
        (i, j) = (pi + 1, pj + 1);
    }
    out
}

/// The shape of `old` against `new`, read against `base`.
pub(super) fn diff(old: &[&str], new: &[&str], base: Base) -> Shape {
    let hunks = changes(old, new)
        .into_iter()
        .map(|(old, removed, new, added)| Hunk {
            old_start: old,
            new_start: new,
            removed: removed.iter().map(|line| line.as_bytes().into()).collect(),
            added: added.iter().map(|line| line.as_bytes().into()).collect(),
        })
        .collect();
    Shape::new(base, hunks).unwrap()
}

/// The hunks of `old` against `new`, as two old sides' hunks.
pub(super) fn base_hunks(old: &[&str], new: &[&str]) -> Vec<BaseHunk> {
    changes(old, new)
        .into_iter()
        .map(|(o, removed, n, added)| BaseHunk {
            old: o..o + removed.len() as u32,
            new: n..n + added.len() as u32,
        })
        .collect()
}

pub(super) const FILE: &[u8] = b"src/a.rs";

pub(super) fn strs(lines: &[String]) -> Vec<&str> {
    lines.iter().map(String::as_str).collect()
}

/// A review whose file [`FILE`] is read as `shape`.
pub(super) fn review_of(shape: Shape) -> Review {
    let mut review = Review::new();
    read(&mut review, shape);
    review
}

/// Carries the review's comments on [`FILE`] to `shape` and applies it.
pub(super) fn read(review: &mut Review, shape: Shape) -> Applied {
    let carried = review
        .carry(FILE, Arc::new(shape))
        .run(|_, _| None, |a: &[&[u8]], b: &[&[u8]]| lcs(a, b));
    review.apply(carried)
}

pub(super) fn new_line(number: u32) -> Anchor {
    Anchor::Line {
        side: Side::New,
        number,
    }
}

pub(super) fn anchor_of(review: &Review, id: CommentId) -> (Anchor, bool) {
    let comment = review.comment(id).unwrap();
    (comment.anchor(), comment.is_outdated())
}

#[test]
fn a_shape_keeps_its_hunks_and_finds_their_lines() {
    let shape = Shape::new(
        blob("1234567"),
        vec![hunk(2, &["b"], 2, &["B", "C"]), hunk(10, &[], 11, &["z"])],
    )
    .unwrap();
    assert_eq!(shape.base(), &blob("1234567"));
    assert_eq!(shape.line(Side::Old, 2), Some(&b"b"[..]));
    assert_eq!(shape.line(Side::New, 3), Some(&b"C"[..]));
    assert_eq!(shape.line(Side::New, 11), Some(&b"z"[..]));
    assert_eq!(shape.line(Side::New, 4), None);
    assert_eq!(shape.line(Side::Old, 10), None);
    assert_eq!(shape.hunk_at(Side::New, 11), Some(1));
    assert_eq!(shape.hunk_at(Side::Old, 1), None);
}

#[test]
fn a_shape_refuses_hunks_out_of_order_or_misnumbered() {
    let shape = |hunks| Shape::new(Base::Unknown, hunks);
    assert_eq!(
        shape(vec![hunk(1, &[], 1, &[])]),
        Err(ShapeError::Empty { hunk: 0 })
    );
    assert_eq!(
        shape(vec![hunk(0, &["a"], 0, &[])]),
        Err(ShapeError::OutOfOrder { hunk: 0 })
    );
    // No old line kept between the two: a diff with no context joins them.
    assert_eq!(
        shape(vec![hunk(2, &["a"], 2, &[]), hunk(3, &["b"], 2, &[])]),
        Err(ShapeError::OutOfOrder { hunk: 1 })
    );
    assert_eq!(
        shape(vec![hunk(4, &[], 4, &["x"]), hunk(4, &[], 5, &["y"])]),
        Err(ShapeError::OutOfOrder { hunk: 1 })
    );
    assert_eq!(
        shape(vec![hunk(2, &["a"], 3, &["b"])]),
        Err(ShapeError::Misnumbered { hunk: 0 })
    );
    assert_eq!(
        shape(vec![hunk(u32::MAX, &["a", "b"], u32::MAX, &[])]),
        Err(ShapeError::TooLong)
    );
    assert!(shape(vec![hunk(1, &[], 1, &["x"]), hunk(2, &["b"], 3, &["B"])]).is_ok());
}

mod comments;
mod readings;
