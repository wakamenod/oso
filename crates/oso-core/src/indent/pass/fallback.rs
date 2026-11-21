//! The answer past the budget (ADR 0115): an existing line keeps its indentation, and a new
//! one follows the nearest non-blank line before it, as its mode's lexer says (ADR 0164).
//!
//! Must not depend on: `buffer`, `motion` or `search`; it reads a rope, from the budget's
//! reserve.

use super::{HEAD_BYTES, Head, Lexer, deepest, measure};
use crate::indent::{Answer, Budget, Line, Settings};
use crate::rope::Rope;
use crate::sum_tree::Bias;

/// What looking for the line a fallback copies may read back: a little first, then more.
const FALLBACK_BACK: [usize; 2] = [128, 1024];

/// The most bytes read after a new line's start to see what it starts with.
const HEAD_READ: usize = HEAD_BYTES + 64;

/// The answer for a line past the budget: an existing line keeps its indentation, and a
/// new one takes that of the nearest non-blank line it follows, as Helix's `simple` does.
/// What that reads comes from the budget's reserve for fallbacks; past it, no indentation.
pub(in crate::indent) fn fallback<L: Lexer>(
    rope: &Rope,
    settings: Settings,
    line: Line,
    budget: &mut Budget,
) -> Answer {
    let line_asked = line;
    let (Line::New(offset) | Line::Empty(offset)) = line else {
        return Answer::Keep;
    };
    let target = rope.clip_offset(offset, Bias::Left);
    for back in FALLBACK_BACK {
        let base = rope.clip_offset(target.saturating_sub(back), Bias::Left);
        if !budget.spend_reserve(target - base) {
            break;
        }
        let text: String = rope.chunks_in_range(base..target).collect();
        // The lines it holds, nearest first; the first one is whole only at the buffer's start.
        let mut lines = text.rsplit('\n').peekable();
        while let Some(line) = lines.next() {
            if lines.peek().is_none() && base > 0 {
                break;
            }
            let indentation = measure(line, settings.tab_width());
            if !indentation.blank {
                // What the new line starts with, read from the reserve too; the empty line a
                // break opens between brackets has nothing.
                let head = match line_asked {
                    Line::New(_) if budget.spend_reserve(HEAD_READ) => head_after(rope, target),
                    _ => Head::blank(),
                };
                let rest = &line.as_bytes()[indentation.bytes..];
                let column = L::after_line_before(settings, rest, indentation.column, head.text());
                return deepest(Answer::Column(column));
            }
        }
        if base == 0 {
            break;
        }
    }
    Answer::Column(0)
}

/// The start of the text at `at`, past spaces and tabs, as far as its line's end: what a new
/// line there starts with.
fn head_after(rope: &Rope, at: usize) -> Head {
    let mut head = Head::blank();
    let end = rope.clip_offset((at + HEAD_READ).min(rope.len()), Bias::Left);
    'chunks: for chunk in rope.chunks_in_range(at..end) {
        for &byte in chunk.as_bytes() {
            match byte {
                b'\n' => break 'chunks,
                b' ' | b'\t' | b'\r' if head.len == 0 => {}
                _ if head.len == HEAD_BYTES => break 'chunks,
                _ => {
                    head.bytes[head.len] = byte;
                    head.len += 1;
                }
            }
        }
    }
    head
}
