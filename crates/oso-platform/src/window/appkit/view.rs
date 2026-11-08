//! The window's one view: it is backed by a `CAMetalLayer` for wgpu, takes keys before the
//! input method does, and is the input method's client (`NSTextInputClient`), as ADR 0070
//! says.
//!
//! Must not depend on: the run loop's hooks or the delegates; it reports to the state.
//!
//! - With Control, Command or Option held and nothing marked, a key never reaches the input
//!   method, so Option-a is `s-a` whatever the input source.
//! - With text marked, Option or Command commits it, then the key runs, and `C-g` drops it,
//!   so no key loses composed text. Other keys go to the input method.
//! - While oso has the input method off, as in a buffer that may hold secrets, no text and no
//!   marked text from it is taken, and text marked meanwhile is dropped at the next key.
//! - A key press comes only from the event `keyDown:` handles. Text from anything else, such
//!   as Dictation or the Character Viewer, is a commit, never a key.

use std::cell::{Cell, RefCell};
use std::panic::AssertUnwindSafe;

use objc2::exception;
use objc2::rc::{AutoreleasePool, Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Sel};
use objc2::{ClassType, DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSEvent, NSEventModifierFlags, NSResponder, NSTextInputClient, NSTextInputContext,
    NSTextInputTraits, NSView, NSWritingToolsBehavior,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound,
    NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
    ns_string,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};

use super::keys::{self, Route};
use super::{Delivery, State};
use crate::window::utf16;
use crate::window::{CursorArea, Event, Held, ImeEvent, KeyInput, SystemKey, Wheel};

/// The key `keyDown:` is handling while the input method has it, and whether the input
/// method gave it back to oso as a key.
struct Handling {
    to_oso: bool,
    /// What the key types, to tell its own character from the input method's.
    characters: Option<String>,
}

pub(super) struct Ivars {
    state: &'static State,
    /// The text being composed, as the input method last gave it; empty when nothing is.
    marked: RefCell<String>,
    /// The caret or the segment in it, in UTF-16 units, as the input method gave it.
    selected: Cell<NSRange>,
    handling: RefCell<Option<Handling>>,
    /// The modifiers last reported to the handler.
    held: Cell<Held>,
    /// oso has committed or dropped the text being composed, and told the input method to drop
    /// its own: what it sends with no key in hand is not taken until the next key, as a reply
    /// may come late.
    muted: Cell<bool>,
}

