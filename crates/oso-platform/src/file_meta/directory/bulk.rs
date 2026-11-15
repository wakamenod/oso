//! Listing a directory with what each entry is on macOS, in batches from `getattrlistbulk`,
//! which reads names and attributes together and follows no link (ADR 0134).
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.
//!
//! Only the call is `unsafe`. What it wrote is read back from a byte slice, each field checked
//! to lie inside its entry, so a short or odd reply ends the listing with an error rather than
//! reading past it.

use std::ffi::OsString;
use std::io;
use std::os::fd::RawFd;
use std::os::unix::ffi::OsStringExt;

use super::super::{EntryKind, EntryMeta};

/// The bytes asked for in one call: room for some hundreds of entries.
const BUFFER_BYTES: usize = 128 * 1024;

/// `ATTR_CMN_ERROR` from `<sys/attr.h>`, which `libc` does not have.
const ATTR_CMN_ERROR: u32 = 0x2000_0000;

/// The common attributes asked for, in the order the reply packs them after the returned set,
/// the error and the name: by their bits, low first.
const COMMON: u32 = libc::ATTR_CMN_RETURNED_ATTRS
    | libc::ATTR_CMN_NAME
    | ATTR_CMN_ERROR
    | libc::ATTR_CMN_OBJTYPE
    | libc::ATTR_CMN_MODTIME
    | libc::ATTR_CMN_OWNERID
    | libc::ATTR_CMN_GRPID
    | libc::ATTR_CMN_ACCESSMASK;

/// `vtype` values from `<sys/vnode.h>`.
const VREG: u32 = 1;
const VDIR: u32 = 2;
const VBLK: u32 = 3;
const VCHR: u32 = 4;
const VLNK: u32 = 5;
const VSOCK: u32 = 6;
const VFIFO: u32 = 7;

/// The bytes of `attribute_set_t`: five `u32` groups.
const SET_BYTES: usize = 20;

/// Lists the directory `fd` from its start for [`super::list_with_metadata`].
pub(super) fn list(
    fd: RawFd,
    user: u32,
    each: &mut dyn FnMut(io::Result<EntryMeta>) -> bool,
) -> io::Result<()> {
    // SAFETY: `fd` is an open descriptor for the call; moving its place touches no memory.
    let rewound = unsafe { libc::lseek(fd, 0, libc::SEEK_SET) };
    if rewound < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut asked = libc::attrlist {
        bitmapcount: libc::ATTR_BIT_MAP_COUNT,
        reserved: 0,
        commonattr: COMMON,
        volattr: 0,
        dirattr: 0,
        fileattr: libc::ATTR_FILE_DATALENGTH,
        forkattr: 0,
    };
    let mut buffer = vec![0u8; BUFFER_BYTES];
    loop {
        // SAFETY: `fd` is open for the call, `asked` is a whole `attrlist` that outlives it, and
        // `buffer` has room for the `buffer.len()` bytes the call may write.
        let count = unsafe {
            libc::getattrlistbulk(
                fd,
                (&raw mut asked).cast(),
                buffer.as_mut_ptr().cast(),
                buffer.len(),
                0,
            )
        };
        let Ok(count) = usize::try_from(count) else {
            return Err(io::Error::last_os_error());
        };
        if count == 0 {
            return Ok(());
        }
        let mut rest = &buffer[..];
        for _ in 0..count {
            let (entry, after) = next_entry(rest)?;
            rest = after;
            let Some(read) = read_entry(entry, user) else {
                continue;
            };
            if !each(read) {
                return Ok(());
            }
        }
    }
}

/// The error for a reply that does not hold what it says.
fn malformed() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "a directory's listing was malformed",
    )
}

/// One entry from the front of `bytes`, by the length it starts with, and what follows it.
fn next_entry(bytes: &[u8]) -> io::Result<(&[u8], &[u8])> {
    let length = u32_at(bytes, 0).ok_or_else(malformed)?;
    let length = usize::try_from(length).map_err(|_| malformed())?;
    if length < 4 || length > bytes.len() {
        return Err(malformed());
    }
    Ok(bytes.split_at(length))
}

fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(u32::from_ne_bytes(field.try_into().ok()?))
}

fn i32_at(bytes: &[u8], at: usize) -> Option<i32> {
    let field = bytes.get(at..at.checked_add(4)?)?;
    Some(i32::from_ne_bytes(field.try_into().ok()?))
}

fn i64_at(bytes: &[u8], at: usize) -> Option<i64> {
    let field = bytes.get(at..at.checked_add(8)?)?;
    Some(i64::from_ne_bytes(field.try_into().ok()?))
}

/// Reads one entry: `None` for `.` and `..`, an error for one the system could not look at or
/// that does not hold what it says.
fn read_entry(entry: &[u8], user: u32) -> Option<io::Result<EntryMeta>> {
    match parse(entry, user) {
        Ok(Some(meta)) => Some(Ok(meta)),
        Ok(None) => None,
        Err(error) => Some(Err(error)),
    }
}

