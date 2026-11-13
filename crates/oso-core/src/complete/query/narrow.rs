//! Whether a completion query narrows the one before (ADR 0076, ADR 0116): every candidate
//! it matches is one that one matched, so only those are looked at.
//!
//! Must not depend on: `editor`, `minibuffer` or `display_map`; it works on strings alone.

use super::{Query, Style};

impl Query {
    /// Whether every candidate this query matches is one `before` matched, so that it
    /// needs to look only at those: each of `before`'s components is one of this query's,
    /// in its place, at most longer at its end, and none says what must not match unless
    /// it is the same. Case may stop being ignored, and never start.
    pub(crate) fn narrows(&self, before: &Query) -> bool {
        if before.too_long {
            return false;
        }
        if self.too_long {
            return true;
        }
        if self.ignore_case && !before.ignore_case {
            return false;
        }
        if self.components.len() < before.components.len() {
            return false;
        }
        self.components
            .iter()
            .zip(&before.components)
            .all(|(new, old)| {
                if new.style != old.style || new.not != old.not || new.annotation != old.annotation
                {
                    return false;
                }
                // A negation that matches less, or with case, keeps more candidates, and a
                // longer expression may match more, as may longer romaji, whose words differ.
                if new.style == Style::Migemo {
                    return new == old && self.ignore_case == before.ignore_case;
                }
                if new.not || new.tried || old.tried {
                    // A literal read as an expression too matches more, and the other way less.
                    return new.tried == old.tried
                        && new.pattern == old.pattern
                        && self.ignore_case == before.ignore_case;
                }
                if self.ignore_case == before.ignore_case {
                    new.pattern.starts_with(&*old.pattern)
                } else {
                    // Lower case keeps a prefix only in ASCII: `İ` lowers to `i` and U+0307.
                    new.pattern.is_ascii() && new.pattern.to_lowercase().starts_with(&*old.pattern)
                }
            })
    }
}
