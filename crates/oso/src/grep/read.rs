//! Reading one file for a project's search (ADR 0114): in blocks cut at a line's end, each
//! matched against the search's query.
//!
//! Must not depend on: `app`, winit or the UI thread; this reads what it is given, and opens
//! nothing.

use std::io::Read;
use std::ops::Range;

use oso_core::project::{BLOCK, LineQuery, MAX_LINE};

/// What a matching line is handed to: its number from 1, its bytes and where its first match
/// is. It says whether to go on.
pub(crate) type Hit<'a> = dyn FnMut(usize, &[u8], Range<usize>) -> bool + 'a;

/// The most bytes a searcher's block keeps between files; one grown for a long line gives the
/// rest back.
const KEPT_BLOCK: usize = 4 * BLOCK;

/// Reads `file` into `block`, a buffer the searcher keeps from file to file, in blocks of
/// [`BLOCK`] bytes cut at a line's end, handing each line that matches `query` to `hit`, until
/// `hit` says no more or `live` says the search is over, which is asked before each block. A
/// line longer than a block grows it, up to [`MAX_LINE`]; past that the rest of the file is
/// skipped. A NUL byte stops the file, keeping the lines before its own.
pub(crate) fn search_file(
    mut file: impl Read,
    query: &LineQuery,
    live: &dyn Fn() -> bool,
    hit: &mut Hit<'_>,
    block: &mut Vec<u8>,
) {
    block.clear();
    read_blocks(&mut file, query, live, hit, block);
    block.clear();
    if block.capacity() > KEPT_BLOCK {
        block.shrink_to(BLOCK);
    }
}

fn read_blocks(
    file: &mut impl Read,
    query: &LineQuery,
    live: &dyn Fn() -> bool,
    hit: &mut Hit<'_>,
    block: &mut Vec<u8>,
) {
    // The number of the block's first line, from 1.
    let mut first_line = 1;
    let mut ended = false;
    // How much of the block was looked at for a NUL and a line break.
    let mut scanned = 0;
    loop {
        if !live() {
            return;
        }
        // Read until the block holds a whole block's bytes past what was carried, or the file
        // ends. It never holds more than the longest line read, so a longer one never fits.
        // Reading to the end of a bound fills the buffer without clearing it first.
        let wanted = (block.len() + BLOCK).min(MAX_LINE) - block.len();
        if !ended && wanted > 0 {
            match (&mut *file).take(wanted as u64).read_to_end(block) {
                Ok(read) => ended = read < wanted,
                Err(_) => ended = true,
            }
        }
        // A NUL stops the file: only the lines before its own are searched. What was looked at
        // already holds neither a NUL nor a line break.
        let nul = block[scanned..]
            .contains(&0)
            .then(|| block[scanned..].iter().position(|&byte| byte == 0))
            .flatten()
            .map(|at| scanned + at);
        let (searched, last) = match nul {
            Some(at) => {
                let newline = block[..at].iter().rposition(|&byte| byte == b'\n');
                (newline.map_or(0, |newline| newline + 1), true)
            }
            None if ended => (block.len(), true),
            None => match block[scanned..].iter().rposition(|&byte| byte == b'\n') {
                Some(newline) => (scanned + newline + 1, false),
                // One line longer than a block: it grows, up to its bound.
                None if block.len() < MAX_LINE => {
                    scanned = block.len();
                    continue;
                }
                None => return,
            },
        };
        let mut more = true;
        let breaks = query.find_in_block(&block[..searched], |found| {
            if more {
                let bytes = &block[found.range.clone()];
                more = hit(first_line + found.line, bytes, found.first);
            }
        });
        if last || !more {
            return;
        }
        first_line += breaks;
        block.drain(..searched);
        scanned = block.len();
    }
}