define_class!(
    // SAFETY:
    // - NSView has no subclassing requirements beyond being used on the main thread.
    // - `OsoView` does not implement `Drop`.
    #[unsafe(super(NSView, NSResponder, objc2_foundation::NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OsoView"]
    #[ivars = Ivars]
    pub(super) struct OsoView;

    // SAFETY: each method overrides NSView's or NSResponder's with the signature AppKit
    // declares for it.
    impl OsoView {
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// None while oso has the IME off, so that no input method or other text service
        /// reads the view then (ADR 0070).
        #[unsafe(method_id(inputContext))]
        fn input_context(&self) -> Option<Retained<NSTextInputContext>> {
            if self.ivars().state.ime_allowed() {
                self.own_input_context()
            } else {
                None
            }
        }

        /// Places count from the top left, as the app's do.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<CALayer> {
            let layer = CAMetalLayer::new();
            layer.setContentsScale(self.scale());
            layer.into_super()
        }

        /// AppKit displays the layer itself, through `updateLayer`, and not by drawing.
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        /// AppKit wants the view shown, as when the window first appears: a frame now,
        /// rather than when the loop next waits, as winit draws from `drawRect:`.
        #[unsafe(method(updateLayer))]
        fn update_layer(&self) {
            // A request the app made stays, and is drawn when the loop next waits.
            self.ivars()
                .state
                .deliver(Delivery::Window(Event::RedrawRequested));
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            if let Some(layer) = self.layer() {
                layer.setContentsScale(self.scale());
            }
            self.ivars().state.report_scale();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.ivars().state.during_event(|| self.route_key(event));
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, _event: &NSEvent) {}

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            let held = keys::held(event.modifierFlags());
            self.ivars().state.during_event(|| self.note_modifiers(held));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            self.ivars().state.during_event(|| {
                let (x, y) = (event.scrollingDeltaX(), event.scrollingDeltaY());
                let wheel = if event.hasPreciseScrollingDeltas() {
                    let scale = self.scale();
                    Wheel::Pixels {
                        x: x * scale,
                        y: y * scale,
                    }
                } else {
                    #[expect(clippy::cast_possible_truncation, reason = "a wheel's notches are few")]
                    Wheel::Lines {
                        x: x as f32,
                        y: y as f32,
                    }
                };
                self.ivars()
                    .state
                    .deliver(Delivery::Window(Event::Wheel(wheel)));
            });
        }
    }

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for OsoView {}

    // SAFETY: the protocol's methods are all optional, and the one here has the signature
    // AppKit declares for it.
    unsafe impl NSTextInputTraits for OsoView {
        /// Writing Tools are off for the view, which rewrites no buffer text for another
        /// process (ADR 0070).
        #[unsafe(method(writingToolsBehavior))]
        fn writing_tools_behavior(&self) -> NSWritingToolsBehavior {
            NSWritingToolsBehavior::None
        }
    }

    // SAFETY: every required method is here, with the signatures AppKit declares.
    unsafe impl NSTextInputClient for OsoView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement: NSRange) {
            if !self.in_key() && !self.takes_text() {
                return;
            }
            let text = text_of(string);
            if !self.ivars().marked.borrow().is_empty() {
                self.ivars().marked.borrow_mut().clear();
                self.ime(ImeEvent::Preedit(String::new(), None));
                self.ime(ImeEvent::Commit(text));
                return;
            }
            let mut handling = self.ivars().handling.borrow_mut();
            if let Some(handling) = handling.as_mut()
                && keys::is_keys_own(&text, handling.characters.as_deref())
            {
                // The key's own character: it goes to oso as the key it is.
                handling.to_oso = true;
                return;
            }
            drop(handling);
            if text.chars().any(char::is_control) {
                return;
            }
            // Text no key typed as itself, such as a full-width space from the input
            // method, or Dictation's: a commit, which goes through typing's checks.
            self.ime(ImeEvent::Commit(text));
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // Only for the key being handled; a selector from elsewhere is ignored.
            if let Some(handling) = self.ivars().handling.borrow_mut().as_mut() {
                handling.to_oso = true;
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, _replacement: NSRange) {
            if !self.in_key() && !self.takes_text() {
                return;
            }
            let text = text_of(string);
            if text.is_empty() {
                self.ivars().marked.borrow_mut().clear();
                self.ime(ImeEvent::Preedit(String::new(), None));
                return;
            }
            let units = selected.location..selected.location.saturating_add(selected.length);
            let range = utf16::utf8_range(&text, units);
            self.ivars().selected.set(selected);
            let mut marked = self.ivars().marked.borrow_mut();
            marked.clear();
            marked.push_str(&text);
            drop(marked);
            self.ime(ImeEvent::Preedit(text, Some(range)));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            if self.in_key() || self.takes_text() {
                self.commit_marked();
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            if self.ivars().marked.borrow().is_empty() {
                NSRange::new(0, 0)
            } else {
                self.ivars().selected.get()
            }
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let marked = self.ivars().marked.borrow();
            if marked.is_empty() {
                NSRange::new(not_found(), 0)
            } else {
                NSRange::new(0, utf16::utf16_len(&marked))
            }
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            !self.ivars().marked.borrow().is_empty()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            _range: NSRange,
            _actual: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            // No text of a buffer yet: the cursor's line comes with the next step of ADR 0070.
            None
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, range: NSRange, actual: NSRangePointer) -> NSRect {
            let units = range.location..range.location.saturating_add(range.length);
            let answer = {
                let marked = self.ivars().marked.borrow();
                self.ivars().state.ime.borrow().rect(&marked, units)
            };
            // While the input method is off, as in a buffer that may hold secrets, no place.
            let Some((area, covered)) = answer else {
                return NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            };
            // The range cut to the composition, as the answer covers no more.
            if let Some(units) = covered
                && !actual.is_null()
            {
                // SAFETY: AppKit passes null or a pointer to an `NSRange` it owns for the length
                // of the call, which is not null here.
                unsafe { actual.write(NSRange::new(units.start, units.end - units.start)) };
            }
            self.on_screen(area)
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, _point: NSPoint) -> NSUInteger {
            not_found()
        }
    }
);

