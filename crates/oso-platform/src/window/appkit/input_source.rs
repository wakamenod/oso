//! The keyboard input source, through HIToolbox's Text Input Sources (Carbon), for ADR 0106:
//! the kind the person has chosen, told to the handler through `gate::SourceGate` when the
//! system says the choice changed, and an ASCII-capable source chosen while the app wants one,
//! as while a prompt that reads text or completes one is open (ADR 0158), with the person's given back after
//! (`gate::AsciiInput`). The app delegate observes the change; `keys::input_source` says what
//! kind a source is.
//!
//! Must not depend on: the view, the delegates or the run loop's hooks.

use std::ffi::c_void;
use std::ptr::NonNull;

use objc2_app_kit::NSApplication;
use objc2_core_foundation::{CFBoolean, CFRetained, CFString, CFType};
use objc2_foundation::MainThreadMarker;

use super::gate::{AsciiInput, AsciiStep, SourceGate};
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

impl State {
    /// The person chose another input source: the handler hears of its kind if that changed
    /// and oso is in front. Nothing else runs here.
    pub(super) fn input_source_changed(&'static self) {
        // The state is the main thread's; a notice delivered on another thread is dropped.
        if MainThreadMarker::new().is_none() {
            return;
        }
        let active = NSApplication::sharedApplication(self.mtm).isActive();
        let report = self.input.borrow_mut().chosen(&Tis(self.mtm), active);
        self.tell_input_source(report);
    }

    /// The app wants ASCII or not (`wanted`), or oso went behind or ends (`None`, or `false`):
    /// chooses an ASCII-capable source, or gives the person's back, at once. Dev builds log how
    /// long that took, as the loop stands still meanwhile (ADR 0158).
    pub(super) fn update_ascii(&self, wanted: Option<bool>) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        #[cfg(feature = "dev")]
        let start = std::time::Instant::now();
        let active = NSApplication::sharedApplication(self.mtm).isActive();
        let secure = self.secure.get().wanted();
        self.input
            .borrow_mut()
            .ascii(&Tis(self.mtm), wanted, active, secure);
        #[cfg(feature = "dev")]
        log::debug!("input source for ASCII {wanted:?}: {:?}", start.elapsed());
    }

    /// The app wants ASCII or not (`wanted`). Choosing a source takes tens of milliseconds, so it
    /// waits for `settle_ascii` (ADR 0158).
    pub(super) fn ask_ascii(&self, wanted: bool) {
        self.input.borrow_mut().ask(wanted);
    }

    /// Does what `ask_ascii` was told last, if that waits: once the frame is drawn, or before a key
    /// goes to the input method, which then has the person's source.
    pub(super) fn settle_ascii(&self) {
        if self.input.borrow().waits() {
            self.update_ascii(None);
        }
    }

    /// Looks at the source if a notice came while `settle_ascii` was due, and it is done.
    pub(super) fn look_at_held_notice(&'static self) {
        if self.input.borrow_mut().take_held() {
            self.input_source_changed();
        }
    }

    /// oso came to the front: ASCII again if the app still wants it, then the kind read.
    pub(super) fn input_source_in_front(&'static self) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        let secure = self.secure.get().wanted();
        let report = self.input.borrow_mut().in_front(&Tis(self.mtm), secure);
        self.tell_input_source(report);
    }

    fn tell_input_source(&'static self, report: Option<InputSource>) {
        if let Some(source) = report {
            self.deliver(Delivery::Window(Event::InputSource(source)));
        }
    }
}

/// The tracker of the system's input sources, which `State` keeps.
pub(super) type SystemTracker = Tracker<CFRetained<CFType>>;

/// What the window layer keeps of the input source (ADR 0106): the kind the handler was last
/// told of, whether the app wants ASCII, and the person's source to give back.
pub(super) struct Tracker<T> {
    told: SourceGate,
    ascii: AsciiInput,
    switched: Option<Switched<T>>,
    /// The source was not looked at, as what the app asked waited: its kind, read then, would be
    /// taken as chosen in a prompt just opened.
    held: bool,
}

