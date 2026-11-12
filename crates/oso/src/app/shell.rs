//! The window system, winit, behind oso's own types: the one file of the app that knows
//! winit (ADR 0070). It runs the event loop, turns winit's events into [`Event`], its keys
//! into [`KeyInput`] and its input method's reports into [`ImeEvent`], and wraps the loop,
//! the window and the waker. It also starts the renderer, which takes the window as wgpu's
//! handle. A window layer of oso's own sits beside this file, then replaces it.
//!
//! Must not depend on: what an event does. Each one goes to the app's handler for it, and
//! nothing here reads the editor.

use std::cell::Cell;
use std::sync::Arc;
use std::time::Instant;

use oso_core::frame::PixelRect;
use oso_platform::text::TextSystem;
use oso_render::RenderState;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, Ime, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy};
use winit::keyboard::{self, ModifiersState};
use winit::window::{UserAttentionType, WindowId};

use oso_platform::window::{
    Event, Held, ImeEvent, KeyInput, Position, SystemKey, SystemNamed, Wheel,
};

use super::{App, UserEvent};

/// winit cannot commit a composition, so turning the IME off waits for one to end
/// (ADR 0069, ADR 0077).
pub(super) const COMMITS_WHEN_TURNED_OFF: bool = false;

/// winit gives the input method one rectangle and no text, so the app works out neither the
/// composition's layout nor the excerpt for it (ADR 0077).
pub(super) const ANSWERS_INPUT_METHOD: bool = false;

/// Runs the event loop until it ends. `start` makes the app, given the waker other threads
/// reach it through, before the window opens. `started` is when the process started.
pub fn run(started: Instant, start: impl FnOnce(Waker) -> App) {
    let mut builder = winit::event_loop::EventLoop::<UserEvent>::with_user_event();
    // With no window system to reach, as when the display has gone, there is nothing to show:
    // it says why and exits, where a panic would say only where (ADR 0028). Nothing is open
    // yet, so nothing is lost.
    let event_loop = match builder.build() {
        Ok(event_loop) => event_loop,
        Err(e) => {
            // Only this level reaches a terminal with no `RUST_LOG`.
            log::error!("cannot reach the window system: {e}");
            std::process::exit(1);
        }
    };
    // Sleep until an event arrives; frames are drawn only on demand.
    event_loop.set_control_flow(ControlFlow::Wait);
    log::debug!("event loop built {:?} after start", started.elapsed());
    let mut app = start(Waker(event_loop.create_proxy()));
    event_loop.run_app(&mut app).unwrap();
}

/// Wakes the event loop from another thread with a [`UserEvent`].
#[derive(Clone)]
pub struct Waker(EventLoopProxy<UserEvent>);

impl Waker {
    /// Sends `event` to the UI thread. `false` once the event loop is gone.
    pub fn send(&self, event: UserEvent) -> bool {
        self.0.send_event(event).is_ok()
    }
}

/// The running event loop, lent to the app while it handles an event.
pub(super) struct Loop<'a>(&'a ActiveEventLoop);

impl Loop<'_> {
    /// Ends the loop once the events queued now have been delivered.
    pub(super) fn exit(&self) {
        self.0.exit();
    }

    /// winit never asks the app whether to end, so it holds no end to answer (ADR 0084).
    pub(super) fn answer_quit(&self, _yes: bool) {}

    /// Whether [`Loop::exit`] was called.
    pub(super) fn exiting(&self) -> bool {
        self.0.exiting()
    }

    /// From now on, the loop sleeps while no event comes. This only says how it waits; it
    /// never blocks.
    pub(super) fn sleep_when_idle(&self) {
        self.0.set_control_flow(ControlFlow::Wait);
    }

    /// From now on, the loop also wakes when `deadline` passes, with no event.
    pub(super) fn wake_at(&self, deadline: Instant) {
        self.0.set_control_flow(ControlFlow::WaitUntil(deadline));
    }

    /// Opens the window.
    pub(super) fn open_window(&self) -> Window {
        let attributes = winit::window::Window::default_attributes().with_title("oso Editor");
        Window {
            window: Arc::new(self.0.create_window(attributes).unwrap()),
            pointer: Cell::new(None),
        }
    }
}

