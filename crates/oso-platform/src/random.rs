//! Random bytes from the system, for ids that must not be guessed, such as an agent session's
//! (ADR 0121). It reads `/dev/urandom` on Unix; other systems have none yet.
//!
//! Must not depend on: `clipboard`, `control`, `file_meta`, `privacy`, `process`, `text` or
//! `window`.

use std::io;

/// Fills `bytes` with random bytes from the system. It reads a file, so never on the UI thread.
///
/// # Errors
///
/// If the system's source cannot be opened or read, and `ErrorKind::Unsupported` on a system
/// with none yet.
pub fn fill(bytes: &mut [u8]) -> io::Result<()> {
    backend_fill(bytes)
}

/// `/dev/urandom`, which never blocks once the system has seeded it, and on Linux and macOS
/// is seeded before any user process runs.
#[cfg(unix)]
fn backend_fill(bytes: &mut [u8]) -> io::Result<()> {
    use std::io::Read as _;

    std::fs::File::open("/dev/urandom")?.read_exact(bytes)
}

#[cfg(not(unix))]
fn backend_fill(_: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "no source of random bytes on this system yet",
    ))
}

#[cfg(test)]
mod tests {
    use super::fill;

    #[cfg(unix)]
    #[test]
    fn fills_with_bytes_that_differ_from_call_to_call() {
        let mut first = [0_u8; 32];
        let mut second = [0_u8; 32];
        fill(&mut first).unwrap();
        fill(&mut second).unwrap();
        // 32 zero bytes, or the same 32 twice, has a chance of 2^-256.
        assert_ne!(first, [0; 32]);
        assert_ne!(first, second);
    }

    #[cfg(unix)]
    #[test]
    fn an_empty_slice_is_filled_at_once() {
        fill(&mut []).unwrap();
    }

    #[cfg(not(unix))]
    #[test]
    fn says_it_is_unsupported() {
        let error = fill(&mut [0; 16]).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::Unsupported);
    }
}