impl OsoView {
    pub(super) fn new(
        mtm: MainThreadMarker,
        state: &'static State,
        frame: NSRect,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            state,
            marked: RefCell::new(String::new()),
            selected: Cell::new(NSRange::new(0, 0)),
            handling: RefCell::new(None),
            held: Cell::new(Held::NONE),
            muted: Cell::new(false),
        });
        // SAFETY: `initWithFrame:` is NSView's initializer, with this signature.
        let view: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        view.setWantsLayer(true);
        view
    }

    fn scale(&self) -> f64 {
        self.window()
            .map_or(1.0, |window| window.backingScaleFactor())
    }

    fn ime(&self, event: ImeEvent) {
        self.ivars()
            .state
            .deliver(Delivery::Window(Event::Ime(event)));
    }

    /// Tells the handler of a change of modifiers, once.
    fn note_modifiers(&self, held: Held) {
        if self.ivars().held.replace(held) != held {
            self.ivars()
                .state
                .deliver(Delivery::Window(Event::ModifiersChanged(held)));
        }
    }

    /// The modifiers are no longer known to be held, as the window lost the keyboard.
    pub(super) fn forget_modifiers(&self) {
        self.ivars().held.set(Held::NONE);
    }

    /// Decides where a key goes: to oso, or to the input method first.
    fn route_key(&self, event: &NSEvent) {
        self.ivars().muted.set(false);
        let held = keys::held(event.modifierFlags());
        self.note_modifiers(held);
        let marked = !self.ivars().marked.borrow().is_empty();
        let allowed = self.ivars().state.ime_allowed();
        let is_g = marked && held == Held::CONTROL && is_g(event);
        match keys::route(held, marked, allowed, is_g) {
            Route::Oso => self.deliver_key(event),
            Route::CommitThenOso => {
                self.commit_marked();
                self.deliver_key(event);
            }
            // `C-g`, Emacs's quit, drops the composition as `ESC` does.
            Route::Discard => self.discard_marked(),
            Route::DiscardThenOso => {
                self.discard_marked();
                self.deliver_key(event);
            }
            Route::InputMethod => {
                *self.ivars().handling.borrow_mut() = Some(Handling {
                    to_oso: false,
                    characters: event.characters().map(|text| text.to_string()),
                });
                let events = NSArray::from_slice(&[event]);
                // An exception from the input method must not leave a key in hand.
                let thrown =
                    exception::catch(AssertUnwindSafe(|| self.interpretKeyEvents(&events)));
                let handling = self.ivars().handling.take();
                if thrown.is_err() {
                    log::error!("an Objective-C exception was raised by the input method");
                    return;
                }
                if handling.is_some_and(|handling| handling.to_oso) {
                    self.deliver_key(event);
                }
            }
        }
    }

    /// Whether the input method is answering a key `keyDown:` gave it.
    fn in_key(&self) -> bool {
        self.ivars().handling.borrow().is_some()
    }

    /// Whether text the input method sends with no key in hand is taken now.
    fn takes_text(&self) -> bool {
        keys::takes_text(self.ivars().state.ime_allowed(), self.ivars().muted.get())
    }

    /// Gives the handler the key `event` is, by the names oso uses.
    fn deliver_key(&self, event: &NSEvent) {
        let named = keys::named(event.keyCode());
        // A named key needs neither text. The key without modifiers, a layout's translation,
        // matters only with Control or Option held (ADR 0033); otherwise it is the key's own.
        let (characters, base) = if named.is_some() {
            (None, None)
        } else {
            let characters = event.characters();
            let held = keys::held(event.modifierFlags());
            let base = if held.contains(Held::CONTROL) || held.contains(Held::ALT) {
                event.charactersByApplyingModifiers(NSEventModifierFlags::empty())
            } else {
                characters.clone()
            };
            (characters, base)
        };
        autoreleasepool(|pool| {
            let input = KeyInput {
                key: system_key(named, characters.as_deref(), pool),
                base: system_key(named, base.as_deref(), pool),
                repeat: event.isARepeat(),
            };
            self.ivars()
                .state
                .deliver(Delivery::Window(Event::Key(input)));
        });
    }

    /// Commits the text being composed as it is shown, and tells the input method to drop
    /// its own.
    pub(super) fn commit_marked(&self) {
        let text = self.ivars().marked.take();
        if text.is_empty() {
            return;
        }
        self.drop_input_context_text();
        self.ime(ImeEvent::Preedit(String::new(), None));
        self.ime(ImeEvent::Commit(text));
    }

    /// Drops the text being composed, with no commit.
    pub(super) fn discard_marked(&self) {
        // Nothing marked, as when the IME goes off for a prefix key: no call to the OS.
        if self.ivars().marked.take().is_empty() {
            return;
        }
        self.drop_input_context_text();
        self.ime(ImeEvent::Preedit(String::new(), None));
    }

    fn drop_input_context_text(&self) {
        self.ivars().muted.set(true);
        // The view's own, also once the IME is off and it gives none.
        if let Some(context) = self.own_input_context()
            && exception::catch(AssertUnwindSafe(|| context.discardMarkedText())).is_err()
        {
            log::error!("an Objective-C exception was raised by the input method");
        }
    }

    /// The input context NSView keeps for the view, whether or not the IME is on: NSView's
    /// own method, past the view's override, which gives none while the IME is off.
    fn own_input_context(&self) -> Option<Retained<NSTextInputContext>> {
        // SAFETY: `self` is an `OsoView`, whose superclass is NSView, so the lookup from NSView
        // finds NSView's `inputContext`. It takes no arguments and returns an
        // `NSTextInputContext *` or nil, which `Retained` retains, and the view is used only on
        // the main thread.
        unsafe { msg_send![super(self, NSView::class()), inputContext] }
    }

    /// Whether the view gives an input context now, and whether AppKit's current one is it,
    /// for the dev socket's check.
    #[cfg(feature = "dev")]
    pub(super) fn input_context_state(&self) -> (bool, bool) {
        let given = self.inputContext();
        let current = NSTextInputContext::currentInputContext(self.mtm());
        let is_current =
            matches!((&given, &current), (Some(given), Some(current)) if given == current);
        (given.is_some(), is_current)
    }

    /// `area`, in physical pixels of the view, in points on the screen.
    fn on_screen(&self, area: CursorArea) -> NSRect {
        let scale = self.scale();
        let rect = NSRect::new(
            NSPoint::new(f64::from(area.x) / scale, f64::from(area.y) / scale),
            NSSize::new(
                f64::from(area.width) / scale,
                f64::from(area.height) / scale,
            ),
        );
        let in_window = self.convertRect_toView(rect, None);
        self.window()
            .map_or(in_window, |window| window.convertRectToScreen(in_window))
    }
}

