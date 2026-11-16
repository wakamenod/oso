//! The preview of a search of conversations' row (ADR 0145): what the host is asked for, and
//! the excerpt it gives, prose wrapped at the preview's width, as a conversation has no file to
//! show from.
//!
//! Must not depend on: `command` or `keymap`, or the host's threads or I/O; the host makes the
//! text from what it holds and builds the excerpt on its own thread.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{EXCERPT_LINE, EXCERPT_LINES, Excerpt};
use crate::display_map::{DisplayMap, TabWidth};
use crate::editor::Editor;
use crate::editor::project_search::ConversationPreviewRequest;
use crate::project::{cut_bounds, push_cut};
use crate::rope::Rope;

impl Editor {
    /// The text the host should give for the selected row's preview, once per row selected,
    /// if the innermost prompt searches conversations (ADR 0145), and not while the float has
    /// no room for a preview.
    pub fn take_conversation_preview_request(&mut self) -> Option<ConversationPreviewRequest> {
        self.project_search.conversations.as_ref()?;
        let key = self.selected_search_row()?;
        if self.project_search.preview.asked == Some(key) {
            return None;
        }
        let region = self
            .prompt_layout()
            .map(|layout| layout.regions.preview)
            .filter(|region| region.rows > 0 && region.columns > 0)?;
        let (_, place) = self.selected_search_place()?;
        let session = place.session.clone()?;
        let (message, line, column) = (place.offset, place.line, place.column);
        let preview = &mut self.project_search.preview;
        preview.reset();
        preview.asked = Some(key);
        Some(ConversationPreviewRequest {
            generation: preview.live.load(Ordering::Relaxed),
            session,
            message: usize::try_from(message).unwrap_or(usize::MAX),
            line,
            column,
            rows: region.rows,
            columns: region.columns,
            live: Arc::clone(&preview.live),
        })
    }
}

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

/// The parts of `line` that fit in `columns` cells each, the first at full width and the rest
/// two cells narrower, as byte ranges: broken after a space where one is in the second half of
/// a part, else where the part is full. An empty line is one empty part.
fn wrap_line(line: &str, columns: usize) -> Vec<(usize, usize)> {
    use unicode_width::UnicodeWidthChar as _;
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
        for (at, ch) in line[start..].char_indices() {
            let cells = ch.width().unwrap_or(1).max(1);
            if width + cells > room {
                end = start + at;
                break;
            }
            width += cells;
            if ch == ' ' && width > room / 2 {
                space = Some(start + at + 1);
            }
        }
        if end < line.len()
            && let Some(space) = space
        {
            end = space;
        }
        if end == start {
            // A character wider than the room still takes a part of its own.
            end = start + line[start..].chars().next().map_or(0, char::len_utf8);
        }
        parts.push((start, end));
        if end >= line.len() {
            break;
        }
        start = end;
    }
    parts
}
