//! The comparison harness for ADR 0120 step 1. Spawns nothing and uses no network: it reads
//! recorded byte streams and generates the rest.
//!
//! - `compare FILE...`: replay each recording into both engines in 4 KiB reads and compare
//!   the screens, cursor, modes, cells and effects after every read.
//! - `bench`: parse throughput on generated output, and snapshot time.
//! - `mem ENGINE LINES COLS [plain|colored|cargo]`: fill LINES of scrollback with full
//!   lines, colored lines or `cargo`-like output; run under `/usr/bin/time -l`.
//! - `string-flood ENGINE KIND MIB`: a control string (osc, dcs, decrqss, xtgettcap, sixel,
//!   tmux, apc, kitty, pm, sos) of MIB MiB that never ends.
//! - `fuzz ENGINE SECONDS SEED STATE-FILE [FIRST]`: generated escape sequences.
//! - `replay ENGINE ITER SEED`: rerun one fuzz iteration.
//! - `script ENGINE FILE`: run steps from FILE, one per line: `new COLS ROWS SCROLLBACK`,
//!   `resize COLS ROWS`, `write HEX`. For reducing a fuzz finding outside the repo.
//! - `input ITER SEED`: print one fuzz iteration as such a script.
//! - `ipc`: what moving a snapshot through an OS pipe would cost an engine in a helper
//!   process (an upper bound: shared memory would copy less).

mod fuzz;
mod workload;

use std::time::{Duration, Instant};

use engine::{Engine, Snapshot};
use engine_alacritty::AlacrittyEngine;
use engine_ghostty::GhosttyEngine;

pub fn make(name: &str, cols: u16, rows: u16, scrollback: usize) -> Box<dyn Engine> {
    match name {
        "alacritty" => Box::new(AlacrittyEngine::new(cols, rows, scrollback)),
        "ghostty" => Box::new(GhosttyEngine::new(cols, rows, scrollback)),
        other => panic!("unknown engine {other}: alacritty or ghostty"),
    }
}

const ENGINES: [&str; 2] = ["alacritty", "ghostty"];

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or_default();
    match arg(0) {
        "compare" => compare(&args[1..]),
        "bench" => bench(args.get(1).map(String::as_str)),
        "mem" => mem(
            arg(1),
            arg(2).parse().unwrap(),
            arg(3).parse().unwrap(),
            arg(4),
        ),
        "string-flood" => string_flood(arg(1), arg(2), arg(3).parse().unwrap()),
        "fuzz" => fuzz::run(
            arg(1),
            arg(2).parse().unwrap(),
            arg(3).parse().unwrap(),
            arg(4),
            arg(5).parse().unwrap_or(0),
        ),
        "replay" => fuzz::replay(arg(1), arg(2).parse().unwrap(), arg(3).parse().unwrap()),
        "dump" => dump(arg(1), arg(2)),
        "script" => script(arg(1), arg(2)),
        "input" => fuzz::print_input(arg(1).parse().unwrap(), arg(2).parse().unwrap()),
        "ipc" => ipc(),
        _ => eprintln!("usage: harness compare|bench|mem|osc-flood|fuzz|replay|dump ..."),
    }
}

/// Print one engine's final screen for a recording.
fn dump(engine: &str, path: &str) {
    let bytes = std::fs::read(path).unwrap();
    let (cols, rows) = size();
    let mut e = make(engine, cols, rows, 10_000);
    for chunk in bytes.chunks(4096) {
        e.feed(chunk);
    }
    for (i, line) in e.screen_text().iter().enumerate() {
        println!("{i:2}|{line}");
    }
    println!("modes: {:?}", e.modes());
}

