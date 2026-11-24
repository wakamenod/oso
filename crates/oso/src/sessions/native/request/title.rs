//! A request's title (ADR 0182): oso's words on a band, which name of Claude's side the tool
//! alone.
//!
//! Must not depend on: the app or the UI; plain functions over a block.

use std::fmt::Write as _;

use oso_core::minibuffer::{cells_of, cut_to};

use super::Block;
use super::body::Count;
use crate::sessions::native::render::{Face, Rendered};

/// What a title starts with.
pub(super) const MARK: &str = "⚠ ";
/// The most cells of a tool's name.
const MAX_NAME: usize = 48;
/// The fewest cells of a tool's name that oso's words after it leave it.
const LEAST_NAME: usize = 4;
/// The cells between what a row of oso's says at its left and at its right.
pub(super) const GAP: usize = 2;

/// Whether a title says the tool `name` well enough at any width: Claude Code's own, which are few
/// and short. Any other's whole name is in its block.
pub(super) fn known(name: &str) -> bool {
    matches!(
        name,
        "Bash"
            | "Edit"
            | "MultiEdit"
            | "Write"
            | "NotebookEdit"
            | "Read"
            | "Glob"
            | "Grep"
            | "WebFetch"
            | "WebSearch"
            | "Agent"
            | "Task"
            | "Skill"
    )
}

/// The tool's name `raw` as oso's words say it: anything but ASCII letters, digits, `_`, `-` and
/// `.` escaped.
pub(super) fn whole_name(raw: &str) -> String {
    if raw.is_empty() {
        // Its space is none of a name's own.
        return "A tool".to_owned();
    }
    let mut name = String::new();
    for c in raw.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
            name.push(c);
        } else {
            let _ = write!(name, "\\u{{{:x}}}", u32::from(c));
        }
    }
    name
}

/// `name` as a title says it.
pub(super) fn short(name: &str) -> String {
    cut(name, MAX_NAME)
}

/// `name` cut to `cells`, if it is longer.
pub(super) fn cut(name: &str, cells: usize) -> String {
    let (kept, cut) = cut_to(name, cells);
    format!("{kept}{cut}")
}

/// `block`'s title, `width` cells wide, as request `at` of `of` that wait.
pub(super) fn title(block: &Block, (at, of): (usize, usize), width: usize) -> Rendered {
    let said = said(block, MARK, width);
    let place = (of > 1).then(|| format!("{at} of {of}"));
    let counted = block.count.map(counted);
    let room = width.saturating_sub(cells_of(&said) + GAP);
    // The place is the shorter, and says what the block does not.
    let right = [
        place.iter().chain(&counted).cloned().collect::<Vec<_>>(),
        place.into_iter().collect(),
    ]
    .into_iter()
    .map(|words| words.join(" · "))
    .find(|words| cells_of(words) <= room)
    .unwrap_or_default();
    let mut title = Rendered::default();
    let text = row(&said, &right, width);
    title.put(&text, Some(Face::CardTitle));
    title.put("\n", Some(Face::CardTitle));
    title
}

/// What `block` asks, in oso's words after `mark`, cut to `width` cells: the tool's name first, so
/// that what it asks stays in sight.
pub(super) fn said(block: &Block, mark: &str, width: usize) -> String {
    let after = cells_of(mark) + 1 + cells_of(block.asks());
    let room = width.saturating_sub(after).max(LEAST_NAME);
    let tool = cut(&block.tool, room);
    cut(&format!("{mark}{tool} {}", block.asks()), width)
}

/// A row of `left`, and `right` at its `width`'s end, if there is room.
fn row(left: &str, right: &str, width: usize) -> String {
    let used = cells_of(left) + cells_of(right);
    match width.checked_sub(used) {
        Some(gap) if !right.is_empty() && gap >= GAP => {
            format!("{left}{}{right}", " ".repeat(gap))
        }
        _ => left.to_owned(),
    }
}

/// A change's count in words.
fn counted(count: Count) -> String {
    match count {
        Count::Written(lines) => format!("writes {}", super::body::lines(lines)),
        Count::Changed { added, removed } => {
            format!("adds {}, removes {removed}", super::body::lines(added))
        }
    }
}
