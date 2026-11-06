//! Which files may hold secrets (ADR 0028, ADR 0044).
//!
//! Must not depend on: `buffer`, `editor` or file I/O. These are pure
//! functions of paths; whoever can touch the file system resolves the path
//! first.
//!
//! The dev control socket withholds these files' text and pixels, and the
//! emergency save skips them. A file's name and directories decide, so a hard
//! link to a secret under another name is not caught. The two differ over a
//! path nobody could resolve, which [`Verdict`] tells apart (ADR 0051).
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

/// What is known of whether a buffer's text may hold secrets. Each use asks its own
/// question of this, so that a new reason (M2-14: a yank from a secret buffer) is one
/// more variant here, and no caller changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// A file just visited, before its path has been resolved.
    Unjudged,
    /// No file, or no name on the path or where it leads matches a pattern.
    Ordinary,
    /// Nobody could tell where the path leads, and no name seen on the way matches.
    Unresolved,
    /// A name on the path or where it leads matches a pattern.
    SecretName,
}

impl Verdict {
    /// Whether dev tooling must withhold the text (ADR 0044). Anything not known to be
    /// ordinary is.
    pub const fn is_withheld(self) -> bool {
        !matches!(self, Self::Ordinary)
    }

    /// Whether an emergency copy may hold the text (ADR 0051). An unresolved path is
    /// allowed, since losing the work is the worse mistake when no name says secret.
    pub const fn may_be_copied(self) -> bool {
        matches!(self, Self::Ordinary | Self::Unresolved)
    }
}

/// Whether `path` matches a secret pattern, as written. A symbolic link is
/// judged by its own name: see [`judge`].
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

/// The verdict on the file opened as `opened`, if any, where `resolved` says where it leads: each
/// link's target on the way and the canonical path. Every one of them counts, so a link
/// to `~/.ssh/id_rsa` is caught whatever it is called, and so is a link through
/// `~/.aws/credentials` to a file with an ordinary name. `None` means it could not be
/// resolved.
pub(crate) fn judge(opened: Option<&Path>, resolved: Option<&[PathBuf]>) -> Verdict {
    let Some(resolved) = resolved else {
        return judge_unresolved(opened, &[]);
    };
    match opened {
        Some(opened) if any_matches(opened, resolved) => Verdict::SecretName,
        _ => Verdict::Ordinary,
    }
}

/// The verdict on the file opened as `opened`, whose path could not be resolved. `seen`
/// holds what the walk passed before it failed, as those names count too.
pub(crate) fn judge_unresolved(opened: Option<&Path>, seen: &[PathBuf]) -> Verdict {
    match opened {
        Some(opened) if any_matches(opened, seen) => Verdict::SecretName,
        Some(_) => Verdict::Unresolved,
        None => Verdict::Ordinary,
    }
}

fn any_matches(opened: &Path, others: &[PathBuf]) -> bool {
    matches_pattern(opened) || others.iter().any(|path| matches_pattern(path))
}

#[cfg(test)]
mod tests {
    use super::{Verdict, judge, judge_unresolved, matches_pattern};
    use proptest::prelude::*;
    use std::path::{Path, PathBuf};

    /// Whether dev tooling withholds the file: `None` means it could not be resolved.
    fn is_secret(opened: &Path, resolved: Option<&[PathBuf]>) -> bool {
        judge(Some(opened), resolved).is_withheld()
    }

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

    #[test]
    fn an_unresolved_path_is_told_apart_from_a_secret_name() {
        let plain = Some(Path::new("/p/notes.txt"));
        let key = PathBuf::from("/h/.ssh/id_rsa");
        assert_eq!(judge_unresolved(plain, &[]), Verdict::Unresolved);
        assert_eq!(
            judge_unresolved(Some(Path::new("/h/.ssh/missing")), &[]),
            Verdict::SecretName
        );
        assert_eq!(
            judge_unresolved(plain, std::slice::from_ref(&key)),
            Verdict::SecretName,
            "a link that went through a secret name before the walk failed"
        );
        assert_eq!(
            judge(plain, Some(std::slice::from_ref(&key))),
            Verdict::SecretName
        );
        assert_eq!(judge(plain, Some(&[])), Verdict::Ordinary);
        assert_eq!(
            judge(None, Some(std::slice::from_ref(&key))),
            Verdict::Ordinary,
            "no file"
        );
        assert_eq!(judge_unresolved(None, &[key]), Verdict::Ordinary, "no file");
    }

    #[test]
    fn each_use_asks_its_own_question_of_a_verdict() {
        // Withheld from dev tooling, and allowed in an emergency copy.
        let table = [
            (Verdict::Unjudged, true, false),
            (Verdict::Ordinary, false, true),
            (Verdict::Unresolved, true, true),
            (Verdict::SecretName, true, false),
        ];
        for (verdict, withheld, copied) in table {
            assert_eq!(verdict.is_withheld(), withheld, "{verdict:?}");
            assert_eq!(verdict.may_be_copied(), copied, "{verdict:?}");
        }
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
            // Not `//`, which starts a network path on Windows.
            let path = format!("/r/{}/{dir}/{}", before.join("/"), after.join("/"));
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
            let path = format!("/r/{}/{name}", dirs.join("/"));
            prop_assert!(secret(&path), "{}", path);
        }

        #[test]
        fn plain_paths_are_not_secret(
            dirs in prop::collection::vec(plain_name(), 0..6),
            name in plain_name(),
            extension in prop::sample::select(vec!["", ".txt", ".rs", ".md"]),
        ) {
            let path = format!("/r/{}/{name}{extension}", dirs.join("/"));
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