impl<T> Default for Tracker<T> {
    fn default() -> Self {
        Self {
            told: SourceGate::default(),
            ascii: AsciiInput::default(),
            switched: None,
            held: false,
        }
    }
}

/// The ASCII-capable source oso chose, by its ID, and the person's before it, to give back.
struct Switched<T> {
    previous: T,
    chosen: String,
}

impl<T> Switched<T> {
    /// Whether the source oso chose is still the one chosen.
    fn is_chosen<S: Sources<Source = T>>(&self, sources: &S) -> bool {
        let now = sources.current().and_then(|source| sources.id(&source));
        now.as_deref() == Some(self.chosen.as_str())
    }
}

impl<T> Tracker<T> {
    /// The person chose another source: what to tell the handler.
    fn chosen<S: Sources<Source = T>>(&mut self, sources: &S, active: bool) -> Option<InputSource> {
        if !active {
            return None;
        }
        self.held = self.ascii.waits();
        if self.held {
            return None;
        }
        let now = kind_now(sources)?;
        self.told.report(active, now)
    }

    /// The app wants ASCII or not, once settled.
    fn ask(&mut self, wanted: bool) {
        self.ascii.ask(wanted);
    }

    fn waits(&self) -> bool {
        self.ascii.waits()
    }

    /// Whether a notice was held and can be looked at now.
    fn take_held(&mut self) -> bool {
        !self.waits() && std::mem::take(&mut self.held)
    }

    /// What was asked is due, or oso came to the front or went behind, or ends (`wanted`).
    fn ascii<S: Sources<Source = T>>(
        &mut self,
        sources: &S,
        wanted: Option<bool>,
        active: bool,
        secure: bool,
    ) {
        match self.ascii.step(wanted, active, secure) {
            AsciiStep::Switch => self.switched = switch_to_ascii(sources),
            AsciiStep::Restore => {
                if let Some(switched) = self.switched.take() {
                    give_back(sources, &switched);
                }
            }
            AsciiStep::Renew => {
                let ours = self
                    .switched
                    .as_ref()
                    .is_some_and(|switched| switched.is_chosen(sources));
                if !ours {
                    self.switched = switch_to_ascii(sources);
                }
            }
            AsciiStep::Nothing => {}
        }
    }

    /// oso came to the front: ASCII is chosen first, if the app wants it, and only then is the
    /// kind read. Read first, the person's Japanese, which oso gave back while behind, would
    /// mark the open prompt as taking Japanese, with ASCII chosen right after.
    fn in_front<S: Sources<Source = T>>(
        &mut self,
        sources: &S,
        secure: bool,
    ) -> Option<InputSource> {
        self.ascii(sources, None, true, secure);
        self.chosen(sources, true)
    }
}

/// The system's input sources, as the window layer uses them: a trait so that what oso does
/// with them, and in what order, is tested without the system. [`Tis`] is the system's.
trait Sources {
    type Source;
    /// The source chosen now.
    fn current(&self) -> Option<Self::Source>;
    /// The current input method's ASCII-capable source, as `英数` chooses.
    fn ascii(&self) -> Option<Self::Source>;
    /// Makes `source` the chosen one; `false` if the system refused.
    fn select(&self, source: &Self::Source) -> bool;
    fn id(&self, source: &Self::Source) -> Option<String>;
    fn types_ascii(&self, source: &Self::Source) -> bool;
}

/// The kind of the source chosen now, or `None` if the system gives none.
fn kind_now<S: Sources>(sources: &S) -> Option<InputSource> {
    let source = sources.current()?;
    let id = sources.id(&source)?;
    Some(keys::input_source(&id, sources.types_ascii(&source)))
}

