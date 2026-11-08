//! Comparing two texts from their ends, for a file read again (ADR 0083).
//!
//! Must not depend on: `decode`, `display_map`, cells or display rows.

use super::Rope;
use crate::sum_tree::Bias;

impl Rope {
    /// How many bytes this text and `other` share from their start. O(n) in that length, for
    /// a thread that compares a file read again with the text before it.
    pub(crate) fn common_prefix(&self, other: &Rope) -> usize {
        let (mut a, mut b) = (self.tree.iter(), other.tree.iter());
        let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
        let mut same = 0;
        loop {
            if x.is_empty() {
                let Some(chunk) = a.next() else { return same };
                x = chunk.as_str().as_bytes();
                continue;
            }
            if y.is_empty() {
                let Some(chunk) = b.next() else { return same };
                y = chunk.as_str().as_bytes();
                continue;
            }
            let n = x.len().min(y.len());
            let equal = x[..n]
                .iter()
                .zip(&y[..n])
                .take_while(|(p, q)| p == q)
                .count();
            same += equal;
            if equal < n {
                return same;
            }
            (x, y) = (&x[n..], &y[n..]);
        }
    }

    /// How many bytes this text and `other` share at their end, at most `limit`. O(n) in that
    /// length, as [`Rope::common_prefix`].
    pub(crate) fn common_suffix(&self, other: &Rope, limit: usize) -> usize {
        let mut a = self.tree.cursor::<usize>();
        a.seek(&self.len(), Bias::Right);
        let mut b = other.tree.cursor::<usize>();
        b.seek(&other.len(), Bias::Right);
        let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
        let mut same = 0;
        while same < limit {
            if x.is_empty() {
                if !a.prev() {
                    break;
                }
                x = a.item().map_or(&[], |chunk| chunk.as_str().as_bytes());
                continue;
            }
            if y.is_empty() {
                if !b.prev() {
                    break;
                }
                y = b.item().map_or(&[], |chunk| chunk.as_str().as_bytes());
                continue;
            }
            let n = x.len().min(y.len()).min(limit - same);
            let (tail_x, tail_y) = (&x[x.len() - n..], &y[y.len() - n..]);
            let equal = tail_x
                .iter()
                .rev()
                .zip(tail_y.iter().rev())
                .take_while(|(p, q)| p == q)
                .count();
            same += equal;
            if equal < n {
                break;
            }
            (x, y) = (&x[..x.len() - n], &y[..y.len() - n]);
        }
        same
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texts_compare_from_both_ends_across_chunks() {
        let a = Rope::from(&*"abcdefgh".repeat(20));
        let b = Rope::from(&*("abcdefgh".repeat(10) + "X" + &"abcdefgh".repeat(10)));
        assert_eq!(a.common_prefix(&b), 80);
        assert_eq!(a.common_suffix(&b, a.len()), 80);
        assert_eq!(a.common_suffix(&b, 7), 7, "no further than the limit");
        assert_eq!(a.common_prefix(&a.clone()), a.len());
        assert_eq!(Rope::new().common_prefix(&a), 0);
        assert_eq!(Rope::new().common_suffix(&a, 0), 0);
    }
}
