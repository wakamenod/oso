//! A hosted session's identity (ADR 0121, Q1): the id oso makes itself, a version 4 UUID, and
//! the name it passes with `--name`.
//!
//! Must not depend on: threads, the UI or `app`; the caller reads the random bytes, on a thread
//! of its own, and gives them here.
//!
//! - The id is made from 16 random bytes from the system, read by [`random_bytes`] off the UI
//!   thread.
//! - A name is the project's directory name, cleaned to letters, digits, `.`, `_` and `-`, at
//!   most [`MAX_NAME`] bytes, then `-2`, `-3` against the names already taken in that directory.
//!   A name the person types is cleaned the same way.

use std::fmt;
use std::io;

/// How a session started, for who may start sessions without asking (ADR 0135).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// By the person's key, or before oso kept how sessions start.
    Person,
    /// By an agent's tool, on the person's `y`.
    Answered,
    /// By an agent's tool, with no question, as the person's configuration allows.
    Unasked,
    /// Not known: a record line another session started that holds no origin, or one oso does
    /// not know. It may not start sessions without asking.
    Unknown,
}

impl Origin {
    /// Whether the person started it, with a key or a `y`.
    pub fn by_the_person(self) -> bool {
        matches!(self, Self::Person | Self::Answered)
    }

    /// Its name in the record.
    pub fn name(self) -> &'static str {
        match self {
            Self::Person => "person",
            Self::Answered => "answered",
            Self::Unasked => "unasked",
            Self::Unknown => "unknown",
        }
    }

    /// The origin a record or a dev restart's state gives by `name`. None from before oso kept
    /// origins was the person's, unless another session started it: that one, or one stripped
    /// of its origin, may not start sessions unasked.
    pub fn read(name: Option<&str>, has_parent: bool) -> Self {
        match name {
            Some(name) => Self::named(name),
            None if has_parent => Self::Unknown,
            None => Self::Person,
        }
    }

    /// The origin a record's name gives; one oso does not know is [`Origin::Unknown`].
    pub fn named(name: &str) -> Self {
        match name {
            "person" => Self::Person,
            "answered" => Self::Answered,
            "unasked" => Self::Unasked,
            _ => Self::Unknown,
        }
    }
}

/// The longest name, in bytes, before a `-N` that tells it from another.
pub const MAX_NAME: usize = 48;

/// The name of a session whose directory gives none, such as `/`.
const FALLBACK_NAME: &str = "session";

/// A session's id: a version 4 UUID, held as its lower-case text, `8-4-4-4-12` hex digits.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// The id made from `bytes`, with the version and variant bits set as RFC 9562 has them.
    pub fn from_random(mut bytes: [u8; 16]) -> Self {
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let mut text = String::with_capacity(36);
        for (i, byte) in bytes.iter().enumerate() {
            if matches!(i, 4 | 6 | 8 | 10) {
                text.push('-');
            }
            text.push_str(&format!("{byte:02x}"));
        }
        Self(text)
    }

    /// `text` as an id, if it is a UUID's text in lower case: what oso writes, and what the
    /// bridge and the registry must give back. Anything else is no id of a session oso knows.
    pub fn parse(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 36 {
            return None;
        }
        let well_formed = bytes.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_digit() || (b'a'..=b'f').contains(&b),
        });
        well_formed.then(|| Self(text.to_owned()))
    }

    /// Its text, as `--session-id` takes it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SessionId({})", self.0)
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// 16 random bytes from the system, for an id (`oso_platform::random`). It reads a file, so
/// never on the UI thread.
pub fn random_bytes() -> io::Result<[u8; 16]> {
    let mut bytes = [0; 16];
    oso_platform::random::fill(&mut bytes)?;
    Ok(bytes)
}

