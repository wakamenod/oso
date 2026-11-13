//! A branch's directory name, its slug, and where its worktree goes under a repository: as
//! herdr names them (ADR 0121, Q13), so that the place is one Claude Code and the previous setup
//! used and a repository's ignore files already cover.
//!
//! Must not depend on: the file system, `app`, `dispatch` or `commands`; [`place`] asks the
//! caller whether a place is taken.

use std::path::{Path, PathBuf};

/// The most bytes a slug takes.
pub const MAX_SLUG: usize = 64;

/// Where worktrees go under a repository's root: `.claude/worktrees` (ADR 0121, Q13).
pub const PARENT: [&str; 2] = [".claude", "worktrees"];

/// How many places [`place`] tries: the slug itself, then `SLUG-2` to `SLUG-10`.
const TRIES: usize = 10;

/// A branch's slug: ASCII-lowercased; `/` becomes `-`; only `[a-z0-9._-]` kept, every other
/// character dropped; leading `.` and `-` dropped; runs of `-` kept as they are; at most
/// [`MAX_SLUG`] bytes, then trailing `.` and `-` trimmed. `None` if nothing is left. It is
/// never `.` or `..` and never holds a `/`.
pub fn slug(branch: &str) -> Option<String> {
    let mut slug: String = branch
        .chars()
        .map(|c| {
            if c == '/' {
                '-'
            } else {
                c.to_ascii_lowercase()
            }
        })
        .filter(|&c| matches!(c, 'a'..='z' | '0'..='9' | '.' | '_' | '-'))
        .skip_while(|&c| c == '.' || c == '-')
        .collect();
    // Every character kept is ASCII, so any byte count is a character boundary.
    slug.truncate(MAX_SLUG);
    trim_end(&mut slug);
    (!slug.is_empty()).then_some(slug)
}

/// The worktree's place for `slug` under `root`: `root/.claude/worktrees/SLUG`, or `SLUG-2` to
/// `SLUG-10` when `taken` says the earlier ones are taken, the slug cut so that the whole stays
/// within [`MAX_SLUG`] bytes. `None` when all ten are taken, or when `slug` is not one that
/// [`slug`] makes.
pub fn place(root: &Path, slug: &str, taken: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    if self::slug(slug).as_deref() != Some(slug) {
        return None;
    }
    let parent = PARENT
        .iter()
        .fold(root.to_path_buf(), |dir, part| dir.join(part));
    (1..=TRIES)
        .map(|n| {
            if n == 1 {
                return slug.to_owned();
            }
            let suffix = format!("-{n}");
            let mut base = slug.to_owned();
            base.truncate(MAX_SLUG - suffix.len());
            trim_end(&mut base);
            base + &suffix
        })
        .map(|name| parent.join(name))
        .find(|candidate| !taken(candidate))
}

/// Trims trailing `.` and `-` from `slug`.
fn trim_end(slug: &mut String) {
    let kept = slug.trim_end_matches(['.', '-']).len();
    slug.truncate(kept);
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use proptest::prelude::*;

    use super::{MAX_SLUG, place, slug};

    #[test]
    fn branches_become_slugs() {
        assert_eq!(slug("feat/Foo_bar").as_deref(), Some("feat-foo_bar"));
        assert_eq!(slug("Ünïcode/x").as_deref(), Some("ncode-x"));
        assert_eq!(slug("../../etc").as_deref(), Some("etc"));
        assert_eq!(slug("a//b").as_deref(), Some("a--b"));
        assert_eq!(slug("-.-x.y").as_deref(), Some("x.y"));
        assert_eq!(slug("fix/").as_deref(), Some("fix"));
        assert_eq!(slug("v1.2.").as_deref(), Some("v1.2"));
        assert_eq!(slug("a b\tc$(d)").as_deref(), Some("abcd"));
    }

    #[test]
    fn nothing_left_is_none() {
        for branch in ["", "///", "..", ".", "-", "日本語", "$ %"] {
            assert_eq!(slug(branch), None, "{branch:?}");
        }
    }

    #[test]
    fn long_branches_are_cut() {
        let long = "a".repeat(200);
        assert_eq!(slug(&long), Some("a".repeat(MAX_SLUG)));
        // A cut that ends in separators drops them.
        let dashed = format!("{}---{}", "b".repeat(62), "c".repeat(10));
        assert_eq!(slug(&dashed), Some("b".repeat(62)));
    }

    #[test]
    fn places_go_under_the_parent_with_suffixes() {
        let root = Path::new("/repo");
        let base = PathBuf::from("/repo/.claude/worktrees");
        assert_eq!(place(root, "x", |_| false), Some(base.join("x")));
        let taken = |p: &Path| p.ends_with("x") || p.ends_with("x-2");
        assert_eq!(place(root, "x", taken), Some(base.join("x-3")));
        let all_but_last = |p: &Path| !p.ends_with("x-10");
        assert_eq!(place(root, "x", all_but_last), Some(base.join("x-10")));
        assert_eq!(place(root, "x", |_| true), None);
    }

    #[test]
    fn suffixed_places_keep_within_the_limit() {
        let long = "d".repeat(MAX_SLUG);
        let found = place(Path::new("/r"), &long, |p| {
            !p.ends_with(format!("{}-7", "d".repeat(62)))
        })
        .unwrap();
        let name = found.file_name().unwrap().to_str().unwrap();
        assert_eq!(name.len(), MAX_SLUG);
        assert!(name.ends_with("-7"));
        let dotted = format!("{}.{}", "e".repeat(61), "ff");
        let found = place(Path::new("/r"), &dotted, |p| p.ends_with(&dotted)).unwrap();
        assert_eq!(
            found.file_name().unwrap(),
            &*format!("{}-2", "e".repeat(61))
        );
    }

    #[test]
    fn a_slug_slug_does_not_make_is_refused() {
        for bad in ["", "..", "a/b", "../x", "A", &"z".repeat(MAX_SLUG + 1)] {
            assert_eq!(place(Path::new("/r"), bad, |_| false), None, "{bad:?}");
        }
    }

    proptest! {
        #[test]
        fn any_branch_makes_a_safe_slug(branch in any::<String>()) {
            if let Some(s) = slug(&branch) {
                prop_assert!(!s.is_empty() && s.len() <= MAX_SLUG);
                prop_assert!(s != "." && s != "..");
                prop_assert!(s.bytes().all(|b| matches!(b, b'a'..=b'z' | b'0'..=b'9' | b'.' | b'_' | b'-')));
                prop_assert!(!s.starts_with(['.', '-']) && !s.ends_with(['.', '-']));
                prop_assert_eq!(slug(&s), Some(s.clone()));
                // Every place tried is a name within the limit, and no two are the same.
                let tried = std::cell::RefCell::new(Vec::new());
                let found = place(Path::new("/r"), &s, |p| {
                    tried.borrow_mut().push(p.to_path_buf());
                    true
                });
                prop_assert!(found.is_none());
                let tried = tried.into_inner();
                prop_assert_eq!(tried.len(), 10);
                for p in &tried {
                    prop_assert_eq!(p.parent(), Some(Path::new("/r/.claude/worktrees")));
                    prop_assert!(p.file_name().is_some_and(|n| n.len() <= MAX_SLUG));
                }
                let distinct: std::collections::BTreeSet<_> = tried.iter().collect();
                prop_assert_eq!(distinct.len(), tried.len());
            }
        }
    }
}
