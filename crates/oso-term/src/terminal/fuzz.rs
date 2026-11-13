//! oso-term's fuzz target (ADR 0120, step 1's condition 1): a generator of crafted escape
//! sequences fed through the engine and the policy, as a terminal's thread feeds them.
//! libFuzzer's coverage does not reach inside Zig, so inputs come from a grammar: CSI with huge
//! parameters, private modes, SGR's colon forms, OSC numbers with fixed payloads, DCS heads,
//! Kitty APC keys, ESC forms, REP and margins, invalid UTF-8, long clusters, a link on each
//! of many cells, and resizes up to the largest grid.
//!
//! Must not depend on: the thread or a PTY; it drives the engine directly.
//!
//! Each input comes from (seed, iteration) alone. `long` writes the iteration to a state file
//! before each feed, so when the process dies that file names the input; `just fuzz-term`
//! restarts it after the input and records the stop. `smoke` runs a few hundred in `just test`.
//!
//! Settings of `long`, through the environment: `OSO_TERM_FUZZ_SECONDS`, `_SEED`, `_FIRST`
//! (the first iteration), `_STATE` and `_REPORT` (files), `_NO_RESIZE`, `_SCROLLBACK`; and
//! `_INPUT=<iteration>`, which writes that input as a script (`new`, `resize`, `write HEX`)
//! to the report instead of running, for reducing a finding outside the repository, or
//! `_SCRIPT=<file>`, which runs one.

#![cfg(unix)]

use std::env;
use std::fmt::Write as _;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::time::{Duration, Instant};

use oso_platform::process::pty::Size;

use super::thread::judge;
use crate::engine::ghostty::Ghostty;
use crate::engine::{Effect, Engine};
use crate::event::Event;
use crate::limits;
use crate::policy;
use crate::snapshot::{Row, Snapshot};

/// A feed slower than this is reported as a hang candidate.
const SLOW: Duration = Duration::from_millis(500);
/// Each block of this many inputs gets a fresh terminal.
const BLOCK: u64 = 200;

const CSI_FINALS: &[u8] = b"@ABCDEFGHIJKLMPSTXZ`abcdefghilmnpqrstuxz{|}~";
const PRIVATE_MODES: &[u32] = &[
    1, 3, 5, 6, 7, 12, 25, 47, 66, 69, 80, 1000, 1002, 1003, 1004, 1005, 1006, 1015, 1016, 1047,
    1048, 1049, 2004, 2026, 2027, 2031, 2048, 7727, 8452,
];
const OSC_NUMBERS: &[u32] = &[
    0, 1, 2, 4, 5, 7, 8, 9, 10, 11, 12, 17, 19, 21, 22, 46, 50, 51, 52, 66, 72, 104, 105, 110, 111,
    112, 133, 633, 777, 1337, 3008, 5522, 7501, 99999,
];
const BIG: &[u64] = &[
    0,
    1,
    2,
    255,
    256,
    999,
    32767,
    32768,
    65535,
    65536,
    2_147_483_647,
    4_294_967_295,
    99_999_999_999_999,
];
const SIZES: &[(u16, u16)] = &[
    (1, 1),
    (2, 1),
    (1, 2),
    (80, 24),
    (300, 100),
    (500, 300),
    (1000, 500),
    (13, 7),
];

/// xorshift64*, enough for a generator.
struct Rng(u64);

impl Rng {
    fn new(seed: u64, iteration: u64) -> Rng {
        let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
            ^ iteration
                .wrapping_add(1)
                .wrapping_mul(0xD1B5_4A32_D192_ED03);
        Rng(mixed | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }

    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }

    fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len() as u64) as usize]
    }
}

fn param(rng: &mut Rng) -> u64 {
    if rng.chance(25) {
        *rng.pick(BIG)
    } else {
        rng.below(200)
    }
}