/// The OS window, and where the pointer last was over it, which winit's wheel events do not
/// say (ADR 0112). Sizes and places are physical pixels.
pub(super) struct Window {
    window: Arc<winit::window::Window>,
    pointer: Cell<Option<Position>>,
}

impl Window {
    /// Sets up the renderer for this window, `width` x `height` pixels, drawing text
    /// with `text_system`.
    pub(super) fn start_renderer(
        &self,
        width: u32,
        height: u32,
        text_system: Box<dyn TextSystem>,
    ) -> RenderState {
        pollster::block_on(RenderState::new(
            self.window.clone(),
            width,
            height,
            text_system,
        ))
    }

    /// The drawable area's size.
    pub(super) fn inner_size(&self) -> (u32, u32) {
        let size = self.window.inner_size();
        (size.width, size.height)
    }

    pub(super) fn scale_factor(&self) -> f64 {
        self.window.scale_factor()
    }

    pub(super) fn request_redraw(&self) {
        self.window.request_redraw();
    }

    /// A frame skipped because the window was hidden: winit's notice that it is visible again
    /// brings it, as the app asks for a frame then.
    pub(super) fn redraw_when_visible(&self) {}

    pub(super) fn set_visible(&self, visible: bool) {
        self.window.set_visible(visible);
    }

    /// Asks for the person's attention, without taking the keyboard from another app.
    pub(super) fn request_attention(&self) {
        self.window
            .request_user_attention(Some(UserAttentionType::Informational));
    }

    /// Where the window's frame is on the screen, if the system says. For the restart
    /// handoff.
    #[cfg(feature = "dev")]
    pub(super) fn outer_position(&self) -> Option<(i32, i32)> {
        let position = self.window.outer_position().ok()?;
        Some((position.x, position.y))
    }

    #[cfg(feature = "dev")]
    pub(super) fn set_outer_position(&self, x: i32, y: i32) {
        self.window.set_outer_position(PhysicalPosition::new(x, y));
    }

    /// Asks for a drawable area of `width` x `height`. The size it came to if the system
    /// applied it at once, as Wayland does, sending no `Resized` for it; `None` if a
    /// `Resized` will say later.
    #[cfg(feature = "dev")]
    pub(super) fn request_inner_size(&self, width: u32, height: u32) -> Option<(u32, u32)> {
        let size = self
            .window
            .request_inner_size(PhysicalSize::new(width, height))?;
        Some((size.width, size.height))
    }

    pub(super) fn set_ime_allowed(&self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    /// Whether the app wants secure input. Only oso's own window layer gives it (ADR 0073).
    pub(super) fn set_secure_input(&self, _wanted: bool) {}

    /// Whether the app wants the input source to type ASCII. Only oso's own window layer
    /// chooses one (ADR 0106).
    pub(super) fn set_ascii_input(&self, _wanted: bool) {}

    /// Secure input is not this layer's (ADR 0073), so the dev socket reports none.
    #[cfg(feature = "dev")]
    pub(super) fn secure_input(&self) -> Option<(bool, bool, bool)> {
        None
    }

    /// winit draws a live resize its own way, and counts nothing.
    #[cfg(feature = "dev")]
    pub(super) fn live_resize(&self) -> Option<(u32, u32)> {
        None
    }

    /// winit keeps the input context to itself, so the dev socket reports none.
    #[cfg(feature = "dev")]
    pub(super) fn input_context(&self) -> Option<(bool, bool)> {
        None
    }

    /// Drops the composition the input method holds, with no commit. winit has no call for
    /// it; it drops the composition when the IME goes off, which follows (ADR 0069).
    pub(super) fn discard_composition(&self) {}

    /// Tells the input method where the cursor's cell is, for its candidate window.
    pub(super) fn set_ime_cursor_area(&self, area: PixelRect) {
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(f64::from(area.x), f64::from(area.y)),
            PhysicalSize::new(f64::from(area.width), f64::from(area.height)),
        );
    }

    /// winit gives the input method one rectangle, the cursor area's, so where a composition
    /// is drawn goes nowhere (ADR 0077).
    pub(super) fn set_ime_preedit(&self, _laid_out: Option<(&str, PixelRect, &[f32])>) {}

