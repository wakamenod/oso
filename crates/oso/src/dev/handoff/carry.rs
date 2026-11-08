//! How the texts a state carries travel (ADR 0090): `export-state` takes an O(1) snapshot
//! of each, `export-text` gives it in pieces from that snapshot, and `import-text` stages
//! the pieces in the new instance until `import-state` takes them.
//!
//! Must not depend on: `app`, `server` or sockets; the jobs here start threads of their
//! own, which reply on the channel the caller gives.
//!
//! Pieces are copied and built on background threads, so the UI thread only clones ropes
//! and looks up a snapshot. No text here ever reaches a log: [`CarriedPiece`]'s `Debug`
//! gives lengths only.

use std::collections::HashMap;
use std::fmt;
use std::sync::mpsc;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use oso_core::display_map::TabWidth;
use oso_core::load::{Builder, LoadSnapshot};
use oso_core::rope::Rope;
use oso_core::sum_tree::Bias;
use serde_json::json;

use super::super::protocol::{MAX_LINE, Reply, escaped_len};

/// The most text one restart carries, the kill ring included.
pub const MAX_CARRIED: usize = 64 * 1024 * 1024;

/// The most text in one piece, before escaping.
pub const PIECE: usize = 512 * 1024;

/// The most a piece takes escaped, leaving room for the rest of the line that carries it:
/// the id, the method and the numbers.
const PIECE_LINE: usize = MAX_LINE - 1024;

/// How long a snapshot, or staged pieces, wait for the next request.
const KEPT: Duration = Duration::from_secs(60);

/// One text a state carries: the ropes it is made of, in order, which share chunks with
/// what they were taken from.
#[derive(Clone, Default)]
pub struct Item(pub Vec<Rope>);

impl Item {
    pub fn len(&self) -> usize {
        self.0.iter().map(Rope::len).sum()
    }
}

/// What one `export-state` took a snapshot of, for `export-text`.
pub struct Exported {
    number: u64,
    items: Vec<Item>,
    at: Instant,
}

impl Exported {
    pub fn new(number: u64, items: Vec<Item>) -> Self {
        Self {
            number,
            items,
            at: Instant::now(),
        }
    }

    /// Whether this is too old for `export-text`, and can go.
    pub fn is_stale(&self) -> bool {
        self.at.elapsed() > KEPT
    }

    /// The job that copies the piece of `item` from `offset`, if this is the export asked for
    /// and it is not too old. O(the item's parts) on the caller's thread.
    pub fn piece(&self, export: u64, item: usize, offset: usize) -> Result<PieceJob, String> {
        if export != self.number || self.at.elapsed() > KEPT {
            return Err(format!("export {export} is not the latest, or is too old"));
        }
        let parts = self
            .items
            .get(item)
            .ok_or_else(|| format!("export {export} has no item {item}"))?;
        Ok(PieceJob {
            parts: parts.clone(),
            offset,
        })
    }
}

/// An `export-text` request, with a snapshot of the item it reads.
pub struct PieceJob {
    parts: Item,
    offset: usize,
}

impl PieceJob {
    /// Runs on a background thread, which replies. If the thread cannot start, the reply
    /// sender is dropped, which the client sees as an error.
    pub fn spawn(self, reply: mpsc::Sender<Reply>) {
        let spawned = thread::Builder::new()
            .name("oso-export-text".into())
            .spawn(move || {
                let _ = reply.send(self.run());
            });
        if let Err(e) = spawned {
            log::warn!("cannot start the export-text thread: {e}");
        }
    }

    /// The text from the offset, at most [`PIECE`] bytes, fewer if escaping would take its
    /// line past the limit, ending on a character boundary. With `end`, where the next one
    /// starts.
    pub fn run(&self) -> Reply {
        let text = piece_at(&self.parts.0, self.offset)?;
        let end = self.offset + text.len();
        Ok(json!({ "text": text, "end": end }))
    }
}

fn piece_at(parts: &[Rope], offset: usize) -> Result<String, String> {
    let total: usize = parts.iter().map(Rope::len).sum();
    if offset > total {
        return Err(format!("offset {offset} is past the item's {total} bytes"));
    }
    let mut text = String::with_capacity(PIECE.min(total - offset));
    let mut escaped = 0;
    let mut start = offset;
    for part in parts {
        if start >= part.len() {
            start -= part.len();
            continue;
        }
        if text.is_empty() && part.clip_offset(start, Bias::Left) != start {
            return Err(format!("offset {offset} is inside a character"));
        }
        // Up to whichever comes first, in one pass: the text's bound or its line's.
        for chunk in part.chunks_in_range(start..part.len()) {
            for c in chunk.chars() {
                let mut buffer = [0; 4];
                escaped += escaped_len(c.encode_utf8(&mut buffer));
                if text.len() + c.len_utf8() > PIECE || escaped > PIECE_LINE {
                    return Ok(text);
                }
                text.push(c);
            }
        }
        start = 0;
    }
    Ok(text)
}

