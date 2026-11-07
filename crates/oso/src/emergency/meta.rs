//! Reading a copy's metadata (ADR 0062, ADR 0064). What is in the directory is untrusted, so
//! this takes bytes and returns a value or nothing, and a path in it stays as it was written:
//! it is compared with what a buffer would write, and decoded only to be shown or matched
//! against the secret patterns. Nothing here touches the disk.
//!
//! Must not depend on: `app`, `dispatch` or winit, or the file system.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use oso_core::decode::LineEnding;

use super::write::META_HEADER;
use crate::saver::all_digits;

/// The most of a `.meta` that is read, as ADR 0062 says.
pub(super) const MAX_META: u64 = 64 * 1024;

/// What a `.meta` says. `path` and `location` are percent-encoded, as they were written.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Meta {
    /// The buffer's name, decoded to be shown.
    pub(super) name: String,
    pub(super) path: Option<String>,
    pub(super) location: Option<String>,
    /// Dev tooling withheld this text when it was copied.
    pub(super) withheld: bool,
    pub(super) bom: bool,
    pub(super) line_ending: Option<LineEnding>,
    pub(super) file_bytes: Option<u64>,
    pub(super) file_modified: Option<SystemTime>,
    pub(super) bytes: u64,
    pub(super) written: Option<u64>,
    /// `panic` or `exit`, or `None` for anything else.
    pub(super) why: Option<&'static str>,
}

impl Meta {
    /// The paths it names, decoded for the secret patterns. A byte that is not UTF-8 becomes
    /// U+FFFD, which keeps every name a pattern can match, and no file is opened by these.
    pub(super) fn decoded_paths(&self) -> Vec<PathBuf> {
        [&self.path, &self.location]
            .into_iter()
            .flatten()
            .filter_map(|encoded| decoded(encoded))
            .map(PathBuf::from)
            .collect()
    }

    /// What to show of where it belongs: the path, or the buffer's name if it had no file.
    pub(super) fn shown(&self) -> String {
        self.path
            .as_ref()
            .or(self.location.as_ref())
            .and_then(|encoded| decoded(encoded))
            .unwrap_or_else(|| self.name.clone())
    }
}