fn compare(files: &[String]) {
    for path in files {
        let bytes = std::fs::read(path).unwrap();
        let (cols, rows) = size();
        let mut engines: Vec<Box<dyn Engine>> = ENGINES
            .iter()
            .map(|n| make(n, cols, rows, 10_000))
            .collect();
        let mut snaps = [Snapshot::default(), Snapshot::default()];
        let (mut text_diffs, mut cell_diffs, mut cursor_diffs, mut mode_diffs, mut checks) =
            (0, 0, 0, 0, 0);
        let mut first: Option<String> = None;
        let mut effects: [Vec<engine::Effect>; 2] = [Vec::new(), Vec::new()];
        for (n, chunk) in bytes.chunks(4096).enumerate() {
            for (i, e) in engines.iter_mut().enumerate() {
                e.feed(chunk);
                e.snapshot(&mut snaps[i]);
                effects[i].extend(e.drain_effects());
            }
            checks += 1;
            let texts: Vec<Vec<String>> = engines.iter_mut().map(|e| e.screen_text()).collect();
            if texts[0] != texts[1] {
                text_diffs += 1;
                if first.is_none() {
                    let row = texts[0]
                        .iter()
                        .zip(&texts[1])
                        .position(|(a, b)| a != b)
                        .unwrap_or(0);
                    first = Some(format!(
                        "read {n}, row {row}:\n  alacritty |{}|\n  ghostty   |{}|",
                        texts[0].get(row).map_or("", |s| s),
                        texts[1].get(row).map_or("", |s| s)
                    ));
                }
            }
            if snaps[0].cells != snaps[1].cells {
                cell_diffs += 1;
            }
            if (
                snaps[0].cursor_row,
                snaps[0].cursor_col,
                snaps[0].cursor_visible,
            ) != (
                snaps[1].cursor_row,
                snaps[1].cursor_col,
                snaps[1].cursor_visible,
            ) {
                cursor_diffs += 1;
            }
            let modes: Vec<_> = engines.iter_mut().map(|e| e.modes()).collect();
            if modes[0] != modes[1] {
                mode_diffs += 1;
            }
        }
        println!(
            "{path}: {} bytes, {checks} reads; differing after a read: text {text_diffs}, cells {cell_diffs}, cursor {cursor_diffs}, modes {mode_diffs}",
            bytes.len()
        );
        if let Some(first) = first {
            println!("  first text difference at {first}");
        }
        if cell_diffs > 0 {
            report_cells(&snaps);
        }
        let modes: Vec<_> = engines.iter_mut().map(|e| e.modes()).collect();
        if modes[0] != modes[1] {
            println!(
                "  final modes differ:\n    alacritty {:?}\n    ghostty   {:?}",
                modes[0], modes[1]
            );
        }
        for (i, name) in ENGINES.iter().enumerate() {
            let mut kinds: Vec<String> = effects[i].iter().map(effect_kind).collect();
            kinds.sort();
            kinds.dedup();
            println!("  {name} effects: {}", kinds.join(", "));
        }
    }
}

fn report_cells(snaps: &[Snapshot; 2]) {
    let mut shown = 0;
    for (i, (a, b)) in snaps[0].cells.iter().zip(&snaps[1].cells).enumerate() {
        if a != b && shown < 5 {
            let cols = usize::from(snaps[0].cols);
            println!(
                "  final cell ({}, {}) alacritty {a:?} ghostty {b:?}",
                i / cols,
                i % cols
            );
            shown += 1;
        }
    }
    let total = snaps[0]
        .cells
        .iter()
        .zip(&snaps[1].cells)
        .filter(|(a, b)| a != b)
        .count();
    if total > 0 {
        println!("  final cells differing: {total}");
    }
}

fn effect_kind(effect: &engine::Effect) -> String {
    use engine::Effect::*;
    match effect {
        Reply(bytes) => format!("Reply({})", String::from_utf8_lossy(bytes).escape_debug()),
        Title(_) => "Title".into(),
        Bell => "Bell".into(),
        WorkingDirectory(_) => "OSC7".into(),
        Notification { .. } => "Notification".into(),
        Progress { .. } => "Progress".into(),
        PromptMark { kind, .. } => format!("OSC133{kind}"),
        ClipboardWrite(_) => "ClipboardWrite".into(),
        ClipboardRead => "ClipboardRead".into(),
        ColorQuery(n) => format!("ColorQuery({n})"),
        UnknownOsc(bytes) => {
            let n: String = bytes
                .iter()
                .take_while(|b| b.is_ascii_digit())
                .map(|&b| char::from(b))
                .collect();
            format!("UnknownOsc({n})")
        }
        Apc(_) => "Apc".into(),
    }
}

