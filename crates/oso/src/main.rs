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
mod config;
#[cfg(feature = "dev")]
mod dev;
mod dispatch;
mod emergency;
mod keys;
mod lister;
mod loader;
mod project;
mod saver;
mod text;
mod walk;

use std::ffi::OsString;
use std::path::PathBuf;

use app::App;
#[cfg(feature = "dev")]
use app::UserEvent;
use emergency::Emergency;

const USAGE: &str = "usage: oso [--no-config] [FILE]";

fn main() {
    let started = std::time::Instant::now();
    // First of all, before any text can be here, also in `oso ctl`. A dump would hold every
    // buffer (ADR 0028).
    let core_dumps = oso_platform::privacy::disable_core_dumps();
    // The agent channel's bridge, in every build; it opens no window (ADR 0085).
    let core_dumps_failed = core_dumps.as_ref().err().map(ToString::to_string);
    if let Some(status) = channel::mcp::main_if_requested(core_dumps_failed) {
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
    #[cfg(feature = "dev")]
    let (args, config_file) = dev::take_config_file(args);
    #[cfg(not(feature = "dev"))]
    let config_file = None;
    let (file, read_config) = match parse_args(args) {
        Ok(Invocation::Help) => exit_with_usage(None),
        Ok(Invocation::Open { file, config }) => (file, config),
        Err(message) => exit_with_usage(Some(&message)),
    };
    let config_source = match (read_config, config_file) {
        (false, _) => config::Source::Nothing,
        (true, Some(file)) => config::Source::File(file),
        (true, None) => config::Source::Home,
    };

    let log_gate = app::LogGate::install();
    if let Err(e) = core_dumps {
        // Only this level reaches a terminal with no `RUST_LOG`.
        log::error!("cannot turn core dumps off: {e}");
    }
    let emergency_directory = emergency_directory.or_else(emergency::default_directory);
    // The theme chosen last is remembered beside the emergency copies (ADR 0089).
    let theme_directory = emergency_directory
        .as_deref()
        .and_then(std::path::Path::parent)
        .map(std::path::Path::to_path_buf);
    // Read while the event loop and the GPU start, and taken as the window opens.
    let theme_read = app::ThemeRead::start(theme_directory.clone());
    let config_reader = config::Reader::start(&config_source);
    let trust_file = config::TrustFile::start(config_source.trust_file());
    let emergency = Emergency::new(emergency_directory);
    emergency.install_hook();
    app::run(started, |waker| {
        #[cfg(feature = "dev")]
        let dev = {
            let waker = waker.clone();
            dev::start(move |request| waker.send(UserEvent::Control(request)))
        };
        let mut app = App::new(
            waker,
            started,
            emergency,
            clipboard,
            log_gate,
            theme_directory,
            theme_read,
        );
        app.set_configuration(&config_source, config_reader);
        app.set_trust_file(trust_file);
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
    /// Open the editor, visiting a file if one is given, with the user configuration file unless
    /// `--no-config` says not to (ADR 0108).
    Open {
        file: Option<PathBuf>,
        config: bool,
    },
}

/// Parses `oso [--no-config] [FILE]`. `--` ends the options, so a file named `-x` can be
/// opened as `oso -- -x`.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, String> {
    let mut file = None;
    let mut config = true;
    let mut options = true;
    for arg in args {
        if options && arg == "--" {
            options = false;
        } else if options && (arg == "-h" || arg == "--help") {
            return Ok(Invocation::Help);
        } else if options && arg == "--no-config" {
            config = false;
        } else if options && arg.to_string_lossy().starts_with('-') && arg != "-" {
            return Err(format!("unknown option {}", arg.to_string_lossy()));
        } else if file.is_some() {
            return Err("only one file can be opened".into());
        } else {
            file = Some(PathBuf::from(arg));
        }
    }
    Ok(Invocation::Open { file, config })
}

#[cfg(test)]
mod tests {
    use super::{Invocation, parse_args};
    use std::path::PathBuf;

    fn parse(args: &[&str]) -> Result<Invocation, String> {
        parse_args(args.iter().map(Into::into))
    }

    fn open(file: Option<&str>, config: bool) -> Result<Invocation, String> {
        Ok(Invocation::Open {
            file: file.map(PathBuf::from),
            config,
        })
    }

    #[test]
    fn parses_an_optional_file() {
        assert_eq!(parse(&[]), open(None, true));
        assert_eq!(parse(&["notes.md"]), open(Some("notes.md"), true));
        assert_eq!(parse(&["--", "-x"]), open(Some("-x"), true));
        assert_eq!(parse(&["--help"]), Ok(Invocation::Help));
    }

    #[test]
    fn no_config_starts_with_the_defaults_in_every_build() {
        assert_eq!(parse(&["--no-config"]), open(None, false));
        assert_eq!(parse(&["--no-config", "a"]), open(Some("a"), false));
        assert_eq!(
            parse(&["--", "--no-config"]),
            open(Some("--no-config"), true)
        );
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
