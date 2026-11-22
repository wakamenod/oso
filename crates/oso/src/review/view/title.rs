//! A review's own section, first in its buffer (ADR 0122): its title, the message of the commit
//! a review of one commit reviews (ADR 0167), and what was said of it.
//!
//! Must not depend on: `app`, `dispatch`, winit, git's runner or the file system; it lays out
//! what it is handed.

use oso_core::review::RowKind;
use oso_core::syntax::Highlight;

use super::section::{Rendered, Writer};
use crate::review::commits::Message;

/// The review's own section, first in its buffer: its title, the message of the commit it
/// reviews when it reviews one (ADR 0167), each line of it indented as `git log` shows it,
/// and what was said of it, each a note.
pub(crate) fn title(title: &str, message: Option<&Message>, said: &[String]) -> Rendered {
    let mut writer = Writer::new(b"");
    let title = title.replace(['\n', '\r'], " ");
    let highlight = Some(Highlight::DiffFileHeader);
    writer.line(RowKind::Note, &[(title.as_bytes(), highlight)]);
    if let Some(message) = message {
        // The author, anyone's words, last, after oso's own.
        let header = format!(
            "commit {} \u{b7} {} \u{b7} {}",
            message.short, message.date, message.author
        );
        let hunk = Some(Highlight::DiffHunkHeader);
        writer.line(RowKind::Message, &[(header.as_bytes(), hunk)]);
        for line in &message.lines {
            // Escaped on one line already: nothing in it ends the row.
            let line = format!("    {line}");
            writer.line(RowKind::Message, &[(line.trim_end().as_bytes(), None)]);
        }
        let left = match (message.left_out, message.cut) {
            (0, false) => None,
            (0, true) => Some("    \u{22ef} the message is cut at 64 KiB".to_owned()),
            (n, _) => Some(format!("    \u{22ef} {n} more lines are not shown")),
        };
        if let Some(left) = left {
            writer.line(
                RowKind::Message,
                &[(left.as_bytes(), Some(Highlight::Outdated))],
            );
        }
        writer.line(RowKind::Message, &[]);
    }
    for line in said {
        writer.note(line);
    }
    writer.done()
}
