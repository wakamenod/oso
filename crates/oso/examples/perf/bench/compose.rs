//! The preedit phase of the key benchmark: what an input method reports while a word
//! is composed, through the dev socket's stand-in for it (ADR 0069).
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the
//! module above.

use std::thread;
use std::time::Duration;

use serde_json::json;

use super::{KEY_JITTER_MS, KEY_PACE, Lcg, settle, text_bytes};
use crate::app::App;

/// What an input method reports while a word is composed, through the dev socket's
/// stand-in for it (ADR 0069): a preedit that grows by a kana, then converts, then
/// clears with no commit, so the text stays as it was.
const PREEDITS: [(&str, Option<[usize; 2]>); 6] = [
    ("に", None),
    ("にほ", None),
    ("にほん", None),
    ("にほんご", None),
    ("日本語", Some([0, 9])),
    ("", None),
];

/// A preedit's report to presented frame, for `count` reports from [`PREEDITS`] after
/// a warm-up for their glyphs. Only reported. Fails unless each drew a frame and the
/// text stayed as it was.
pub(super) fn composing(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    fn report(app: &mut App, steps: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
        for (text, range) in PREEDITS.iter().cycle().take(steps) {
            app.call("ime", Some(json!({ "preedit": text, "range": range })))?;
            let jitter = rng.below(KEY_JITTER_MS) as u64;
            thread::sleep(KEY_PACE + Duration::from_millis(jitter));
        }
        settle(app)
    }
    report(app, PREEDITS.len(), rng)?;
    let before = text_bytes(app)?;
    let samples = report(app, count, rng)?;
    let after = text_bytes(app)?;
    if after != before {
        return Err(format!(
            "{count} preedits took the text from {before} to {after} bytes"
        ));
    }
    if samples.len() != count {
        return Err(format!("{count} preedits drew {} frames", samples.len()));
    }
    Ok(samples)
}
