//! What a fix request leaves out of a failed run's output (ADR 0127, step 6a, the author's
//! choice): lines shaped like a key or a token, and private key blocks, so that a hook that
//! prints what it found, as a secret scanner does, gives the agent none of it.
//!
//! Must not depend on: `app`, sessions, git or the file system; this is a pure function over
//! text.
//!
//! - It matches shapes, not names: a private key's armour, the prefixes of well-known tokens
//!   (cloud, forge, chat and model providers' keys, JSON web tokens), an assignment to a word
//!   such as `password` or `token`, and a long run of base64 characters that mixes upper and
//!   lower case and digits. A commit id, lowercase hex, is kept.
//! - It errs towards leaving a line out: a line that only looks like a secret is dropped too,
//!   and the request says how many went.

use regex::{Regex, RegexBuilder};

/// The shapes of a line that may hold a secret.
const SHAPES: &str = r#"(?x)
    -----BEGIN\ [A-Z0-9\ ]*PRIVATE\ KEY-----
  | \b(?:AKIA|ASIA)[0-9A-Z]{16}\b
  | \bgh[pousr]_[A-Za-z0-9]{30,}
  | \bgithub_pat_[A-Za-z0-9_]{20,}
  | \bglpat-[A-Za-z0-9_-]{20,}
  | \bxox[abprs]-[A-Za-z0-9-]{10,}
  | \bsk-[A-Za-z0-9_-]{20,}
  | \b[rs]k_(?:live|test)_[A-Za-z0-9]{16,}
  | \bAIza[0-9A-Za-z_-]{35}
  | \beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}
  | (?i:secret|token|passw(?:or)?d|api[_-]?key|access[_-]?key|private[_-]?key|credential)
    [A-Za-z0-9_-]*["']?\s*[:=]\s*["']?[^\s"']{8,}
"#;

/// A run this long of base64 characters, with upper and lower case and digits in it, is taken
/// for a key.
const LONG_RUN: usize = 32;

/// The most bytes the compiled shapes may take.
const SIZE_LIMIT: usize = 1 << 20;

/// [`SHAPES`], compiled: once per fix request, which the person asks for, so no state is
/// kept; `None` if it does not compile, which a test rules out.
pub(super) fn pattern() -> Option<Regex> {
    RegexBuilder::new(SHAPES)
        .size_limit(SIZE_LIMIT)
        .build()
        .ok()
}

/// `text` without its lines that may hold a secret, and how many went. A private key's armour
/// leaves out every line to its end. If the shapes did not compile, which a test rules out,
/// every line goes.
pub(crate) fn withheld(text: &str) -> (String, usize) {
    let Some(pattern) = pattern() else {
        return (String::new(), text.lines().count());
    };
    let mut kept = String::with_capacity(text.len());
    let mut dropped = 0;
    let mut in_key = false;
    for line in text.split_inclusive('\n') {
        if in_key {
            dropped += 1;
            in_key = !line.contains("-----END ");
            continue;
        }
        if pattern.is_match(line) || long_run(line) {
            dropped += 1;
            in_key = line.contains("PRIVATE KEY-----") && !line.contains("-----END ");
            continue;
        }
        kept.push_str(line);
    }
    (kept, dropped)
}

/// Whether `line` holds a run of at least [`LONG_RUN`] base64 characters with upper and lower
/// case letters and digits in it.
fn long_run(line: &str) -> bool {
    let base64 = |c: char| c.is_ascii_alphanumeric() || matches!(c, '+' | '/' | '=' | '_' | '-');
    line.split(|c: char| !base64(c)).any(|run| {
        run.len() >= LONG_RUN
            && run.chars().any(|c| c.is_ascii_uppercase())
            && run.chars().any(|c| c.is_ascii_lowercase())
            && run.chars().any(|c| c.is_ascii_digit())
    })
}