/// Chooses the ASCII-capable source if the person's cannot type ASCII. Returns what to give
/// back, or `None` if nothing was chosen.
fn switch_to_ascii<S: Sources>(sources: &S) -> Option<Switched<S::Source>> {
    let previous = sources.current()?;
    if sources.types_ascii(&previous) {
        return None;
    }
    let ascii = sources.ascii()?;
    let chosen = sources.id(&ascii)?;
    sources
        .select(&ascii)
        .then_some(Switched { previous, chosen })
}

/// Gives the person's source back, unless they have chosen another since oso chose ASCII.
fn give_back<S: Sources>(sources: &S, switched: &Switched<S::Source>) {
    if switched.is_chosen(sources) {
        sources.select(&switched.previous);
    }
}

/// The Text Input Sources, on the main thread, which the marker proves.
struct Tis(MainThreadMarker);

impl Sources for Tis {
    type Source = CFRetained<CFType>;

    fn current(&self) -> Option<Self::Source> {
        // SAFETY: a Carbon call with no arguments, made on the main thread as the Text Input
        // Sources ask, which the marker proves. It returns a source the caller owns, or null.
        unsafe { owned(TISCopyCurrentKeyboardInputSource()) }
    }

    fn ascii(&self) -> Option<Self::Source> {
        // SAFETY: as for `current`.
        unsafe { owned(TISCopyCurrentASCIICapableKeyboardInputSource()) }
    }

    fn select(&self, source: &Self::Source) -> bool {
        // SAFETY: `source` is a live input source, and selecting one only changes which the
        // system uses; the call is made on the main thread, which the marker proves.
        unsafe { TISSelectInputSource(NonNull::from(&**source)) == 0 }
    }

    fn id(&self, source: &Self::Source) -> Option<String> {
        // SAFETY: `source` is a live input source and the key is HIToolbox's own constant. The
        // ID is a CFString, as the Text Input Sources document it, borrowed under the Get rule
        // while `source` lives, and copied out here.
        unsafe {
            TISGetInputSourceProperty(NonNull::from(&**source), kTISPropertyInputSourceID)
                .cast::<CFString>()
                .as_ref()
                .map(ToString::to_string)
        }
    }

    fn types_ascii(&self, source: &Self::Source) -> bool {
        // SAFETY: as for `id`; the flag is a CFBoolean, read while `source` lives.
        unsafe {
            TISGetInputSourceProperty(
                NonNull::from(&**source),
                kTISPropertyInputSourceIsASCIICapable,
            )
            .cast::<CFBoolean>()
            .as_ref()
            .is_some_and(CFBoolean::as_bool)
        }
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
    use std::cell::Cell;

    use super::{
        Sources, Tracker, kTISPropertyInputSourceID, kTISPropertyInputSourceIsASCIICapable,
    };
    use crate::window::InputSource;

    const HIRAGANA: &str = "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese";
    const ROMAN: &str = "com.apple.inputmethod.Kotoeri.RomajiTyping.Roman";
    const GOOGLE: &str = "com.google.inputmethod.Japanese.base";

    /// The system's sources, by ID, and how many times one was selected: choosing ASCII gives the same input method's Roman mode.
    struct Fake(Cell<&'static str>, Cell<usize>);

    fn system() -> Fake {
        Fake(Cell::new(HIRAGANA), Cell::new(0))
    }

    impl Sources for Fake {
        type Source = &'static str;

        fn current(&self) -> Option<&'static str> {
            Some(self.0.get())
        }

        fn ascii(&self) -> Option<&'static str> {
            Some(ROMAN)
        }

        fn select(&self, source: &&'static str) -> bool {
            self.0.set(source);
            self.1.set(self.1.get() + 1);
            true
        }

        fn id(&self, source: &&'static str) -> Option<String> {
            Some((*source).to_owned())
        }

        fn types_ascii(&self, source: &&'static str) -> bool {
            *source == ROMAN
        }
    }

