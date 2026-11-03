//! The oso binary and tip crate: commands, default keymaps and the glue
//! that wires `oso-core`, `oso-render` and `oso-platform` to the window
//! (ADR 0004).

mod app;

use app::App;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    // Sleep until an event arrives; frames are drawn only on demand.
    event_loop.set_control_flow(ControlFlow::Wait);

    let mut app = App::default();
    event_loop.run_app(&mut app).unwrap();
}