fn params(rng: &mut Rng, out: &mut Vec<u8>) {
    for i in 0..rng.below(6) {
        if i > 0 {
            out.push(if rng.chance(15) { b':' } else { b';' });
        }
        if !rng.chance(10) {
            out.extend_from_slice(param(rng).to_string().as_bytes());
        }
    }
}

fn payload(rng: &mut Rng, out: &mut Vec<u8>, max: u64) {
    for _ in 0..rng.below(max) {
        out.push(match rng.below(10) {
            0 => rng.below(256) as u8,
            1 => b';',
            2 => b'=',
            3 => b',',
            _ => b'!' + rng.below(94) as u8,
        });
    }
}

fn terminator(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(10) {
        0 => {}
        1 => out.push(0x18),
        2 => out.push(0x1b),
        3..=5 => out.push(0x07),
        _ => out.extend_from_slice(b"\x1b\\"),
    }
}

fn text(rng: &mut Rng, out: &mut Vec<u8>) {
    const CLUSTERS: &[&str] = &[
        "日本",
        "👍",
        "👨\u{200d}👩\u{200d}👧",
        "e\u{301}\u{302}\u{303}",
        "\u{200d}",
        "\u{fe0f}",
        "🇯",
        "\u{0}",
        "ｱ",
        "\u{10ffff}",
    ];
    match rng.below(6) {
        0 | 1 => (0..rng.below(40)).for_each(|_| out.push(b' ' + rng.below(95) as u8)),
        2 => (0..rng.below(20)).for_each(|_| out.extend_from_slice(rng.pick(CLUSTERS).as_bytes())),
        3 => long_clusters(rng, out),
        4 => linked_cells(rng, out),
        _ => (0..rng.below(8)).for_each(|_| out.push(0x80 + rng.below(0x80) as u8)),
    }
}

/// Cells of a base and up to 80 marks, past the most the library keeps of one cluster.
fn long_clusters(rng: &mut Rng, out: &mut Vec<u8>) {
    const BASES: &[&str] = &["a", "\u{1F600}", "日", "\u{10ffff}"];
    const MARKS: &[&str] = &["\u{301}", "\u{E0100}", "\u{FE0F}", "\u{200D}", "\u{1F3FB}"];
    for _ in 0..1 + rng.below(8) {
        out.extend_from_slice(rng.pick(BASES).as_bytes());
        for _ in 0..rng.below(81) {
            out.extend_from_slice(rng.pick(MARKS).as_bytes());
        }
    }
}

/// A new OSC 8 link on each of many cells, some with ids, some ended.
fn linked_cells(rng: &mut Rng, out: &mut Vec<u8>) {
    for cell in 0..rng.below(300) {
        let id = if rng.chance(50) {
            format!("id={}", rng.below(1000))
        } else {
            String::new()
        };
        out.extend_from_slice(format!("\x1b]8;{id};http://e/{cell}\x1b\\x").as_bytes());
        if rng.chance(10) {
            out.extend_from_slice(b"\x1b]8;;\x1b\\");
        }
    }
}