    #[test]
    fn coming_back_to_an_open_prompt_chooses_ascii_before_the_kind_is_read() {
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ascii(&system, Some(true), true, false);
        assert_eq!(system.0.get(), ROMAN, "the prompt opens in ASCII");
        assert_eq!(tracker.chosen(&system, true), Some(InputSource::Other));
        tracker.ascii(&system, None, false, false);
        assert_eq!(
            system.0.get(),
            HIRAGANA,
            "another app gets the person's back"
        );
        assert_eq!(
            tracker.chosen(&system, false),
            None,
            "behind, nothing is told"
        );
        // Back in front with the prompt still open: Japanese, read first, would mark it.
        assert_eq!(tracker.in_front(&system, false), None);
        assert_eq!(system.0.get(), ROMAN);
    }

    #[test]
    fn the_persons_source_comes_back_unless_they_chose_another() {
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ascii(&system, Some(true), true, false);
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(system.0.get(), HIRAGANA, "the prompt closed");
        tracker.ascii(&system, Some(true), true, false);
        system.0.set(GOOGLE);
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(system.0.get(), GOOGLE, "chosen meanwhile, it stays");
        // Already typing ASCII: nothing is chosen, and nothing given back.
        system.0.set(ROMAN);
        tracker.ascii(&system, Some(true), true, false);
        system.0.set(HIRAGANA);
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(system.0.get(), HIRAGANA);
    }

    #[test]
    fn each_close_gives_back_only_what_its_own_open_took() {
        // The author's K5 and K6: from Japanese, C-x C-f, away and back, C-g; then C-x C-f
        // and C-g again.
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ascii(&system, Some(true), true, false);
        tracker.ascii(&system, None, false, false);
        tracker.in_front(&system, false);
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(system.0.get(), HIRAGANA, "K5 ends in the person's Japanese");
        tracker.ascii(&system, Some(true), true, false);
        assert_eq!(system.0.get(), ROMAN, "K6 opens in ASCII");
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(
            system.0.get(),
            HIRAGANA,
            "and gives K5's ending source back"
        );
        // Had the person chosen 英数 before K6, its prompt would take and give back nothing.
        system.0.set(ROMAN);
        tracker.ascii(&system, Some(true), true, false);
        tracker.ascii(&system, Some(false), true, false);
        assert_eq!(system.0.get(), ROMAN);
    }

    impl Fake {
        /// What is selected now, and how many selects that took since the last look.
        fn look(&self) -> (&'static str, usize) {
            (self.0.get(), self.1.take())
        }
    }

    fn settle(tracker: &mut Tracker<&'static str>, system: &Fake) {
        tracker.ascii(system, None, true, false);
    }