/// `raw` cleaned to a name: each character that is not an ASCII letter or digit, `.`, `_` or
/// `-` becomes `-`, runs of `-` become one, and `-` and `.` at either end go, so that no name
/// starts like an option. At most [`MAX_NAME`] bytes, and `session` if nothing is left.
pub fn clean_name(raw: &str) -> String {
    cleaned(raw).unwrap_or_else(|| FALLBACK_NAME.to_owned())
}

/// `raw` cleaned as [`clean_name`] does, or `None` if nothing is left of it.
pub fn cleaned(raw: &str) -> Option<String> {
    let mut name = String::with_capacity(raw.len().min(MAX_NAME));
    for c in raw.chars() {
        let c = if c.is_ascii_alphanumeric() || c == '.' || c == '_' {
            c
        } else {
            '-'
        };
        if c == '-' && name.ends_with('-') {
            continue;
        }
        if name.len() == MAX_NAME {
            break;
        }
        name.push(c);
    }
    let name = name.trim_matches(['-', '.']);
    (!name.is_empty()).then(|| name.to_owned())
}

/// `base`, cleaned, or with `-2`, `-3` and on after it, the first that `taken` does not hold.
pub fn unique_name<'a>(base: &str, taken: impl Iterator<Item = &'a str> + Clone) -> String {
    let base = clean_name(base);
    let is_taken = |name: &str| taken.clone().any(|other| other == name);
    if !is_taken(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !is_taken(name))
        .unwrap_or(base)
}

#[cfg(test)]
mod tests {
    use super::{MAX_NAME, SessionId, clean_name, unique_name};

    #[test]
    fn an_id_is_a_version_4_uuid() {
        let id = SessionId::from_random([0xff; 16]);
        assert_eq!(id.as_str(), "ffffffff-ffff-4fff-bfff-ffffffffffff");
        let id = SessionId::from_random([0; 16]);
        assert_eq!(id.as_str(), "00000000-0000-4000-8000-000000000000");
        assert_eq!(SessionId::parse(id.as_str()), Some(id));
    }

    #[test]
    fn only_a_lower_case_uuid_parses() {
        let good = "0123abcd-4567-4890-abcd-ef0123456789";
        assert!(SessionId::parse(good).is_some());
        for bad in [
            "",
            "0123ABCD-4567-4890-abcd-ef0123456789",
            "0123abcd-4567-4890-abcd-ef012345678",
            "0123abcd-4567-4890-abcd-ef01234567890",
            "0123abcd_4567-4890-abcd-ef0123456789",
            "0123abcd-4567-4890-abcd-ef012345678g",
            "-123abcd-4567-4890-abcd-ef0123456789",
            "0123abcd-4567-4890-abcd-ef012345678\n",
        ] {
            assert_eq!(SessionId::parse(bad), None, "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn two_ids_differ() {
        let a = SessionId::from_random(super::random_bytes().unwrap());
        let b = SessionId::from_random(super::random_bytes().unwrap());
        assert_ne!(a, b);
    }

    #[test]
    fn a_name_is_cleaned() {
        assert_eq!(clean_name("oso"), "oso");
        assert_eq!(clean_name("my project"), "my-project");
        assert_eq!(clean_name("--rm -rf"), "rm-rf");
        assert_eq!(clean_name("日本語"), "session");
        assert_eq!(clean_name("a\u{202e}b\nc"), "a-b-c");
        assert_eq!(clean_name("..hidden."), "hidden");
        assert_eq!(clean_name("v1.2_x"), "v1.2_x");
        assert_eq!(clean_name(""), "session");
        assert_eq!(clean_name(&"x".repeat(100)).len(), MAX_NAME);
    }

    #[test]
    fn a_name_taken_gets_a_number() {
        let taken = ["oso", "oso-2", "api"];
        assert_eq!(unique_name("oso", taken.iter().copied()), "oso-3");
        assert_eq!(unique_name("api", taken.iter().copied()), "api-2");
        assert_eq!(unique_name("new", taken.iter().copied()), "new");
        assert_eq!(unique_name("my oso", taken.iter().copied()), "my-oso");
    }
}