fn throughput(engine: &str, name: &str, data: &[u8]) {
    let mut e = make(engine, 200, 50, 10_000);
    // Warm up: one pass over the first MiB.
    for chunk in data[..data.len().min(1 << 20)].chunks(65536) {
        e.feed(chunk);
    }
    let mut best = Duration::MAX;
    for _ in 0..3 {
        let start = Instant::now();
        for chunk in data.chunks(65536) {
            e.feed(chunk);
        }
        best = best.min(start.elapsed());
    }
    let mbps = data.len() as f64 / 1e6 / best.as_secs_f64();
    println!(
        "{engine:10} {name:22} {mbps:8.1} MB/s ({:.0} ms for {} MB)",
        best.as_secs_f64() * 1e3,
        data.len() / 1_000_000
    );
}

fn bench(only: Option<&str>) {
    let workloads: Vec<(&str, Vec<u8>)> = vec![
        ("plain ASCII", workload::plain(100_000_000)),
        ("SGR colored", workload::colored(100_000_000)),
        ("CJK and emoji", workload::wide(100_000_000)),
        ("TUI redraws", workload::tui(100_000_000)),
    ];
    for (name, data) in &workloads {
        for engine in ENGINES {
            if only.is_none_or(|o| o == engine) {
                throughput(engine, name, data);
            }
        }
    }
    for engine in ENGINES {
        if only.is_none_or(|o| o == engine) {
            snapshot_time(engine);
        }
    }
}

/// Snapshot of a 300 x 100 screen: all rows changed, and one row changed.
fn snapshot_time(engine: &str) {
    let mut e = make(engine, 300, 100, 10_000);
    let mut snap = Snapshot::default();
    let screen = workload::colored_screen(300, 100);
    e.feed(&screen);
    e.snapshot(&mut snap);
    let mut full = Vec::new();
    let mut one = Vec::new();
    for i in 0..300 {
        e.feed(b"\x1b[H");
        e.feed(&screen);
        let start = Instant::now();
        e.snapshot(&mut snap);
        full.push(start.elapsed());
        e.feed(format!("\x1b[{};1H\x1b[3{}mrow {i} changed", i % 100 + 1, i % 8).as_bytes());
        let start = Instant::now();
        e.snapshot(&mut snap);
        one.push(start.elapsed());
    }
    full.sort();
    one.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    println!(
        "{engine:10} snapshot 300x100: all rows p50 {:.0} us p99 {:.0} us; one row p50 {:.1} us p99 {:.1} us",
        us(full[150]),
        us(full[297]),
        us(one[150]),
        us(one[297])
    );
}

fn mem(engine: &str, lines: usize, cols: u16, kind: &str) {
    let mut e = make(engine, cols, 50, lines);
    if lines > 0 {
        let full = workload::line_of(usize::from(cols));
        let mut rng = workload::Rng(9);
        for _ in 0..lines + 50 {
            match kind {
                "colored" => e.feed(&workload::colored_line(&mut rng)),
                "cargo" => e.feed(&workload::plain_line(&mut rng)),
                _ => e.feed(&full),
            }
        }
    }
    println!(
        "{engine} {kind} lines {lines} cols {cols}: history {} engine-reported {:?}",
        e.history_len(),
        e.memory().grid_bytes
    );
}

fn string_flood(engine: &str, kind: &str, mib: usize) {
    let (head, fill): (&[u8], u8) = match kind {
        "osc" => (b"\x1b]0;", b'A'),
        "osc-unknown" => (b"\x1b]7400;", b'A'),
        "dcs" => (b"\x1bP", b'A'),
        "decrqss" => (b"\x1bP$q", b'A'),
        "xtgettcap" => (b"\x1bP+q", b'4'),
        "sixel" => (b"\x1bPq", b'~'),
        "tmux" => (b"\x1bPtmux;", b'A'),
        "apc" => (b"\x1b_", b'A'),
        "kitty" => (b"\x1b_Ga=T,f=32,s=1,v=1,m=1;", b'A'),
        "pm" => (b"\x1b^", b'A'),
        "sos" => (b"\x1bX", b'A'),
        other => panic!("unknown string kind {other}"),
    };
    let mut e = make(engine, 80, 24, 1000);
    e.feed(head);
    let chunk = vec![fill; 1 << 20];
    let start = Instant::now();
    for _ in 0..mib {
        e.feed(&chunk);
    }
    let effects = e.drain_effects().len();
    println!(
        "{engine} {kind}: {mib} MiB unterminated in {:.0} ms, {effects} effects",
        start.elapsed().as_secs_f64() * 1e3
    );
}

