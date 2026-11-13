//! The fuzz target's scripts: one input's terminal written out as lines (`new`, `resize`,
//! `write HEX`, and the UI's `key`, `paste`, `scroll`, `focus` and `tick`), and run back, for
//! reducing a finding outside the repository.
//!
//! Must not depend on: the thread or a PTY; it drives the engine through the fuzz target's
//! driver.

use std::fmt::Write as _;
use std::time::Duration;

use oso_platform::process::pty::Size;

use super::{BLOCK, Driver, input, requests, scrollback};
use crate::limits;

/// The script that rebuilds `iteration`'s terminal: its block's inputs up to it.
pub(super) fn script_of(seed: u64, iteration: u64, resizes: bool) -> String {
    let mut script = format!("new 80 24 {}\n", scrollback(iteration));
    for i in iteration - iteration % BLOCK..=iteration {
        let case = input(seed, i, resizes);
        for request in &case.requests {
            script.push_str(&requests::line(request));
            script.push('\n');
        }
        let _ = write!(script, "tick {}\nwrite ", case.tick.as_millis());
        for byte in case.data {
            let _ = write!(script, "{byte:02x}");
        }
        script.push('\n');
        if let Some(size) = case.resize {
            let _ = writeln!(script, "resize {} {}", size.columns, size.rows);
        }
    }
    script
}

pub(super) fn run_script(script: &str) {
    let mut driver = Driver::new(scrollback(0));
    let mut tick = Duration::ZERO;
    for line in script.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        if let Some(request) = requests::parse(&words) {
            driver.serve(request);
            continue;
        }
        match words.as_slice() {
            ["tick", ms] => tick = Duration::from_millis(ms.parse().expect("ms")),
            ["new", _, _, lines] => driver = Driver::new(lines.parse().expect("lines")),
            ["resize", columns, rows] => {
                let size = sized(
                    columns.parse().expect("columns"),
                    rows.parse().expect("rows"),
                );
                driver.feed(&[], Some(size), Duration::ZERO);
            }
            ["write", hex] => {
                let data: Vec<u8> = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
                    .collect();
                driver.feed(&data, None, std::mem::take(&mut tick));
            }
            _ => panic!("unknown script line {line:?}"),
        }
    }
}

/// A grid of `columns` × `rows`, clamped as a terminal's thread clamps every size.
pub(super) fn sized(columns: u16, rows: u16) -> Size {
    limits::clamp(Size {
        columns,
        rows,
        width: columns.saturating_mul(8),
        height: rows.saturating_mul(16),
    })
}
