//! The messages being sent to a native session, as its transcript shows them until the CLI
//! replays them (ADR 0165): no rows, and none of a row's faces, so that none passes for a
//! message the CLI took. Each stands where its row will, so that only its look changes then.
//!
//! Must not depend on: the app; it reads the rows and writes text.

use std::ops::Range;

use super::{Face, Rendered, one_line};
use crate::sessions::native::rows::{Rows, Sending};

/// In its turn, and once that ended: three columns, as `〉` and its space take.
const MARKS: (&str, &str) = ("…  ", "✗  ");
/// What is said under one whose turn ended.
const NOT_CONFIRMED: &str = "   claude did not say it took this";
/// And of one the person sent.
const BRINGS_BACK: &str = ": M-p in the composer brings it back";

/// The messages of `rows` being sent; nothing while none is.
pub fn sending_rows(rows: &Rows) -> Rendered {
    let mut out = Rendered::default();
    for (at, sent) in rows.sending().enumerate() {
        if at > 0 || rows.gap_next() {
            out.put("\n", None);
        }
        let Sending {
            text, from, ended, ..
        } = sent;
        let from_here = out.faces.len();
        let mark = if ended { MARKS.1 } else { MARKS.0 };
        match from {
            Some(from) => {
                // Its `@` stands where no message's words do.
                let from = format!("{}@ from {}\n", mark.trim_end(), one_line(from));
                out.put(&from, Some(Face::Dim));
                out.said("   ", text, (Face::Dim, Face::Dim));
            }
            None => out.said(mark, text, (Face::Dim, Face::Dim)),
        }
        if ended {
            let back = if sent.persons { BRINGS_BACK } else { "" };
            out.put(&format!("{NOT_CONFIRMED}{back}\n"), Some(Face::Dim));
        }
        join(&mut out.faces, from_here);
    }
    out
}

/// Puts those of `faces` from `from` on that touch and are alike together, as they are kept.
fn join(faces: &mut Vec<(Range<usize>, Face)>, from: usize) {
    let mut kept = from;
    for at in from..faces.len() {
        let (range, face) = faces[at].clone();
        match faces[from..kept].last_mut() {
            Some((last, same)) if *same == face && last.end == range.start => last.end = range.end,
            _ => {
                faces[kept] = (range, face);
                kept += 1;
            }
        }
    }
    faces.truncate(kept);
}
