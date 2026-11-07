//! Comparing a copy with the file it belongs to, on a thread of its own (ADR 0064): whether
//! they hold the same bytes, and a line diff for a person to read.
//!
//! Must not depend on: `app`, `dispatch` or winit; a caller hands over paths and takes a
//! result. Text goes from here only into what the caller shows in a buffer, and an error
//! says sizes and never the text or where it differs.
//!
//! Running out of memory or stack aborts with no emergency save, so each is bounded before
//! it starts. The line limit bounds memory, as a diff keeps about 32 bytes per line of both
//! sides, and `RawMyers` halves the work at each split, so that its recursion stays shallow.

use std::fmt::Write as _;
use std::fs::{self, File, Metadata};
use std::io::{self, Read};
use std::path::Path;
use std::time::{Duration, Instant};

use oso_core::decode::BOM;
use similar::{Algorithm, DiffOp, capture_diff_slices_deadline, group_diff_ops};

use crate::loader::{same_file, surely_same_file};

/// The largest side a diff takes.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The most lines in a side.
const MAX_LINES: usize = 1_000_000;
/// How long a diff may take. Past it the diff has given up, and reads as one replacement of
/// nearly everything.
const DEADLINE: Duration = Duration::from_secs(5);
/// The unchanged lines shown around a change.
const CONTEXT: usize = 3;
/// How much of each file is read at a time.
const PIECE: usize = 1024 * 1024;

/// Why there is no diff. It says what is known, and holds no text.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum NoDiff {
    /// A side is over [`MAX_BYTES`].
    TooLarge { file: u64, copy: u64 },
    /// A side is over [`MAX_LINES`].
    TooManyLines { file: u64, copy: u64 },
    /// It took longer than [`DEADLINE`].
    GaveUp { file: u64, copy: u64 },
    /// A side is not UTF-8, so its lines are not text to show.
    NotText,
    /// A side could not be read.
    Io(String),
}

impl std::fmt::Display for NoDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TooLarge { file, copy } => write!(
                f,
                "too large to compare, so all that is known is the sizes: the file has {file} bytes and the copy {copy}"
            ),
            Self::TooManyLines { file, copy } => write!(
                f,
                "too many lines to compare, so all that is known is the sizes: the file has {file} bytes and the copy {copy}"
            ),
            Self::GaveUp { file, copy } => write!(
                f,
                "too different to compare in 5 s, so all that is known is the sizes: the file has {file} bytes and the copy {copy}"
            ),
            Self::NotText => f.write_str("one side is not text, so there is no line diff"),
            Self::Io(problem) => write!(f, "cannot read it: {problem}"),
        }
    }
}

/// Opens the regular file at `location`, the real place of a file a person opened. It is
/// where the file was as it was read, so it is no link, and whatever has been put there since
/// is not read, as what it shows would pass for that file's. A pipe would block whatever opens
/// it. It is not `open_checked`, as such a file may have another owner or names of its own.
fn open_the_file(location: &Path) -> io::Result<(File, Metadata)> {
    // A directory on the way can have been swapped too. The location's is canonical.
    if let Some(directory) = location.parent()
        && fs::canonicalize(directory)? != directory
    {
        return Err(io::Error::other("its directory now leads elsewhere"));
    }
    let looked = fs::symlink_metadata(location)?;
    if !looked.is_file() {
        return Err(io::Error::other("it is not a regular file"));
    }
    let file = File::open(location)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || !same_file(&looked, &metadata) {
        return Err(io::Error::other("it changed as it was opened"));
    }
    Ok((file, metadata))
}

/// `file`'s bytes, if it is no more than the limit.
fn read_limited(file: File, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= limit).then_some(bytes))
}

/// The lines of `text`, each with its ending, or `None` past the limit.
fn lines_of(text: &str) -> Option<Vec<&str>> {
    let lines: Vec<&str> = text.split_inclusive('\n').take(MAX_LINES + 1).collect();
    (lines.len() <= MAX_LINES).then_some(lines)
}

