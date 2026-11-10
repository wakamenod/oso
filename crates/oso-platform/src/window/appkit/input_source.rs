//! The kind of keyboard input source the person has chosen, read from HIToolbox's Text Input
//! Sources (Carbon) when the system says the choice changed (ADR 0106), and told to the
//! handler through `gate::SourceGate`. The app delegate observes the change;
//! `keys::input_source` says what kind a source is.
//!
//! Must not depend on: the view, the delegates or the run loop's hooks.

use std::ffi::{CStr, c_void};
use std::ptr::NonNull;

use objc2_app_kit::NSApplication;
use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CFType};
use objc2_foundation::MainThreadMarker;

use super::{Delivery, State, keys};
use crate::window::{Event, InputSource};

/// What the system posts, to every process, when the person chooses another input source.
pub(super) const CHANGED: &str = "com.apple.Carbon.TISNotifySelectedKeyboardInputSourceChanged";

// HIToolbox is part of the Carbon framework, which AppKit loads already. No crate binds these.
#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    fn TISCopyCurrentKeyboardInputSource() -> *mut CFType;
    fn TISGetInputSourceProperty(source: NonNull<CFType>, key: &CFString) -> *const c_void;
}

/// One of HIToolbox's property keys, by its symbol's name, found where Carbon exports it, as
/// a `static` of oso's own is not allowed (ADR 0038). `None` if the system has no such key.
fn property_key(name: &CStr) -> Option<&'static CFString> {
    // SAFETY: `dlsym` with `RTLD_DEFAULT` and a NUL-terminated name only looks a symbol up,
    // in the images loaded already, Carbon among them as AppKit loads it.
    let symbol = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    let key = NonNull::new(symbol.cast::<*const CFString>())?;
    // SAFETY: HIToolbox exports each of these keys as a `const CFStringRef`, so the symbol is
    // the address of a pointer to a CFString constant that lives as long as the process.
    unsafe { key.read().as_ref() }
}

impl State {
    /// The person chose another input source: the handler hears of its kind if that changed
    /// and oso is in front. Nothing else runs here.
    pub(super) fn input_source_changed(&'static self) {
        let active = NSApplication::sharedApplication(self.mtm).isActive();
        let Some(now) = active.then(|| current(self.mtm)).flatten() else {
            return;
        };
        let mut gate = self.source.get();
        let report = gate.report(active, now);
        self.source.set(gate);
        if let Some(source) = report {
            self.deliver(Delivery::Window(Event::InputSource(source)));
        }
    }
}

/// The kind of the input source chosen now, or `None` if the system gives none.
fn current(_mtm: MainThreadMarker) -> Option<InputSource> {
    // SAFETY: a Carbon call with no arguments, made on the main thread as the Text Input
    // Sources ask, which the marker proves. It returns a source the caller owns, or null.
    let source = NonNull::new(unsafe { TISCopyCurrentKeyboardInputSource() })?;
    // SAFETY: the source came from a Copy function, so this process holds one reference to it,
    // which dropping `source` releases.
    let source = unsafe { CFRetained::from_raw(source) };
    let source_ref = NonNull::from(&*source);
    let id_key = property_key(c"kTISPropertyInputSourceID")?;
    let ascii_key = property_key(c"kTISPropertyInputSourceIsASCIICapable")?;
    // SAFETY: `source_ref` is a live input source and the keys are HIToolbox's own constants.
    // Each property follows the Get rule: it is borrowed for as long as `source` lives, which
    // is past every use below, and the ID is a CFString and the flag a CFBoolean, as the Text
    // Input Sources document them.
    let (id, ascii_capable) = unsafe {
        let id = TISGetInputSourceProperty(source_ref, id_key)
            .cast::<CFString>()
            .as_ref()?
            .to_string();
        let ascii = TISGetInputSourceProperty(source_ref, ascii_key)
            .cast::<CFBoolean>()
            .as_ref()
            .is_some_and(CFBoolean::as_bool);
        (id, ascii)
    };
    Some(keys::input_source(&id, ascii_capable))
}

#[cfg(test)]
mod tests {
    use super::property_key;

    #[test]
    fn the_property_keys_are_carbons_own() {
        let id = property_key(c"kTISPropertyInputSourceID").map(ToString::to_string);
        assert_eq!(id.as_deref(), Some("TISPropertyInputSourceID"));
        let ascii = property_key(c"kTISPropertyInputSourceIsASCIICapable").map(ToString::to_string);
        assert_eq!(
            ascii.as_deref(),
            Some("TISPropertyInputSourceIsASCIICapable")
        );
        assert!(property_key(c"kTISPropertyNoSuchKey").is_none());
    }
}
