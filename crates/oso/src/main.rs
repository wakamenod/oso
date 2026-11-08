//! The oso binary and tip crate: commands, default keymaps and the glue
//! that wires `oso-core`, `oso-render` and `oso-platform` to the window
//! (ADR 0004).
//!
//! Must not depend on: wgpu or OS bindings directly; they stay in the library
//! crates (ADR 0038).

mod app;
mod bindings;
mod channel;
mod commands;
#[cfg(feature = "dev")]
mod dev;
mod dispatch;
mod emergency;
mod keys;
mod loader;
mod saver;
mod text;

use std::ffi::OsString;
use std::path::PathBuf;

use app::App;
#[cfg(feature = "dev")]
use app::UserEvent;
use emergency::Emergency;

const USAGE: &str = "usage: oso [FILE]";

fn main() {
    let started = std::time::Instant::now();
    // First of all, before any text can be here, also in `oso ctl`. A dump would hold every
    // buffer (ADR 0028).
    let core_dumps = oso_platform::privacy::disable_core_dumps();
    // The agent channel's bridge, in every build; it opens no window (ADR 0085).
    if let Some(status) = channel::mcp::main_if_requested() {
        std::process::exit(status);
    }
    #[cfg(feature = "dev")]
    if let Some(status) = dev::ctl::main_if_requested().or_else(dev::supervisor::main_if_requested)
    {
        std::process::exit(status);
    }
    // Without the dev tools, `oso ctl` and `oso dev` must not open the editor.
    #[cfg(not(feature = "dev"))]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "ctl" || arg == "dev")
    {
        refuse_dev_tool();
    }
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    #[cfg(feature = "dev")]
    let (args, emergency_directory) = dev::take_emergency_dir(args);
    #[cfg(not(feature = "dev"))]
    let emergency_directory = None;
    #[cfg(feature = "dev")]
    let (args, clipboard) = dev::take_clipboard_option(args);
    // A release build always uses the system's (ADR 0074).
    #[cfg(not(feature = "dev"))]
    let clipboard = oso_platform::clipboard::Source::System;
    let file = match parse_args(args) {
        Ok(Invocation::Help) => exit_with_usage(None),
        Ok(Invocation::Open(file)) => file,
        Err(message) => exit_with_usage(Some(&message)),
    };

    let log_gate = app::LogGate::install();
    if let Err(e) = core_dumps {
        // Only this level reaches a terminal with no `RUST_LOG`.
        log::error!("cannot turn core dumps off: {e}");
    }
    let emergency = Emergency::new(emergency_directory.or_else(emergency::default_directory));
    emergency.install_hook();
    app::run(started, |waker| {
        #[cfg(feature = "dev")]
        let dev = {
            let waker = waker.clone();
            dev::start(move |request| waker.send(UserEvent::Control(request)))
        };
        let mut app = App::new(waker, started, emergency, clipboard, log_gate);
        #[cfg(feature = "dev")]
        app.set_dev(dev);
        app.check_emergency_directory();
        // Loading starts before the window exists, so the file is read while
        // the GPU starts up.
        if let Some(file) = file {
            app.open(file);
        }
        app
    });
}

/// Exits with an error: `oso ctl` and `oso dev` need a dev build.
#[cfg(not(feature = "dev"))]
#[expect(
    clippy::disallowed_macros,
    reason = "a command-line error, printed before logging starts"
)]
fn refuse_dev_tool() -> ! {
    eprintln!("oso ctl and oso dev need a dev build (cargo run -p oso --features dev)");
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
