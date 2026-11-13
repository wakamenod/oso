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
use crate::engine::Effect;
use crate::event::{Progress, Prompt};
use crate::limits;

/// What `XTVERSION` reports.
const XTVERSION: &str = concat!("oso ", env!("CARGO_PKG_VERSION"));

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

/// A `GhosttyString`'s bytes; empty for NULL. Only for strings the library handed out.
pub(super) fn bytes(s: &ffi::GString) -> &[u8] {
    if s.ptr.is_null() || s.len == 0 {
        return &[];
    }
    // SAFETY: every `GhosttyString` the library hands out points to `len` readable bytes for
    // as long as the struct holding it (a callback's argument, or static build info), which
    // the returned borrow cannot outlive.
    unsafe { std::slice::from_raw_parts(s.ptr, s.len) }
}

/// A terminal string (the title or the directory) read inside a callback.
fn terminal_string(terminal: ffi::Terminal, data: c_int) -> Vec<u8> {
    let mut s = ffi::GString::EMPTY;
    // SAFETY: `terminal` is the live terminal the callback got, and `data` is `DATA_TITLE` or
    // `DATA_PWD`, whose output type is `GhosttyString`.
    let read = unsafe { ffi::ghostty_terminal_get(terminal, data, (&raw mut s).cast()) };
    if read == ffi::SUCCESS {
        let mut text = bytes(&s).to_vec();
        text.truncate(limits::UNKNOWN_SEQUENCE_MAX);
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
        let reply = ffi::GString { ptr: data, len };
        let reply = bytes(&reply);
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.reply(reply);
    });
}

pub(super) extern "C" fn on_bell(_terminal: ffi::Terminal, ud: *mut c_void) {
    // SAFETY: called by the library with `configure`'s userdata.
    guarded((), || unsafe { sink(ud) }.push(Effect::Bell));
}

pub(super) extern "C" fn on_xtversion(_terminal: ffi::Terminal, _ud: *mut c_void) -> ffi::GString {
    ffi::GString {
        ptr: XTVERSION.as_ptr(),
        len: XTVERSION.len(),
    }
}

pub(super) extern "C" fn on_title(terminal: ffi::Terminal, ud: *mut c_void) {
    guarded((), || {
        let title = terminal_string(terminal, ffi::DATA_TITLE);
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(Effect::Title(title));
    });
}

pub(super) extern "C" fn on_pwd(terminal: ffi::Terminal, ud: *mut c_void) {
    guarded((), || {
        let pwd = terminal_string(terminal, ffi::DATA_PWD);
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(Effect::Directory(pwd));
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
            .find(|c| bytes(&c.mime).starts_with(b"text/plain"))
            .map(|c| bytes(&c.data));
        if let Some(text) = text.filter(|t| t.len() <= limits::CLIPBOARD_WRITE_MAX) {
            // SAFETY: called by the library with `configure`'s userdata.
            unsafe { sink(ud) }.push(Effect::ClipboardWrite(text.to_vec()));
        }
        if let Some(reply) = request.reply {
            let answer = ffi::ClipboardWriteReply {
                size: size_of::<ffi::ClipboardWriteReply>(),
                result: ffi::CLIPBOARD_WRITE_RESULT_SUCCESS,
                remember: false,
            };
            // SAFETY: the reply function the library gave, with its own request and a live
            // sized reply, inside the callback as terminal.h requires.
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
            title: bytes(&notification.title).to_vec(),
            body: bytes(&notification.body).to_vec(),
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
            ffi::SEMANTIC_PROMPT_PROMPT_START => Effect::Prompt(Prompt::Start, Vec::new()),
            ffi::SEMANTIC_PROMPT_INPUT_START => Effect::Prompt(Prompt::Input, Vec::new()),
            ffi::SEMANTIC_PROMPT_OUTPUT_START => Effect::Prompt(
                Prompt::Output { command: None },
                bytes(&mark.command).to_vec(),
            ),
            ffi::SEMANTIC_PROMPT_COMMAND_END => Effect::Prompt(
                Prompt::End {
                    exit: mark.has_exit_code.then_some(mark.exit_code),
                },
                Vec::new(),
            ),
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
        let content = bytes(&content).to_vec();
        // SAFETY: called by the library with `configure`'s userdata.
        unsafe { sink(ud) }.push(Effect::Unknown(content));
    });
}
