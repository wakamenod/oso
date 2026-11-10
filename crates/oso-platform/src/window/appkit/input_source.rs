//! The keyboard input source, through HIToolbox's Text Input Sources (Carbon), for ADR 0106:
//! the kind the person has chosen, told to the handler through `gate::SourceGate` when the
//! system says the choice changed, and an ASCII-capable source chosen while the app wants one,
//! as while a prompt for a file's name is open, with the person's given back after
//! (`gate::AsciiInput`). The app delegate observes the change; `keys::input_source` says what
//! kind a source is.
//!
//! Must not depend on: the view, the delegates or the run loop's hooks.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_app_kit::NSApplication;
use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CFType};
use objc2_foundation::MainThreadMarker;

use super::gate::AsciiStep;
use super::{Delivery, State, keys};
use crate::window::{Event, InputSource};

/// What the system posts, to every process, when the person chooses another input source.
pub(super) const CHANGED: &str = "com.apple.Carbon.TISNotifySelectedKeyboardInputSourceChanged";

// HIToolbox is part of the Carbon framework, which AppKit loads already. No crate binds these.
// Its property keys are foreign constants, which an `extern` block may declare (ADR 0107).
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut CFType;
    fn TISCopyCurrentASCIICapableKeyboardInputSource() -> *mut CFType;
    fn TISSelectInputSource(source: NonNull<CFType>) -> i32;
    fn TISGetInputSourceProperty(source: NonNull<CFType>, key: &CFString) -> *const c_void;
    static kTISPropertyInputSourceID: &'static CFString;
    static kTISPropertyInputSourceIsASCIICapable: &'static CFString;
}

/// The ASCII-capable source oso chose, by its ID, and the person's before it, to give back.
pub(super) struct Switched {
    previous: CFRetained<CFType>,
    chosen: String,
}

impl State {
    /// The person chose another input source: the handler hears of its kind if that changed
    /// and oso is in front. Nothing else runs here.
    pub(super) fn input_source_changed(&'static self) {
        // The state is the main thread's; a notice delivered on another thread is dropped.
        if MainThreadMarker::new().is_none() {
            return;
        }
        let active = NSApplication::sharedApplication(self.mtm).isActive();
        let Some(now) = active.then(|| current_kind(self.mtm)).flatten() else {
            return;
        };
        let mut gate = self.source.get();
        let report = gate.report(active, now);
        self.source.set(gate);
        if let Some(source) = report {
            self.deliver(Delivery::Window(Event::InputSource(source)));
        }
    }

    /// The app wants ASCII or not (`wanted`), or oso came to the front or went behind
    /// (`None`): chooses an ASCII-capable source, or gives the person's back.
    pub(super) fn update_ascii(&self, wanted: Option<bool>) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let active = NSApplication::sharedApplication(self.mtm).isActive();
        let mut gate = self.ascii.get();
        let step = gate.step(wanted, active);
        self.ascii.set(gate);
        match step {
            AsciiStep::Switch => *self.switched.borrow_mut() = switch_to_ascii(self.mtm),
            AsciiStep::Restore => {
                if let Some(switched) = self.switched.take() {
                    give_back(switched, self.mtm);
                }
            }
            AsciiStep::Nothing => {}
        }
    }
}

/// The kind of the input source chosen now, or `None` if the system gives none.
fn current_kind(mtm: MainThreadMarker) -> Option<InputSource> {
    let source = copy_current(mtm)?;
    Some(keys::input_source(&id_of(&source)?, types_ascii(&source)))
}

/// Chooses the current input method's ASCII-capable source, as `英数` does, if the person's
/// cannot type ASCII. Returns what to give back, or `None` if nothing was chosen.
fn switch_to_ascii(mtm: MainThreadMarker) -> Option<Switched> {
    let previous = copy_current(mtm)?;
    if types_ascii(&previous) {
        return None;
    }
    // SAFETY: a Carbon call with no arguments, on the main thread, which the marker proves; it
    // returns a source the caller owns, or null.
    let ascii = unsafe { owned(TISCopyCurrentASCIICapableKeyboardInputSource()) }?;
    let chosen = id_of(&ascii)?;
    select(&ascii).then_some(Switched { previous, chosen })
}

/// Gives the person's source back, unless they have chosen another since oso chose ASCII.
fn give_back(switched: Switched, mtm: MainThreadMarker) {
    let now = copy_current(mtm).and_then(|source| id_of(&source));
    if now.as_deref() == Some(switched.chosen.as_str()) {
        select(&switched.previous);
    }
}

/// The input source chosen now, owned.
fn copy_current(_mtm: MainThreadMarker) -> Option<CFRetained<CFType>> {
    // SAFETY: a Carbon call with no arguments, made on the main thread as the Text Input
    // Sources ask, which the marker proves. It returns a source the caller owns, or null.
    unsafe { owned(TISCopyCurrentKeyboardInputSource()) }
}

/// Makes `source` the selected one. `false` if the system refused.
fn select(source: &CFType) -> bool {
    // SAFETY: `source` is a live input source, and selecting one only changes which the
    // system uses; the call is made on the main thread, as every caller holds the marker.
    unsafe { TISSelectInputSource(NonNull::from(source)) == 0 }
}

/// The source's ID, copied out.
fn id_of(source: &CFType) -> Option<String> {
    // SAFETY: `source` is a live input source and the key is HIToolbox's own constant. The ID
    // is a CFString, as the Text Input Sources document it, borrowed under the Get rule while
    // `source` lives, and copied out here.
    unsafe {
        TISGetInputSourceProperty(NonNull::from(source), kTISPropertyInputSourceID)
            .cast::<CFString>()
            .as_ref()
            .map(ToString::to_string)
    }
}

/// Whether the source types ASCII.
fn types_ascii(source: &CFType) -> bool {
    // SAFETY: as for `id_of`; the flag is a CFBoolean, read while `source` lives.
    unsafe {
        TISGetInputSourceProperty(NonNull::from(source), kTISPropertyInputSourceIsASCIICapable)
            .cast::<CFBoolean>()
            .as_ref()
            .is_some_and(CFBoolean::as_bool)
    }
}

/// `source` as owned, or `None` for null.
///
/// # Safety
///
/// `source` is null or came from a Text Input Sources Copy function, so that this process
/// holds one reference to it, which dropping the result releases.
unsafe fn owned(source: *mut CFType) -> Option<CFRetained<CFType>> {
    // SAFETY: the caller's promise above.
    NonNull::new(source).map(|source| unsafe { CFRetained::from_raw(source) })
}

#[cfg(test)]
mod tests {
    use super::{kTISPropertyInputSourceID, kTISPropertyInputSourceIsASCIICapable};

    #[test]
    fn the_property_keys_are_carbons_own() {
        // SAFETY: the keys are constants Carbon exports, linked into the test as into oso.
        let (id, ascii) = unsafe {
            (
                kTISPropertyInputSourceID.to_string(),
                kTISPropertyInputSourceIsASCIICapable.to_string(),
            )
        };
        assert_eq!(id, "TISPropertyInputSourceID");
        assert_eq!(ascii, "TISPropertyInputSourceIsASCIICapable");
    }
}
