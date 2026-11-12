//! A grammar-based generator of crafted escape sequences, fed to one engine in-process. The
//! same generator for both engines: libFuzzer's coverage would not see inside Zig.
//!
//! Each iteration's input comes from (seed, iteration) alone, so `replay` rebuilds it. The
//! iteration number is written to the state file before each feed: if the process dies, that
//! file names the input that killed it.

use std::io::{Seek, SeekFrom, Write};
use std::time::{Duration, Instant};

use crate::workload::Rng;

/// A feed slower than this is reported as a hang candidate.
const SLOW: Duration = Duration::from_millis(500);

const CSI_FINALS: &[u8] = b"@ABCDEFGHIJKLMPSTXZ`abcdefghilmnpqrstuxz{|}~";
const PRIVATE_MODES: &[u32] = &[
    1, 3, 5, 6, 7, 12, 25, 47, 66, 69, 80, 1000, 1002, 1003, 1004, 1005, 1006, 1015, 1016, 1047,
    1048, 1049, 2004, 2026, 2027, 2031, 2048, 7727, 8452,
];
const OSC_NUMBERS: &[u32] = &[
    0, 1, 2, 4, 5, 7, 8, 9, 10, 11, 12, 17, 19, 21, 22, 46, 50, 51, 52, 66, 104, 105, 110, 111,
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
    2147483647,
    4294967295,
    99999999999999,
];

fn param(rng: &mut Rng) -> u64 {
    if rng.chance(25) {
        BIG[rng.below(BIG.len() as u64) as usize]
    } else {
        rng.below(200)
    }
}

fn push_params(rng: &mut Rng, out: &mut Vec<u8>) {
    let n = rng.below(6);
    for i in 0..n {
        if i > 0 {
            out.push(if rng.chance(15) { b':' } else { b';' });
        }
        if !rng.chance(10) {
            out.extend_from_slice(param(rng).to_string().as_bytes());
        }
    }
}

fn payload(rng: &mut Rng, out: &mut Vec<u8>, max: u64) {
    let len = rng.below(max);
    for _ in 0..len {
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
        0 => {}                                // never ends
        1 => out.push(0x18),                   // CAN
        2 => out.push(0x1b),                   // a lone ESC
        3..=5 => out.push(0x07),               // BEL
        _ => out.extend_from_slice(b"\x1b\\"), // ST
    }
}

