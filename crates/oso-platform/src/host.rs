//! The name of the machine oso runs on, which tells the `file:` URI a terminal's program sends
//! of its directory (OSC 7) for this machine from one naming another, as after `ssh` (ADR 0141).
//!
//! Must not depend on: `clipboard`, `control`, `file_meta`, `notify`, `privacy`, `process`,
//! `random`, `text` or `window`.

/// The machine's name, as `hostname` prints it, if the system gives one that is UTF-8. It asks
/// the kernel and reads no file. Other systems than Unix give none yet.
pub fn name() -> Option<String> {
    backend_name()
}

#[cfg(unix)]
fn backend_name() -> Option<String> {
    // POSIX's HOST_NAME_MAX is at most 255 on the systems oso runs on, and one more is the NUL.
    let mut buffer = [0_u8; 256];
    // SAFETY: `buffer` is valid for writes of `buffer.len()` bytes, the length passed, and
    // `gethostname` writes no more than that into it and keeps no pointer to it.
    let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if result != 0 {
        return None;
    }
    // A name cut short may have no NUL: it is not the machine's, so there is none.
    let end = buffer.iter().position(|&byte| byte == 0)?;
    let name = std::str::from_utf8(&buffer[..end]).ok()?;
    (!name.is_empty()).then(|| name.to_owned())
}

#[cfg(not(unix))]
fn backend_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn the_machine_has_a_name_with_no_nul_or_control() {
        let name = super::name().unwrap();
        assert!(!name.is_empty());
        assert!(!name.chars().any(char::is_control), "{name:?}");
    }
}
