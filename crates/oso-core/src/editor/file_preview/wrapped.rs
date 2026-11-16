//! A preview's excerpt of prose rather than of a file (ADR 0145): lines wrapped at the
//! preview's width, by the cell grid's widths, as a conversation's messages are long lines that
//! a file's preview would scroll across.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host builds the
//! excerpt on its own thread.

use super::{EXCERPT_LINE, EXCERPT_LINES, Excerpt};
use crate::display_map::{CellKind, Clusters, DisplayMap, TabWidth};
use crate::project::{cut_bounds, push_cut};
use crate::rope::Rope;

impl Excerpt {
    /// The excerpt of `text`, lines of prose such as a conversation's messages (ADR 0145),
    /// each wrapped at `columns` cells, a word at a time where a word fits, its continuations
    /// indented by two cells. The row's match is on the line `line`, from 0, at `column`, in
    /// characters. Keeps at most [`EXCERPT_LINES`] lines of what it wraps to, each at most
    /// [`EXCERPT_LINE`] bytes; it has no file, so no open buffer shows in its place. Builds its
    /// display model, O(n) in what it keeps, so the host's thread makes it.
    pub fn wrapped(text: &str, columns: usize, (line, column): (usize, usize)) -> Self {
        let columns = columns.max(8);
        let mut out = String::with_capacity(text.len().min(EXCERPT_LINES * EXCERPT_LINE));
        let mut cursor = None;
        let mut rows = 0;
        'lines: for (index, whole) in text.split('\n').enumerate() {
            let hit = (index == line).then_some(column);
            for (start, end) in wrap_line(whole, columns) {
                if rows == EXCERPT_LINES {
                    break 'lines;
                }
                if rows > 0 {
                    out.push('\n');
                }
                if start > 0 {
                    out.push_str("  ");
                }
                let part = &whole[start..end];
                let before = out.len();
                let (cut_start, cut_end) =
                    cut_bounds(part.as_bytes(), 0, EXCERPT_LINE.min(part.len()));
                push_cut(&mut out, part.as_bytes(), (cut_start, cut_end));
                if let Some(column) = hit
                    && cursor.is_none()
                {
                    let chars_before = whole[..start].chars().count();
                    let chars_here = part.chars().count();
                    if column < chars_before + chars_here || end == whole.len() {
                        let within = column.saturating_sub(chars_before).min(chars_here);
                        let at = part
                            .char_indices()
                            .nth(within)
                            .map_or(part.len(), |(at, _)| at);
                        cursor = Some(before + at.min(out.len() - before));
                    }
                }
                rows += 1;
            }
        }
        let cursor = cursor.unwrap_or(0).min(out.len());
        let text = Rope::from(out.as_str());
        let display = DisplayMap::new(&text, TabWidth::default());
        Self {
            text,
            display,
            cursor,
            identity: None,
        }
    }
}

/// The cells a tab is counted as when wrapping: the display model draws it to the next stop,
/// which wrapping here does not know.
const TAB_CELLS: usize = 4;

/// The parts of `line` that fit in `columns` cells each, the first at full width and the rest
/// two cells narrower, as byte ranges: broken after a space where one is in the second half of
/// a part, else where the part is full. Cells are the cell grid's, a grapheme cluster at a
/// time, so a boxed control or invisible character takes what it is drawn as. An empty line is
/// one empty part.
fn wrap_line(line: &str, columns: usize) -> Vec<(usize, usize)> {
    let mut parts = Vec::new();
    let mut start = 0;
    while start < line.len() || parts.is_empty() {
        let room = if parts.is_empty() {
            columns
        } else {
            columns - 2
        };
        let mut width = 0;
        let mut end = line.len();
        let mut space = None;
        for (at, cluster, kind, cells) in Clusters::new(&line[start..]) {
            let cells = if kind == CellKind::Tab {
                TAB_CELLS
            } else {
                cells
            };
            if width + cells > room {
                end = start + at;
                break;
            }
            width += cells;
            if cluster == " " && width > room / 2 {
                space = Some(start + at + 1);
            }
        }
        if end < line.len()
            && let Some(space) = space
        {
            end = space;
        }
        if end == start {
            // A cluster wider than the room still takes a part of its own.
            end = start
                + Clusters::new(&line[start..])
                    .next()
                    .map_or(0, |(_, cluster, _, _)| cluster.len());
        }
        parts.push((start, end));
        if end >= line.len() {
            break;
        }
        start = end;
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wrapped_excerpt_breaks_at_words_and_keeps_the_match() {
        let text = "‹ one two three four five six seven eight nine ten";
        let excerpt = Excerpt::wrapped(text, 20, (0, 30));
        let shown = excerpt.text.to_string();
        let lines: Vec<&str> = shown.lines().collect();
        assert!(lines.len() > 1, "{shown}");
        assert!(
            lines.iter().skip(1).all(|line| line.starts_with("  ")),
            "{shown}"
        );
        assert!(
            lines.iter().all(|line| line.chars().count() <= 20),
            "{shown}"
        );
        assert!(shown[excerpt.cursor..].starts_with("seven"), "{shown}");
        // Wide characters take two cells each.
        let wide = Excerpt::wrapped(&"あ".repeat(30), 20, (0, 0));
        assert!(
            wide.text
                .to_string()
                .lines()
                .next()
                .unwrap()
                .chars()
                .count()
                <= 10
        );
        // An empty text is one empty line.
        assert_eq!(Excerpt::wrapped("", 20, (0, 0)).text.to_string(), "");
    }
}