fn piece(rng: &mut Rng, out: &mut Vec<u8>) {
    match rng.below(20) {
        0..=3 => {
            for _ in 0..rng.below(40) {
                out.push(b' ' + rng.below(95) as u8);
            }
        }
        4 => {
            let s = [
                "日本",
                "👍",
                "👨‍👩‍👧",
                "e\u{301}\u{302}\u{303}",
                "\u{200d}",
                "\u{fe0f}",
                "🇯",
                "\u{0}",
                "ｱ",
                "\u{10ffff}",
            ];
            for _ in 0..rng.below(20) {
                out.extend_from_slice(s[rng.below(s.len() as u64) as usize].as_bytes());
            }
        }
        5 => {
            // Invalid and truncated UTF-8.
            for _ in 0..rng.below(8) {
                out.push(0x80 + rng.below(0x80) as u8);
            }
        }
        6 => out.push(rng.below(32) as u8),
        7..=10 => {
            out.extend_from_slice(b"\x1b[");
            if rng.chance(30) {
                out.push(b"?>=<"[rng.below(4) as usize]);
            }
            push_params(rng, out);
            if rng.chance(15) {
                out.push(b" !\"#$%&'*+"[rng.below(10) as usize]);
            }
            out.push(CSI_FINALS[rng.below(CSI_FINALS.len() as u64) as usize]);
        }
        11 => {
            let mode = PRIVATE_MODES[rng.below(PRIVATE_MODES.len() as u64) as usize];
            let end = [b'h', b'l', b's', b'r'][rng.below(4) as usize];
            out.extend_from_slice(format!("\x1b[?{mode}").as_bytes());
            if rng.chance(20) {
                out.extend_from_slice(b"$p");
            } else {
                out.push(end);
            }
        }
        12 => {
            // SGR with subparameters and huge colors.
            out.extend_from_slice(b"\x1b[");
            let forms = [
                "38:2::", "38;2;", "48:5:", "58:2:", "4:", "38;5;", "1;", "7;", "21;", "53;",
            ];
            for _ in 0..1 + rng.below(4) {
                out.extend_from_slice(forms[rng.below(forms.len() as u64) as usize].as_bytes());
                out.extend_from_slice(param(rng).to_string().as_bytes());
                out.push(b';');
            }
            out.push(b'm');
        }
        13 | 14 => {
            let n = OSC_NUMBERS[rng.below(OSC_NUMBERS.len() as u64) as usize];
            out.extend_from_slice(format!("\x1b]{n};").as_bytes());
            let fixed: &[&[u8]] = &[
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
            if rng.chance(50) {
                out.extend_from_slice(fixed[rng.below(fixed.len() as u64) as usize]);
            }
            let max = if rng.chance(3) { 70_000 } else { 64 };
            payload(rng, out, max);
            terminator(rng, out);
        }
        15 => {
            // DCS: DECRQSS, XTGETTCAP, sixel, tmux passthrough.
            let heads: &[&[u8]] = &[
                b"\x1bP$q",
                b"\x1bP+q",
                b"\x1bPq",
                b"\x1bP1;2;3q",
                b"\x1bPtmux;",
                b"\x1bP=1s",
            ];
            out.extend_from_slice(heads[rng.below(heads.len() as u64) as usize]);
            let max = if rng.chance(3) { 70_000 } else { 64 };
            payload(rng, out, max);
            terminator(rng, out);
        }
        16 => {
            // APC: Kitty graphics, and SOS/PM.
            if rng.chance(70) {
                out.extend_from_slice(b"\x1b_G");
                let keys = [
                    "a=T", "a=t", "a=p", "a=d", "a=q", "f=24", "f=32", "f=100", "t=d", "t=f",
                    "t=t", "t=s", "m=1", "m=0", "o=z", "q=2",
                ];
                for i in 0..1 + rng.below(6) {
                    if i > 0 {
                        out.push(b',');
                    }
                    out.extend_from_slice(keys[rng.below(keys.len() as u64) as usize].as_bytes());
                }
                for k in [
                    "s", "v", "i", "p", "x", "y", "w", "h", "c", "r", "z", "S", "O",
                ] {
                    if rng.chance(20) {
                        out.extend_from_slice(format!(",{k}={}", param(rng)).as_bytes());
                    }
                }
                out.push(b';');
                let len = if rng.chance(5) { 40_000 } else { 200 };
                for _ in 0..rng.below(len) {
                    out.push(
                        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/="
                            [rng.below(65) as usize],
                    );
                }
            } else {
                out.extend_from_slice([b"\x1bX".as_slice(), b"\x1b^"][rng.below(2) as usize]);
                payload(rng, out, 200);
            }
            terminator(rng, out);
        }
        17 => {
            let escs: &[&[u8]] = &[
                b"\x1b#8", b"\x1b(0", b"\x1b(B", b"\x1b)0", b"\x0e", b"\x0f", b"\x1b7", b"\x1b8",
                b"\x1bc", b"\x1bD", b"\x1bE", b"\x1bM", b"\x1bH", b"\x1b=", b"\x1b>", b"\x1b#3",
                b"\x1b#6", b"\x1bN", b"\x1bO", b"\x1b%G",
            ];
            out.extend_from_slice(escs[rng.below(escs.len() as u64) as usize]);
        }
        18 => {
            // Repeat, tabs, regions and margins with large counts.
            let forms = [
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
            let f = forms[rng.below(forms.len() as u64) as usize];
            let s = f.replacen("{}", &param(rng).to_string(), 1).replacen(
                "{}",
                &param(rng).to_string(),
                1,
            );
            out.extend_from_slice(b"\x1b[");
            out.extend_from_slice(s.as_bytes());
        }
        _ => {
            // Random bytes.
            for _ in 0..rng.below(32) {
                out.push(rng.below(256) as u8);
            }
        }
    }
}

/// The input for one iteration, at most about 128 KiB, and a resize to apply after it.
pub fn input(seed: u64, iteration: u64) -> (Vec<u8>, Option<(u16, u16)>) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ iteration
            .wrapping_add(1)
            .wrapping_mul(0xD1B5_4A32_D192_ED03)
        | 1);
    let mut out = Vec::new();
    let pieces = 1 + rng.below(200);
    for _ in 0..pieces {
        piece(&mut rng, &mut out);
        if out.len() > 128 * 1024 {
            break;
        }
    }
    let resize = rng.chance(5).then(|| {
        let sizes = [
            (1, 1),
            (2, 1),
            (1, 2),
            (80, 24),
            (300, 100),
            (500, 300),
            (13, 7),
        ];
        sizes[rng.below(sizes.len() as u64) as usize]
    });
    (out, resize)
}

