//! `get-text`: whole rows of the tiled window's buffer, view rows while it wraps (ADR 0044,
//! ADR 0104), copied on a thread of their own from O(1) snapshots of the buffer and its wrap,
//! after their size is checked against [`MAX_TEXT`].
//!
//! Must not depend on: `app`, winit or sockets.

use std::sync::mpsc;
use std::thread;

use oso_core::display_map::DisplaySnapshot;
use oso_core::display_map::wrap_map::WrapSnapshot;
use oso_core::editor::Editor;
use oso_core::motion;
use oso_core::rope::Rope;
use serde_json::json;

use super::{MAX_TEXT, MAX_TEXT_ROWS, Reply, WITHHELD};
use crate::channel::state::slice;
use crate::channel::text::{row_range, row_texts};

/// Starts `get-text`: whole rows of the tiled window's buffer, also beneath a prompt,
/// without their line breaks. An error if the buffer may hold secrets. Everything
/// else happens in [`TextJob::run`], off the UI thread.
pub fn get_text(editor: &Editor, start_row: usize, rows: usize) -> Result<TextJob, String> {
    let (window, buffer) = editor.tiled();
    if buffer.is_secret() {
        return Err(WITHHELD.into());
    }
    Ok(TextJob {
        buffer: window.buffer().get(),
        text: buffer.text().clone(),
        display: buffer.display(),
        // O(1), so that the rows are read off the UI thread too.
        wrap: window.wrap().cloned(),
        start_row,
        rows,
    })
}

/// A `get-text` request, with a snapshot of a buffer that was not secret
/// when it was asked for.
pub struct TextJob {
    buffer: u64,
    text: Rope,
    display: DisplaySnapshot,
    /// The window's view rows while its buffer wraps; logical lines otherwise.
    wrap: Option<WrapSnapshot>,
    start_row: usize,
    rows: usize,
}

impl TextJob {
    /// Runs on a background thread, which replies. If the thread cannot
    /// start, the reply sender is dropped, which the client sees as an error.
    pub fn spawn(self, reply: mpsc::Sender<Reply>) {
        let spawned = thread::Builder::new()
            .name("oso-get-text".into())
            .spawn(move || {
                let _ = reply.send(self.run());
            });
        if let Err(e) = spawned {
            log::warn!("cannot start the get-text thread: {e}");
        }
    }

    /// The result: rows past the end are left out. An error if the rows are
    /// more than a response takes.
    pub(super) fn run(&self) -> Reply {
        let display = &self.display;
        if self.rows > MAX_TEXT_ROWS {
            return Err(format!(
                "get-text returns at most {MAX_TEXT_ROWS} rows; {} were asked for",
                self.rows
            ));
        }
        if let Some(wrap) = &self.wrap {
            return self.view_rows(wrap);
        }
        let row_count = display.row_count();
        let range = row_range(display, self.start_row, self.rows);
        if !range.is_empty() {
            let bytes =
                motion::line_end(display, range.end - 1) - motion::line_start(display, range.start);
            if bytes > MAX_TEXT {
                return Err(format!(
                    "rows {} to {} hold {bytes} bytes, and get-text returns at most {MAX_TEXT}; ask for fewer rows",
                    range.start,
                    range.end - 1,
                ));
            }
        }
        let start_row = range.start;
        // Refused above when too long, so nothing is cut short here.
        let (rows, _) = row_texts(&self.text, display, range, usize::MAX);
        Ok(json!({
            "buffer": self.buffer,
            "start_row": start_row,
            "row_count": row_count,
            "rows": rows,
        }))
    }

    /// What [`TextJob::run`] gives while the window wraps: its view rows, each from its start
    /// to the next's, without the line break that ends a logical line (ADR 0104).
    fn view_rows(&self, wrap: &WrapSnapshot) -> Reply {
        let (display, len) = (&self.display, self.text.len());
        let row_count = wrap.row_count();
        let end = self.start_row.saturating_add(self.rows).min(row_count);
        let start_row = self.start_row.min(end);
        let span = |row| {
            let row = wrap.row(display, row);
            let end = if row.continues {
                row.end
            } else {
                row.end.min(motion::line_end(display, row.line))
            };
            row.start.min(len)..end.clamp(row.start.min(len), len)
        };
        if start_row < end {
            let bytes = span(end - 1).end - span(start_row).start;
            if bytes > MAX_TEXT {
                return Err(format!(
                    "rows {start_row} to {} hold {bytes} bytes, and get-text returns at most {MAX_TEXT}; ask for fewer rows",
                    end - 1,
                ));
            }
        }
        let rows: Vec<String> = (start_row..end)
            .map(|row| slice(&self.text, span(row)))
            .collect();
        Ok(json!({
            "buffer": self.buffer,
            "start_row": start_row,
            "row_count": row_count,
            "rows": rows,
        }))
    }
}
