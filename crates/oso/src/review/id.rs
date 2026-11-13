//! Object ids as git writes them, and the line breaks after its short answers (ADR 0122).
//!
//! Must not depend on: `git`, `batch` or anything that runs git; this reads bytes.

use std::ffi::OsStr;

/// An object id, as hexadecimal digits: 40 for SHA-1, 64 for SHA-256.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct Id(Box<str>);

impl Id {
    /// The id `bytes` spell, if they are one.
    pub(crate) fn parse(bytes: &[u8]) -> Option<Id> {
        let hex = matches!(bytes.len(), 40 | 64)
            && bytes
                .iter()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte));
        hex.then(|| Id(String::from_utf8_lossy(bytes).into()))
    }

    /// Whether it is all zeros, as git writes for no object.
    pub(crate) fn is_zero(&self) -> bool {
        self.0.bytes().all(|byte| byte == b'0')
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn as_os_str(&self) -> &OsStr {
        OsStr::new(&*self.0)
    }
}

/// `bytes` without the line break that ends them.
pub(crate) fn trim_line(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Id, trim_line};

    #[test]
    fn only_full_lowercase_hex_ids_are_ids() {
        assert!(Id::parse(&[b'a'; 40]).is_some());
        assert!(Id::parse(&[b'0'; 64]).is_some_and(|id| id.is_zero()));
        for bad in [&b"abc"[..], &[b'A'; 40], &[b'g'; 40], &[b'a'; 41]] {
            assert!(Id::parse(bad).is_none(), "{bad:?}");
        }
        assert_eq!(trim_line(b"x\r\n"), b"x");
    }
}
