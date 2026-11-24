//! Whether a git remote's address may be sent to claude.ai as a session connects (ADR 0181): not
//! one that holds a password, a user name other than ssh's `git`, or that cannot be read for them.
//!
//! Must not depend on: git, files or the UI; plain functions over an address as git wrote it.
//!
//! - It refuses whatever it cannot read as one of git's plain forms: an address of a remote
//!   helper (`helper::address`), one that holds a second `://`, a query or a fragment, and one
//!   with an `@` past where its user name is, as a password with a `/` leaves one.
//! - What it refuses is never shown: nothing here names the address.

use std::fmt;

/// Why an address is not sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    User,
    Password,
    /// It is none of git's plain forms, so it may hold either.
    Unread,
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::User => "a git remote's address holds a user name",
            Self::Password => "a git remote's address holds a password",
            Self::Unread => "a git remote's address cannot be read for a user name or a password",
        })
    }
}

/// The one user name let through, over ssh: a host's account for everyone.
const GIT: &str = "git";

/// The schemes of ssh, as git takes them.
const SSH: [&str; 3] = ["ssh", "git+ssh", "ssh+git"];

/// Whether the address `bytes`, as git wrote it, may be sent: its text.
///
/// # Errors
///
/// Why not.
pub fn judge(bytes: &[u8]) -> Result<&str, Refused> {
    let address = std::str::from_utf8(bytes).map_err(|_| Refused::Unread)?;
    if address.is_empty()
        || address.contains("::")
        || address.chars().any(|c| c.is_control() || c.is_whitespace())
    {
        return Err(Refused::Unread);
    }
    match address.split_once("://") {
        Some((scheme, rest)) => url(scheme, rest)?,
        None => scp_like(address)?,
    }
    Ok(address)
}

/// An address `scheme://rest`.
fn url(scheme: &str, rest: &str) -> Result<(), Refused> {
    let plain = !scheme.is_empty()
        && (scheme.bytes()).all(|b| b.is_ascii_alphanumeric() || matches!(b, b'+' | b'-' | b'.'));
    if !plain || rest.contains("://") {
        return Err(Refused::Unread);
    }
    // Up to the first `/` alone: a password may hold a `?` or a `#`.
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    if let Some((user, _)) = authority.rsplit_once('@') {
        if user.contains(':') {
            return Err(Refused::Password);
        }
        let ssh = SSH.iter().any(|ssh| scheme.eq_ignore_ascii_case(ssh));
        if !(ssh && user == GIT) {
            return Err(Refused::User);
        }
    }
    let cut = |c| matches!(c, '@' | '?' | '#');
    if path.contains(cut) || authority.contains(['?', '#']) {
        return Err(Refused::Unread);
    }
    Ok(())
}

/// A path, or `user@host:path`.
fn scp_like(address: &str) -> Result<(), Refused> {
    let Some((head, path)) = address.split_once(':') else {
        return Ok(());
    };
    // A `/` before the colon makes it a path, as a drive's letter does.
    if head.contains('/') || (head.len() == 1 && path.starts_with(['/', '\\'])) {
        return Ok(());
    }
    if path.contains('@') {
        return Err(Refused::Unread);
    }
    match head.rsplit_once('@') {
        None => Ok(()),
        Some((user, _)) if user == GIT => Ok(()),
        Some(_) => Err(Refused::User),
    }
}

#[cfg(test)]
mod tests {
    use super::{Refused, judge};

    #[test]
    fn an_address_with_no_user_or_ssh_s_git_is_sent() {
        for address in [
            "https://github.com/o/r.git",
            "http://localhost:8080/o/r",
            "git@github.com:o/r.git",
            "github.com:o/r.git",
            "ssh://git@github.com/o/r.git",
            "ssh://git@github.com:2222/o/r.git",
            "git+ssh://git@github.com/o/r",
            "SSH://git@github.com/o/r",
            "ssh://host/o/r",
            "git://host/o/r",
            "file:///srv/r.git",
            "/srv/r.git",
            "../r.git",
            "./a:b@c",
            "C:\\r",
            "C:/r",
            "origin",
            ".",
        ] {
            assert_eq!(judge(address.as_bytes()), Ok(address));
        }
    }

    #[test]
    fn a_user_name_or_a_password_refuses() {
        for address in [
            "https://me@github.com/o/r",
            "https://git@github.com/o/r",
            "https://ghp_abc@github.com/o/r",
            "http://me@h/o/r",
            "ssh://me@h/o/r",
            "ssh://Git@h/o/r",
            "me@h:o/r",
            "a@git@h:o/r",
            "ftp://git@h/r",
            "https://@h/r",
            "https://a@b@h/r",
        ] {
            assert_eq!(judge(address.as_bytes()), Err(Refused::User), "{address}");
        }
        for address in [
            "https://me:secret@github.com/o/r",
            "https://:secret@github.com/o/r",
            "https://me:@github.com/o/r",
            "ssh://git:secret@h/o/r",
            "https://me:se?cr#et@h/r",
            "https://me:p%40@h/r",
        ] {
            assert_eq!(
                judge(address.as_bytes()),
                Err(Refused::Password),
                "{address}"
            );
        }
    }

    #[test]
    fn what_cannot_be_read_refuses() {
        for address in [
            "",
            "helper::https://me:secret@h/r",
            "ext::sh -c x",
            "https://h/https://me:secret@h/r",
            "https://me:pa/ss@h/r",
            "https://me:pa//ss@h/r",
            "https://h/r?token=abc",
            "https://h/r#abc",
            "https://h?token=abc",
            "me:secret@h:r",
            "git@h:x@y",
            "://h/r",
            "ht tp://h/r",
            "https://h/r\n",
            "https://h/\u{1b}[31m",
            "https://h/ r",
            "https://h/\u{a0}r",
        ] {
            assert_eq!(judge(address.as_bytes()), Err(Refused::Unread), "{address}");
        }
        assert_eq!(judge(b"https://h/\xff"), Err(Refused::Unread));
    }

    #[test]
    fn what_is_said_names_no_address() {
        for refused in [Refused::User, Refused::Password, Refused::Unread] {
            assert!(refused.to_string().starts_with("a git remote's address "));
        }
    }
}