/// A key by its code if it has a name, and otherwise by the text it types, borrowed for as
/// long as `pool`.
fn system_key<'p>(
    named: Option<crate::window::SystemNamed>,
    text: Option<&'p NSString>,
    pool: AutoreleasePool<'p>,
) -> SystemKey<'p> {
    match (named, text) {
        (Some(named), _) => SystemKey::Named(named),
        // SAFETY: `pool` is the innermost autorelease pool here, as `deliver_key` makes it, and
        // NSString's text does not change while it is borrowed; the key lives inside the pool.
        (None, Some(text)) => match unsafe { text.to_str(pool) } {
            "" => SystemKey::Dead,
            text => SystemKey::Character(text),
        },
        (None, None) => SystemKey::Unidentified,
    }
}

/// The text of what an input method gives, which is a string or an attributed one.
fn text_of(string: &AnyObject) -> String {
    if let Some(attributed) = string.downcast_ref::<NSAttributedString>() {
        attributed.string().to_string()
    } else if let Some(plain) = string.downcast_ref::<NSString>() {
        plain.to_string()
    } else {
        String::new()
    }
}

/// Whether the key is `g` on the layout, whatever is held.
fn is_g(event: &NSEvent) -> bool {
    event
        .charactersByApplyingModifiers(NSEventModifierFlags::empty())
        .is_some_and(|text| text.isEqualToString(ns_string!("g")))
}

/// AppKit's `NSNotFound`, as an unsigned location.
#[expect(clippy::cast_sign_loss, reason = "NSNotFound is NSIntegerMax")]
fn not_found() -> NSUInteger {
    NSNotFound as NSUInteger
}
