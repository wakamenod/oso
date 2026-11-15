//! An entry's mode as a directory listing's mode column shows it (ADR 0134): as `ls -l` writes
//! it on Unix, as PowerShell's `Mode` column on Windows, which has no mode bits.
//!
//! Must not depend on: `control`, `privacy`, `process` or `text`.

use super::{EntryKind, EntryMeta};

impl EntryMeta {
    /// Its mode column: `drwxr-xr-x` or `-rwsr-x--T` on Unix, `d----` or `la---` on Windows.
    #[must_use]
    pub fn mode_text(&self) -> String {
        if cfg!(windows) {
            attributes_text(self.attributes, self.kind == EntryKind::Directory)
        } else {
            unix_mode_text(self.mode)
        }
    }
}

/// Unix mode bits as `ls -l` writes them.
fn unix_mode_text(mode: u32) -> String {
    let kind = match mode & 0o170_000 {
        0o040_000 => 'd',
        0o120_000 => 'l',
        0o010_000 => 'p',
        0o020_000 => 'c',
        0o060_000 => 'b',
        0o140_000 => 's',
        _ => '-',
    };
    let mut text = String::with_capacity(10);
    text.push(kind);
    // Each of owner, group and others: read, write, and run with what a set id or the sticky
    // bit puts in its place.
    let specials = [(0o4000, 's'), (0o2000, 's'), (0o1000, 't')];
    for (index, (special, letter)) in specials.into_iter().enumerate() {
        let shift = 6 - 3 * index;
        let bits = (mode >> shift) & 0o7;
        text.push(if bits & 0o4 != 0 { 'r' } else { '-' });
        text.push(if bits & 0o2 != 0 { 'w' } else { '-' });
        text.push(match (mode & special != 0, bits & 0o1 != 0) {
            (true, true) => letter,
            (true, false) => letter.to_ascii_uppercase(),
            (false, true) => 'x',
            (false, false) => '-',
        });
    }
    text
}

/// Windows attribute bits as PowerShell's `Mode` column writes them.
fn attributes_text(attributes: u32, directory: bool) -> String {
    const READ_ONLY: u32 = 0x1;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    const ARCHIVE: u32 = 0x20;
    const REPARSE_POINT: u32 = 0x400;
    let flag = |bit: u32, letter: char| if attributes & bit != 0 { letter } else { '-' };
    let first = if attributes & REPARSE_POINT != 0 {
        'l'
    } else if directory {
        'd'
    } else {
        '-'
    };
    [
        first,
        flag(ARCHIVE, 'a'),
        flag(READ_ONLY, 'r'),
        flag(HIDDEN, 'h'),
        flag(SYSTEM, 's'),
    ]
    .into_iter()
    .collect()
}

#[cfg(test)]
mod tests {
    use super::{attributes_text, unix_mode_text};

    #[test]
    fn a_mode_reads_as_ls_writes_it() {
        assert_eq!(unix_mode_text(0o040_755), "drwxr-xr-x");
        assert_eq!(unix_mode_text(0o100_644), "-rw-r--r--");
        assert_eq!(unix_mode_text(0o120_777), "lrwxrwxrwx");
        assert_eq!(unix_mode_text(0o104_755), "-rwsr-xr-x");
        assert_eq!(unix_mode_text(0o102_644), "-rw-r-Sr--");
        assert_eq!(unix_mode_text(0o041_777), "drwxrwxrwt");
        assert_eq!(unix_mode_text(0o041_776), "drwxrwxrwT");
        assert_eq!(unix_mode_text(0o010_600), "prw-------");
        assert_eq!(unix_mode_text(0o140_755), "srwxr-xr-x");
    }

    #[test]
    fn windows_attributes_read_as_powershell_writes_them() {
        assert_eq!(attributes_text(0x10, true), "d----");
        assert_eq!(attributes_text(0x20, false), "-a---");
        assert_eq!(attributes_text(0x420, false), "la---");
        assert_eq!(attributes_text(0x27, false), "-arhs");
    }
}
