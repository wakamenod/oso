//! Which files may hold secrets (ADR 0028, ADR 0043).
//!
//! Must not depend on: `buffer`, `editor` or file I/O. These are pure
//! functions of paths; whoever can touch the file system resolves the path
//! first.
//!
//! The dev control socket withholds these files' text and pixels, and the
//! emergency save (M2) skips them. A file's name and directories decide, so
//! a hard link to a secret under another name is not caught.
//!
//! Matching is broader than the ADR 0028 list in two ways, both toward
//! calling more files secret:
//!
//! - ASCII case is ignored, since the default file systems on macOS and
//!   Windows open `.ENV` as `.env`.
//! - `.ssh`, `.gnupg` and `.aws` count anywhere in a path, not only in the
//!   home directory. This needs no `$HOME`, which can be wrong or unset, and
//!   also covers other users' homes and backups.

use std::path::{Component, Path, PathBuf};

/// Directories whose contents are all secret.
const DIRECTORIES: [&[u8]; 3] = [b".ssh", b".gnupg", b".aws"];

/// Whole file names.
const NAMES: [&[u8]; 2] = [b".env", b".netrc"];

/// Starts of file names.
const PREFIXES: [&[u8]; 3] = [b".env.", b"id_", b"credentials"];

/// Ends of file names.
const SUFFIXES: [&[u8]; 4] = [b".pem", b".key", b".p12", b".pfx"];

/// Whether `path` matches a secret pattern, as written. A symbolic link is
/// judged by its own name: see [`is_secret`].
fn matches_pattern(path: &Path) -> bool {
    let in_directory = path.components().any(|component| match component {
        Component::Normal(name) => {
            let name = name.as_encoded_bytes();
            DIRECTORIES.iter().any(|dir| name.eq_ignore_ascii_case(dir))
        }
        _ => false,
    });
    in_directory
        || path
            .file_name()
            .is_some_and(|name| name_matches(name.as_encoded_bytes()))
}

