//! The agent channel's `text` (ADR 0085): whole rows of a buffer, or the selected window's
//! region, checked on the UI thread and copied off it from an O(1) snapshot, as the dev
//! socket's `get-text` is (ADR 0044). Never of a buffer that may hold secrets, or of one
//! withheld from agents (ADR 0121). A terminal's rows come from what the UI keeps of its
//! screen, only where the person let agents read it and never while its program reads a
//! password (ADR 0120, Q16, Q17).
//!
//! Must not depend on: sockets or `app`; the caller hands the job a reply channel.

use std::ops::Range;
use std::sync::mpsc;
use std::thread;

use oso_core::buffer::BufferId;
use oso_core::display_map::DisplaySnapshot;
use oso_core::editor::Editor;
use oso_core::motion;
use oso_core::rope::Rope;
use serde_json::json;

use super::protocol::TextOf;
use super::server::Reply;
use super::state::slice;

/// The most text of rows one `text` gives, in bytes.
pub const MAX_ROWS_TEXT: usize = 512 * 1024;

/// The most text of a region one `text` gives, in bytes.
pub const MAX_REGION: usize = 64 * 1024;

/// Why text is refused for a buffer the channel is still opening.
const OPENING: &str = "the buffer is still opening; try again";

/// Why text is refused for a buffer that may hold secrets.
const WITHHELD: &str = "the buffer may hold secrets, so its text stays in the editor";

/// Why text is refused for a buffer withheld from agents (ADR 0121).
const PERSONS: &str = "only the person reads that buffer, so its text stays in the editor";

/// Why rows are refused for a terminal the person has not let agents read (ADR 0120, Q16).
const TERMINAL: &str = "the person has not let agents read that terminal \
                        (terminal-allow-agent-read), so its rows stay in the editor";

/// Why rows are refused for a terminal whose program reads a password (ADR 0120, Q17).
const ECHO_OFF: &str = "echo off: the terminal's program reads a password, so its rows stay \
                        in the editor";

/// What `text` asks of a terminal's screen: rows from `start_row`, at most `rows` of them, and
/// at most `budget` bytes of their text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScreenAsk {
    pub start_row: usize,
    pub rows: usize,
    pub budget: usize,
}

/// Rows of a terminal's screen as the host keeps it, for `text` where the person let agents
/// read it and its program reads no password (ADR 0120, Q16, Q17). The host copies only the
/// rows asked, within the budget.
pub struct TerminalScreen {
    /// Rows the screen has.
    pub row_count: usize,
    /// The first row given, and each given row's text with its trailing blanks dropped.
    pub start_row: usize,
    pub rows: Vec<String>,
    /// Whether each given row goes on in the next, soft-wrapped.
    pub wraps: Vec<bool>,
    /// The next row would have taken the text past the budget.
    pub truncated: bool,
    pub history: usize,
    pub scrolled: usize,
}

/// How `text` gets the rows it asks of a terminal's screen from the host, by its buffer.
pub type ScreenOf<'a> = &'a dyn Fn(BufferId, ScreenAsk) -> Option<TerminalScreen>;

/// A `text` request, with a snapshot of a buffer that may hold no secrets.
pub struct TextJob {
    buffer: u64,
    text: Rope,
    display: DisplaySnapshot,
    what: What,
}

impl std::fmt::Debug for TextJob {
    /// Never the text.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextJob")
            .field("buffer", &self.buffer)
            .finish_non_exhaustive()
    }
}

enum What {
    Rows {
        start_row: usize,
        rows: usize,
    },
    Region(Range<usize>),
    /// A terminal's rows the host copied, within the budget; the reply is made off the UI
    /// thread.
    Terminal(TerminalScreen),
}

