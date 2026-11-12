//! Reading a review's file list from `git diff --raw -z` (ADR 0122): paths as bytes,
//! NUL-separated, never parsed from patch headers.
//!
//! Must not depend on: `app`, `dispatch`, `git` or `batch`; this parses bytes it is given.
//!
//! At most [`MAX_FILES`] are kept; the rest are counted. Line counts and whether a file is
//! binary come from the review's own reading, not from git.

use super::id::Id;

/// The most files a review lists.
pub(crate) const MAX_FILES: usize = 10_000;

/// The mode git writes, in octal, for a link.
#[cfg(test)]
pub(crate) const MODE_LINK: u32 = 0o120_000;
/// The mode git writes, in octal, for a submodule.
pub(crate) const MODE_SUBMODULE: u32 = 0o160_000;

/// What happened to a file, by `--raw`'s letter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Added,
    Copied,
    Deleted,
    Modified,
    Renamed,
    /// Its kind changed: a file became a link, or the other way.
    TypeChanged,
    Unmerged,
    Unknown,
}

/// One file of the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    /// The path from the work tree's top, `/`-separated, as git spells it.
    pub(crate) path: Box<[u8]>,
    /// Where a renamed or copied file came from.
    pub(crate) old_path: Option<Box<[u8]>>,
    pub(crate) status: Status,
    pub(crate) old_mode: u32,
    pub(crate) new_mode: u32,
    /// The old side's blob, all zeros for none.
    pub(crate) old_id: Id,
    /// The new side's blob, all zeros when it is a work tree file git did not hash.
    pub(crate) new_id: Id,
}

/// A parsed list.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Listing {
    pub(crate) entries: Vec<Entry>,
    /// Records past [`MAX_FILES`].
    pub(crate) more: usize,
    /// git wrote more than a review reads, so there may be more still.
    pub(crate) cut: bool,
}

/// The list `bytes` hold, `cut` if git's output was cut at the review's limit. A record that
/// does not parse ends the list there.
pub(crate) fn parse(bytes: &[u8], cut: bool) -> Listing {
    let mut fields = bytes.split(|&byte| byte == 0).peekable();
    let mut listing = Listing {
        cut,
        ..Listing::default()
    };
    while fields.peek().is_some_and(|field| field.starts_with(b":")) {
        let Some(head) = fields.next() else { break };
        let Some(record) = raw_record(head, &mut fields) else {
            listing.cut = true;
            break;
        };
        if listing.entries.len() < MAX_FILES {
            listing.entries.push(record);
        } else {
            listing.more += 1;
        }
    }
    listing
}

/// A `--raw` record from its head (`:mode mode id id status`) and the paths after it.
fn raw_record<'a>(head: &[u8], paths: &mut impl Iterator<Item = &'a [u8]>) -> Option<Entry> {
    let mut words = head.get(1..)?.split(|&byte| byte == b' ');
    let old_mode = mode(words.next()?)?;
    let new_mode = mode(words.next()?)?;
    let old_id = Id::parse(words.next()?)?;
    let new_id = Id::parse(words.next()?)?;
    let status = match *words.next()?.first()? {
        b'A' => Status::Added,
        b'C' => Status::Copied,
        b'D' => Status::Deleted,
        b'M' => Status::Modified,
        b'R' => Status::Renamed,
        b'T' => Status::TypeChanged,
        b'U' => Status::Unmerged,
        _ => Status::Unknown,
    };
    let first = path(paths.next()?)?;
    let (path, old_path) = if matches!(status, Status::Renamed | Status::Copied) {
        (self::path(paths.next()?)?, Some(first))
    } else {
        (first, None)
    };
    Some(Entry {
        path,
        old_path,
        status,
        old_mode,
        new_mode,
        old_id,
        new_id,
    })
}

fn path(bytes: &[u8]) -> Option<Box<[u8]>> {
    (!bytes.is_empty()).then(|| bytes.into())
}

fn mode(bytes: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(bytes).ok()?, 8).ok()
}

#[cfg(test)]
mod tests {
    use super::{MAX_FILES, MODE_LINK, Status, parse};

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn raw(status: &str, paths: &[&str]) -> String {
        format!(":100644 100644 {A} {Z} {status}\0{}\0", paths.join("\0"))
    }

    #[test]
    fn records_keep_their_paths_renames_and_modes() {
        let text = format!(
            "{}{}:000000 120000 {Z} {B} A\0link\0",
            raw("M", &["plain"]),
            raw("R087", &["old name", "new\nname"]),
        );
        let listing = parse(text.as_bytes(), false);
        let [plain, renamed, link] = &listing.entries[..] else {
            panic!("{listing:?}")
        };
        assert_eq!(plain.status, Status::Modified);
        assert!(plain.new_id.is_zero());
        assert_eq!(&*renamed.path, b"new\nname");
        assert_eq!(renamed.old_path.as_deref(), Some(&b"old name"[..]));
        assert_eq!((link.status, link.new_mode), (Status::Added, MODE_LINK));
    }

    #[test]
    fn past_the_limit_records_are_counted_and_not_kept() {
        let text: String = (0..MAX_FILES + 3)
            .map(|n| raw("M", &[&format!("f{n}")]))
            .collect();
        let listing = parse(text.as_bytes(), false);
        assert_eq!((listing.entries.len(), listing.more), (MAX_FILES, 3));
    }

    #[test]
    fn a_cut_or_broken_list_keeps_what_parsed() {
        let text = format!("{}:100644 100644 {A} short M\0x\0", raw("M", &["a"]));
        let listing = parse(text.as_bytes(), false);
        assert!(listing.cut);
        let paths: Vec<&[u8]> = listing.entries.iter().map(|e| &*e.path).collect();
        assert_eq!(paths, [&b"a"[..]]);
        assert!(parse(b"", true).entries.is_empty());
    }
}
