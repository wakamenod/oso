//! Opening a web address with the system's opener (ADR 0127, step 5a, Q5; ADR 0120's rule for
//! links): `open` on macOS, `xdg-open` from `PATH` elsewhere on Unix, `explorer.exe` on Windows.
//!
//! Must not depend on: what the address is of; the caller decides which may open, and shows it
//! first.
//!
//! - Only an `https` address of plain characters opens: nothing a shell or the opener could
//!   take as an option, a quote or a second argument.
//! - The opener is spawned as every child is, with the variables a desktop's opener needs on
//!   top of the base set, its input closed and its output dropped. The caller waits for it off
//!   the UI thread.

use std::ffi::OsString;
use std::io;

use super::{Child, Executable, Sink, Source, Spawn, spawn};

/// The longest address opened.
const MAX_LINK: usize = 2048;

/// What `xdg-open` needs to find the person's desktop and browser.
#[cfg(all(unix, not(target_os = "macos")))]
const DESKTOP: [&str; 8] = [
    "DISPLAY",
    "WAYLAND_DISPLAY",
    "XDG_RUNTIME_DIR",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_CURRENT_DESKTOP",
    "XDG_DATA_DIRS",
    "XDG_CONFIG_DIRS",
    "BROWSER",
];

/// Starts the system's opener on `url`, an `https` address.
///
/// # Errors
///
/// If `url` is not one that may open, or the opener cannot be found or started.
pub fn open_link(url: &str) -> io::Result<Child> {
    if !may_open(url) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "only an https address of plain characters opens",
        ));
    }
    let (executable, inherit) = opener()?;
    let args = [OsString::from(url)];
    spawn(&Spawn {
        executable: &executable,
        args: &args,
        cwd: None,
        project: None,
        inherit,
        stdin: Source::Closed,
        stdout: Sink::Null,
        stderr: Sink::Null,
        own_process_group: false,
        own_session: false,
        set: &[],
        terminal: None,
        pass_fd: None,
    })
}

/// Whether `url` may open: `https://`, a host, at most [`MAX_LINK`] bytes, and only the
/// characters an address is written with, but no quote, space or backslash.
fn may_open(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let plain = |b: u8| b.is_ascii_alphanumeric() || b"-._~:/?#@!$&()*+,;=%".contains(&b);
    url.len() <= MAX_LINK
        && rest
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && url.bytes().all(plain)
}

#[cfg(target_os = "macos")]
fn opener() -> io::Result<(Executable, &'static [&'static str])> {
    Ok((Executable::absolute("/usr/bin/open")?, &[]))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn opener() -> io::Result<(Executable, &'static [&'static str])> {
    Ok((Executable::from_path("xdg-open", None)?, &DESKTOP))
}

#[cfg(not(unix))]
fn opener() -> io::Result<(Executable, &'static [&'static str])> {
    Ok((Executable::from_path("explorer.exe", None)?, &[]))
}

#[cfg(test)]
mod tests {
    use super::may_open;

    #[test]
    fn only_a_plain_https_address_may_open() {
        assert!(may_open("https://github.com/o/r/pull/12"));
        assert!(may_open("https://ghe.example.com/o/r/pull/3#discussion_r1"));
        for url in [
            "http://github.com/o/r/pull/12",
            "https://",
            "https://-x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://github.com/o/r pull",
            "https://github.com/o/r\"pull",
            "https://github.com/o/r'pull",
            "https://github.com/o/r\\pull",
            "https://github.com/\no",
            "https://github.com/ö",
            " https://github.com",
        ] {
            assert!(!may_open(url), "{url:?}");
        }
        assert!(!may_open(&format!("https://x/{}", "a".repeat(2048))));
    }
}