    /// A tracker with a search open, ASCII chosen and told of.
    fn searching(system: &Fake) -> Tracker<&'static str> {
        let mut tracker = Tracker::default();
        tracker.chosen(system, true);
        tracker.ask(true);
        settle(&mut tracker, system);
        tracker.chosen(system, true);
        system.look();
        tracker
    }

    #[test]
    fn what_the_app_asks_selects_nothing_until_it_is_settled() {
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ask(true);
        assert_eq!(
            system.look(),
            (HIRAGANA, 0),
            "the key that opens it waits for none"
        );
        assert!(tracker.waits());
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (ROMAN, 1), "after the frame");
        assert!(!tracker.waits());
        tracker.ask(false);
        assert_eq!(system.look(), (ROMAN, 0), "nor does the one that closes it");
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (HIRAGANA, 1));
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (HIRAGANA, 0), "once");
    }

    #[test]
    fn an_open_and_a_close_before_a_frame_select_nothing() {
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ask(true);
        tracker.ask(false);
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (HIRAGANA, 0));
        // A search that closes as a prompt opens keeps the ASCII source it has.
        let mut tracker = searching(&system);
        tracker.ask(false);
        tracker.ask(true);
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (ROMAN, 0));
        tracker.ask(false);
        settle(&mut tracker, &system);
        assert_eq!(
            system.look(),
            (HIRAGANA, 1),
            "still the person's to have back"
        );
    }

    #[test]
    fn what_opens_as_one_with_japanese_closes_has_ascii_chosen_again() {
        let system = system();
        let mut tracker = searching(&system);
        system.0.set(GOOGLE);
        tracker.ask(false);
        tracker.ask(true);
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (ROMAN, 1));
        tracker.ask(false);
        settle(&mut tracker, &system);
        assert_eq!(
            system.look(),
            (GOOGLE, 1),
            "the one the person chose comes back"
        );
    }

    #[test]
    fn a_late_notice_of_osos_own_never_marks_the_search_just_opened() {
        let system = system();
        let mut tracker = searching(&system);
        tracker.ask(false);
        settle(&mut tracker, &system);
        // The next search opens before the notice of the source given back comes.
        tracker.ask(true);
        assert_eq!(tracker.chosen(&system, true), None, "held");
        assert!(!tracker.take_held(), "until it is settled");
        settle(&mut tracker, &system);
        assert!(tracker.take_held(), "looked at then");
        assert_eq!(tracker.chosen(&system, true), None, "ASCII, as told");
        assert!(!tracker.take_held());
    }

    #[test]
    fn a_choice_of_the_persons_before_the_give_back_stays_and_is_told() {
        let system = system();
        let mut tracker = searching(&system);
        tracker.ask(false);
        system.0.set(GOOGLE);
        assert_eq!(tracker.chosen(&system, true), None, "held");
        settle(&mut tracker, &system);
        assert_eq!(system.look(), (GOOGLE, 0));
        assert!(tracker.take_held());
        assert_eq!(tracker.chosen(&system, true), Some(InputSource::Japanese));
    }

    #[test]
    fn going_behind_and_the_end_give_back_at_once_whatever_waits() {
        for (asked, end) in [
            (None, None),
            (Some(true), None),
            (Some(false), None),
            (None, Some(false)),
            (Some(true), Some(false)),
            (Some(false), Some(false)),
        ] {
            let system = system();
            let mut tracker = searching(&system);
            asked.into_iter().for_each(|wanted| tracker.ask(wanted));
            tracker.ascii(&system, end, end.is_some(), false);
            assert_eq!(system.look(), (HIRAGANA, 1), "{asked:?} {end:?}");
            assert!(!tracker.waits());
        }
        // Asked, and not chosen yet: nothing is.
        for end in [None, Some(false)] {
            let system = system();
            let mut tracker = Tracker::default();
            tracker.ask(true);
            tracker.ascii(&system, end, end.is_some(), false);
            assert_eq!(system.look(), (HIRAGANA, 0), "{end:?}");
        }
    }

    #[test]
    fn nothing_is_chosen_once_a_buffer_that_may_hold_secrets_is_selected() {
        let system = system();
        let mut tracker = Tracker::default();
        tracker.ask(true);
        tracker.ascii(&system, None, true, true);
        assert_eq!(system.look(), (HIRAGANA, 0));
        // Given back all the same.
        let mut tracker = searching(&system);
        tracker.ask(false);
        tracker.ascii(&system, None, true, true);
        assert_eq!(system.look(), (HIRAGANA, 1));
        // Nor is it chosen again there, nor as oso comes to the front.
        let mut tracker = searching(&system);
        system.0.set(GOOGLE);
        tracker.ask(false);
        tracker.ask(true);
        tracker.ascii(&system, None, true, true);
        assert_eq!(system.look(), (GOOGLE, 0));
        let mut tracker = Tracker::default();
        tracker.ask(true);
        tracker.ascii(&system, None, false, true);
        tracker.in_front(&system, true);
        assert_eq!(system.look(), (GOOGLE, 0));
    }

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
