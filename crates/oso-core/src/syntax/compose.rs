//! Two batches of splices as one, so that what waits for a parser or a report stays one batch
//! however many commands made it (ADR 0111).
//!
//! Must not depend on: `highlights`; it sees only offsets.

use super::Splice;

/// The batch that does what `first` and then `then` do: `then`'s offsets are in the text after
/// `first`, and the result's in the text before `first`, ascending and apart, as a batch's are.
/// `None` if either batch is not ascending and apart. O(splices).
///
/// The splices of both batches are placed in the text between them, where `first`'s cover the
/// text it inserted and `then`'s the text it replaced. Those that overlap or touch become one,
/// and each such group starts and ends where `first` changed nothing, so its length before
/// `first` and after `then` follow from the lengths each batch changed in it.
pub fn compose(first: &[Splice], then: &[Splice]) -> Option<Vec<Splice>> {
    // `first`'s splices in the text between, with how much each changed the length.
    let mut middle = Vec::with_capacity(first.len());
    let (mut shift, mut end) = (0isize, 0usize);
    for splice in first {
        if splice.start < end {
            return None;
        }
        if splice.old_len == 0 && splice.new_len == 0 {
            continue;
        }
        let start = splice.start.checked_add_signed(shift)?;
        middle.push((
            start,
            start.checked_add(splice.new_len)?,
            delta(splice)?,
            true,
        ));
        shift = shift.checked_add(delta(splice)?)?;
        end = splice.start.checked_add(splice.old_len)?;
    }
    let mut items = Vec::with_capacity(first.len() + then.len());
    let (mut a, mut b) = (middle.into_iter().peekable(), then.iter().peekable());
    let mut then_end = 0;
    loop {
        let take_first = match (a.peek(), b.peek()) {
            (Some(f), Some(t)) => f.0 <= t.start,
            (Some(_), None) => true,
            (None, Some(_)) => false,
            (None, None) => break,
        };
        if take_first {
            items.extend(a.next());
        } else if let Some(t) = b.next() {
            if t.start < then_end {
                return None;
            }
            then_end = t.start.checked_add(t.old_len)?;
            if t.old_len > 0 || t.new_len > 0 {
                items.push((t.start, then_end, delta(t)?, false));
            }
        }
    }
    let mut out: Vec<Splice> = Vec::new();
    // How much `first`'s splices before the current group changed the length.
    let mut first_before = 0isize;
    let mut index = 0;
    while index < items.len() {
        let (start, mut end, _, _) = items[index];
        let (mut first_in, mut then_in) = (0isize, 0isize);
        while let Some(&(s, e, d, is_first)) = items.get(index) {
            if s > end {
                break;
            }
            end = end.max(e);
            if is_first {
                first_in = first_in.checked_add(d)?;
            } else {
                then_in = then_in.checked_add(d)?;
            }
            index += 1;
        }
        let old_start = start.checked_add_signed(-first_before)?;
        let between = end - start;
        let old_len = between.checked_add_signed(-first_in)?;
        let new_len = between.checked_add_signed(then_in)?;
        // A group whose text `then` took back out changed nothing.
        if old_len > 0 || new_len > 0 {
            out.push(Splice {
                start: old_start,
                old_len,
                new_len,
            });
        }
        first_before = first_before.checked_add(first_in)?;
    }
    Some(out)
}

/// How much `splice` changes the length.
fn delta(splice: &Splice) -> Option<isize> {
    isize::try_from(splice.new_len)
        .ok()?
        .checked_sub(isize::try_from(splice.old_len).ok()?)
}

#[cfg(test)]
mod tests;
