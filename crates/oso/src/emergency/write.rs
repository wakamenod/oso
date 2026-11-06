//! Putting one emergency copy on disk (ADR 0062).
//!
//! Must not depend on: `app`, winit or the editor; this gets a snapshot, and knows of no
//! thread but its own.
//!
//! A copy is two files in the private directory, under one stem:
//!
//! - `<stem>.txt` holds the buffer's text exactly: no BOM, no final newline added, no
//!   conversion. Anyone can open it.
//! - `<stem>.meta` says where it belongs, in the format of [`meta_text`]. It is renamed in
//!   last, so that its presence means the copy is whole.
//!
//! The stem is `<hash>-<seconds>-<process>-<attempt>`, where the hash is FNV-1a of the real
//! place the path led, or the path if that could not be told, or the buffer's name if it
//! has no file. So recovery can find the copies for a path without any path's text being
//! in a name, where it could be too long for the system. A collision is harmless, as
//! recovery compares the metadata too. Both files start under a temporary name made with
//! `create_new` and mode `0600`, and a free stem is never one already there, so that no
//! later copy can touch an earlier one.
//!
//! A copy is only ever offered. Whatever is in this directory, recovery must not let it
//! choose a path to write to without the person.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::time::SystemTime;

use oso_core::buffer::FileStamp;
use oso_core::rope::Rope;

use super::{Entry, Identity, Why};
use crate::saver::{PRIVATE_MODE, Temporary, WRITE_SIZE, sync_directory, unless_unsupported};

/// The first line of every metadata file, with the format's version.
const META_HEADER: &str = "oso-emergency-copy 1";

/// How many stems to try before giving up.
const STEM_ATTEMPTS: u32 = 64;

/// Writes a copy of `entry` in `directory`. `tick` is called as it gets somewhere.
pub(super) fn write_copy(
    directory: &Path,
    entry: &Entry,
    why: Why,
    now: SystemTime,
    tick: &dyn Fn(),
) -> io::Result<()> {
    let Some(text) = &entry.text else {
        return Err(io::Error::other("this text may not be copied"));
    };
    let seconds = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let stem = free_stem(directory, &entry.identity, seconds)?;

    let (text_file, file) = Temporary::create(directory, PRIVATE_MODE)?;
    write_text(&file, text, tick)?;
    unless_unsupported(file.sync_all())?;
    tick();
    drop(file);
    if fs::symlink_metadata(text_file.path())?.len() != text.len() as u64 {
        return Err(io::Error::other(
            "the file system kept less than was written",
        ));
    }

    let (meta_file, mut file) = Temporary::create(directory, PRIVATE_MODE)?;
    let meta = meta_text(&entry.identity, entry.file, text.len(), why, seconds);
    file.write_all(meta.as_bytes())?;
    unless_unsupported(file.sync_all())?;
    drop(file);

    text_file.rename_over(&directory.join(format!("{stem}.txt")))?;
    meta_file.rename_over(&directory.join(format!("{stem}.meta")))?;
    // The copy is there either way, and all a failure could cost is a power cut's worth.
    let _ = sync_directory(directory);
    Ok(())
}

fn write_text(file: &File, text: &Rope, tick: &dyn Fn()) -> io::Result<()> {
    let mut out = BufWriter::with_capacity(WRITE_SIZE, file);
    let mut since_tick = 0;
    for chunk in text.chunks() {
        out.write_all(chunk.as_bytes())?;
        since_tick += chunk.len();
        if since_tick >= WRITE_SIZE {
            since_tick = 0;
            tick();
        }
    }
    out.flush()
}

/// The first stem with nothing under it, so that a rename replaces nothing.
pub(super) fn free_stem(directory: &Path, identity: &Identity, seconds: u64) -> io::Result<String> {
    let hash = fnv1a(key_of(identity));
    let process = std::process::id();
    for attempt in 0..STEM_ATTEMPTS {
        let stem = format!("{hash:016x}-{seconds}-{process}-{attempt}");
        let free = |extension: &str| {
            matches!(
                fs::symlink_metadata(directory.join(format!("{stem}.{extension}"))),
                Err(e) if e.kind() == io::ErrorKind::NotFound
            )
        };
        if free("txt") && free("meta") {
            return Ok(stem);
        }
    }
    Err(io::Error::other("could not find a free name for a copy"))
}

/// What the hash in a copy's name is taken of.
pub(super) fn key_of(identity: &Identity) -> &[u8] {
    identity
        .location
        .as_deref()
        .or(identity.path.as_deref())
        .map_or(identity.name.as_bytes(), |path| {
            path.as_os_str().as_encoded_bytes()
        })
}

/// FNV-1a, 64 bits. `DefaultHasher` may change with the toolchain, and a copy must be
/// found by whatever build reads it.
pub(super) fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The metadata of a copy: a line naming the format, then one `key value` per line, in this
/// order. A line is left out when the buffer has nothing to say there.
///
/// ```text
/// oso-emergency-copy 1
/// name notes.txt
/// path /home/me/link%20to/notes.txt
/// location /home/me/real/notes.txt
/// bom false
/// line-ending lf
/// file-bytes 1200
/// file-modified 1789999000.123456789
/// bytes 1234
/// written 1790000000
/// why panic
/// ```
///
/// Names and paths are percent-encoded byte by byte, so that any file name fits on one line
/// of ASCII and no terminal sees its own control characters. A reader finds a path by
/// encoding its own and comparing, so it need not trust or even decode what is here.
/// `location` says where the buffer would have saved, and `file-bytes` and `file-modified`
/// what was there as the buffer last read or wrote it, so that recovery can tell whether the
/// file has changed since. `bytes` is the length of the text, and `written` the seconds since
/// the Unix epoch.
pub(super) fn meta_text(
    identity: &Identity,
    file: Option<FileStamp>,
    bytes: usize,
    why: Why,
    seconds: u64,
) -> String {
    let mut lines = vec![
        META_HEADER.to_owned(),
        format!("name {}", encoded(identity.name.as_bytes())),
    ];
    if let Some(path) = &identity.path {
        let path = encoded(path.as_os_str().as_encoded_bytes());
        lines.push(format!("path {path}"));
    }
    if let Some(location) = &identity.location {
        let location = encoded(location.as_os_str().as_encoded_bytes());
        lines.push(format!("location {location}"));
    }
    lines.push(format!("bom {}", identity.bom));
    lines.push(format!("line-ending {}", identity.line_ending.as_str()));
    if let Some(file) = file {
        lines.push(format!("file-bytes {}", file.len));
        if let Ok(since) = file.modified.duration_since(SystemTime::UNIX_EPOCH) {
            let (seconds, nanos) = (since.as_secs(), since.subsec_nanos());
            lines.push(format!("file-modified {seconds}.{nanos:09}"));
        }
    }
    lines.push(format!("bytes {bytes}"));
    lines.push(format!("written {seconds}"));
    lines.push(format!("why {}", why.as_str()));
    lines.push(String::new());
    lines.join("\n")
}

/// `bytes` with everything but plain ASCII written as `%XX`.
fn encoded(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len());
    for byte in bytes {
        if byte.is_ascii_alphanumeric() || b"/._-~*:".contains(byte) {
            out.push(char::from(*byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}