fn csi(rng: &mut Rng, out: &mut Vec<u8>) {
    out.extend_from_slice(b"\x1b[");
    match rng.below(4) {
        0 => {
            let mode = rng.pick(PRIVATE_MODES);
            out.extend_from_slice(format!("?{mode}").as_bytes());
            if rng.chance(20) {
                out.extend_from_slice(b"$p");
            } else {
                out.push(*rng.pick(b"hlsr"));
            }
        }
        1 => {
            const FORMS: &[&str] = &[
                "38:2::", "38;2;", "48:5:", "58:2:", "4:", "38;5;", "1;", "7;", "21;", "53;",
            ];
            for _ in 0..1 + rng.below(4) {
                out.extend_from_slice(rng.pick(FORMS).as_bytes());
                out.extend_from_slice(param(rng).to_string().as_bytes());
                out.push(b';');
            }
            out.push(b'm');
        }
        2 => {
            const FORMS: &[&str] = &[
                "{}b",
                "{}I",
                "{}Z",
                "{};{}r",
                "?69h\x1b[{};{}s",
                "{}@",
                "{}P",
                "{}L",
                "{}M",
                "{}X",
                "{}S",
                "{}T",
                "{};{}H",
                "{}g",
            ];
            let form = rng.pick(FORMS);
            let first = param(rng).to_string();
            let second = param(rng).to_string();
            let form = form.replacen("{}", &first, 1).replacen("{}", &second, 1);
            out.extend_from_slice(form.as_bytes());
        }
        _ => {
            if rng.chance(30) {
                out.push(*rng.pick(b"?>=<"));
            }
            params(rng, out);
            if rng.chance(15) {
                out.push(*rng.pick(b" !\"#$%&'*+"));
            }
            out.push(*rng.pick(CSI_FINALS));
        }
    }
}

fn osc(rng: &mut Rng, out: &mut Vec<u8>) {
    const FIXED: &[&[u8]] = &[
        b"4;1;50",
        b"4;3;",
        b"notify;title;body",
        b"A",
        b"B",
        b"C;cmd",
        b"D;0",
        b"c;?",
        b"c;aGVsbG8=",
        b"1;rgb:ff/00/00",
        b"?",
        b"id=x;http://e/",
        b"file://host/tmp",
    ];
    let number = rng.pick(OSC_NUMBERS);
    out.extend_from_slice(format!("\x1b]{number};").as_bytes());
    if rng.chance(50) {
        out.extend_from_slice(rng.pick(FIXED));
    }
    let max = if rng.chance(3) { 70_000 } else { 64 };
    payload(rng, out, max);
    terminator(rng, out);
}

fn dcs_or_apc(rng: &mut Rng, out: &mut Vec<u8>) {
    const DCS: &[&[u8]] = &[
        b"\x1bP$q",
        b"\x1bP+q",
        b"\x1bPq",
        b"\x1bP1;2;3q",
        b"\x1bPtmux;",
        b"\x1bP=1s",
    ];
    const KEYS: &[&str] = &[
        "a=T", "a=t", "a=p", "a=d", "a=q", "f=24", "f=32", "f=100", "t=d", "t=f", "t=t", "t=s",
        "m=1", "m=0", "o=z", "q=2",
    ];
    const BASE64: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/=";
    match rng.below(3) {
        0 => {
            out.extend_from_slice(rng.pick(DCS));
            let max = if rng.chance(3) { 70_000 } else { 64 };
            payload(rng, out, max);
        }
        1 => {
            out.extend_from_slice(b"\x1b_G");
            for i in 0..1 + rng.below(6) {
                if i > 0 {
                    out.push(b',');
                }
                out.extend_from_slice(rng.pick(KEYS).as_bytes());
            }
            for key in [
                "s", "v", "i", "p", "x", "y", "w", "h", "c", "r", "z", "S", "O",
            ] {
                if rng.chance(20) {
                    out.extend_from_slice(format!(",{key}={}", param(rng)).as_bytes());
                }
            }
            out.push(b';');
            let len = if rng.chance(5) { 40_000 } else { 200 };
            (0..rng.below(len)).for_each(|_| out.push(*rng.pick(BASE64)));
        }
        _ => {
            out.extend_from_slice(rng.pick(&[b"\x1bX".as_slice(), b"\x1b^"]));
            payload(rng, out, 200);
        }
    }
    terminator(rng, out);
}

fn esc(rng: &mut Rng, out: &mut Vec<u8>) {
    const ESCS: &[&[u8]] = &[
        b"\x1b#8", b"\x1b(0", b"\x1b(B", b"\x1b)0", b"\x0e", b"\x0f", b"\x1b7", b"\x1b8", b"\x1bc",
        b"\x1bD", b"\x1bE", b"\x1bM", b"\x1bH", b"\x1b=", b"\x1b>", b"\x1b#3", b"\x1b#6", b"\x1bN",
        b"\x1bO", b"\x1b%G",
    ];
    out.extend_from_slice(rng.pick(ESCS));
}

