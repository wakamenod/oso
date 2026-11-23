//! TypeWhisper's discovery file (ADR 0175): the port of its API server and the token it takes,
//! read fresh for each dictation, off the UI thread, and refused unless it is plainly the
//! person's own.
//!
//! Must not depend on: `app`, processes or the API; this reads one file and says what it holds.
//!
//! - The file is refused when it is a link, not this user's, writable by its group or by
//!   others, or over [`MAX_BYTES`]. A link swapped in after the look is refused as well.
//! - It must say `version` 1, a `port` from 1 to 65535 and a `token` of 16 to 256 characters
//!   from `A-Z a-z 0-9 . _ ~ + / = -`, which can go in a curl configuration as it is. Nothing
//!   falls back to TypeWhisper's usual port.
//! - The token is a [`Token`], whose `Debug` says nothing of it and which has no `Display`, so
//!   that no message, log line or argument holds it.

use std::fmt;
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// The most bytes the file may hold.
pub const MAX_BYTES: u64 = 4096;

/// The only version of the file oso reads.
const VERSION: u64 = 1;

/// The shortest and the longest token taken.
const TOKEN_CHARS: std::ops::RangeInclusive<usize> = 16..=256;

/// Where TypeWhisper keeps the file, under the person's home, on macOS, where it runs: its
/// release build's place. A dev build of it keeps one under `TypeWhisper-Dev`, which
/// `[dictation] discovery` can name.
pub fn default_path(home: &Path) -> Option<PathBuf> {
    cfg!(target_os = "macos").then(|| {
        home.join("Library/Application Support/TypeWhisper")
            .join("api-discovery.json")
    })
}

/// The token TypeWhisper's API takes. Its `Debug` gives nothing of it, and it has no
/// `Display`: it goes only into curl's configuration on curl's standard input.
#[derive(Clone, PartialEq, Eq)]
pub struct Token(String);

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Token(withheld)")
    }
}

impl Token {
    /// The token, for the one line of curl's configuration that carries it.
    pub(super) fn reveal(&self) -> &str {
        &self.0
    }

    /// `text` as a token, if it holds as one.
    fn of(text: &str) -> Option<Self> {
        let holds = TOKEN_CHARS.contains(&text.len())
            && text
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._~+/=-".contains(&b));
        holds.then(|| Self(text.to_owned()))
    }
}

/// What the file says: where TypeWhisper listens, and the token it takes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discovery {
    /// The port on `127.0.0.1`.
    pub port: u16,
    /// The token.
    pub token: Token,
}

/// Why the file gave no [`Discovery`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unread {
    /// There is none: TypeWhisper is not running, or its API server is off.
    Missing,
    /// It is there, but refused or not understood, for this reason, which quotes nothing of it.
    Refused(&'static str),
}

/// Reads and checks the file at `path`. It touches the disk, so it belongs off the UI thread.
///
/// # Errors
///
/// Why it gave nothing, as [`Unread`] says.
pub fn read(path: &Path) -> Result<Discovery, Unread> {
    let bytes = read_checked(path)?;
    parse(&bytes).map_err(Unread::Refused)
}

/// The file's bytes, if it passes the checks.
fn read_checked(path: &Path) -> Result<Vec<u8>, Unread> {
    let missing = |e: &io::Error| e.kind() == io::ErrorKind::NotFound;
    let looked = match std::fs::symlink_metadata(path) {
        Ok(looked) => looked,
        Err(e) if missing(&e) => return Err(Unread::Missing),
        Err(_) => return Err(Unread::Refused("it cannot be looked at")),
    };
    if looked.file_type().is_symlink() {
        return Err(Unread::Refused("it is a link"));
    }
    let (file, metadata) = match oso_platform::file_meta::open_regular_here(path) {
        Ok(Some(opened)) => opened,
        // A directory, a device, or a link swapped in after the look.
        Ok(None) => return Err(Unread::Refused("it is not a regular file")),
        Err(e) if missing(&e) => return Err(Unread::Missing),
        Err(_) => return Err(Unread::Refused("it cannot be opened")),
    };
    if !oso_platform::privacy::is_this_users(&metadata) {
        return Err(Unread::Refused("it is not yours"));
    }
    if writable_by_others(&metadata) {
        return Err(Unread::Refused("other users can change it"));
    }
    if metadata.len() > MAX_BYTES {
        return Err(Unread::Refused("it is larger than 4 KiB"));
    }
    let mut bytes = Vec::with_capacity(512);
    if file.take(MAX_BYTES + 1).read_to_end(&mut bytes).is_err() {
        return Err(Unread::Refused("it cannot be read"));
    }
    if bytes.len() as u64 > MAX_BYTES {
        return Err(Unread::Refused("it is larger than 4 KiB"));
    }
    Ok(bytes)
}