/// Starts `text`, on the UI thread: an error if the buffer is not there or may hold
/// secrets, or the region is too long. Everything else happens in [`TextJob::spawn`].
///
/// # Errors
///
/// What is wrong, to send back.
pub fn start(editor: &Editor, screen_of: ScreenOf<'_>, of: TextOf) -> Result<TextJob, String> {
    let (id, what) = match of {
        TextOf::Rows {
            buffer,
            start_row,
            rows,
        } => {
            let id = editor
                .buffers()
                .map(|(id, _)| id)
                .find(|id| id.get() == buffer)
                .ok_or("no buffer with that id")?;
            if editor
                .buffer(id)
                .is_some_and(oso_core::buffer::Buffer::is_terminal)
            {
                return terminal_rows(editor, screen_of, id, start_row, rows);
            }
            (id, What::Rows { start_row, rows })
        }
        TextOf::Region => {
            let (window, shown) = editor.tiled();
            // Nothing of the region of a buffer that may hold secrets, not even whether
            // there is one.
            if shown.is_secret() {
                return Err(WITHHELD.into());
            }
            if shown.withheld_from_agents() {
                return Err(PERSONS.into());
            }
            let primary = window.selections.primary();
            let mark = primary
                .mark
                .filter(|_| window.selections.mark_active().is_active())
                .ok_or("no region is active")?;
            let range = mark.min(primary.cursor)..mark.max(primary.cursor);
            if range.len() > MAX_REGION {
                return Err(format!("the region is more than {MAX_REGION} bytes"));
            }
            (window.buffer(), What::Region(range))
        }
    };
    snapshot(editor, id, what)
}

/// The rows `start_row` to `start_row + rows` of the terminal `id`'s screen as its windows
/// show it, if the person let agents read it and its program reads no password. Each row
/// has its trailing blanks dropped; `wraps` says which go on in the next, soft-wrapped.
fn terminal_rows(
    editor: &Editor,
    screen_of: ScreenOf<'_>,
    id: BufferId,
    start_row: usize,
    rows: usize,
) -> Result<TextJob, String> {
    let buffer = editor.buffer(id).ok_or("no buffer with that id")?;
    if !buffer.agent_reads() {
        return Err(TERMINAL.into());
    }
    // The buffer's verdict is the one record of a password prompt, set from the snapshot
    // the rows came with.
    if buffer.is_secret() {
        return Err(ECHO_OFF.into());
    }
    let ask = ScreenAsk {
        start_row,
        rows,
        budget: MAX_ROWS_TEXT,
    };
    let screen = screen_of(id, ask).ok_or("the terminal is starting")?;
    Ok(TextJob {
        buffer: id.get(),
        text: Rope::new(),
        display: buffer.display(),
        what: What::Terminal(screen),
    })
}

fn snapshot(editor: &Editor, id: BufferId, what: What) -> Result<TextJob, String> {
    let buffer = editor.buffer(id).ok_or("no buffer with that id")?;
    if buffer.is_secret() {
        return Err(WITHHELD.into());
    }
    if buffer.withheld_from_agents() {
        return Err(PERSONS.into());
    }
    // Nothing of a file the channel is still opening: its open may yet refuse it (ADR 0094).
    if buffer.agent_pending() {
        return Err(OPENING.into());
    }
    Ok(TextJob {
        buffer: id.get(),
        text: buffer.text().clone(),
        display: buffer.display(),
        what,
    })
}

/// The rows `start_row` to `start_row + rows` of `display` that are there: none past the
/// last.
pub fn row_range(display: &DisplaySnapshot, start_row: usize, rows: usize) -> Range<usize> {
    let end = start_row.saturating_add(rows).min(display.row_count());
    start_row.min(end)..end
}

/// The text of each row of `range`, without its line break, and whether it stopped short, as
/// the next row would take the text past `budget` bytes. The dev socket's `get-text` and the
/// channel's `text` both read rows so (ADR 0044, ADR 0085).
pub fn row_texts(
    text: &Rope,
    display: &DisplaySnapshot,
    range: Range<usize>,
    mut budget: usize,
) -> (Vec<String>, bool) {
    let mut out = Vec::new();
    for row in range {
        let bytes = motion::line_start(display, row)..motion::line_end(display, row);
        if bytes.len() > budget {
            return (out, true);
        }
        budget -= bytes.len();
        out.push(slice(text, bytes));
    }
    (out, false)
}

impl TextJob {
    /// Copies the text on a thread of its own, which replies. If the thread cannot start,
    /// the reply sender is dropped, which the client sees as an error.
    pub fn spawn(self, reply: mpsc::Sender<Reply>) {
        let spawned = thread::Builder::new()
            .name("oso-agent-text".into())
            .spawn(move || {
                let _ = reply.send(self.run());
            });
        if let Err(e) = spawned {
            log::warn!("agent channel: cannot start the text thread: {e}");
        }
    }