/// Each run of 200 inputs gets a fresh terminal with 100, 1,000 or 10,000 lines of
/// scrollback, oso's default being the last.
fn scrollback(iteration: u64) -> usize {
    [100, 1000, 10_000][(iteration / 200 % 3) as usize]
}

/// Start at iteration `first`, so a run can go on after a crash.
pub fn run(engine: &str, seconds: u64, seed: u64, state: &str, first: u64) {
    let mut state = std::fs::File::create(state).unwrap();
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut snap = engine::Snapshot::default();
    let (mut iteration, mut bytes, mut slow, mut max_mem) = (first, 0u64, 0u64, 0u64);
    let mut e = crate::make(engine, 80, 24, scrollback(iteration));
    let mut last_report = Instant::now();
    while Instant::now() < deadline {
        if iteration % 200 == 0 {
            e = crate::make(engine, 80, 24, scrollback(iteration));
        }
        state.seek(SeekFrom::Start(0)).unwrap();
        state
            .write_all(format!("{iteration:020}\n").as_bytes())
            .unwrap();
        let (data, resize) = input(seed, iteration);
        let start = Instant::now();
        e.feed(&data);
        if let Some((cols, rows)) = resize {
            e.resize(cols, rows);
        }
        e.snapshot(&mut snap);
        let _ = e.screen_text();
        let _ = e.drain_effects();
        let elapsed = start.elapsed();
        if elapsed > SLOW {
            slow += 1;
            println!(
                "SLOW iteration {iteration} seed {seed}: {} ms for {} bytes",
                elapsed.as_millis(),
                data.len()
            );
        }
        if iteration % 100 == 0 {
            max_mem = max_mem.max(e.memory().grid_bytes.unwrap_or(0));
        }
        bytes += data.len() as u64;
        iteration += 1;
        if last_report.elapsed() > Duration::from_secs(300) {
            println!(
                "{engine}: {iteration} iterations, {} MB, {slow} slow, max engine memory {} MB",
                bytes / 1_000_000,
                max_mem / 1_000_000
            );
            last_report = Instant::now();
        }
    }
    println!(
        "{engine} done: {iteration} iterations, {} MB, {slow} slow, max engine memory {} MB",
        bytes / 1_000_000,
        max_mem / 1_000_000
    );
}

/// Rerun iteration `iteration` of `seed`, after the 200-iteration run of inputs before it
/// on the same terminal, as `run` fed them.
pub fn replay(engine: &str, iteration: u64, seed: u64) {
    let mut e = crate::make(engine, 80, 24, scrollback(iteration));
    let mut snap = engine::Snapshot::default();
    for i in (iteration - iteration % 200)..=iteration {
        let (data, resize) = input(seed, i);
        let start = Instant::now();
        e.feed(&data);
        if let Some((cols, rows)) = resize {
            e.resize(cols, rows);
        }
        e.snapshot(&mut snap);
        let _ = e.screen_text();
        let _ = e.drain_effects();
        println!(
            "iteration {i}: {} bytes in {} ms",
            data.len(),
            start.elapsed().as_millis()
        );
    }
}
