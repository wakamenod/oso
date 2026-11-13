//! libghostty-vt's callbacks into oso-term (ADR 0120). They run inside
//! `ghostty_terminal_vt_write` and `ghostty_terminal_resize`, only push onto the `Sink`, never
//! call back into a function that writes the terminal, and catch any panic, which must not
//! unwind into the library.
//!
//! Must not depend on: anything but `ffi`, the `Sink` and the effect types.

#![expect(unsafe_code, reason = "the library's C API (ADR 0120)")]

use std::ffi::{c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use super::super::ffi;
use super::Sink;
use crate::engine::{Effect, PromptMark};
use crate::event::Progress;
use crate::limits;

/// Runs a callback's body, so that a panic never unwinds into the library.
fn guarded<T>(fallback: T, body: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(body)).unwrap_or(fallback)
}

/// The sink a callback got as userdata.
///
/// # Safety
/// `ud` must be the userdata `Vt::configure` installed, inside a callback, while no other
/// reference to the sink is alive (`Vt::sink`'s borrow cannot be: the callback runs inside
/// `write` or `resize`).
unsafe fn sink<'a>(ud: *mut c_void) -> &'a mut Sink {
    // SAFETY: the function's contract.
    unsafe { &mut *ud.cast::<Sink>() }
}

/// A `GhosttyString`'s bytes; empty for NULL.
///
/// # Safety
/// `s` must be empty, static text (`from_static`), or a string the library wrote for the
/// call that is running (a callback's argument, or what a `get` inside it returned), and the
/// returned borrow must not outlive that call. A `GString` is `Copy`, so its borrow says
/// nothing about how long the bytes it points to live.
pub(super) unsafe fn bytes(s: &ffi::GString) -> &[u8] {
    let (ptr, len) = s.parts();
    if ptr.is_null() || len == 0 {
        return &[];
    }
    // SAFETY: the function's contract: `ptr` points to `len` readable bytes for the borrow.
    unsafe { std::slice::from_raw_parts(ptr, len) }
}

/// A terminal string (the title or the directory) read inside a callback.
///
/// # Safety
/// `terminal` must be the live terminal the callback got, and `data` `DATA_TITLE` or
/// `DATA_PWD`.
unsafe fn terminal_string(terminal: ffi::Terminal, data: c_int) -> Vec<u8> {
    let mut s = ffi::GString::EMPTY;
    // SAFETY: the function's contract; both data have the output type `GhosttyString`.
    let read = unsafe { ffi::ghostty_terminal_get(terminal, data, (&raw mut s).cast()) };
    if read == ffi::SUCCESS {
        // SAFETY: the library wrote `s` for this call, and it is copied out at once.
        let mut text = unsafe { bytes(&s) }.to_vec();
        text.truncate(limits::TERMINAL_STRING_MAX);
        text
    } else {
        Vec::new()
    }
}

pub(super) extern "C" fn on_write_pty(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    data: *const u8,
    len: usize,
) {
    guarded((), || {
        if data.is_null() {
            return;
        }
        // SAFETY: the library passes `len` readable bytes for the call.
        let reply = unsafe { std::slice::from_raw_parts(data, len) };
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.reply(reply);
    });
}

pub(super) extern "C" fn on_bell(_terminal: ffi::Terminal, ud: *mut c_void) {
    // SAFETY: called by the library with `configure`'s userdata.
    guarded((), || unsafe { sink(ud) }.push(Effect::Bell));
}

pub(super) extern "C" fn on_xtversion(_terminal: ffi::Terminal, ud: *mut c_void) -> ffi::GString {
    guarded(ffi::GString::EMPTY, || {
        // SAFETY: called by the library with `configure`'s userdata.
        ffi::GString::from_static(unsafe { sink(ud) }.xtversion)
    })
}

pub(super) extern "C" fn on_title(terminal: ffi::Terminal, ud: *mut c_void) {
    guarded((), || {
        // SAFETY: called by the library with `configure`'s userdata.
        let sink = unsafe { sink(ud) };
        if sink.has_room() {
            // SAFETY: the terminal the callback got, and a string datum.
            let title = unsafe { terminal_string(terminal, ffi::DATA_TITLE) };
            sink.push(Effect::Title(title));
        }
    });
}

pub(super) extern "C" fn on_pwd(terminal: ffi::Terminal, ud: *mut c_void) {
    guarded((), || {
        // SAFETY: called by the library with `configure`'s userdata.
        let sink = unsafe { sink(ud) };
        if sink.has_room() {
            // SAFETY: the terminal the callback got, and a string datum.
            let pwd = unsafe { terminal_string(terminal, ffi::DATA_PWD) };
            sink.push(Effect::Directory(pwd));
        }
    });
}

pub(super) extern "C" fn on_size(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    out: *mut ffi::SizeReportSize,
) -> bool {
    guarded(false, || {
        // SAFETY: called by the library with `configure`'s userdata.
        let size = unsafe { sink(ud) }.size;
        if out.is_null() {
            return false;
        }
        // SAFETY: the library passes a writable `GhosttySizeReportSize`.
        unsafe { out.write(size) };
        true
    })
}

