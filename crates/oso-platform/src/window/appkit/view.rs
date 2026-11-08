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
//! - A key press comes only from the event `keyDown:` handles. Text from anything else, such
//!   as Dictation or the Character Viewer, is a commit, never a key.

use std::cell::{Cell, RefCell};

use objc2::rc::{AutoreleasePool, Retained, autoreleasepool};
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSResponder, NSTextInputClient, NSView};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSNotFound,
    NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSSize, NSString, NSUInteger,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};

use super::{Delivery, State, keys};
use crate::window::{Event, Held, ImeEvent, KeyInput, SystemKey, Wheel};

/// The key `keyDown:` is handling while the input method has it, and whether the input
/// method gave it back to oso as a key.
struct Handling {
    to_oso: bool,
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
    /// The input method is being told to drop its text, which oso has committed or dropped
    /// already: what it sends back meanwhile is not taken again.
    quiet: Cell<bool>,
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

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn view_did_change_backing_properties(&self) {
            if let Some(layer) = self.layer() {
                layer.setContentsScale(self.scale());
            }
            let state = self.ivars().state;
            state.deliver(Delivery::Window(state.scale_event()));
            state.deliver(Delivery::Window(state.resized_event()));
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            self.route_key(event);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, _event: &NSEvent) {}

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            self.note_modifiers(keys::held(event.modifierFlags()));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
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
        }
    }

    // SAFETY: `NSObjectProtocol` has no requirements.
    unsafe impl NSObjectProtocol for OsoView {}

    // SAFETY: every required method is here, with the signatures AppKit declares.
    unsafe impl NSTextInputClient for OsoView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement: NSRange) {
            if self.ivars().quiet.get() {
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
                && text.chars().count() == 1
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
            if self.ivars().quiet.get() {
                return;
            }
            let text = text_of(string);
            if text.is_empty() {
                self.ivars().marked.borrow_mut().clear();
                self.ime(ImeEvent::Preedit(String::new(), None));
                return;
            }
            let units = selected.location..selected.location.saturating_add(selected.length);
            let range = keys::utf8_range(&text, units);
            self.ivars().selected.set(selected);
            *self.ivars().marked.borrow_mut() = text.clone();
            self.ime(ImeEvent::Preedit(text, Some(range)));
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            if !self.ivars().quiet.get() {
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
                NSRange::new(0, keys::utf16_len(&marked))
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
        fn first_rect(&self, _range: NSRange, _actual: NSRangePointer) -> NSRect {
            self.cursor_rect_on_screen()
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
            quiet: Cell::new(false),
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
        let held = keys::held(event.modifierFlags());
        self.note_modifiers(held);
        let state = self.ivars().state;
        let marked = !self.ivars().marked.borrow().is_empty();
        let chord = held.contains(Held::CONTROL) || held.contains(Held::LOGO);
        let option = held.contains(Held::ALT);
        if marked {
            if option || held.contains(Held::LOGO) {
                self.commit_marked();
                self.deliver_key(event);
                return;
            }
            if held.contains(Held::CONTROL) && is_g(event) {
                // `C-g`, Emacs's quit, drops the composition as `ESC` does.
                self.discard_marked();
                return;
            }
        } else if chord || option || !state.ime_allowed.get() {
            self.deliver_key(event);
            return;
        }
        *self.ivars().handling.borrow_mut() = Some(Handling { to_oso: false });
        self.interpretKeyEvents(&NSArray::from_slice(&[event]));
        let handling = self.ivars().handling.take();
        if handling.is_some_and(|handling| handling.to_oso) {
            self.deliver_key(event);
        }
    }

    /// Gives the handler the key `event` is, by the names oso uses.
    fn deliver_key(&self, event: &NSEvent) {
        let named = keys::named(event.keyCode());
        let characters = event.characters();
        let base = event.charactersByApplyingModifiers(NSEventModifierFlags::empty());
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
        let had = !self.ivars().marked.take().is_empty();
        self.drop_input_context_text();
        if had {
            self.ime(ImeEvent::Preedit(String::new(), None));
        }
    }

    fn drop_input_context_text(&self) {
        if let Some(context) = self.inputContext() {
            self.ivars().quiet.set(true);
            context.discardMarkedText();
            self.ivars().quiet.set(false);
        }
    }

    /// The cursor's cell the app last gave, on the screen, in points.
    fn cursor_rect_on_screen(&self) -> NSRect {
        let scale = self.scale();
        let rect = self.ivars().state.ime_area.get().map_or(
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0)),
            |area| {
                NSRect::new(
                    NSPoint::new(f64::from(area.x) / scale, f64::from(area.y) / scale),
                    NSSize::new(
                        f64::from(area.width) / scale,
                        f64::from(area.height) / scale,
                    ),
                )
            },
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
        // SAFETY: the string outlives the pool, and the key does not.
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
        .is_some_and(|text| text.to_string() == "g")
}

/// AppKit's `NSNotFound`, as an unsigned location.
#[expect(clippy::cast_sign_loss, reason = "NSNotFound is NSIntegerMax")]
fn not_found() -> NSUInteger {
    NSNotFound as NSUInteger
}