fn piece(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(20) {
        0..=5 => text(rng, out),
        6 => out.push(rng.below(32) as u8),
        7..=12 => csi(rng, out),
        13 | 14 => osc(rng, out),
        15 | 16 => dcs_or_apc(rng, out),
        17 => esc(rng, out),
        _ => (0..rng.below(32)).for_each(|_| out.push(rng.below(256) as u8)),
    }
}

/// The input of `iteration` under `seed`: bytes of output, and a resize after them.
fn input(seed: u64, iteration: u64, resizes: bool) -> (Vec<u8>, Option<Size>) {
    let mut rng = Rng::new(seed, iteration);
    let mut out = Vec::new();
    for _ in 0..1 + rng.below(200) {
        piece(&mut rng, &mut out);
        if out.len() > 128 * 1024 {
            break;
        }
    }
    let resize = (resizes && rng.chance(5)).then(|| {
        let &(columns, rows) = rng.pick(SIZES);
        sized(columns, rows)
    });
    (out, resize)
}

fn sized(columns: u16, rows: u16) -> Size {
    // As a terminal's thread clamps every size.
    limits::clamp(Size {
        columns,
        rows,
        width: columns.saturating_mul(8),
        height: rows.saturating_mul(16),
    })
}

/// The scrollback of `iteration`'s block, rotating 100, 1,000 and 10,000 lines.
fn scrollback(iteration: u64) -> usize {
    let set = env::var("OSO_TERM_FUZZ_SCROLLBACK")
        .ok()
        .and_then(|v| v.parse().ok());
    set.unwrap_or([100, 1000, 10_000][(iteration / BLOCK % 3) as usize])
}

/// Feeds one input as a terminal's thread would: parse, resize, judge (with the thread's own
/// `judge`), snapshot.
struct Driver {
    engine: Ghostty,
    snapshot: Snapshot,
    spare: Vec<Row>,
    effects: Vec<Effect>,
    replies: Vec<u8>,
}

impl Driver {
    fn new(scrollback: usize) -> Driver {
        let size = sized(80, 24);
        Driver {
            engine: Ghostty::new(size, scrollback, policy::XTVERSION).expect("an engine"),
            snapshot: Snapshot::new(size),
            spare: Vec::new(),
            effects: Vec::new(),
            replies: Vec::new(),
        }
    }

    fn feed(&mut self, data: &[u8], resize: Option<Size>) {
        self.engine.feed(data);
        if let Some(size) = resize {
            self.engine.resize(size);
        }
        self.engine.take_effects(&mut self.effects);
        judge(&mut self.effects, &mut self.replies, &mut self.snapshot);
        self.replies.clear();
        for event in &self.snapshot.events {
            check(event);
        }
        self.engine.snapshot(&mut self.snapshot, &mut self.spare);
        self.snapshot.clear(&mut self.spare);
    }
}

/// What any input may raise: shown text is short and has no control characters.
fn check(event: &Event) {
    let shown: &[&str] = match event {
        Event::Title(title) => &[title.as_str()],
        Event::Notification { title, body } => &[title.as_str(), body.as_str()],
        _ => &[],
    };
    for text in shown {
        assert!(text.len() <= 256, "{event:?}");
        assert!(!text.chars().any(char::is_control), "{event:?}");
    }
}

#[test]
fn smoke() {
    let mut driver = Driver::new(scrollback(0));
    for iteration in 0..500u64 {
        if iteration.is_multiple_of(BLOCK) {
            driver = Driver::new(scrollback(iteration));
        }
        let (data, resize) = input(0x5eed, iteration, true);
        driver.feed(&data, resize);
    }
}

