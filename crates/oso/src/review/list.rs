//! Reading a review's file list from `git diff --raw --numstat -z` (ADR 0122): paths as bytes,
//! NUL-separated, never parsed from patch headers.
//!
//! Must not depend on: `app`, `dispatch` or anything that runs git; this parses bytes it is
//! given.
//!
//! git writes every `--raw` record first, then the `--numstat` records in the same order, so
//! the two are paired by place. At most [`MAX_FILES`] are kept; the rest are counted.

use super::git::Id;

/// The most files a review lists.
pub(crate) const MAX_FILES: usize = 10_000;

/// A file's mode, as git writes it in octal.
pub(crate) const MODE_LINK: u32 = 0o120_000;
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
    /// Lines added and removed, as `--numstat` counts them: `None` for a binary file, or one
    /// past a cut list.
    pub(crate) counts: Option<(u64, u64)>,
    /// `--numstat` said `-`: git takes it for binary.
    pub(crate) binary: bool,
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
    for entry in &mut listing.entries {
        let Some(head) = fields.next() else { break };
        let Some((counts, rename)) = numstat_head(head) else {
            break;
        };
        // A rename's numstat names both paths after an empty one.
        if rename {
            let (Some(_), Some(_)) = (fields.next(), fields.next()) else {
                break;
            };
        }
        entry.binary = counts.is_none();
        entry.counts = counts;
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
    let letter = *words.next()?.first()?;
    let status = match letter {
        b'A' => Status::Added,
        b'C' => Status::Copied,
        b'D' => Status::Deleted,
        b'M' => Status::Modified,
        b'R' => Status::Renamed,
        b'T' => Status::TypeChanged,
        b'U' => Status::Unmerged,
        _ => Status::Unknown,
    };
    let first: Box<[u8]> = path(paths.next()?)?;
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
        counts: None,
        binary: false,
    })
}

/// A `--numstat` record's counts, `None` for binary, and whether its paths follow.
fn numstat_head(head: &[u8]) -> Option<(Option<(u64, u64)>, bool)> {
    let mut words = head.splitn(3, |&byte| byte == b'\t');
    let (added, removed, path) = (words.next()?, words.next()?, words.next()?);
    let counts = if added == b"-" && removed == b"-" {
        None
    } else {
        Some((number(added)?, number(removed)?))
    };
    Some((counts, path.is_empty()))
}

fn path(bytes: &[u8]) -> Option<Box<[u8]>> {
    (!bytes.is_empty()).then(|| bytes.into())
}

fn mode(bytes: &[u8]) -> Option<u32> {
    u32::from_str_radix(std::str::from_utf8(bytes).ok()?, 8).ok()
}

fn number(bytes: &[u8]) -> Option<u64> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::{Entry, MAX_FILES, Status, parse};

    const A: &str = "1111111111111111111111111111111111111111";
    const B: &str = "2222222222222222222222222222222222222222";
    const Z: &str = "0000000000000000000000000000000000000000";

    fn raw(status: &str, paths: &[&str]) -> String {
        format!(":100644 100644 {A} {Z} {status}\0{}\0", paths.join("\0"))
    }

    #[test]
    fn records_pair_with_their_counts_renames_and_binaries_included() {
        let text = format!(
            "{}{}:000000 120000 {Z} {B} A\0link\0{}2\t1\tplain\x003\t0\t\0old name\0new\nname\0-\t-\tlink\0",
            raw("M", &["plain"]),
            raw("R087", &["old name", "new\nname"]),
            "",
        );
        let listing = parse(text.as_bytes(), false);
        assert_eq!(listing.entries.len(), 3);
        let [plain, renamed, link] = &listing.entries[..] else {
            panic!()
        };
        assert_eq!(
            (plain.status, plain.counts),
            (Status::Modified, Some((2, 1)))
        );
        assert!(plain.new_id.is_zero());
        assert_eq!(&*renamed.path, b"new\nname");
        assert_eq!(renamed.old_path.as_deref(), Some(&b"old name"[..]));
        assert_eq!(renamed.counts, Some((3, 0)));
        assert_eq!(
            (link.status, link.new_mode),
            (Status::Added, super::MODE_LINK)
        );
        assert!(link.binary && link.counts.is_none());
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
        assert_eq!(
            listing.entries.iter().map(|e| &*e.path).collect::<Vec<_>>(),
            [&b"a"[..]]
        );
        let empty: &[Entry] = &parse(b"", true).entries;
        assert!(empty.is_empty());
    }
}