/// What `encoded` says, or `None` if a `%` is not followed by two hex digits.
pub(super) fn decoded(encoded: &str) -> Option<String> {
    let bytes = encoded.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while let Some(byte) = bytes.get(index) {
        if *byte == b'%' {
            let digits = bytes.get(index + 1..index + 3)?;
            // `from_str_radix` also takes a sign.
            if !digits.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            let digits = std::str::from_utf8(digits).ok()?;
            out.push(u8::from_str_radix(digits, 16).ok()?);
            index += 3;
        } else {
            out.push(*byte);
            index += 1;
        }
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// A number as `meta_text` writes one: digits only, as `str::parse` also takes a `+`.
fn number(value: &str) -> Option<u64> {
    all_digits(value).then(|| value.parse().ok()).flatten()
}

/// A modification time, as `<seconds>.<nine digits>`.
fn modified(value: &str) -> Option<SystemTime> {
    let (seconds, nanos) = value.split_once('.')?;
    if nanos.len() != 9 {
        return None;
    }
    let nanos = u32::try_from(number(nanos)?).ok()?;
    let since = Duration::new(number(seconds)?, nanos);
    SystemTime::UNIX_EPOCH.checked_add(since)
}

/// Sets `slot`, or fails if it was set already, as a key said twice says nothing clearly.
fn once<T>(slot: &mut Option<T>, value: T) -> Option<()> {
    slot.is_none().then(|| *slot = Some(value))
}

/// The keys as they arrive, each at most once.
#[derive(Default)]
struct Seen {
    name: Option<String>,
    path: Option<String>,
    location: Option<String>,
    bom: Option<bool>,
    withheld: Option<bool>,
    line_ending: Option<Option<LineEnding>>,
    file_bytes: Option<u64>,
    file_modified: Option<SystemTime>,
    bytes: Option<u64>,
    written: Option<u64>,
    why: Option<Option<&'static str>>,
}

impl Seen {
    /// Takes in one line. `None` if it spoils the whole file. A key not known is passed by.
    fn line(&mut self, line: &str) -> Option<()> {
        // A key this does not know may hold anything, or nothing.
        let (key, value) = line.split_once(' ').unwrap_or((line, ""));
        match key {
            "name" => once(&mut self.name, decoded(value)?),
            "path" => {
                decoded(value)?;
                once(&mut self.path, value.to_owned())
            }
            "location" => {
                decoded(value)?;
                once(&mut self.location, value.to_owned())
            }
            "bom" => once(&mut self.bom, value == "true"),
            "withheld" => once(&mut self.withheld, value != "false"),
            "line-ending" => once(
                &mut self.line_ending,
                match value {
                    "lf" => Some(LineEnding::Lf),
                    "crlf" => Some(LineEnding::Crlf),
                    _ => None,
                },
            ),
            "file-bytes" => once(&mut self.file_bytes, number(value)?),
            "file-modified" => once(&mut self.file_modified, modified(value)?),
            "bytes" => once(&mut self.bytes, number(value)?),
            "written" => once(&mut self.written, number(value)?),
            "why" => once(
                &mut self.why,
                match value {
                    "panic" => Some("panic"),
                    "exit" => Some("exit"),
                    _ => None,
                },
            ),
            _ => Some(()),
        }
    }
}

/// What `bytes` say, or `None` if they are not a metadata file this version can read: more
/// than it reads, a first line it does not know, anything but printable ASCII, a known key
/// twice, a bad value, or no `bytes`.
pub(super) fn parse(bytes: &[u8]) -> Option<Meta> {
    if bytes.len() as u64 > MAX_META {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let printable = |c: char| c == '\n' || (' '..='~').contains(&c);
    if !text.chars().all(printable) {
        return None;
    }
    let mut lines = text.strip_suffix('\n').unwrap_or(text).split('\n');
    if lines.next() != Some(META_HEADER) {
        return None;
    }
    let mut seen = Seen::default();
    for line in lines {
        seen.line(line)?;
    }
    Some(Meta {
        name: seen.name.unwrap_or_else(|| "?".to_owned()),
        path: seen.path,
        location: seen.location,
        // Only a copy that says it is not withheld is not.
        withheld: seen.withheld.unwrap_or(true),
        bom: seen.bom.unwrap_or(false),
        line_ending: seen.line_ending.flatten(),
        file_bytes: seen.file_bytes,
        file_modified: seen.file_modified,
        bytes: seen.bytes?,
        written: seen.written,
        why: seen.why.flatten(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emergency::write::meta_text;
    use crate::emergency::{Identity, Why};
    use oso_core::buffer::FileStamp;
    use std::path::Path;

    fn identity(path: Option<&str>) -> Identity {
        Identity {
            name: "メモ 1.txt".to_owned(),
            path: path.map(PathBuf::from),
            location: path.map(|path| Path::new("/real").join(path.trim_start_matches('/'))),
            withheld: false,
            bom: true,
            line_ending: LineEnding::Crlf,
        }
    }

    #[test]
    fn what_is_written_parses_back() {
        let modified = SystemTime::UNIX_EPOCH + Duration::new(1_789_999_000, 123_456_789);
        let stamp = FileStamp {
            modified,
            len: 1200,
            mode: 0o644,
        };
        let text = meta_text(
            &identity(Some("/home/me/a b.txt")),
            Some(stamp),
            1234,
            Why::Panic,
            1_790_000_000,
        );
        let meta = parse(text.as_bytes()).unwrap();
        assert_eq!(meta.name, "メモ 1.txt");
        assert_eq!(meta.path.as_deref(), Some("/home/me/a%20b.txt"));
        assert_eq!(meta.location.as_deref(), Some("/real/home/me/a%20b.txt"));
        assert!(meta.bom);
        assert_eq!(meta.line_ending, Some(LineEnding::Crlf));
        assert_eq!(meta.file_bytes, Some(1200));
        assert_eq!(meta.file_modified, Some(modified));
        assert_eq!(meta.bytes, 1234);
        assert_eq!(meta.written, Some(1_790_000_000));
        assert_eq!(meta.why, Some("panic"));
        assert_eq!(meta.shown(), "/home/me/a b.txt");
    }

    #[test]
    fn a_buffer_with_no_file_shows_its_name() {
        let text = meta_text(&identity(None), None, 3, Why::Exit, 5);
        let meta = parse(text.as_bytes()).unwrap();
        assert_eq!(meta.path, None);
        assert_eq!(meta.shown(), "メモ 1.txt");
        assert!(meta.decoded_paths().is_empty());
        assert_eq!(meta.why, Some("exit"));
    }

    fn with(line: &str) -> Option<Meta> {
        parse(format!("{META_HEADER}\nbytes 1\n{line}\n").as_bytes())
    }

    #[test]
    fn keys_it_does_not_know_are_passed_by_and_a_line_with_no_value_spoils_it() {
        assert!(with("next-version-key something").is_some());
        assert!(with("no-value").is_some());
        assert!(with("").is_some());
        assert!(with("bytes").is_none(), "twice, and no number");
        assert!(with("file-bytes").is_none(), "a known key with no value");
    }

    #[test]
    fn a_first_line_it_does_not_know_offers_nothing() {
        assert!(parse(b"oso-emergency-copy 2\nbytes 1\n").is_none());
        assert!(parse(b"bytes 1\n").is_none());
        assert!(parse(b"").is_none());
    }

    #[test]
    fn a_known_key_twice_or_a_bad_value_spoils_it() {
        assert!(with("bytes 2").is_none(), "twice");
        assert!(with("name a\nname b").is_none());
        assert!(with("file-bytes x").is_none());
        assert!(with("file-bytes +5").is_none());
        assert!(with("file-bytes -5").is_none());
        assert!(with("written 99999999999999999999999").is_none());
        assert!(with("path /a%2").is_none());
        assert!(with("path /a%zz").is_none());
        assert!(with("path /a%+1").is_none());
        assert!(with("location /a%").is_none());
        assert!(with("name %ff%").is_none());
        assert!(with("file-modified 5").is_none());
        assert!(with("file-modified 5.1").is_none());
        assert!(with("file-modified 5.1000000000").is_none());
        assert!(with("file-modified 18446744073709551615.000000000").is_none());
        assert!(
            parse(format!("{META_HEADER}\nname a\n").as_bytes()).is_none(),
            "no bytes"
        );
    }

    #[test]
    fn only_a_copy_that_says_it_is_not_withheld_is_not() {
        assert!(with("name a").unwrap().withheld, "it does not say");
        assert!(with("withheld true").unwrap().withheld);
        assert!(with("withheld maybe").unwrap().withheld);
        assert!(!with("withheld false").unwrap().withheld);
    }

    #[test]
    fn only_printable_ascii_counts() {
        assert!(with("name \u{1b}[2J").is_none());
        assert!(with("name 日本語").is_none());
        assert!(with("name a\tb").is_none());
        assert!(parse(b"oso-emergency-copy 1\nbytes 1\nname \xff\n").is_none());
        assert!(with("path a\r").is_none());
    }

    #[test]
    fn it_reads_no_more_than_the_limit() {
        let mut bytes = format!("{META_HEADER}\nbytes 1\n").into_bytes();
        let room = MAX_META as usize - bytes.len();
        bytes.extend(std::iter::repeat_n(b'x', room - 2));
        bytes.extend(b" \n");
        assert_eq!(bytes.len() as u64, MAX_META);
        assert!(parse(&bytes).is_some());
        bytes.push(b'y');
        assert!(parse(&bytes).is_none());
    }

    #[test]
    fn words_it_does_not_know_read_as_nothing() {
        let meta = with("why reasons\nline-ending weird\nbom maybe").unwrap();
        assert_eq!(meta.why, None);
        assert_eq!(meta.line_ending, None);
        assert!(!meta.bom);
    }

    #[test]
    fn bytes_that_are_not_utf_8_are_kept_apart_from_what_is_compared() {
        let meta = with("path /a/%FF.txt").unwrap();
        assert_eq!(meta.path.as_deref(), Some("/a/%FF.txt"));
        assert_eq!(meta.shown(), "/a/\u{fffd}.txt");
        assert_eq!(meta.decoded_paths(), [PathBuf::from("/a/\u{fffd}.txt")]);
    }

    /// Bytes that look like nothing in particular, the same on every run.
    fn noise(seed: u64, len: usize, choose: impl Fn(u8) -> u8) -> Vec<u8> {
        let mut state = seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1;
        (0..len)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                choose((state >> 32) as u8)
            })
            .collect()
    }

    #[test]
    fn any_bytes_give_a_value_or_nothing() {
        for seed in 0..2000 {
            let _ = parse(&noise(seed, (seed % 512) as usize, |byte| byte));
        }
    }

    #[test]
    fn any_lines_give_a_value_or_nothing() {
        // Printable, with line breaks and what the parser looks for, so that it gets further.
        const ALPHABET: &[u8] = b"namepthlocbyws-%0aF. \n\n\n";
        for seed in 0..2000 {
            let body = noise(seed, (seed % 400) as usize, |byte| {
                ALPHABET[usize::from(byte) % ALPHABET.len()]
            });
            let mut text = format!("{META_HEADER}\nbytes 1\n").into_bytes();
            text.extend(body);
            let _ = parse(&text);
        }
    }

    #[test]
    fn any_name_written_reads_back_shown_as_it_was() {
        for name in [
            "",
            "a b",
            "日本語.txt",
            "a\nb\u{1b}[2J",
            "%41",
            "\u{202e}",
            "*scratch*",
            "e\u{301}",
        ] {
            let mut identity = identity(None);
            identity.name = name.to_owned();
            let text = meta_text(&identity, None, 1, Why::Panic, 1);
            assert_eq!(
                parse(text.as_bytes()).map(|meta| meta.name).as_deref(),
                Some(name)
            );
        }
    }
}