    /// winit gives the input method no text of a buffer (ADR 0070).
    pub(super) fn set_ime_excerpt(&self, _excerpt: &mut String, _cursor: Option<usize>) {}

    /// winit holds no excerpt, so the dev socket reports none.
    #[cfg(feature = "dev")]
    pub(super) fn ime_excerpt_extent(&self) -> Option<(usize, usize)> {
        None
    }
}

impl ApplicationHandler<UserEvent> for App {
    /// Called when the OS signals that the window is ready to be created (or resumed).
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        self.open_window(&Loop(event_loop));
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        self.handle_user_event(&Loop(event_loop), event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.before_waiting(&Loop(event_loop));
    }

    /// The event loop is ending, on every exit. On macOS `applicationWillTerminate` leads
    /// here for a Dock quit or a logout.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.ending();
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let event_loop = Loop(event_loop);
        if let Some(view) = &self.view {
            note_pointer(&view.window.pointer, &event);
        }
        match &event {
            // Keys borrow from the event, so they are built here, and only for a real
            // press: releases, lone modifiers' and made-up presses go no further than
            // `Other`, as before.
            WindowEvent::KeyboardInput {
                event,
                is_synthetic,
                ..
            } if is_real_press(event.state == ElementState::Pressed, *is_synthetic) => {
                let base = key_without_modifiers(event);
                let input = KeyInput {
                    key: key(&event.logical_key),
                    base: key(&base),
                    repeat: event.repeat,
                };
                self.handle_window_event(&event_loop, Event::Key(input));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let at = self
                    .view
                    .as_ref()
                    .and_then(|view| view.window.pointer.get());
                let delta = wheel(*delta);
                self.handle_window_event(&event_loop, Event::Wheel { delta, at });
            }
            _ => self.handle_window_event(&event_loop, translate(event)),
        }
    }
}

/// Keeps in `pointer` where winit last saw the pointer over the window, and none once it
/// left, for the wheel to scroll the window under it (ADR 0112).
fn note_pointer(pointer: &Cell<Option<Position>>, event: &WindowEvent) {
    match event {
        WindowEvent::CursorMoved { position, .. } => pointer.set(Some(Position {
            x: position.x,
            y: position.y,
        })),
        WindowEvent::CursorLeft { .. } => pointer.set(None),
        _ => {}
    }
}

/// Whether a key event is a press the user made now: `pressed`, and not `synthetic`. On
/// X11 and Windows, winit generates presses for keys already held when the window gains
/// focus, and one of those would run a command or type text nobody asked for. Only these
/// become a [`KeyInput`].
fn is_real_press(pressed: bool, synthetic: bool) -> bool {
    pressed && !synthetic
}

/// winit's window event as oso's. A real key press, which borrows from the event, is built
/// in `window_event`; any other key event is `Other`.
fn translate(event: WindowEvent) -> Event<'static> {
    match event {
        WindowEvent::CloseRequested => Event::CloseRequested,
        WindowEvent::Resized(size) => Event::Resized {
            width: size.width,
            height: size.height,
        },
        WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
            Event::ScaleFactorChanged(scale_factor)
        }
        WindowEvent::Occluded(occluded) => Event::Occluded(occluded),
        WindowEvent::Focused(true) => Event::Focused,
        WindowEvent::RedrawRequested => Event::RedrawRequested,
        WindowEvent::ModifiersChanged(modifiers) => {
            Event::ModifiersChanged(held(modifiers.state()))
        }
        WindowEvent::Ime(ime) => translate_ime(ime).map_or(Event::Other, Event::Ime),
        WindowEvent::KeyboardInput { .. } => Event::Other,
        _ => Event::Other,
    }
}

/// winit's wheel as oso's.
fn wheel(delta: MouseScrollDelta) -> Wheel {
    match delta {
        MouseScrollDelta::LineDelta(x, y) => Wheel::Lines { x, y },
        MouseScrollDelta::PixelDelta(position) => Wheel::Pixels {
            x: position.x,
            y: position.y,
        },
    }
}

