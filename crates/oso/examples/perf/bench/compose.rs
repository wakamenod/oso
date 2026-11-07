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

/// The kana a preedit grows by, picked at random so that no two reports are the
/// same text and the renderer's cache of short pieces cannot hold the preedit.
const KANA: [char; 20] = [
    'あ', 'い', 'う', 'え', 'お', 'か', 'き', 'く', 'け', 'こ', 'さ', 'し', 'す', 'せ', 'そ', 'に',
    'ほ', 'ん', 'ご', 'を',
];

/// The most kana a word grows to before it converts: past 64 bytes, so that the
/// renderer shapes the preedit anew for every report, as a long phrase does.
const LONGEST: usize = 30;

/// A word being composed: its kana so far, how many it grows to, and whether it
/// has converted.
struct Word {
    text: String,
    target: usize,
    converted: bool,
}

impl Word {
    /// The next report: a preedit with its caret at the end, the last three kana
    /// being converted, or an empty one that ends the composition with no commit.
    fn next_report(&mut self, rng: &mut Lcg) -> serde_json::Value {
        let end = self.text.len();
        if self.text.chars().count() < self.target {
            self.text.push(KANA[rng.below(KANA.len())]);
            let end = self.text.len();
            json!({ "preedit": self.text, "range": [end, end] })
        } else if !self.converted {
            self.converted = true;
            // Three kana of three bytes each.
            json!({ "preedit": self.text, "range": [end - 9, end] })
        } else {
            self.text.clear();
            self.target = 3 + rng.below(LONGEST - 2);
            self.converted = false;
            json!({ "preedit": "" })
        }
    }
}

/// A preedit's report to presented frame, for `count` reports after a warm-up of
/// one word: words of new kana that grow, convert and clear with no commit, and one
/// more report to clear the last word if it is open. Only reported. Fails unless
/// each drew a frame and the text stayed as it was.
pub(super) fn composing(app: &mut App, count: usize, rng: &mut Lcg) -> Result<Vec<f64>, String> {
    /// Sends `steps` reports, and clears what is left open. Returns the latencies and
    /// how many reports were sent.
    fn report(app: &mut App, steps: usize, rng: &mut Lcg) -> Result<(Vec<f64>, usize), String> {
        let mut word = Word {
            text: String::new(),
            target: LONGEST,
            converted: false,
        };
        let send = |app: &mut App, params, rng: &mut Lcg| -> Result<(), String> {
            app.call("ime", Some(params))?;
            let jitter = rng.below(KEY_JITTER_MS) as u64;
            thread::sleep(KEY_PACE + Duration::from_millis(jitter));
            Ok(())
        };
        for _ in 0..steps {
            let params = word.next_report(rng);
            send(app, params, rng)?;
        }
        let open = !word.text.is_empty();
        if open {
            send(app, json!({ "preedit": "" }), rng)?;
        }
        Ok((settle(app)?, steps + usize::from(open)))
    }
    report(app, LONGEST + 2, rng)?;
    let before = text_bytes(app)?;
    let (samples, sent) = report(app, count, rng)?;
    let after = text_bytes(app)?;
    if after != before {
        return Err(format!(
            "{sent} preedits took the text from {before} to {after} bytes"
        ));
    }
    if samples.len() != sent {
        return Err(format!("{sent} preedits drew {} frames", samples.len()));
    }
    Ok(samples)
}
