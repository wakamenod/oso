//! The oso binary and tip crate: commands, default keymaps and the glue
//! that wires `oso-core`, `oso-render` and `oso-platform` to the window
//! (ADR 0004).

mod app;
mod bindings;
mod commands;
#[cfg(feature = "dev")]
mod dev;
mod dispatch;
mod keys;
mod text;

use app::{App, UserEvent};
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    #[cfg(feature = "dev")]
    if let Some(status) = dev::ctl::main_if_requested() {
        std::process::exit(status);
    }
    // Without the control socket, `oso ctl` must not open the editor.
    #[cfg(not(feature = "dev"))]
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "ctl") {
        refuse_ctl();
    }

    env_logger::init();
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();
    // Sleep until an event arrives; frames are drawn only on demand.
    event_loop.set_control_flow(ControlFlow::Wait);

    #[cfg(feature = "dev")]
    let mut app = {
        let proxy = event_loop.create_proxy();
        App::with_dev(dev::start(move |request| proxy.send_event(request).is_ok()))
    };
    #[cfg(not(feature = "dev"))]
    let mut app = App::new();
    event_loop.run_app(&mut app).unwrap();
}

/// Exits with an error: `oso ctl` needs the control socket of a dev build.
#[cfg(not(feature = "dev"))]
#[expect(
    clippy::disallowed_macros,
    reason = "a command-line error, printed before logging starts"
)]
fn refuse_ctl() -> ! {
    eprintln!("oso ctl needs a dev build (cargo run -p oso --features dev)");
    std::process::exit(2);
}