fn parse(entry: &[u8], user: u32) -> io::Result<Option<EntryMeta>> {
    let common = u32_at(entry, 4).ok_or_else(malformed)?;
    let file = u32_at(entry, 4 + 12).ok_or_else(malformed)?;
    let mut at = 4 + SET_BYTES;
    if common & ATTR_CMN_ERROR != 0 {
        let error = u32_at(entry, at).ok_or_else(malformed)?;
        at += 4;
        if error != 0 {
            let code = i32::try_from(error).map_err(|_| malformed())?;
            return Err(io::Error::from_raw_os_error(code));
        }
    }
    if common & libc::ATTR_CMN_NAME == 0 {
        return Err(malformed());
    }
    let name = name_at(entry, at).ok_or_else(malformed)?;
    at += 8;
    if name == b"." || name == b".." {
        return Ok(None);
    }
    let mut kind = EntryKind::Other;
    let mut type_bits = 0;
    if common & libc::ATTR_CMN_OBJTYPE != 0 {
        let vtype = u32_at(entry, at).ok_or_else(malformed)?;
        at += 4;
        (kind, type_bits) = match vtype {
            VREG => (EntryKind::File, u32::from(libc::S_IFREG)),
            VDIR => (EntryKind::Directory, u32::from(libc::S_IFDIR)),
            VLNK => (EntryKind::Link, u32::from(libc::S_IFLNK)),
            VBLK => (EntryKind::Other, u32::from(libc::S_IFBLK)),
            VCHR => (EntryKind::Other, u32::from(libc::S_IFCHR)),
            VSOCK => (EntryKind::Other, u32::from(libc::S_IFSOCK)),
            VFIFO => (EntryKind::Other, u32::from(libc::S_IFIFO)),
            _ => (EntryKind::Other, 0),
        };
    }
    let mut modified = None;
    if common & libc::ATTR_CMN_MODTIME != 0 {
        let seconds = i64_at(entry, at).ok_or_else(malformed)?;
        let nanos = i64_at(entry, at + 8).ok_or_else(malformed)?;
        at += 16;
        modified = super::time_of(seconds, nanos);
    }
    let mut uid = None;
    if common & libc::ATTR_CMN_OWNERID != 0 {
        uid = Some(u32_at(entry, at).ok_or_else(malformed)?);
        at += 4;
    }
    let mut gid = None;
    if common & libc::ATTR_CMN_GRPID != 0 {
        gid = Some(u32_at(entry, at).ok_or_else(malformed)?);
        at += 4;
    }
    let mut permissions = 0;
    if common & libc::ATTR_CMN_ACCESSMASK != 0 {
        permissions = u32_at(entry, at).ok_or_else(malformed)? & 0o7777;
        at += 4;
    }
    let mut len = 0;
    if file & libc::ATTR_FILE_DATALENGTH != 0 {
        len = u64::try_from(i64_at(entry, at).ok_or_else(malformed)?).unwrap_or(0);
    }
    Ok(Some(EntryMeta {
        name: OsString::from_vec(name.to_vec()),
        kind,
        mode: type_bits | permissions,
        len,
        modified,
        owner: uid.zip(gid),
        others: uid.is_some_and(|uid| uid != user),
        attributes: 0,
    }))
}

/// The name an `attrreference_t` at `at` points to, without its NUL: the offset is from the
/// reference itself, and the length counts the NUL.
fn name_at(entry: &[u8], at: usize) -> Option<&[u8]> {
    let offset = usize::try_from(i32_at(entry, at)?).ok()?;
    let length = usize::try_from(u32_at(entry, at + 4)?).ok()?;
    let start = at.checked_add(offset)?;
    let bytes = entry.get(start..start.checked_add(length)?)?;
    let name = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    let end = name
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(name.len());
    (end > 0).then(|| &name[..end])
}

#[cfg(test)]
mod tests {
    use super::{malformed, next_entry, parse};

    /// An entry with only a name, as the reply packs it.
    fn named(name: &[u8]) -> Vec<u8> {
        let mut entry = Vec::new();
        entry.extend_from_slice(&0u32.to_ne_bytes());
        entry.extend_from_slice(&libc::ATTR_CMN_NAME.to_ne_bytes());
        entry.extend_from_slice(&[0; 16]);
        entry.extend_from_slice(&8i32.to_ne_bytes());
        let length = u32::try_from(name.len() + 1).unwrap();
        entry.extend_from_slice(&length.to_ne_bytes());
        entry.extend_from_slice(name);
        entry.push(0);
        let total = u32::try_from(entry.len()).unwrap();
        entry[..4].copy_from_slice(&total.to_ne_bytes());
        entry
    }

    #[test]
    fn a_name_alone_is_read() {
        let entry = named(b"notes.md");
        let meta = parse(&entry, 501).unwrap().unwrap();
        assert_eq!(meta.name, "notes.md");
        assert_eq!(meta.owner, None);
        assert!(!meta.others);
    }

    #[test]
    fn dot_entries_are_left_out() {
        assert!(parse(&named(b"."), 501).unwrap().is_none());
        assert!(parse(&named(b".."), 501).unwrap().is_none());
    }

    #[test]
    fn a_name_past_its_entry_is_refused() {
        let mut entry = named(b"a");
        let at = 4 + 20;
        entry[at..at + 4].copy_from_slice(&1000i32.to_ne_bytes());
        assert_eq!(parse(&entry, 501).unwrap_err().kind(), malformed().kind());
    }

    #[test]
    fn a_length_past_the_reply_is_refused() {
        let mut entry = named(b"a");
        entry[..4].copy_from_slice(&9999u32.to_ne_bytes());
        assert!(next_entry(&entry).is_err());
        assert!(next_entry(&[1, 0]).is_err());
    }
}
