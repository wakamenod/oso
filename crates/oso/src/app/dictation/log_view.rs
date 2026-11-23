//! `dictation-learning-log` (ADR 0175, step 4): the learning log, read on a thread of its own
//! and shown read-only in `*Dictation log*`, which agents cannot read.
//!
//! Must not depend on: how a frame is drawn.

use std::path::Path;
use std::thread;
use std::time::{Duration, UNIX_EPOCH};

use super::{App, learning};
use crate::app::UserEvent;
use crate::app::sessions::show_withheld;
use crate::dictation::learning::{self as record, view};
use crate::sessions::record::read_file;
use crate::vcs::graph::date;

/// The log's buffer.
const BUFFER: &str = "*Dictation log*";

impl App {
    /// Reads the log, on a thread of its own.
    pub(super) fn read_dictation_log(&mut self) {
        let Some(state) = self.hosting.state_directory() else {
            return self
                .editor
                .message("Dictation has learned nothing: oso has no state directory");
        };
        let emptied = self.dictations.log_emptied;
        let waker = self.waker.clone();
        let started = thread::Builder::new()
            .name("oso-dictation-log".into())
            .spawn(move || {
                let text = read(&learning::directory(&state));
                waker.send(UserEvent::DictationLog(text, emptied));
            });
        if let Err(e) = started {
            log::error!("cannot start a thread to read dictation's log: {e}");
            self.editor.message("Dictation's log could not be read");
        }
    }

    /// The log was read, asked for when its buffer had been emptied `emptied` times: it is
    /// shown, unless a forget came since.
    pub(in crate::app) fn show_dictation_log(&mut self, text: Option<String>, emptied: u64) {
        if emptied != self.dictations.log_emptied {
            return;
        }
        let Some(text) = text else {
            return self
                .editor
                .message("Dictation's log could not be read: oso's log says why");
        };
        let kept = &mut self.dictations.log_buffer;
        if !show_withheld(&mut self.editor, kept, BUFFER, &text) {
            self.editor.message("Dictation's log could not be shown");
        }
        self.editor.follow_cursor();
        self.request_redraw();
    }

    /// Empties the log's buffer, as what it shows is forgotten.
    pub(super) fn empty_dictation_log(&mut self) {
        self.dictations.log_emptied += 1;
        if let Some(buffer) = self.dictations.log_buffer {
            self.editor.set_internal_text(buffer, "");
        }
    }
}

/// The text of the log in `directory`: `None` if it may not be read.
fn read(directory: &Path) -> Option<String> {
    match read_file(directory, record::LOG, record::MAX_BYTES) {
        Ok(bytes) => {
            let log = bytes.unwrap_or_default();
            Some(view::shown(&String::from_utf8_lossy(&log), when))
        }
        Err(e) => {
            log::warn!("dictation's learning log is not read: {e}");
            None
        }
    }
}

/// `at`, seconds since the epoch, as `YYYY-MM-DD HH:MM` in the local time zone.
fn when(at: u64) -> String {
    let offset = UNIX_EPOCH
        .checked_add(Duration::from_secs(at))
        .and_then(oso_platform::file_meta::utc_offset_at)
        .unwrap_or(0);
    let at = i64::try_from(at).unwrap_or(i64::MAX);
    let minutes = at.saturating_add(i64::from(offset)).rem_euclid(86_400) / 60;
    format!(
        "{} {:02}:{:02}",
        date(at, offset),
        minutes / 60,
        minutes % 60
    )
}