fn setting(name: &str) -> Option<u64> {
    env::var(format!("OSO_TERM_FUZZ_{name}"))
        .ok()
        .and_then(|v| v.parse().ok())
}

/// The long run `just fuzz-term` drives; see the module's header for its settings.
#[test]
#[ignore = "run by `just fuzz-term`"]
fn long() {
    let report_path = env::var("OSO_TERM_FUZZ_REPORT").unwrap_or_else(|_| "fuzz-report.txt".into());
    let mut report = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&report_path)
        .expect("the report file");
    let seed = setting("SEED").unwrap_or(1);
    let resizes = env::var_os("OSO_TERM_FUZZ_NO_RESIZE").is_none();
    if let Some(iteration) = setting("INPUT") {
        let script = script_of(seed, iteration, resizes);
        report.write_all(script.as_bytes()).expect("the report");
        return;
    }
    if let Ok(path) = env::var("OSO_TERM_FUZZ_SCRIPT") {
        run_script(&fs::read_to_string(path).expect("the script"));
        return;
    }
    let seconds = setting("SECONDS").unwrap_or(60);
    let state_path = env::var("OSO_TERM_FUZZ_STATE").unwrap_or_else(|_| "fuzz-state".into());
    let mut state = fs::File::create(state_path).expect("the state file");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut iteration = setting("FIRST").unwrap_or(0);
    let (mut bytes, mut slow, first) = (0u64, 0u64, iteration);
    let mut driver = Driver::new(scrollback(iteration));
    while Instant::now() < deadline {
        if iteration.is_multiple_of(BLOCK) {
            driver = Driver::new(scrollback(iteration));
        }
        state.seek(SeekFrom::Start(0)).expect("the state file");
        state
            .write_all(format!("{iteration:020}\n").as_bytes())
            .expect("the state file");
        let (data, resize) = input(seed, iteration, resizes);
        let started = Instant::now();
        driver.feed(&data, resize);
        let took = started.elapsed();
        if took > SLOW {
            slow += 1;
            let ms = took.as_millis();
            let line = format!(
                "SLOW iteration {iteration} seed {seed}: {ms} ms, {} bytes\n",
                data.len()
            );
            report.write_all(line.as_bytes()).expect("the report");
        }
        bytes += data.len() as u64;
        iteration += 1;
    }
    let done = format!(
        "done: iterations {first}..{iteration}, {} MB, {slow} slow, seed {seed}\n",
        bytes / 1_000_000
    );
    report.write_all(done.as_bytes()).expect("the report");
}

/// The script that rebuilds `iteration`'s terminal: its block's inputs up to it.
fn script_of(seed: u64, iteration: u64, resizes: bool) -> String {
    let mut script = format!("new 80 24 {}\n", scrollback(iteration));
    for i in iteration - iteration % BLOCK..=iteration {
        let (data, resize) = input(seed, i, resizes);
        script.push_str("write ");
        for byte in data {
            let _ = write!(script, "{byte:02x}");
        }
        script.push('\n');
        if let Some(size) = resize {
            let _ = writeln!(script, "resize {} {}", size.columns, size.rows);
        }
    }
    script
}

fn run_script(script: &str) {
    let mut driver = Driver::new(scrollback(0));
    for line in script.lines() {
        let words: Vec<&str> = line.split_whitespace().collect();
        match words.as_slice() {
            ["new", _, _, lines] => driver = Driver::new(lines.parse().expect("lines")),
            ["resize", columns, rows] => {
                let size = sized(
                    columns.parse().expect("columns"),
                    rows.parse().expect("rows"),
                );
                driver.feed(&[], Some(size));
            }
            ["write", hex] => {
                let data: Vec<u8> = (0..hex.len() / 2)
                    .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).expect("hex"))
                    .collect();
                driver.feed(&data, None);
            }
            _ => panic!("unknown script line {line:?}"),
        }
    }
}
