//! The phases of the key benchmark that edit what typing left: `DEL` in the file, and
//! digits typed into an open prompt (ADR 0056, ADR 0072).
//!
//! Must not depend on: anything but the harness's own pacing and settling, from the
//! module above.

use super::{Lcg, paced_keys, settle, text_bytes};
use crate::app::App;

/// What is typed into a prompt: digits, as `goto-line` reads them.
const DIGITS: [&str; 10] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"];

/// Key press to presented frame while `DEL` removes what `typing` typed. Only
/// reported. Fails unless each key drew a frame and the text shrank.
pub(super) fn deleting(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    let before = text_bytes(app)?;
    let samples = paced_keys(app, &["DEL"], count, rng)?;
    let after = text_bytes(app)?;
    if after + count as u64 > before {
        return Err(format!(
            "{count} deleting keys took the text from {before} to {after} bytes"
        ));
    }
    if samples.len() != count {
        return Err(format!(
            "{count} deleting keys drew {} frames",
            samples.len()
        ));
    }
    Ok(samples)
}

/// Key press to presented frame while digits are typed into `goto-line`'s prompt, for
/// `count` keys after a warm-up: the answer's row is shaped again, and every row of the
/// window beneath is reused. Only reported. Fails unless each key drew a frame and the
/// prompt holds what they typed. The prompt closes with `C-g` after, so the file is as it
/// was.
pub(super) fn prompting(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    app.send_keys("M-g g")?;
    settle(app)?;
    let warm_up = DIGITS.len() * 2;
    paced_keys(app, &DIGITS, warm_up, rng)?;
    let samples = paced_keys(app, &DIGITS, count, rng)?;
    let state = app.call("get-state", None)?;
    let typed = state["prompts"][0]["text"].as_str().map_or(0, str::len);
    app.send_keys("C-g")?;
    settle(app)?;
    if typed != warm_up + count {
        return Err(format!(
            "{} keys should type {} digits into the prompt, and it holds {typed}",
            warm_up + count,
            warm_up + count
        ));
    }
    if samples.len() != count {
        return Err(format!(
            "{count} keys in a prompt drew {} frames",
            samples.len()
        ));
    }
    Ok(samples)
}