/// A piece `import-text` brings. Its `Debug` gives no text, as that could reach a log.
#[derive(Clone, PartialEq, Eq)]
pub struct CarriedPiece {
    pub export: u64,
    pub item: usize,
    pub offset: usize,
    pub text: String,
}

impl fmt::Debug for CarriedPiece {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CarriedPiece")
            .field("export", &self.export)
            .field("item", &self.item)
            .field("offset", &self.offset)
            .field("len", &self.text.len())
            .finish()
    }
}

/// The pieces of one export staged so far.
struct Staged {
    export: u64,
    items: HashMap<usize, Builder>,
    bytes: usize,
    last: Instant,
}

/// What `import-text` stages, shared between the UI thread and the threads that build it.
#[derive(Clone, Default)]
pub struct Staging(Arc<Mutex<Option<Staged>>>);

impl Staging {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Staged>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Stages `piece` on a background thread, which replies.
    pub fn spawn(&self, piece: CarriedPiece, max_items: usize, reply: mpsc::Sender<Reply>) {
        let staging = self.clone();
        let spawned = thread::Builder::new()
            .name("oso-import-text".into())
            .spawn(move || {
                let _ = reply.send(staging.stage(piece, max_items));
            });
        if let Err(e) = spawned {
            log::warn!("cannot start the import-text thread: {e}");
        }
    }

    /// Appends `piece` to its item. A piece of another export, or one that comes after
    /// [`KEPT`] without any, starts over, and must then start an item.
    pub fn stage(&self, piece: CarriedPiece, max_items: usize) -> Reply {
        let mut staged = self.lock();
        let current = staged
            .as_ref()
            .is_some_and(|s| s.export == piece.export && s.last.elapsed() <= KEPT);
        if !current {
            *staged = None;
        }
        if piece.item >= max_items {
            return Err(format!("a state has at most {max_items} items"));
        }
        let staged = staged.get_or_insert_with(|| Staged {
            export: piece.export,
            items: HashMap::new(),
            bytes: 0,
            last: Instant::now(),
        });
        if staged.bytes + piece.text.len() > MAX_CARRIED {
            return Err(format!("a state carries at most {MAX_CARRIED} bytes"));
        }
        let builder = staged
            .items
            .entry(piece.item)
            .or_insert_with(|| Builder::new(None, TabWidth::default()));
        if builder.len() != piece.offset {
            return Err(format!(
                "item {} has {} bytes, and the piece starts at {}",
                piece.item,
                builder.len(),
                piece.offset
            ));
        }
        builder.push_text(&piece.text);
        staged.bytes += piece.text.len();
        staged.last = Instant::now();
        Ok(json!({ "end": builder.len() }))
    }

    /// Forgets what was staged if no piece has come for [`KEPT`], on another thread.
    pub fn drop_stale(&self) {
        let mut staged = self.lock();
        if staged.as_ref().is_some_and(|s| s.last.elapsed() > KEPT) {
            drop_later(staged.take());
        }
    }

    /// Takes every item of `export`, whose lengths are `lengths`, and forgets what was
    /// staged, whether they had all arrived or not. O(the items).
    pub fn take(&self, export: u64, lengths: &[usize]) -> Result<Vec<LoadSnapshot>, String> {
        let Some(staged) = self.lock().take() else {
            return lengths
                .iter()
                .map(|&len| (len == 0).then(empty).ok_or_else(missing))
                .collect();
        };
        let mut staged = DropLater(Some(staged));
        let Some(staged) = staged.0.as_mut() else {
            return Err(missing());
        };
        let stale = staged.last.elapsed() > KEPT;
        if (stale || staged.export != export) && lengths.iter().any(|&len| len > 0) {
            return Err(missing());
        }
        lengths
            .iter()
            .enumerate()
            .map(|(item, &len)| match staged.items.remove(&item) {
                Some(mut builder) if builder.len() == len => {
                    builder.finish();
                    Ok(builder.snapshot())
                }
                None if len == 0 => Ok(empty()),
                _ => Err(missing()),
            })
            .collect()
    }
}

/// Drops `value` on a thread of its own, as freeing up to [`MAX_CARRIED`] of ropes and
/// display maps would hold the UI thread for milliseconds. Inline if no thread starts.
pub fn drop_later<T: Send + 'static>(value: T) {
    let spawned = thread::Builder::new()
        .name("oso-drop".into())
        .spawn(move || drop(value));
    if let Err(e) = spawned {
        log::debug!("cannot start a thread to drop staged text: {e}");
    }
}

/// What staged pieces are left of an import, dropped with [`drop_later`].
struct DropLater<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> Drop for DropLater<T> {
    fn drop(&mut self) {
        drop_later(self.0.take());
    }
}

fn empty() -> LoadSnapshot {
    let mut builder = Builder::new(None, TabWidth::default());
    builder.finish();
    builder.snapshot()
}