fn script(engine: &str, path: &str) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut e = make(engine, 80, 24, 10_000);
    let mut snap = Snapshot::default();
    for line in text.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize| words[i].parse::<usize>().unwrap();
        match words.first().copied() {
            Some("new") => e = make(engine, num(1) as u16, num(2) as u16, num(3)),
            Some("resize") => e.resize(num(1) as u16, num(2) as u16),
            Some("write") => {
                let hex = words[1].as_bytes();
                let bytes: Vec<u8> = hex
                    .chunks(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
                    .collect();
                e.feed(&bytes);
                e.snapshot(&mut snap);
            }
            _ => {}
        }
        println!("{line:.40}: ok");
    }
}

/// The replay size: `COLS` and `ROWS` from the environment, 120 x 40 (the recordings') by
/// default.
fn size() -> (u16, u16) {
    let var = |name: &str, default: u16| {
        std::env::var(name)
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    (var("COLS", 120), var("ROWS", 40))
}

/// Time sending the changed rows of a 300 x 100 snapshot through a pipe to another thread
/// and back as an acknowledgement, as a helper process holding the engine would.
fn ipc() {
    use std::io::{Read, Write};
    let (cols, rows) = (300usize, 100usize);
    let cell = std::mem::size_of::<engine::Cell>();
    let (mut to_reader, mut from_writer) = (std::io::pipe().unwrap(), std::io::pipe().unwrap());
    let reader = std::thread::spawn(move || {
        let mut len = [0u8; 8];
        let mut buf = vec![0u8; cols * rows * cell];
        loop {
            if to_reader.0.read_exact(&mut len).is_err() {
                return;
            }
            let n = u64::from_le_bytes(len) as usize;
            to_reader.0.read_exact(&mut buf[..n]).unwrap();
            from_writer.1.write_all(&[1]).unwrap();
        }
    });
    let mut snap = Snapshot::default();
    let mut e = make("alacritty", cols as u16, rows as u16, 1000);
    e.feed(&workload::colored_screen(cols, rows));
    e.snapshot(&mut snap);
    // Cells as their fields, 12 bytes each.
    let encode = |snap: &Snapshot, rows_wanted: usize, out: &mut Vec<u8>| {
        out.clear();
        for c in &snap.cells[..rows_wanted * cols] {
            out.extend_from_slice(&(c.ch as u32).to_le_bytes());
            for color in [c.fg, c.bg] {
                out.extend_from_slice(&match color {
                    engine::Color::Default => [0, 0, 0],
                    engine::Color::Indexed(i) => [1, i, 0],
                    engine::Color::Rgb(r, g, b) => [r, g ^ 0x80, b],
                });
            }
            out.extend_from_slice(&c.flags.to_le_bytes());
        }
    };
    let mut buf = Vec::new();
    let mut ack = [0u8; 1];
    for (label, rows_sent) in [("all rows", rows), ("one row", 1)] {
        let mut times = Vec::new();
        for _ in 0..2000 {
            let start = Instant::now();
            encode(&snap, rows_sent, &mut buf);
            to_reader
                .1
                .write_all(&(buf.len() as u64).to_le_bytes())
                .unwrap();
            to_reader.1.write_all(&buf).unwrap();
            from_writer.0.read_exact(&mut ack).unwrap();
            times.push(start.elapsed());
        }
        times.sort();
        let us = |d: Duration| d.as_secs_f64() * 1e6;
        println!(
            "pipe round trip, {label} ({} KB): p50 {:.0} us, p99 {:.0} us",
            buf.len() / 1000,
            us(times[1000]),
            us(times[1980])
        );
    }
    drop(to_reader.1);
    reader.join().unwrap();
}