/// Adds `lines` to `out`, each after `sign`, saying so of a last line with no ending.
fn push_lines(out: &mut String, sign: char, lines: &[&str]) {
    for line in lines {
        out.push(sign);
        out.push_str(line);
        if !line.ends_with('\n') {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

/// The groups of `ops` as a unified diff.
fn unified(old: &[&str], new: &[&str], ops: Vec<DiffOp>) -> String {
    let mut out = String::new();
    for group in group_diff_ops(ops, CONTEXT) {
        let (Some(first), Some(last)) = (group.first(), group.last()) else {
            continue;
        };
        let (old_range, new_range) = (first.old_range().start, first.new_range().start);
        let (old_end, new_end) = (last.old_range().end, last.new_range().end);
        let _ = writeln!(
            out,
            "@@ -{},{} +{},{} @@",
            old_range + 1,
            old_end - old_range,
            new_range + 1,
            new_end - new_range,
        );
        for op in group {
            match op {
                DiffOp::Equal { old_index, len, .. } => {
                    push_lines(&mut out, ' ', &old[old_index..old_index + len]);
                }
                DiffOp::Delete {
                    old_index, old_len, ..
                } => push_lines(&mut out, '-', &old[old_index..old_index + old_len]),
                DiffOp::Insert {
                    new_index, new_len, ..
                } => push_lines(&mut out, '+', &new[new_index..new_index + new_len]),
                DiffOp::Replace {
                    old_index,
                    old_len,
                    new_index,
                    new_len,
                } => {
                    push_lines(&mut out, '-', &old[old_index..old_index + old_len]);
                    push_lines(&mut out, '+', &new[new_index..new_index + new_len]);
                }
            }
        }
    }
    out
}

/// A diff from `file` to `copy`, with `deadline` as its limit. Lines start with a sign, and
/// `-` is what the file has. Empty if they say the same.
pub(super) fn diff_within(file: &str, copy: &str, deadline: Duration) -> Result<String, NoDiff> {
    let sizes = (file.len() as u64, copy.len() as u64);
    let many = NoDiff::TooManyLines {
        file: sizes.0,
        copy: sizes.1,
    };
    let (Some(old), Some(new)) = (lines_of(file), lines_of(copy)) else {
        return Err(many);
    };
    let started = Instant::now();
    let ops =
        capture_diff_slices_deadline(Algorithm::RawMyers, &old, &new, Some(started + deadline));
    if started.elapsed() >= deadline {
        return Err(NoDiff::GaveUp {
            file: sizes.0,
            copy: sizes.1,
        });
    }
    Ok(unified(&old, &new, ops))
}

/// A diff from the file at `file` to the copy, read from `copy`.
pub(super) fn diff_files(file: &Path, copy: File, copy_len: u64) -> Result<String, NoDiff> {
    let io_problem = |e: io::Error| NoDiff::Io(e.to_string());
    let (opened, metadata) = open_the_file(file).map_err(io_problem)?;
    if copy
        .metadata()
        .is_ok_and(|own| surely_same_file(&metadata, &own))
    {
        return Err(NoDiff::Io("the file is the copy itself".to_owned()));
    }
    let large = NoDiff::TooLarge {
        file: metadata.len(),
        copy: copy_len,
    };
    if metadata.len() > MAX_BYTES || copy_len > MAX_BYTES {
        return Err(large);
    }
    // Again as it is read, as either can have grown.
    let Some(old) = read_limited(opened, MAX_BYTES).map_err(io_problem)? else {
        return Err(large);
    };
    let Some(new) = read_limited(copy, MAX_BYTES).map_err(io_problem)? else {
        return Err(large);
    };
    let old = old.strip_prefix(BOM).unwrap_or(&old);
    let (Ok(old), Ok(new)) = (std::str::from_utf8(old), std::str::from_utf8(&new)) else {
        return Err(NoDiff::NotText);
    };
    diff_within(old, new, DEADLINE)
}

/// Whether the file at `file` holds exactly `copy`'s `copy_len` bytes, after a BOM if it has
/// one. Reads both a piece at a time, so a large file takes no more memory than a small one.
/// Anything that cannot be read is not equal, and neither is a file that is the copy itself,
/// as a link or a hard link could make it.
pub(super) fn same_bytes(file: &Path, mut copy: File, copy_len: u64) -> bool {
    let Ok((mut opened, metadata)) = open_the_file(file) else {
        return false;
    };
    let Ok(copy_metadata) = copy.metadata() else {
        return false;
    };
    let len = metadata.len();
    if surely_same_file(&metadata, &copy_metadata)
        || (len != copy_len && len != copy_len + BOM.len() as u64)
    {
        return false;
    }
    if len != copy_len {
        let mut start = [0; BOM.len()];
        if opened.read_exact(&mut start).is_err() || start.as_slice() != BOM {
            return false;
        }
    }
    let mut left = vec![0; PIECE];
    let mut right = vec![0; PIECE];
    loop {
        let Ok(read) = opened.read(&mut left) else {
            return false;
        };
        if read == 0 {
            return copy.read(&mut right).is_ok_and(|read| read == 0);
        }
        if copy.read_exact(&mut right[..read]).is_err() || left[..read] != right[..read] {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Applies a diff to `file`, to check it says what it should.
    fn lines_left(diff: &str, sign: char) -> Vec<&str> {
        diff.lines()
            .filter_map(|line| line.strip_prefix(sign))
            .collect()
    }

    #[test]
    fn it_says_nothing_of_the_same_text() {
        assert_eq!(diff_within("a\nb\n", "a\nb\n", DEADLINE), Ok(String::new()));
        assert_eq!(diff_within("", "", DEADLINE), Ok(String::new()));
    }

    #[test]
    fn it_marks_what_each_side_has_with_some_context() {
        let file = "1\n2\n3\n4\n5\n6\n7\n8\n9\n";
        let copy = "1\n2\n3\n4\nfive\n6\n7\n8\n9\n";
        assert_eq!(
            diff_within(file, copy, DEADLINE).unwrap(),
            "@@ -2,7 +2,7 @@\n 2\n 3\n 4\n-5\n+five\n 6\n 7\n 8\n"
        );
    }

    #[test]
    fn it_says_so_of_a_last_line_with_no_ending() {
        let diff = diff_within("a\n", "a", DEADLINE).unwrap();
        assert!(
            diff.contains("+a\n\\ No newline at end of file\n"),
            "{diff}"
        );
    }

    #[test]
    fn past_the_lines_it_takes_it_gives_the_sizes() {
        let copy = "\n".repeat(MAX_LINES + 1);
        assert_eq!(
            diff_within("a\n", &copy, DEADLINE),
            Err(NoDiff::TooManyLines {
                file: 2,
                copy: copy.len() as u64,
            })
        );
        assert!(diff_within("a\n", &"\n".repeat(MAX_LINES), DEADLINE).is_ok());
    }

    #[test]
    fn past_its_deadline_it_gives_the_sizes_and_no_text() {
        let lines = |start: usize| -> String {
            (start..start + 20_000).map(|i| format!("{i}\n")).collect()
        };
        let (file, copy) = (lines(0), lines(100_000));
        let result = diff_within(&file, &copy, Duration::ZERO);
        let no_diff = result.unwrap_err();
        assert!(matches!(no_diff, NoDiff::GaveUp { .. }), "{no_diff:?}");
        assert!(no_diff.to_string().contains("too different"));
        assert!(!no_diff.to_string().contains("100000"), "no text in it");
    }

    #[test]
    fn it_stays_on_a_small_stack_with_everything_changed() {
        // The stack of a thread here is the default; a recursion that grew with the changes
        // would need far more than this for so many.
        let lines = |word: &str| -> String { (0..4000).map(|i| format!("{word}{i}\n")).collect() };
        let (file, copy) = (lines("a"), lines("b"));
        let diff = std::thread::Builder::new()
            .stack_size(128 * 1024)
            .spawn(move || diff_within(&file, &copy, DEADLINE))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(lines_left(&diff.unwrap(), '-').len(), 4000);
    }

    /// A file holding `bytes`, in a directory of the test's, and the handle `same_bytes` wants.
    fn pair(test: &str, file: &[u8], copy: &[u8]) -> (std::path::PathBuf, File, u64) {
        // Canonical, as a location is.
        let temporary = fs::canonicalize(std::env::temp_dir()).unwrap();
        let directory = temporary.join(format!("oso-compare-test-{}-{test}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let (a, b) = (directory.join("file"), directory.join("copy"));
        fs::write(&a, file).unwrap();
        fs::write(&b, copy).unwrap();
        (a, File::open(b).unwrap(), copy.len() as u64)
    }

    #[test]
    fn the_same_bytes_are_equal_with_or_without_a_bom() {
        let (file, copy, len) = pair("same", b"text\n", b"text\n");
        assert!(same_bytes(&file, copy, len));
        let mut with_bom = BOM.to_vec();
        with_bom.extend(b"text\n");
        let (file, copy, len) = pair("bom", &with_bom, b"text\n");
        assert!(same_bytes(&file, copy, len));
        let (file, copy, len) = pair("empty", b"", b"");
        assert!(same_bytes(&file, copy, len));
    }

    #[test]
    fn another_byte_or_size_is_not_equal_and_neither_is_a_file_that_has_gone() {
        let (file, copy, len) = pair("byte", b"text\n", b"test\n");
        assert!(!same_bytes(&file, copy, len));
        let (file, copy, len) = pair("size", b"text\n\n", b"text\n");
        assert!(!same_bytes(&file, copy, len));
        let (file, copy, len) = pair("three", b"abctext\n", b"text\n");
        assert!(!same_bytes(&file, copy, len), "three bytes that are no BOM");
        let (file, copy, len) = pair("gone", b"text\n", b"text\n");
        fs::remove_file(&file).unwrap();
        assert!(!same_bytes(&file, copy, len));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_put_where_the_file_was_is_not_followed() {
        let (file, copy, len) = pair("swapped", b"text\n", b"text\n");
        let secret = file.with_file_name("key");
        fs::write(&secret, b"text\n").unwrap();
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&secret, &file).unwrap();
        assert!(!same_bytes(&file, copy, len));
        let (_, copy, len) = pair("swapped", b"text\n", b"text\n");
        fs::remove_file(&file).unwrap();
        std::os::unix::fs::symlink(&secret, &file).unwrap();
        assert!(matches!(diff_files(&file, copy, len), Err(NoDiff::Io(_))));
    }

    #[cfg(unix)]
    #[test]
    fn a_file_that_is_the_copy_itself_is_not_equal_to_it() {
        let (file, copy, len) = pair("itself", b"text\n", b"text\n");
        let same = file.with_file_name("copy");
        fs::remove_file(&file).unwrap();
        fs::hard_link(&same, &file).unwrap();
        assert!(!same_bytes(&file, copy, len), "the only text there is");
        let copy = File::open(&same).unwrap();
        assert!(matches!(diff_files(&file, copy, len), Err(NoDiff::Io(_))));
    }

    #[test]
    fn a_large_equal_file_is_read_in_pieces() {
        let bytes = vec![b'x'; PIECE * 2 + 7];
        let (file, copy, len) = pair("large", &bytes, &bytes);
        assert!(same_bytes(&file, copy, len));
        let mut other = bytes.clone();
        *other.last_mut().unwrap() = b'y';
        let (file, copy, len) = pair("large-differs", &bytes, &other);
        assert!(!same_bytes(&file, copy, len));
    }

    #[test]
    fn a_diff_of_files_leaves_out_a_bom_and_refuses_what_is_not_text() {
        let mut with_bom = BOM.to_vec();
        with_bom.extend(b"a\n");
        let (file, copy, len) = pair("diff-bom", &with_bom, b"a\n");
        assert_eq!(diff_files(&file, copy, len), Ok(String::new()));
        let (file, copy, len) = pair("not-text", b"\xff\n", b"a\n");
        assert_eq!(diff_files(&file, copy, len), Err(NoDiff::NotText));
    }

    #[test]
    fn what_a_diff_keeps_is_the_two_sides() {
        // Short lines from a small alphabet, the same on every run, so that lines repeat.
        let mut state: u64 = 0x2545_f491_4f6c_dd1d;
        let mut next = move |below: u64| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            state % below
        };
        for _ in 0..300 {
            let mut text = |lines: u64| -> String {
                (0..lines)
                    .map(|_| ["\n", "a\n", "b\n", "ab\n"][next(4) as usize])
                    .collect()
            };
            let (lines_file, lines_copy) = (30, 30);
            let (file, copy) = (text(lines_file), text(lines_copy));
            let diff = diff_within(&file, &copy, DEADLINE).unwrap();
            assert_eq!(diff.is_empty(), file == copy);
            // Every removed line is one of the file's, and every added one is the copy's.
            for line in lines_left(&diff, '-') {
                assert!(file.lines().any(|other| other == line));
            }
            for line in lines_left(&diff, '+') {
                assert!(copy.lines().any(|other| other == line));
            }
        }
    }
}