    fn run(&self) -> Reply {
        match &self.what {
            What::Terminal(screen) => Ok(json!({
                "buffer": self.buffer,
                "start_row": screen.start_row,
                "row_count": screen.row_count,
                "rows": screen.rows,
                "wraps": screen.wraps,
                "truncated": screen.truncated,
                "history": screen.history,
                "scrolled": screen.scrolled,
            })),
            What::Region(range) => Ok(json!({
                "buffer": self.buffer,
                "region": slice(&self.text, range.clone()),
            })),
            What::Rows { start_row, rows } => {
                let display = &self.display;
                let range = row_range(display, *start_row, *rows);
                let start = range.start;
                // Cut short where the next row would take the text past its limit.
                let (out, truncated) = row_texts(&self.text, display, range, MAX_ROWS_TEXT);
                Ok(json!({
                    "buffer": self.buffer,
                    "start_row": start,
                    "row_count": display.row_count(),
                    "rows": out,
                    "truncated": truncated,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ECHO_OFF, MAX_REGION, TERMINAL, row_range, row_texts, start};
    use super::{ScreenAsk, TerminalScreen};
    use crate::channel::protocol::TextOf;
    use oso_core::buffer::Buffer;
    use oso_core::editor::Editor;
    use oso_core::selection::{MarkActive, Selection, SelectionSet};
    use oso_core::window::ViewSize;
    use std::path::PathBuf;

    fn editor(text: &str) -> Editor {
        let mut editor = Editor::with_buffer(Buffer::from_text("notes.md", text));
        editor.set_view_size(ViewSize::new(10, 40));
        editor
    }

    #[test]
    fn rows_stop_at_the_last_and_at_the_budget() {
        let editor = editor("one\ntwo\nthree");
        let buffer = editor.current().1;
        let (text, display) = (buffer.text(), buffer.display());
        assert_eq!(row_range(&display, 1, 10), 1..3, "none past the last");
        assert_eq!(row_range(&display, 9, 2), 3..3);
        let all = row_range(&display, 0, 3);
        assert_eq!(
            row_texts(text, &display, all.clone(), usize::MAX),
            (vec!["one".into(), "two".into(), "three".into()], false)
        );
        // "one" and "two" take 6 bytes; "three" would take the text past 8.
        assert_eq!(
            row_texts(text, &display, all, 8),
            (vec!["one".into(), "two".into()], true)
        );
    }

    /// An active region from `mark` to `cursor`.
    fn select(editor: &mut Editor, mark: usize, cursor: usize) {
        let selection = Selection {
            mark: Some(mark),
            ..Selection::at(cursor)
        };
        let selections = &mut editor.current_mut().0.selections;
        *selections = SelectionSet::from_parts(vec![selection], 0).unwrap();
        selections.set_mark_active(MarkActive::Active);
    }

    fn id(editor: &Editor) -> u64 {
        editor.window().buffer().get()
    }

    #[test]
    fn nothing_of_a_file_the_channel_is_still_opening() {
        let mut editor = editor("one");
        let mut opening = Buffer::from_text("plain.txt", "a secret, by another name");
        opening.set_agent_pending(true);
        let id = editor.add_buffer(opening).get();
        let why = start(
            &editor,
            &|_, _| None,
            TextOf::Rows {
                buffer: id,
                start_row: 0,
                rows: 5,
            },
        )
        .err()
        .unwrap();
        assert!(why.contains("still opening"), "{why}");
    }

    #[test]
    fn gives_whole_rows_of_a_buffer() {
        let editor = editor("one\ntwo\nthree");
        let job = start(
            &editor,
            &|_, _| None,
            TextOf::Rows {
                buffer: id(&editor),
                start_row: 1,
                rows: 5,
            },
        )
        .unwrap();
        let text = job.run().unwrap();
        assert_eq!(text["rows"], serde_json::json!(["two", "three"]));
        assert_eq!(text["row_count"], 3);
        assert!(
            start(
                &editor,
                &|_, _| None,
                TextOf::Rows {
                    buffer: 999,
                    start_row: 0,
                    rows: 1
                }
            )
            .is_err()
        );
    }

    #[test]
    fn gives_the_region_only_while_it_is_active_and_short() {
        let mut editor = editor("abcdef");
        assert!(
            start(&editor, &|_, _| None, TextOf::Region)
                .unwrap_err()
                .contains("no region")
        );
        select(&mut editor, 1, 4);
        let text = start(&editor, &|_, _| None, TextOf::Region)
            .unwrap()
            .run()
            .unwrap();
        assert_eq!(text["region"], "bcd");
        let mut long = self::editor(&"a".repeat(MAX_REGION + 1));
        select(&mut long, 0, MAX_REGION + 1);
        assert!(
            start(&long, &|_, _| None, TextOf::Region)
                .unwrap_err()
                .contains("more than")
        );
    }

    #[test]
    fn a_prompt_buffer_is_no_buffer_to_read() {
        let mut editor = editor("");
        editor.ask("Exit?", "save-buffers-kill-terminal").unwrap();
        let prompt = editor.prompt().unwrap().buffer().get();
        let refused = start(
            &editor,
            &|_, _| None,
            TextOf::Rows {
                buffer: prompt,
                start_row: 0,
                rows: 1,
            },
        );
        assert!(refused.unwrap_err().contains("no buffer"));
    }

    #[test]
    fn refuses_a_buffer_that_may_hold_secrets() {
        let mut editor = Editor::with_buffer(Buffer::visiting(PathBuf::from("/tmp/.env")));
        editor.set_view_size(ViewSize::new(10, 40));
        let buffer = id(&editor);
        let refused = start(
            &editor,
            &|_, _| None,
            TextOf::Rows {
                buffer,
                start_row: 0,
                rows: 1,
            },
        );
        assert!(refused.unwrap_err().contains("may hold secrets"));
        // The region gets the same answer, whether there is one or not.
        let region = start(&editor, &|_, _| None, TextOf::Region).unwrap_err();
        assert!(region.contains("may hold secrets"), "{region}");
    }

    /// A terminal showing `hello` above `x`, the first going on in the second.
    fn terminal() -> (Editor, oso_core::buffer::BufferId) {
        let mut editor = Editor::default();
        let id = editor.add_terminal_buffer("*terminal*");
        (editor, id)
    }

    /// The screen a host gives for `ask`: `hello` above `x`, the first going on in the
    /// second.
    fn screen(ask: ScreenAsk) -> Option<TerminalScreen> {
        let all = [("hello", true), ("x", false)];
        let end = ask.start_row.saturating_add(ask.rows).min(all.len());
        let given = &all[ask.start_row.min(end)..end];
        Some(TerminalScreen {
            row_count: all.len(),
            start_row: ask.start_row,
            rows: given.iter().map(|(text, _)| (*text).to_owned()).collect(),
            wraps: given.iter().map(|(_, wraps)| *wraps).collect(),
            truncated: false,
            history: 3,
            scrolled: 0,
        })
    }

    fn rows_of(id: oso_core::buffer::BufferId) -> TextOf {
        TextOf::Rows {
            buffer: id.get(),
            start_row: 0,
            rows: 10,
        }
    }

    #[test]
    fn a_terminal_gives_its_rows_only_once_the_person_allows_it() {
        let (mut editor, id) = terminal();
        let shown = |_, ask| screen(ask);
        let refused = start(&editor, &shown, rows_of(id)).unwrap_err();
        assert_eq!(refused, TERMINAL);
        editor.set_agent_reads(id, true);
        let text = start(&editor, &shown, rows_of(id)).unwrap().run().unwrap();
        assert_eq!(text["rows"], serde_json::json!(["hello", "x"]));
        assert_eq!(text["wraps"], serde_json::json!([true, false]));
        assert_eq!(text["row_count"], 2);
        assert_eq!(text["history"], 3);
        editor.set_agent_reads(id, false);
        assert!(start(&editor, &shown, rows_of(id)).is_err());
    }

    #[test]
    fn a_terminal_reading_a_password_gives_no_rows_even_when_allowed() {
        let (mut editor, id) = terminal();
        editor.set_agent_reads(id, true);
        editor.set_echo_off(id, true);
        let shown = |_, ask| screen(ask);
        assert_eq!(start(&editor, &shown, rows_of(id)).unwrap_err(), ECHO_OFF);
    }

    #[test]
    fn a_terminals_region_is_never_read() {
        let (mut editor, id) = terminal();
        editor.set_agent_reads(id, true);
        editor.show(id);
        let shown = |_, ask| screen(ask);
        assert!(start(&editor, &shown, TextOf::Region).is_err());
    }
}
