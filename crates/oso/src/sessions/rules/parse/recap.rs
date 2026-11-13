//! A rule's `recap` (ADR 0121, step 3): the pattern that finds a session's one-line recap and
//! the region it reads. Kept apart from the matchers, as it never decides whether a rule
//! matches: a wrong one is skipped and said, and its rule kept.
//!
//! Must not depend on: `app`, `commands`, `dispatch` or `config`; text in, a pattern out.

use std::fmt;

use toml::Spanned;
use toml::de::DeValue;

use super::{MAX_PATTERN_BYTES, Quoted, REGIONS, Reader, string};
use crate::sessions::recap::Recap;
use crate::sessions::screen::Region;

/// What a problem says is not used: only the recap.
pub(super) const RECAP_SKIPPED: &str = "the recap is skipped";

impl Reader<'_> {
    /// The rule's `recap` pattern and the region it reads, the rule's own unless
    /// `recap_region` names one.
    pub(super) fn recap(
        &mut self,
        pattern: Option<&Spanned<DeValue<'_>>>,
        region: Option<&Spanned<DeValue<'_>>>,
        own: Region,
        name: &str,
    ) -> Option<Recap> {
        let mut skip = |at: usize, why: fmt::Arguments<'_>| {
            let message = format_args!("rule {}: {why}, {RECAP_SKIPPED}", Quoted(name));
            self.problem(at, message);
            None
        };
        let Some(pattern) = pattern else {
            let at = region?.span().start;
            return skip(at, format_args!("recap_region needs a recap"));
        };
        let region = match region {
            None => own,
            Some(value) => match string(value.get_ref()).and_then(Region::parse) {
                Some(region) => region,
                None => {
                    let why = format_args!("recap_region must be {REGIONS}");
                    return skip(value.span().start, why);
                }
            },
        };
        let at = pattern.span().start;
        let text = string(pattern.get_ref())
            .filter(|text| !text.is_empty() && text.len() <= MAX_PATTERN_BYTES);
        let Some(text) = text else {
            let why = format_args!("recap must be a pattern of 1 to {MAX_PATTERN_BYTES} bytes");
            return skip(at, why);
        };
        let compiled = self.compile(text, at, name, RECAP_SKIPPED)?;
        Some(Recap::new(compiled, region))
    }
}
