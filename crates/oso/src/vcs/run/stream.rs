//! Streaming one long run of git to its end (ADR 0127, steps 1b and 2): what it writes goes out
//! as log lines, its time each second and, for a push, a pull or a fetch, its progress; a stop,
//! the root's revocation or the event loop's going ends it.
//!
//! Must not depend on: `app`, `dispatch`, winit or the UI thread; this runs on the repository's
//! run thread and waits for git.

use std::ffi::OsStr;
use std::process::ExitStatus;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::{FLUSH, MAX_FLUSH, RunReport, Watch};
use crate::git::{Git, Next};
use crate::vcs::{commit, remote};

/// How a streamed run ended.
pub(super) struct Streamed {
    /// git's status, or why it could not be read.
    pub(super) status: Result<ExitStatus, String>,
    /// Whether it was stopped: by the person, the root's revocation or the event loop's going.
    pub(super) stopped: bool,
    /// The end of what it wrote, as the log shows it: at most [`remote::MAX_LOOKED`] bytes,
    /// from a line's start, which a failure's message is read from.
    pub(super) tail: String,
}

/// Runs git with `words` after oso's own, `input` on standard input if any, sending what it
/// writes, its time each second and, if `progress`, the progress it shows; `false` from `send`
/// stops it. `Err` if it did not start.
pub(super) fn stream(
    git: &Git,
    words: &[String],
    input: Option<Vec<u8>>,
    watch: &Watch<'_>,
    send: &mut impl FnMut(RunReport) -> bool,
    progress: bool,
) -> Result<Streamed, String> {
    let args: Vec<&OsStr> = words.iter().map(OsStr::new).collect();
    let mut long = git
        .start_long(&args, &[], input)
        .map_err(|e| e.to_string())?;
    *watch.ender.lock().unwrap_or_else(PoisonError::into_inner) = Some(long.ender());
    let line = format!("$ git {}\n", words.join(" "));
    if !send(RunReport::Began { line }) {
        long.stop();
    }
    let began = Instant::now();
    let mut flushed = Instant::now();
    let mut seconds = 0;
    let (mut partial, mut pending) = (Vec::new(), Waiting::default());
    let (mut raw, mut shown) = (Vec::new(), None::<String>);
    let mut tail = String::new();
    let status = loop {
        // A project no longer trusted runs nothing more, whatever the UI thread does.
        if watch.stop.load(Ordering::SeqCst) || watch.root.revoked() {
            long.stop();
        }
        match long.next(FLUSH) {
            Ok(Next::Out(piece)) => {
                let lines = commit::log_lines(&piece, &mut partial);
                keep(&mut tail, &lines);
                pending.push(&lines);
                if progress {
                    raw.extend_from_slice(&piece);
                    if raw.len() > 2 * remote::MAX_LOOKED {
                        let cut = raw.len() - remote::MAX_LOOKED;
                        raw.drain(..cut);
                    }
                }
            }
            Ok(Next::Quiet) => {}
            Ok(Next::Ended(status)) => break Ok(status),
            Err(e) => break Err(e.to_string()),
        }
        if flushed.elapsed() >= FLUSH {
            flushed = Instant::now();
            if let Some(text) = pending.take()
                && !send(RunReport::Output(text))
            {
                long.stop();
            }
            let now = remote::progress(&raw);
            if now.is_some() && now != shown {
                shown.clone_from(&now);
                if let Some(now) = now
                    && !send(RunReport::Progress(now))
                {
                    long.stop();
                }
            }
        }
        let now = began.elapsed().as_secs();
        if now > seconds {
            seconds = now;
            if !send(RunReport::Elapsed(now)) {
                long.stop();
            }
        }
    };
    let rest = commit::log_rest(&mut partial);
    keep(&mut tail, &rest);
    pending.push(&rest);
    if let Some(text) = pending.take() {
        send(RunReport::Output(text));
    }
    Ok(Streamed {
        status,
        stopped: long.stopped(),
        tail,
    })
}

/// Adds `lines` to `tail`, keeping its last [`remote::MAX_LOOKED`] bytes from a line's start.
fn keep(tail: &mut String, lines: &str) {
    tail.push_str(lines);
    keep_tail(tail, remote::MAX_LOOKED);
}

/// What a run wrote that waits to be sent on, and how many bytes were left out before it.
#[derive(Default)]
pub(super) struct Waiting {
    text: String,
    left_out: usize,
}

impl Waiting {
    /// Adds whole `lines`; past twice [`MAX_FLUSH`], only the last of them stay.
    pub(super) fn push(&mut self, lines: &str) {
        self.text.push_str(lines);
        if self.text.len() > 2 * MAX_FLUSH {
            self.left_out += keep_tail(&mut self.text, MAX_FLUSH);
        }
    }

    /// What to send: at most [`MAX_FLUSH`] bytes, after a line that says how many were left
    /// out, if any; `None` with nothing to say.
    pub(super) fn take(&mut self) -> Option<String> {
        self.left_out += keep_tail(&mut self.text, MAX_FLUSH);
        if self.text.is_empty() && self.left_out == 0 {
            return None;
        }
        let mut out = match std::mem::take(&mut self.left_out) {
            0 => String::new(),
            n => format!("[… {n} bytes left out]\n"),
        };
        out.push_str(&std::mem::take(&mut self.text));
        Some(out)
    }
}

/// Keeps the end of `text`, at most `keep` bytes from a line's start, and says how many went.
pub(super) fn keep_tail(text: &mut String, keep: usize) -> usize {
    if text.len() <= keep {
        return 0;
    }
    let from = text.ceil_char_boundary(text.len() - keep);
    let from = text[from..].find('\n').map_or(from, |at| from + at + 1);
    text.drain(..from);
    from
}