/// Whether a file's name alone makes it secret.
fn name_matches(name: &[u8]) -> bool {
    NAMES.iter().any(|whole| name.eq_ignore_ascii_case(whole))
        || PREFIXES.iter().any(|prefix| {
            name.get(..prefix.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        })
        || SUFFIXES.iter().any(|suffix| {
            name.len()
                .checked_sub(suffix.len())
                .is_some_and(|start| name[start..].eq_ignore_ascii_case(suffix))
        })
}

/// Whether the file opened as `opened` is secret. The path as opened counts,
/// and so does every path in `resolved`: each link's target on the way and
/// the canonical path. So a link to `~/.ssh/id_rsa` is caught whatever it is
/// called, and so is a link through `~/.aws/credentials` to a file with an
/// ordinary name. `None` means the path could not be resolved, and fails
/// closed.
pub(crate) fn is_secret(opened: &Path, resolved: Option<&[PathBuf]>) -> bool {
    matches_pattern(opened)
        || resolved.is_none_or(|paths| paths.iter().any(|path| matches_pattern(path)))
}

#[cfg(test)]
mod tests {
    use super::{is_secret, matches_pattern};
    use proptest::prelude::*;
    use std::path::{Path, PathBuf};

    fn secret(path: &str) -> bool {
        matches_pattern(Path::new(path))
    }

    #[test]
    fn matches_every_pattern_of_adr_0028() {
        for path in [
            "/p/.env",
            "/p/.env.local",
            "/p/.env.",
            "/p/server.pem",
            "/p/tls.key",
            "/p/bundle.p12",
            "/p/bundle.pfx",
            "/h/id_rsa",
            "/h/id_ed25519.pub",
            "/h/.netrc",
            "/h/credentials",
            "/h/credentials.json",
            "/h/.ssh/config",
            "/h/.ssh/keys/deploy",
            "/h/.gnupg/gpg.conf",
            "/h/.aws/config",
            ".env",
            "relative/.ssh/known_hosts",
        ] {
            assert!(secret(path), "{path}");
        }
    }

    #[test]
    fn leaves_ordinary_files_alone() {
        for path in [
            "/p/main.rs",
            "/p/env",
            "/p/.envrc",
            "/p/prod.env",
            "/p/pem",
            "/p/key",
            "/p/my.keys",
            "/p/identity",
            "/p/my_id_rsa",
            "/p/.netrc.bak",
            "/p/my-credentials",
            "/p/.ssh.txt",
            "/p/ssh/config",
            "/p/credentials/readme.md",
            "/",
            "",
        ] {
            assert!(!secret(path), "{path}");
        }
    }

    #[test]
    fn ignores_ascii_case() {
        for path in [
            "/p/.ENV",
            "/p/.Env.Local",
            "/p/SERVER.PEM",
            "/h/ID_RSA",
            "/h/.NETRC",
            "/h/Credentials",
            "/h/.SSH/config",
        ] {
            assert!(secret(path), "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn handles_names_that_are_not_utf8() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;
        let path = |bytes: &[u8]| Path::new(OsStr::from_bytes(bytes)).to_owned();
        assert!(matches_pattern(&path(b"/p/\xff.pem")));
        assert!(matches_pattern(&path(b"/p/.ssh/\xff")));
        assert!(!matches_pattern(&path(b"/p/\xff.txt")));
    }

    #[test]
    fn the_canonical_path_counts_and_an_unresolved_path_fails_closed() {
        let link = Path::new("/p/notes.txt");
        let target = PathBuf::from("/h/.ssh/id_rsa");
        let plain = PathBuf::from("/p/real.txt");
        assert!(
            is_secret(link, Some(std::slice::from_ref(&target))),
            "a link to a secret"
        );
        assert!(
            is_secret(link, Some(&[target.clone(), plain.clone()][..])),
            "a link through a secret"
        );
        assert!(
            is_secret(&target, Some(std::slice::from_ref(&plain))),
            "named like one"
        );
        assert!(is_secret(link, None), "cannot be resolved");
        assert!(!is_secret(link, Some(std::slice::from_ref(&plain))));
        assert!(!is_secret(link, Some(&[])));
    }

    /// A file name that no name pattern matches: lower-case letters, which
    /// can spell no `.`, `_` or digit, so it can only collide with a
    /// pattern that is all letters.
    fn plain_name() -> impl Strategy<Value = String> {
        "[a-z]{1,12}".prop_filter("spells a prefix pattern", |name| {
            !name.starts_with("credentials")
        })
    }

    proptest! {
        #[test]
        fn anything_under_a_secret_directory_is_secret(
            before in prop::collection::vec(plain_name(), 0..4),
            dir in prop::sample::select(vec![".ssh", ".gnupg", ".aws", ".SSH"]),
            after in prop::collection::vec(plain_name(), 0..4),
        ) {
            let path = format!("/{}/{dir}/{}", before.join("/"), after.join("/"));
            prop_assert!(secret(&path), "{}", path);
        }

        #[test]
        fn a_secret_file_name_is_secret_in_any_directory(
            dirs in prop::collection::vec(plain_name(), 0..4),
            stem in plain_name(),
            kind in 0..6usize,
        ) {
            let name = match kind {
                0 => format!(".env.{stem}"),
                1 => format!("{stem}.pem"),
                2 => format!("{stem}.key"),
                3 => format!("{stem}.PFX"),
                4 => format!("id_{stem}"),
                _ => format!("credentials{stem}"),
            };
            let path = format!("/{}/{name}", dirs.join("/"));
            prop_assert!(secret(&path), "{}", path);
        }

        #[test]
        fn plain_paths_are_not_secret(
            dirs in prop::collection::vec(plain_name(), 0..6),
            name in plain_name(),
            extension in prop::sample::select(vec!["", ".txt", ".rs", ".md"]),
        ) {
            let path = format!("/{}/{name}{extension}", dirs.join("/"));
            prop_assert!(!secret(&path), "{}", path);
        }

        #[test]
        fn never_panics_and_an_unresolved_path_is_always_secret(path in ".{0,40}") {
            let path = Path::new(&path);
            let _ = matches_pattern(path);
            prop_assert!(is_secret(path, None));
        }
    }
}
