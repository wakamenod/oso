//! A server's workspace edit placed on the session thread (ADR 0138, step 13): each file's edits
//! in the text the server has of the document there, against the revisions it had as the
//! request went out, and the others kept as the server counts them.
//!
//! Must not depend on: the process or the pipes; this places values, and the UI applies them.
//!
//! - An edit that names a version must name the text the server has, and a document whose text
//!   changed since the request went out refuses the whole edit (Q8): its ranges were the
//!   server's for the text before.
//! - A path oso never opens, under an automount or with a `..`, refuses the whole edit.

use std::collections::HashMap;
use std::path::Path;

use crate::position::{self, Encoding};
use crate::protocol::WireEdit;
use crate::uri;

use super::super::{OpenEdit, ServerEdit, UnsentEdit};
use super::Session;
use super::asks::automounted;

impl Session {
    /// `edit` placed: each file's edits in the text the server has of the document at its path,
    /// merged by document, and the others' kept as the server counts them; or why it cannot be
    /// applied. `base` holds the revisions the server had as the request went out.
    pub(super) fn placed(
        &self,
        encoding: Encoding,
        edit: WireEdit,
        base: Option<&HashMap<u64, u64>>,
    ) -> Result<ServerEdit, String> {
        let sent = self.documents.sent_by_path();
        let mut placed = ServerEdit::default();
        for file in edit.files {
            let path = (uri::to_path(&file.uri))
                .filter(|path| !automounted(path))
                .ok_or("it names a file oso does not edit")?;
            let Some(&(document, revision, text)) = sent.get(Path::new(&path)) else {
                match placed.unsent.iter_mut().find(|kept| kept.path == path) {
                    Some(kept) => kept.edits.extend(file.edits),
                    None => placed.unsent.push(UnsentEdit {
                        path,
                        edits: file.edits,
                        encoding,
                    }),
                }
                continue;
            };
            if base.and_then(|base| base.get(&document)) != Some(&revision) {
                return Err("a buffer it edits changed while the server answered".to_owned());
            }
            if file.version.is_some() && file.version != self.documents.sent_version(document) {
                return Err("it was made for an older text of a buffer".to_owned());
            }
            let edits = (file.edits.into_iter())
                .map(|(start, end, new)| (position::to_range(text, start, end, encoding), new));
            match placed
                .open
                .iter_mut()
                .find(|kept| kept.document == document)
            {
                Some(kept) => kept.edits.extend(edits),
                None => placed.open.push(OpenEdit {
                    document,
                    revision,
                    edits: edits.collect(),
                }),
            }
        }
        Ok(placed)
    }
}
