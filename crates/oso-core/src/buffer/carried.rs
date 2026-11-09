//! What `oso dev` carries of a buffer with unsaved text to the instance that replaces this
//! one (ADR 0090): the text, and what a save needs to know of the file beside it.
//!
//! Must not depend on: file I/O or the control socket; the host moves these facts.

use std::path::PathBuf;

use super::{Baseline, Buffer, FileStamp};
use crate::decode::FileInfo;
use crate::load::LoadSnapshot;

/// What a buffer knows of its file beside its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileFacts {
    pub info: FileInfo,
    /// Where the file is and what was there, which a save checks.
    pub baseline: Option<Baseline>,
    /// A save found the file changed, and nothing has written it since.
    pub changed_on_disk: bool,
    /// The file that save found, which a save the person agrees to may replace.
    pub save_over: Option<FileStamp>,
    /// The file's own spelling, if something else may have been typed.
    pub own_spelling: Option<PathBuf>,
}

impl Buffer {
    /// What this buffer knows of its file beside its text.
    pub fn file_facts(&self) -> FileFacts {
        FileFacts {
            info: self.info,
            baseline: self.baseline.clone(),
            changed_on_disk: self.changed_on_disk,
            save_over: self.save_over,
            own_spelling: self.own_spelling.clone(),
        }
    }

    /// Takes the whole text another instance held unsaved, with what it knew of the file.
    /// The buffer is then modified, has no history to undo, and can be edited. Its verdict
    /// stays as it is: a visited file is judged again by the host. O(1).
    pub fn apply_carried(&mut self, snapshot: LoadSnapshot, facts: FileFacts) {
        self.apply_load(LoadSnapshot {
            done: true,
            ..snapshot
        });
        // No state of this text is on the disk, as far as this instance knows.
        self.saved = self.new_content();
        let FileFacts {
            info,
            baseline,
            changed_on_disk,
            save_over,
            own_spelling,
        } = facts;
        self.info = info;
        self.baseline = baseline;
        self.changed_on_disk = changed_on_disk;
        self.save_over = save_over;
        self.own_spelling = own_spelling;
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::SystemTime;

    use super::FileFacts;
    use crate::buffer::{Baseline, Buffer, FileStamp, Verdict};
    use crate::decode::{FileInfo, LineEnding};
    use crate::display_map::TabWidth;
    use crate::load::Builder;

    fn snapshot(text: &str) -> crate::load::LoadSnapshot {
        let mut builder = Builder::new(None, TabWidth::default());
        builder.push_text(text);
        builder.snapshot()
    }

    fn facts() -> FileFacts {
        let stamp = FileStamp {
            identity: Some((1, 2)),
            modified: SystemTime::UNIX_EPOCH,
            len: 3,
            mode: 0o644,
        };
        FileFacts {
            info: FileInfo {
                bom: true,
                line_ending: LineEnding::Crlf,
                binary: false,
            },
            baseline: Some(Baseline {
                location: PathBuf::from("/tmp/real/a.txt"),
                file: Some(stamp),
            }),
            changed_on_disk: true,
            save_over: Some(stamp),
            own_spelling: Some(PathBuf::from("/tmp/real/A.txt")),
        }
    }

    #[test]
    fn carried_text_is_modified_editable_and_keeps_what_was_known_of_the_file() {
        let mut buffer = Buffer::visiting(PathBuf::from("/tmp/a.txt"));
        buffer.apply_carried(snapshot("unsaved\n"), facts());
        assert_eq!(buffer.text().to_string(), "unsaved\n");
        assert!(buffer.is_modified());
        assert_eq!(buffer.loading(), None);
        assert_eq!(buffer.read_only(), None);
        assert_eq!(buffer.file_facts(), facts());
        assert_eq!(buffer.save_over(), facts().save_over);
        assert_eq!(
            buffer.verdict(),
            Verdict::Unjudged,
            "the host judges the path again"
        );
    }

    #[test]
    fn a_carried_scratch_needs_no_file() {
        let mut buffer = Buffer::new("*scratch*");
        buffer.apply_carried(snapshot("notes"), FileFacts::default());
        assert!(buffer.is_modified());
        assert_eq!(buffer.verdict(), Verdict::Ordinary);
        assert_eq!(buffer.file_facts(), FileFacts::default());
    }
}