pub(super) extern "C" fn on_clipboard_write(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    write: *const ffi::ClipboardWrite,
) {
    guarded((), || {
        // SAFETY: the library passes a valid request for the length of the call, or NULL.
        let Some(request) = (unsafe { write.as_ref() }) else {
            return;
        };
        let contents = if request.contents.is_null() {
            &[][..]
        } else {
            // SAFETY: `contents` points to `contents_len` entries for the call.
            unsafe { std::slice::from_raw_parts(request.contents, request.contents_len) }
        };
        let text = contents
            .iter()
            // SAFETY: the request's strings, for the length of this callback.
            .find(|c| unsafe { bytes(&c.mime) }.starts_with(b"text/plain"))
            // SAFETY: as above; `text` is copied out before the callback returns.
            .map(|c| unsafe { bytes(&c.data) });
        // A write not taken (no text, too large, or a request to clear) gets no reply, which
        // denies it.
        let Some(text) = text.filter(|t| t.len() <= limits::CLIPBOARD_WRITE_MAX) else {
            return;
        };
        // SAFETY: called by the library with `configure`'s userdata.
        let sink = unsafe { sink(ud) };
        if !sink.has_room() {
            return;
        }
        sink.push(Effect::ClipboardWrite(text.to_vec()));
        if let Some(reply) = request.reply {
            let answer = ffi::ClipboardWriteReply {
                size: size_of::<ffi::ClipboardWriteReply>(),
                result: ffi::CLIPBOARD_WRITE_RESULT_SUCCESS,
                remember: false,
            };
            // SAFETY: the reply function the library gave, with its own request and a live
            // sized reply, inside the callback as terminal.h requires. It only records the
            // result: OSC 52 ignores it, and OSC 5522 answers the program with it after this
            // callback returns (through `on_write_pty` then), so no callback runs inside it
            // and the sink, last used above, is not reached twice.
            unsafe { reply(write, &answer) };
        }
    });
}

pub(super) extern "C" fn on_notification(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    notification: *const ffi::DesktopNotification,
) {
    guarded((), || {
        // SAFETY: the library passes a valid notification for the call, or NULL.
        let Some(notification) = (unsafe { notification.as_ref() }) else {
            return;
        };
        let effect = Effect::Notification {
            // SAFETY: the notification's strings, for this callback, copied out at once.
            title: unsafe { bytes(&notification.title) }.to_vec(),
            // SAFETY: as above.
            body: unsafe { bytes(&notification.body) }.to_vec(),
        };
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(effect);
    });
}

pub(super) extern "C" fn on_progress(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    report: *const ffi::ProgressReport,
) {
    guarded((), || {
        // SAFETY: the library passes a valid report for the call, or NULL.
        let Some(report) = (unsafe { report.as_ref() }) else {
            return;
        };
        let percent = u8::try_from(report.progress).ok().map(|p| p.min(100));
        let progress = match report.state {
            ffi::PROGRESS_REMOVE => Progress::Remove,
            ffi::PROGRESS_SET => Progress::Set(percent.unwrap_or(0)),
            ffi::PROGRESS_ERROR => Progress::Error(percent),
            ffi::PROGRESS_INDETERMINATE => Progress::Indeterminate,
            ffi::PROGRESS_PAUSE => Progress::Paused(percent),
            _ => return,
        };
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(Effect::Progress(progress));
    });
}

pub(super) extern "C" fn on_prompt(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    mark: *const ffi::SemanticPrompt,
) {
    guarded((), || {
        // SAFETY: the library passes a valid mark for the call, or NULL.
        let Some(mark) = (unsafe { mark.as_ref() }) else {
            return;
        };
        let effect = match mark.kind {
            ffi::SEMANTIC_PROMPT_PROMPT_START => Effect::Prompt(PromptMark::Start),
            ffi::SEMANTIC_PROMPT_INPUT_START => Effect::Prompt(PromptMark::Input),
            ffi::SEMANTIC_PROMPT_OUTPUT_START => {
                // SAFETY: the mark's command, for this callback, copied out at once.
                let command = unsafe { bytes(&mark.command) }.to_vec();
                Effect::Prompt(PromptMark::Output(command))
            }
            ffi::SEMANTIC_PROMPT_COMMAND_END => Effect::Prompt(PromptMark::End(
                mark.has_exit_code.then_some(mark.exit_code),
            )),
            _ => return,
        };
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(effect);
    });
}

pub(super) extern "C" fn on_unknown(
    _terminal: ffi::Terminal,
    ud: *mut c_void,
    sequence: *const ffi::UnknownSequence,
) {
    guarded((), || {
        // SAFETY: called by the library with `configure`'s userdata.
        let sink = unsafe { sink(ud) };
        if !sink.has_room() {
            return;
        }
        // SAFETY: the library passes a valid sequence for the call, or NULL.
        let Some(sequence) = (unsafe { sequence.as_ref() }) else {
            return;
        };
        let content = match sequence.tag {
            // SAFETY: the tag says which union member is set.
            ffi::UNKNOWN_SEQUENCE_OSC => unsafe { sequence.value.osc }.content,
            // SAFETY: as above.
            ffi::UNKNOWN_SEQUENCE_APC => unsafe { sequence.value.apc }.content,
            _ => return,
        };
        // SAFETY: the sequence's content, for this callback, copied out at once.
        let content = unsafe { bytes(&content) }.to_vec();
        sink.push(Effect::Unknown(content));
    });
}