/// Whether its group or every user may write the file `metadata` describes, by its mode.
#[cfg(unix)]
fn writable_by_others(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o022 != 0
}

/// Windows has no modes; the file is in the person's own profile there.
#[cfg(not(unix))]
fn writable_by_others(metadata: &std::fs::Metadata) -> bool {
    oso_platform::privacy::others_can_write(metadata)
}

/// What the file's `bytes` say, if they hold as a discovery file of [`VERSION`].
///
/// # Errors
///
/// Why not, which quotes nothing of it.
pub fn parse(bytes: &[u8]) -> Result<Discovery, &'static str> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| "it is not JSON")?;
    if value.get("version").and_then(Value::as_u64) != Some(VERSION) {
        return Err("it is not of version 1");
    }
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok())
        .filter(|&port| port != 0)
        .ok_or("it names no port from 1 to 65535")?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .and_then(Token::of)
        .ok_or("its token is not one oso takes")?;
    Ok(Discovery { port, token })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN: &str = "abcdefghijklmnopqrstuvwxyz0123456789._~+/=-";

    fn file(port: &str, token: &str) -> Vec<u8> {
        format!(r#"{{"port": {port}, "token": "{token}", "version": 1}}"#).into_bytes()
    }

    #[test]
    fn a_file_of_version_1_gives_its_port_and_token() {
        let found = parse(&file("8978", TOKEN)).unwrap();
        assert_eq!(found.port, 8978);
        assert_eq!(found.token.reveal(), TOKEN);
    }

    #[test]
    fn the_token_never_shows_in_debug() {
        let found = parse(&file("8978", TOKEN)).unwrap();
        let shown = format!("{found:?}");
        assert!(!shown.contains("abcdef"), "{shown}");
        assert!(shown.contains("withheld"), "{shown}");
    }

    #[test]
    fn a_bad_version_port_or_token_is_refused() {
        let other = br#"{"port": 8978, "token": "abcdefghijklmnop", "version": 2}"#;
        assert_eq!(parse(other), Err("it is not of version 1"));
        assert!(parse(br#"{"port": 8978, "token": "abcdefghijklmnop"}"#).is_err());
        for port in ["0", "65536", "-1", "\"8978\"", "1.5"] {
            assert!(parse(&file(port, TOKEN)).is_err(), "{port}");
        }
        assert!(parse(&file("1", TOKEN)).is_ok());
        assert!(parse(&file("65535", TOKEN)).is_ok());
        let long = "a".repeat(257);
        for token in ["short", "has space here!!", "quote\\\"insideeeeeeee", &long] {
            assert_eq!(
                parse(&file("8978", token)).map(|_| ()),
                Err("its token is not one oso takes"),
                "{token}"
            );
        }
        assert!(parse(&file("8978", &"a".repeat(16))).is_ok());
        assert!(parse(&file("8978", &"a".repeat(256))).is_ok());
        assert_eq!(parse(b"not json"), Err("it is not JSON"));
    }

    #[test]
    fn no_file_is_missing_and_refusals_say_why() {
        let dir = std::env::temp_dir().join(format!("oso-discovery-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("api-discovery.json");
        assert_eq!(read(&path), Err(Unread::Missing));
        std::fs::write(&path, file("8978", TOKEN)).unwrap();
        set_mode(&path, 0o600);
        assert_eq!(read(&path).map(|found| found.port), Ok(8978));
        #[cfg(unix)]
        {
            set_mode(&path, 0o620);
            assert_eq!(
                read(&path),
                Err(Unread::Refused("other users can change it"))
            );
            set_mode(&path, 0o602);
            assert_eq!(
                read(&path),
                Err(Unread::Refused("other users can change it"))
            );
            set_mode(&path, 0o644);
            assert!(
                read(&path).is_ok(),
                "others may read it: TypeWhisper's own care"
            );
            let link = dir.join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert_eq!(read(&link), Err(Unread::Refused("it is a link")));
        }
        std::fs::write(&path, vec![b' '; 4097]).unwrap();
        set_mode(&path, 0o600);
        assert_eq!(read(&path), Err(Unread::Refused("it is larger than 4 KiB")));
        assert_eq!(read(&dir), Err(Unread::Refused("it is not a regular file")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn set_mode(path: &Path, mode: u32) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = (path, mode);
    }

    #[test]
    fn the_default_place_is_typewhispers_release_build_on_macos() {
        let home = Path::new("/Users/someone");
        let path = default_path(home);
        if cfg!(target_os = "macos") {
            assert_eq!(
                path.unwrap(),
                Path::new(
                    "/Users/someone/Library/Application Support/TypeWhisper/api-discovery.json"
                )
            );
        } else {
            assert_eq!(path, None);
        }
    }
}