/// winit's report of the input method as oso's. `Enabled` says nothing oso needs: it comes
/// at the first preedit after a start or a `Disabled`.
fn translate_ime(ime: Ime) -> Option<ImeEvent> {
    match ime {
        Ime::Enabled => None,
        Ime::Preedit(text, range) => Some(ImeEvent::Preedit(text, range.map(|(s, e)| s..e))),
        Ime::Commit(text) => Some(ImeEvent::Commit(text)),
        Ime::Disabled => Some(ImeEvent::Disabled),
    }
}

/// The modifiers winit reports as held: on macOS its Super is Command and its Alt Option.
fn held(state: ModifiersState) -> Held {
    [
        (ModifiersState::SHIFT, Held::SHIFT),
        (ModifiersState::CONTROL, Held::CONTROL),
        (ModifiersState::ALT, Held::ALT),
        (ModifiersState::SUPER, Held::LOGO),
    ]
    .into_iter()
    .filter(|(winit, _)| state.contains(*winit))
    .fold(Held::NONE, |all, (_, oso)| all | oso)
}

#[cfg(any(target_os = "windows", target_os = "linux"))]
fn key_without_modifiers(event: &winit::event::KeyEvent) -> keyboard::Key {
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    event.key_without_modifiers()
}

#[cfg(not(any(target_os = "windows", target_os = "linux")))]
fn key_without_modifiers(event: &winit::event::KeyEvent) -> keyboard::Key {
    event.logical_key.clone()
}

/// winit's key as oso's.
fn key(key: &keyboard::Key) -> SystemKey<'_> {
    use keyboard::NamedKey as N;
    match key {
        keyboard::Key::Named(named) => SystemKey::Named(match named {
            N::Enter => SystemNamed::Enter,
            N::Tab => SystemNamed::Tab,
            N::Escape => SystemNamed::Escape,
            N::Backspace => SystemNamed::Backspace,
            N::Delete => SystemNamed::Delete,
            N::Insert => SystemNamed::Insert,
            N::ArrowLeft => SystemNamed::Left,
            N::ArrowRight => SystemNamed::Right,
            N::ArrowUp => SystemNamed::Up,
            N::ArrowDown => SystemNamed::Down,
            N::Home => SystemNamed::Home,
            N::End => SystemNamed::End,
            N::PageUp => SystemNamed::PageUp,
            N::PageDown => SystemNamed::PageDown,
            N::Space => SystemNamed::Space,
            other => function_number(*other).map_or(SystemNamed::Other, SystemNamed::Function),
        }),
        keyboard::Key::Character(text) => SystemKey::Character(text.as_str()),
        keyboard::Key::Dead(_) => SystemKey::Dead,
        keyboard::Key::Unidentified(_) => SystemKey::Unidentified,
    }
}

/// The number of a function key, such as 12 for F12.
fn function_number(key: keyboard::NamedKey) -> Option<u8> {
    use keyboard::NamedKey as N;
    const KEYS: [keyboard::NamedKey; 35] = [
        N::F1,
        N::F2,
        N::F3,
        N::F4,
        N::F5,
        N::F6,
        N::F7,
        N::F8,
        N::F9,
        N::F10,
        N::F11,
        N::F12,
        N::F13,
        N::F14,
        N::F15,
        N::F16,
        N::F17,
        N::F18,
        N::F19,
        N::F20,
        N::F21,
        N::F22,
        N::F23,
        N::F24,
        N::F25,
        N::F26,
        N::F27,
        N::F28,
        N::F29,
        N::F30,
        N::F31,
        N::F32,
        N::F33,
        N::F34,
        N::F35,
    ];
    let index = KEYS.iter().position(|k| *k == key)?;
    u8::try_from(index + 1).ok()
}

#[cfg(test)]
mod tests {
    use super::{held, is_real_press, key, note_pointer, translate, translate_ime, wheel};
    use oso_platform::window::{Event, Held, ImeEvent, Position, SystemKey, SystemNamed, Wheel};
    use std::cell::Cell;
    use winit::dpi::{PhysicalPosition, PhysicalSize};
    use winit::event::DeviceId;
    use winit::event::{Ime, MouseScrollDelta, WindowEvent};
    use winit::keyboard::{self, ModifiersState, NamedKey, NativeKey};

