//! The window layer's events, in oso's own types (ADR 0070): what a window reports, its
//! keys, the modifiers held, the input method's reports, and the OS's requests to end with the
//! app's answer (ADR 0084). They carry no OS type, so the
//! tip crate's winit adapter and the native layers build the same ones.
//!
//! Must not depend on: `text`, `control` or `process`. The types here use no OS binding;
//! the backends below them, one submodule per system, do.
//!
//! None of the types that can hold what the person types has `Debug` (ADR 0028).
//!
//! On macOS, oso's own layer is the window system: `run` drives a `Handler` (ADR 0070,
//! stage 3). Windows and Linux are on winit, in the tip crate, until stage 4.

use std::ops::{BitOr, Range};

#[cfg(target_os = "macos")]
mod appkit;
#[cfg(target_os = "macos")]
mod utf16;
#[cfg(target_os = "macos")]
pub use appkit::{Control, RenderTarget, Waker, Window, run};

/// What the window reports, as the app sees it.
pub enum Event<'a> {
    CloseRequested,
    /// The drawable area's new size, in physical pixels.
    Resized {
        width: u32,
        height: u32,
    },
    /// The window moved to a display with another scale.
    ScaleFactorChanged(f64),
    /// Whether the window is hidden by others.
    Occluded(bool),
    /// The window has gained the keyboard focus.
    Focused,
    /// The window has lost the keyboard focus.
    Unfocused,
    RedrawRequested,
    /// The mouse wheel or the trackpad scrolled, with the pointer `at` a place over the
    /// drawable area, if the window system says where (ADR 0112).
    Wheel {
        delta: Wheel,
        at: Option<Position>,
    },
    /// A mouse button was pressed or released, or the pointer moved over the drawable area:
    /// with a button held, or, while the app asked for it, with none (ADR 0120, step 4b-2).
    Mouse(Mouse),
    ModifiersChanged(Held),
    Key(KeyInput<'a>),
    Ime(ImeEvent),
    /// The person chose another kind of keyboard input source while oso was in front, from
    /// a key such as `かな`, the menu bar or a shortcut (ADR 0106). Only oso's own macOS layer
    /// says so.
    InputSource(InputSource),
    /// Anything the app does not handle.
    Other,
}

/// What kind of keyboard input source the person has chosen (ADR 0106).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputSource {
    /// A Japanese input mode of an input method, which composes kana and kanji.
    Japanese,
    /// Anything else: a keyboard layout, or an input method's mode for Latin letters.
    Other,
}

/// How far the mouse wheel or the trackpad scrolled. Positive moves the content down and
/// right.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wheel {
    /// Notches of a wheel.
    Lines { x: f32, y: f32 },
    /// Physical pixels, from a trackpad.
    Pixels { x: f64, y: f64 },
}

/// What the mouse did, where, with the modifiers held then.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mouse {
    pub action: MouseAction,
    /// The pointer on the drawable area; past its edges while a button drags there.
    pub at: Position,
    pub held: Held,
}

/// A mouse button pressed or released, or the pointer moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    Moved,
}

/// The mouse buttons oso tells apart; the others report nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

/// A place on the drawable area, in physical pixels from its top left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

/// A real key press as the window system reports it: a release, or a press the system
/// made up for a key held when the window gained the focus, is none. The text it borrows
/// lives as long as the event.
#[derive(Clone, Copy)]
pub struct KeyInput<'a> {
    /// The key with the modifiers and the layout applied: `A` with Shift, `å` with Option.
    pub key: SystemKey<'a>,
    /// The key as it would be without modifiers, but for Shift on some systems.
    pub base: SystemKey<'a>,
    /// Held down, and sent again by the system.
    pub repeat: bool,
}

/// A key as the window system names it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SystemKey<'a> {
    Named(SystemNamed),
    /// The text the key types, usually one character.
    Character(&'a str),
    /// A dead key, which waits for the next one to compose.
    Dead,
    /// A key the system gave no name.
    Unidentified,
}

/// The named keys oso tells apart. The rest, lone modifiers among them, are `Other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemNamed {
    Enter,
    Tab,
    Escape,
    Backspace,
    Delete,
    Insert,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Space,
    /// A function key and its number, such as 12 for F12.
    Function(u8),
    Other,
}

/// The modifier keys held, by the names the window system gives them: on macOS Alt is
/// Option and Logo is Command.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held(u8);

impl Held {
    pub const NONE: Self = Self(0);
    pub const SHIFT: Self = Self(1);
    pub const CONTROL: Self = Self(1 << 1);
    pub const ALT: Self = Self(1 << 2);
    pub const LOGO: Self = Self(1 << 3);

    /// Whether every key in `other` is held.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Held {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

/// What an input method reports.
pub enum ImeEvent {
    /// The text being composed, and the caret or the segment being converted, in
    /// bytes. No text ends the composition with no commit.
    Preedit(String, Option<Range<usize>>),
    /// The text the composition gave, which comes after an empty preedit.
    Commit(String),
    /// The composition is over, whether oso asked for it or not.
    Disabled,
}

/// The cursor's cell in physical pixels, from the top left of the window's drawable area, for
/// the input method's candidate window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorArea {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Why the OS asks oso to end (ADR 0070, ADR 0084). Only oso's own macOS layer asks today;
/// this and [`QuitAnswer`] are here for every system, as its Windows layer will ask the same at
/// the end of a session (`WM_QUERYENDSESSION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitReason {
    /// A person asked, from the Dock or another app, or the request gave no reason. Any local
    /// process can send one with any reason, so this decides only whether an end is refused.
    Person,
    /// The session ends: a logout, a restart or a shutdown.
    SessionEnd,
}

/// What the app answers when the OS asks it to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuitAnswer {
    /// End now: the loop's `exiting` comes next.
    Now,
    /// Do not end. The app may still end later, by itself.
    Cancel,
    /// Hold the end until the app gives its answer (`Control::answer_quit` on macOS).
    Later,
}

/// What the app does with the native layer's events, one method per callback of the loop
/// (ADR 0070). It is called on the main thread, never twice at once: what it asks of the window
/// is done once it returns.
#[cfg(target_os = "macos")]
pub trait Handler<T> {
    /// The window may be opened now.
    fn resume(&mut self, control: &Control);
    /// Another thread sent `event`.
    fn user_event(&mut self, control: &Control, event: T);
    fn window_event(&mut self, control: &Control, event: Event<'_>);
    /// The loop is about to wait for the next events.
    fn before_waiting(&mut self, control: &Control);
    /// The OS asks to end the process, for `reason`, and the answer decides. One that comes
    /// while another method runs is asked once that returns.
    fn quit_requested(&mut self, control: &Control, reason: QuitReason) -> QuitAnswer;
    /// The loop is ending, on every exit: the last call.
    fn exiting(&mut self);
}

#[cfg(test)]
mod tests {
    use super::Held;

    #[test]
    fn held_keys_combine_and_are_found() {
        let both = Held::CONTROL | Held::SHIFT;
        assert!(both.contains(Held::CONTROL));
        assert!(both.contains(Held::SHIFT));
        assert!(both.contains(Held::NONE));
        assert!(!both.contains(Held::ALT));
        assert!(!Held::NONE.contains(Held::LOGO));
    }
}
