//! The OS's requests to end (ADR 0070, ADR 0084): why one came, from the quit Apple event, the
//! app's answer, and an end AppKit holds for it (`NSTerminateLater`) until the app answers.
//!
//! Must not depend on: the view or the run loop's hooks. The handler is asked through the
//! state, as for any other callback.
//!
//! - A request that comes while the handler runs is not held inside it, where AppKit would wait
//!   for an answer that cannot come until it returns: a person's becomes the window's close
//!   request, which waits for the handler as that does, and the end of the session is refused.
//! - An end held is let go if the handler panics, as no answer would come; the panic hook has
//!   written what is at risk by then (ADR 0062).

use objc2::msg_send;
use objc2::rc::Retained;
use objc2_app_kit::{NSApplication, NSApplicationTerminateReply};
use objc2_foundation::{NSAppleEventDescriptor, NSAppleEventManager};

use super::{Delivery, Pending, State};
use crate::window::{QuitAnswer, QuitReason};

/// The quit Apple event's reason (`keyAEQuitReason`), which the system sends as an attribute.
const QUIT_REASON_KEY: u32 = u32::from_be_bytes(*b"why?");

/// Why the OS asks to end, from the quit Apple event's reason, if it gave one: a logout, a
/// restart or a shutdown, with or without the system's dialog, ends the session, and anything
/// else is a person's.
fn quit_reason(code: Option<u32>) -> QuitReason {
    const SESSION_ENDS: [&[u8; 4]; 6] = [b"logo", b"rlgo", b"rrst", b"rest", b"rsdn", b"shut"];
    match code {
        Some(code)
            if SESSION_ENDS
                .iter()
                .any(|end| u32::from_be_bytes(**end) == code) =>
        {
            QuitReason::SessionEnd
        }
        _ => QuitReason::Person,
    }
}

/// Why the OS asks to end now, from the Apple event being handled, if any. The system puts the
/// reason in an attribute; a parameter of the same name is read too, as a sender may use one.
pub(super) fn quit_reason_now() -> QuitReason {
    let Some(event) = NSAppleEventManager::sharedAppleEventManager().currentAppleEvent() else {
        return QuitReason::Person;
    };
    // SAFETY: the receiver is an `NSAppleEventDescriptor`, retained for the call, which declares
    // `- (nullable NSAppleEventDescriptor *)attributeDescriptorForKeyword:(AEKeyword)keyword`:
    // one argument, an `AEKeyword`, a four-character code that is a `u32`, as `QUIT_REASON_KEY`
    // is, and a descriptor or nil returned at +0, which the `Option<Retained<_>>` retains or
    // reads as `None`.
    let attribute: Option<Retained<NSAppleEventDescriptor>> =
        unsafe { msg_send![&event, attributeDescriptorForKeyword: QUIT_REASON_KEY] };
    // SAFETY: as above, for `- (nullable NSAppleEventDescriptor *)paramDescriptorForKeyword:
    // (AEKeyword)keyword`, which has the same signature.
    let reason = attribute
        .or_else(|| unsafe { msg_send![&event, paramDescriptorForKeyword: QUIT_REASON_KEY] });
    // A type code as the system sends, or an enumeration as a script may; a coercion that
    // fails gives 0.
    let code =
        reason.map(
            |reason: Retained<NSAppleEventDescriptor>| match reason.typeCodeValue() {
                0 => reason.enumCodeValue(),
                code => code,
            },
        );
    quit_reason(code)
}

impl State {
    /// The OS asks to end (`applicationShouldTerminate:`), and the handler answers. After a
    /// panic or the loop's end nothing of the app is asked, and the end goes ahead.
    pub(super) fn quit_requested(&'static self, reason: QuitReason) -> NSApplicationTerminateReply {
        if self.panic.borrow().is_some() || self.exited.get() {
            return NSApplicationTerminateReply::TerminateNow;
        }
        if self.dispatch.try_borrow_mut().is_err() {
            return self.quit_while_handling(reason);
        }
        self.quit_answer.set(None);
        self.deliver(Delivery::Quit(reason));
        match self.quit_answer.take() {
            Some(QuitAnswer::Now) => NSApplicationTerminateReply::TerminateNow,
            Some(QuitAnswer::Later) => {
                self.quit_held.set(true);
                NSApplicationTerminateReply::TerminateLater
            }
            // A panic while it was asked stops the loop, which then resumes it.
            Some(QuitAnswer::Cancel) | None => NSApplicationTerminateReply::TerminateCancel,
        }
    }

    /// A request to end while the handler runs, which no run loop in it does today.
    fn quit_while_handling(&self, reason: QuitReason) -> NSApplicationTerminateReply {
        match reason {
            QuitReason::Person => self.pending.borrow_mut().push(Pending::CloseRequested),
            QuitReason::SessionEnd => {
                log::error!("the session's end came while oso handled an event, and was refused");
            }
        }
        NSApplicationTerminateReply::TerminateCancel
    }

    /// The app's answer to the end AppKit holds for it. With none held, nothing happens. A yes
    /// ends the process: `applicationWillTerminate:` runs `exiting` first.
    pub(super) fn answer_held_quit(&self, yes: bool) {
        if self.quit_held.replace(false) {
            NSApplication::sharedApplication(self.mtm).replyToApplicationShouldTerminate(yes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{QUIT_REASON_KEY, quit_reason};
    use crate::window::QuitReason;

    #[test]
    fn only_a_session_end_is_told_from_a_persons_quit() {
        let code = |four: &[u8; 4]| Some(u32::from_be_bytes(*four));
        for end in [b"logo", b"rlgo", b"rrst", b"rest", b"rsdn", b"shut"] {
            assert_eq!(quit_reason(code(end)), QuitReason::SessionEnd);
        }
        let person = [None, code(b"quit"), Some(0)];
        assert!(
            person
                .into_iter()
                .all(|code| quit_reason(code) == QuitReason::Person)
        );
        assert_eq!(QUIT_REASON_KEY, 0x7768_793F, "keyAEQuitReason");
    }
}
