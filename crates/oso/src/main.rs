//! The oso binary and tip crate: commands, default keymaps and the glue
//! that wires `oso-core`, `oso-render` and `oso-platform` to the window
//! (ADR 0004).
//!
//! Must not depend on: wgpu or OS bindings directly; they stay in the library
//! crates (ADR 0038).

mod app;
mod bindings;
mod commands;
#[cfg(feature = "dev")]
mod dev;
mod dispatch;
mod keys;
mod loader;
mod text;

use std::ffi::OsString;
use std::path::PathBuf;

use app::{App, UserEvent};
use winit::event_loop::{ControlFlow, EventLoop};

const USAGE: &str = "usage: oso [FILE]";

fn main() {
    let started = std::time::Instant::now();
    #[cfg(feature = "dev")]
    if let Some(status) = dev::ctl::main_if_requested() {
        std::process::exit(status);
    }
    // Without the control socket, `oso ctl` must not open the editor.
    #[cfg(not(feature = "dev"))]
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "ctl") {
        refuse_ctl();
    }
    let file = match parse_args(std::env::args_os().skip(1)) {
        Ok(Invocation::Help) => exit_with_usage(None),
        Ok(Invocation::Open(file)) => file,
        Err(message) => exit_with_usage(Some(&message)),
    };

    env_logger::init();
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // Building AppKit's default menu delays the first frame by about 55 ms,
    // and its Command shortcuts would clash with Command = Meta (ADR 0046).
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS;
        builder.with_default_menu(false);
    }
    let event_loop = builder.build().unwrap();
    // Sleep until an event arrives; frames are drawn only on demand.
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    log::debug!("event loop built {:?} after start", started.elapsed());

    #[cfg(feature = "dev")]
    let dev = {
        let proxy = proxy.clone();
        dev::start(move |request| proxy.send_event(UserEvent::Control(request)).is_ok())
    };
    let mut app = App::new(proxy, started);
    #[cfg(feature = "dev")]
    app.set_dev(dev);
    // Loading starts before the window exists, so the file is read while
    // the GPU starts up.
    if let Some(file) = file {
        // Logged; the window opens with no file.
        let _ = app.open(file);
    }
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

/// Prints the usage, after an error if there is one, and exits: with
/// status 0 for `--help`, 2 for an error.
#[expect(
    clippy::disallowed_macros,
    reason = "command-line output, printed before logging starts"
)]
fn exit_with_usage(error: Option<&str>) -> ! {
    match error {
        None => {
            println!("{USAGE}");
            std::process::exit(0);
        }
        Some(message) => {
            eprintln!("oso: {message}\n{USAGE}");
            std::process::exit(2);
        }
    }
}

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    Help,
    /// Open the editor, visiting a file if one is given.
    Open(Option<PathBuf>),
}

/// Parses `oso [FILE]`. `--` ends the options, so a file named `-x` can be
/// opened as `oso -- -x`.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut file = None;
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && (arg == "-h" || arg == "--help") {
            return Ok(Invocation::Help);
        } else if options && arg.to_string_lossy().starts_with('-') && arg != "-" {
            return Err(format!("unknown option {}", arg.to_string_lossy()));
        } else if file.is_some() {
            return Err("only one file can be opened".into());
        } else {
            file = Some(PathBuf::from(arg));
        }
    }
    Ok(Invocation::Open(file))
}

#[cfg(test)]
mod tests {
    use super::{Invocation, parse_args};
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Result<Invocation, String> {
        parse_args(args.iter().map(Into::into))
    }

    #[test]
    fn parses_an_optional_file() {
        assert_eq!(parse(&[]), Ok(Invocation::Open(None)));
        assert_eq!(
            parse(&["notes.md"]),
            Ok(Invocation::Open(Some(PathBuf::from("notes.md"))))
        );
        assert_eq!(
            parse(&["--", "-x"]),
            Ok(Invocation::Open(Some(PathBuf::from("-x"))))
        );
        assert_eq!(parse(&["--help"]), Ok(Invocation::Help));
    }

    #[test]
    fn rejects_options_and_extra_files() {
        assert_eq!(parse(&["-q"]), Err("unknown option -q".into()));
        assert_eq!(
            parse(&["a", "b"]),
            Err("only one file can be opened".into())
        );
    }
}