    #[test]
    fn only_a_real_press_counts() {
        assert!(is_real_press(true, false));
        assert!(!is_real_press(true, true), "held at focus");
        assert!(!is_real_press(false, false));
        assert!(!is_real_press(false, true));
    }

    #[test]
    fn keys_keep_their_names_and_text() {
        let unnamed = |n| key(&keyboard::Key::Named(n)) == SystemKey::Named(SystemNamed::Other);
        let names = |n, oso| key(&keyboard::Key::Named(n)) == SystemKey::Named(oso);
        assert!(names(NamedKey::Enter, SystemNamed::Enter));
        assert!(names(NamedKey::ArrowLeft, SystemNamed::Left));
        assert!(names(NamedKey::F1, SystemNamed::Function(1)));
        assert!(names(NamedKey::F35, SystemNamed::Function(35)));
        assert!(unnamed(NamedKey::Shift), "a lone modifier is no key");
        let character = keyboard::Key::Character("لا".into());
        assert!(key(&character) == SystemKey::Character("لا"));
        assert!(key(&keyboard::Key::Dead(Some('`'))) == SystemKey::Dead);
        let unknown = keyboard::Key::Unidentified(NativeKey::Unidentified);
        assert!(key(&unknown) == SystemKey::Unidentified);
    }

    #[test]
    fn modifiers_keep_winits_names() {
        assert_eq!(held(ModifiersState::empty()), Held::NONE);
        assert_eq!(
            held(ModifiersState::SUPER | ModifiersState::SHIFT),
            Held::LOGO | Held::SHIFT
        );
        assert_eq!(
            held(ModifiersState::ALT | ModifiersState::CONTROL),
            Held::ALT | Held::CONTROL
        );
    }

    #[test]
    fn the_wheel_is_where_the_pointer_last_moved_until_it_leaves() {
        let pointer = Cell::new(None);
        let device_id = DeviceId::dummy();
        let moved = |x, y| WindowEvent::CursorMoved {
            device_id,
            position: PhysicalPosition::new(x, y),
        };
        note_pointer(&pointer, &moved(12.5, 40.0));
        note_pointer(&pointer, &WindowEvent::Focused(true));
        assert_eq!(pointer.get(), Some(Position { x: 12.5, y: 40.0 }));
        note_pointer(&pointer, &moved(3.0, 4.0));
        assert_eq!(pointer.get(), Some(Position { x: 3.0, y: 4.0 }));
        note_pointer(&pointer, &WindowEvent::CursorLeft { device_id });
        assert_eq!(pointer.get(), None, "left the window");
    }

    #[test]
    fn window_events_carry_their_values() {
        assert!(matches!(
            translate(WindowEvent::Resized(PhysicalSize::new(640, 480))),
            Event::Resized {
                width: 640,
                height: 480
            }
        ));
        assert_eq!(
            wheel(MouseScrollDelta::LineDelta(1.0, -2.0)),
            Wheel::Lines { x: 1.0, y: -2.0 }
        );
        let pixels = MouseScrollDelta::PixelDelta(PhysicalPosition::new(3.0, 4.0));
        assert_eq!(wheel(pixels), Wheel::Pixels { x: 3.0, y: 4.0 });
        assert!(matches!(
            translate(WindowEvent::Focused(true)),
            Event::Focused
        ));
        assert!(matches!(
            translate(WindowEvent::Focused(false)),
            Event::Other
        ));
    }

    #[test]
    fn the_input_method_reports_in_bytes_and_enabled_says_nothing() {
        assert!(translate_ime(Ime::Enabled).is_none());
        let Some(ImeEvent::Preedit(text, range)) =
            translate_ime(Ime::Preedit("かな".into(), Some((3, 6))))
        else {
            panic!("a preedit");
        };
        assert_eq!((text.as_str(), range), ("かな", Some(3..6)));
        assert!(matches!(
            translate_ime(Ime::Commit("仮名".into())),
            Some(ImeEvent::Commit(text)) if text == "仮名"
        ));
        assert!(matches!(
            translate_ime(Ime::Disabled),
            Some(ImeEvent::Disabled)
        ));
        assert!(matches!(
            translate(WindowEvent::Ime(Ime::Enabled)),
            Event::Other
        ));
    }
}