fn missing() -> String {
    "the state's text has not all arrived".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{CarriedPiece, Exported, Item, PIECE, PIECE_LINE, Staging, piece_at};
    use crate::dev::protocol::escaped_len;
    use oso_core::rope::Rope;

    fn piece(export: u64, item: usize, offset: usize, text: &str) -> CarriedPiece {
        CarriedPiece {
            export,
            item,
            offset,
            text: text.to_owned(),
        }
    }

    /// Every piece of `parts`, as `export-text` would give them one after another.
    fn pieces(parts: &[Rope]) -> Vec<String> {
        let total: usize = parts.iter().map(Rope::len).sum();
        let mut offset = 0;
        let mut pieces = Vec::new();
        while offset < total {
            let text = piece_at(parts, offset).unwrap();
            assert!(!text.is_empty(), "a piece must move on");
            offset += text.len();
            pieces.push(text);
        }
        pieces
    }

    #[test]
    fn pieces_join_to_the_item_across_its_parts_and_end_on_characters() {
        let big = "é".repeat(PIECE / 2 + 7);
        let parts = [
            Rope::from("ab"),
            Rope::from(""),
            Rope::from(big.as_str()),
            Rope::from("z"),
        ];
        let pieces = pieces(&parts);
        assert!(pieces.iter().all(|piece| piece.len() <= PIECE));
        assert_eq!(pieces.concat(), format!("ab{big}z"));
        assert!(pieces.len() >= 2);
    }

    #[test]
    fn a_piece_of_controls_is_shorter_so_that_its_line_fits() {
        let controls = "\u{1}\u{85}".repeat(PIECE);
        let parts = [Rope::from(controls.as_str())];
        let pieces = pieces(&parts);
        assert!(pieces.iter().all(|piece| escaped_len(piece) <= PIECE_LINE));
        assert_eq!(pieces.concat(), controls);
    }

    #[test]
    fn tab_indented_code_with_short_lines_fills_whole_pieces() {
        let code = "\t\tx();\n".repeat(PIECE);
        let parts = [Rope::from(code.as_str())];
        assert_eq!(piece_at(&parts, 0).unwrap().len(), PIECE);
    }

    #[test]
    fn a_piece_starts_only_on_a_character_inside_the_item() {
        let parts = [Rope::from("aé")];
        assert!(
            piece_at(&parts, 2)
                .unwrap_err()
                .contains("inside a character")
        );
        assert!(piece_at(&parts, 4).unwrap_err().contains("past"));
        assert_eq!(piece_at(&parts, 3).unwrap(), "");
    }

    #[test]
    fn export_text_reads_only_the_latest_export() {
        let exported = Exported::new(4, vec![Item(vec![Rope::from("text")])]);
        assert!(exported.piece(3, 0, 0).is_err());
        assert!(exported.piece(4, 1, 0).is_err());
        let reply = exported.piece(4, 0, 1).unwrap().run().unwrap();
        assert_eq!(reply["text"], "ext");
        assert_eq!(reply["end"], 4);
    }

    #[test]
    fn staged_pieces_build_each_item_in_order() {
        let staging = Staging::default();
        assert_eq!(staging.stage(piece(1, 1, 0, "on"), 4).unwrap()["end"], 2);
        assert!(
            staging.stage(piece(1, 1, 1, "x"), 4).is_err(),
            "out of order"
        );
        staging.stage(piece(1, 1, 2, "e\ntwo"), 4).unwrap();
        staging.stage(piece(1, 3, 0, "kill"), 4).unwrap();
        assert!(
            staging.stage(piece(1, 4, 0, "x"), 4).is_err(),
            "too many items"
        );
        let taken = staging.take(1, &[0, 7, 0, 4]).unwrap();
        let texts: Vec<String> = taken.iter().map(|s| s.text().to_string()).collect();
        assert_eq!(texts, ["", "one\ntwo", "", "kill"]);
        assert!(staging.take(1, &[0, 7, 0, 4]).is_err(), "taken once");
    }

    #[test]
    fn a_missing_or_short_item_or_another_export_takes_nothing() {
        let staging = Staging::default();
        staging.stage(piece(1, 0, 0, "abc"), 4).unwrap();
        assert!(staging.take(1, &[4]).is_err());
        staging.stage(piece(1, 0, 0, "abc"), 4).unwrap();
        assert!(staging.take(2, &[3]).is_err());
        staging.stage(piece(1, 0, 0, "abc"), 4).unwrap();
        staging.stage(piece(2, 0, 0, "new"), 4).unwrap();
        assert_eq!(
            staging.take(2, &[3]).unwrap()[0].text().to_string(),
            "new",
            "a new export starts over"
        );
        assert!(
            staging.take(5, &[0, 0]).is_ok(),
            "nothing to carry needs nothing"
        );
    }

    #[test]
    fn a_piece_shows_no_text_in_debug() {
        let shown = format!("{:?}", piece(1, 0, 0, "secret words"));
        assert!(!shown.contains("secret"), "{shown}");
        assert!(shown.contains("len: 12"), "{shown}");
    }
}
