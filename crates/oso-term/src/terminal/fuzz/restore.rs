//! The snapshot decoder's fuzz target (ADR 0120, step 11): what the next instance restores a
//! terminal from, damaged. A terminal fed by the grammar is written as the library's snapshot;
//! its records are then changed (payload bytes, lengths, tags, records dropped, repeated,
//! swapped or taken from the snapshot before), their CRC32C written again so the damage
//! reaches the record decoders, and the engine restored from it. A restored terminal is fed
//! and drawn on, and sometimes written again. An unchanged snapshot must restore.
//!
//! Must not depend on: the thread or a PTY; it drives the engine directly.
//!
//! `restore_long` takes `long`'s settings but `_INPUT` and `_SCRIPT`; `restore_smoke` runs a
//! few dozen in `just test` when the `dev` feature is on.

#![cfg(feature = "dev")]

use std::env;
use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::sync::atomic::AtomicUsize;
use std::time::{Duration, Instant};

use oso_platform::process::pty::Size;

use super::script::sized;
use super::{BIG, BLOCK, Driver, Rng, SIZES, SLOW, input, scrollback, setting};
use crate::engine::Engine;
use crate::engine::ghostty::Ghostty;
use crate::engine::ghostty::carry::Encoding;
use crate::policy;

/// The envelope before the records: `GHOSTSNP` and a version.
const ENVELOPE: usize = 10;
/// A record's header: tag, payload length, CRC32C.
const HEADER: usize = 10;
/// The tags the library knows, and one it does not.
const TAGS: &[u16] = &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 0x4000];

/// CRC32C (Castagnoli, reflected), as the snapshot's records carry it.
fn crc32c(parts: &[&[u8]]) -> u32 {
    let mut crc = !0u32;
    for byte in parts.iter().flat_map(|part| part.iter()) {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82F6_3B78 & (crc & 1).wrapping_neg());
        }
    }
    !crc
}

struct Record {
    tag: u16,
    payload: Vec<u8>,
}

/// The snapshot's envelope and its records; `None` if it is not framed as the library writes.
fn parse(bytes: &[u8]) -> Option<(Vec<u8>, Vec<Record>)> {
    let envelope = bytes.get(..ENVELOPE)?.to_vec();
    let mut records = Vec::new();
    let mut at = ENVELOPE;
    while at < bytes.len() {
        let header = bytes.get(at..at + HEADER)?;
        let tag = u16::from_le_bytes([header[0], header[1]]);
        let len = u32::from_le_bytes([header[2], header[3], header[4], header[5]]) as usize;
        let payload = bytes.get(at + HEADER..at + HEADER + len)?.to_vec();
        records.push(Record { tag, payload });
        at += HEADER + len;
    }
    Some((envelope, records))
}

/// The records framed again, each with a CRC that matches, or `len` in place of the length.
fn frame(envelope: &[u8], records: &[Record], lie: Option<(usize, u32)>) -> Vec<u8> {
    let mut out = envelope.to_vec();
    for (i, record) in records.iter().enumerate() {
        let tag = record.tag.to_le_bytes();
        let len = match lie {
            Some((at, len)) if at == i => len,
            _ => record.payload.len() as u32,
        }
        .to_le_bytes();
        out.extend_from_slice(&tag);
        out.extend_from_slice(&len);
        out.extend_from_slice(&crc32c(&[&tag, &len, &record.payload]).to_le_bytes());
        out.extend_from_slice(&record.payload);
    }
    out
}

/// One change to a record's payload.
fn damage_payload(rng: &mut Rng, payload: &mut Vec<u8>) {
    let len = payload.len() as u64;
    match rng.below(6) {
        // A few bytes.
        0 | 1 if len > 0 => {
            for _ in 0..1 + rng.below(8) {
                let at = rng.below(len) as usize;
                payload[at] = rng.next() as u8;
            }
        }
        // A count, a length or an index, as an edge value.
        2 if len >= 2 => {
            let width = *rng.pick(&[2usize, 4, 8]);
            let at = rng.below(len.saturating_sub(1)) as usize;
            let value = rng.pick(BIG).wrapping_add(rng.below(3)).wrapping_sub(1);
            let end = (at + width).min(payload.len());
            payload[at..end].copy_from_slice(&value.to_le_bytes()[..end - at]);
        }
        // Cut short.
        3 => payload.truncate(rng.below(len + 1) as usize),
        // Bytes in the middle.
        4 => {
            let at = rng.below(len + 1) as usize;
            let extra: Vec<u8> = (0..1 + rng.below(64)).map(|_| rng.next() as u8).collect();
            payload.splice(at..at, extra);
        }
        // A run of the payload repeated.
        _ if len > 0 => {
            let from = rng.below(len) as usize;
            let to = (from + 1 + rng.below(256) as usize).min(payload.len());
            let run = payload[from..to].to_vec();
            let at = rng.below(len + 1) as usize;
            payload.splice(at..at, run);
        }
        _ => payload.push(rng.next() as u8),
    }
}

/// `bytes`, damaged by one to three changes; `before` is the snapshot before it, to splice.
fn damage(rng: &mut Rng, bytes: &[u8], before: &[u8]) -> Vec<u8> {
    let Some((envelope, mut records)) = parse(bytes) else {
        return bytes.to_vec();
    };
    let mut lie = None;
    for _ in 0..1 + rng.below(3) {
        if records.is_empty() {
            break;
        }
        let n = records.len() as u64;
        let i = rng.below(n) as usize;
        match rng.below(10) {
            0..=4 => damage_payload(rng, &mut records[i].payload),
            5 => records[i].tag = *rng.pick(TAGS),
            6 => drop(records.remove(i)),
            7 => {
                let copy = Record {
                    tag: records[i].tag,
                    payload: records[i].payload.clone(),
                };
                records.insert(rng.below(n + 1) as usize, copy);
            }
            8 => records.swap(i, rng.below(n) as usize),
            _ => {
                if let Some((_, mut others)) = parse(before).filter(|(_, o)| !o.is_empty()) {
                    let j = rng.below(others.len() as u64) as usize;
                    records[i] = others.swap_remove(j);
                }
            }
        }
        if rng.chance(3) {
            lie = Some((i.min(records.len().saturating_sub(1)), rng.next() as u32));
        }
    }
    let mut out = frame(&envelope, &records, lie);
    // Now and then a change the CRC does not cover, or one to the envelope.
    if rng.chance(3) && !out.is_empty() {
        let at = rng.below(out.len() as u64) as usize;
        out[at] ^= 1 << rng.below(8);
    }
    if rng.chance(2) {
        out.truncate(rng.below(out.len() as u64 + 1) as usize);
    }
    out
}

/// The terminal one input is restored to: written, damaged, restored, fed and drawn on.
struct Source {
    driver: Driver,
    size: Size,
    scrollback: usize,
    before: Vec<u8>,
    /// Damaged snapshots, and those of them the library restored.
    damaged: u64,
    taken: u64,
}

impl Source {
    fn new(iteration: u64) -> Source {
        let scrollback = scrollback(iteration);
        Source {
            driver: Driver::new(scrollback),
            size: sized(80, 24),
            scrollback,
            before: Vec::new(),
            damaged: 0,
            taken: 0,
        }
    }

    /// Runs the input of `iteration`; returns the bytes restored from.
    fn run(&mut self, seed: u64, iteration: u64, resizes: bool) -> usize {
        let case = input(seed, iteration, resizes);
        if let Some(size) = case.resize {
            self.size = size;
        }
        self.driver.run(case);
        let mut rng = Rng::new(seed ^ 0x7E57_0E57_0E57_0E57, iteration);
        let escapes = rng.chance(5);
        let encoding = if escapes {
            Encoding::Escapes
        } else {
            Encoding::Snapshot
        };
        let budget = AtomicUsize::new(usize::MAX / 2);
        let Ok(bytes) = self.driver.engine.encode(encoding, &budget) else {
            return 0;
        };
        let kept = escapes || rng.chance(5);
        let state = if kept {
            bytes.clone()
        } else {
            damage(&mut rng, &bytes, &self.before)
        };
        let size = if rng.chance(20) {
            let &(columns, rows) = rng.pick(SIZES);
            sized(columns, rows)
        } else {
            self.size
        };
        let restored = Ghostty::restore(encoding, &state, size, self.scrollback, policy::XTVERSION);
        if !escapes {
            self.before = bytes;
        }
        if !kept {
            self.damaged += 1;
            self.taken += u64::from(restored.is_some());
        }
        match restored {
            Some(engine) => Source::drive(engine, size, seed, iteration, &mut rng),
            None => assert!(!kept, "an unchanged snapshot did not restore"),
        }
        state.len()
    }

    /// Feeds and draws on a restored terminal as its thread would, and writes it again.
    fn drive(engine: Ghostty, size: Size, seed: u64, iteration: u64, rng: &mut Rng) {
        let mut driver = Driver::around(engine, size);
        driver
            .engine
            .snapshot(&mut driver.snapshot, &mut driver.spare);
        driver.snapshot.clear(&mut driver.spare);
        driver.run(input(seed ^ 0xAF7E_0000_AF7E, iteration, true));
        if rng.chance(20) {
            let budget = AtomicUsize::new(usize::MAX / 2);
            let _ = driver.engine.encode(Encoding::Snapshot, &budget);
        }
    }
}

#[test]
fn restore_smoke() {
    let mut source = Source::new(0);
    for iteration in 0..60u64 {
        source.run(0x5eed, iteration, true);
    }
}

/// The long run of the decoder; see the module's header for its settings.
#[test]
#[ignore = "run on the Mac mini, as `just fuzz-term` runs `long`"]
fn restore_long() {
    let report_path = env::var("OSO_TERM_FUZZ_REPORT").unwrap_or_else(|_| "fuzz-report.txt".into());
    let mut report = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&report_path)
        .expect("the report file");
    let seed = setting("SEED").unwrap_or(1);
    let resizes = env::var_os("OSO_TERM_FUZZ_NO_RESIZE").is_none();
    let seconds = setting("SECONDS").unwrap_or(60);
    let state_path = env::var("OSO_TERM_FUZZ_STATE").unwrap_or_else(|_| "fuzz-state".into());
    let mut state = fs::File::create(state_path).expect("the state file");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut iteration = setting("FIRST").unwrap_or(0);
    let (mut bytes, mut slow, first) = (0u64, 0u64, iteration);
    let (mut damaged, mut taken) = (0u64, 0u64);
    let mut source = Source::new(iteration);
    while Instant::now() < deadline {
        if iteration.is_multiple_of(BLOCK) {
            (damaged, taken) = (damaged + source.damaged, taken + source.taken);
            source = Source::new(iteration);
        }
        state.seek(SeekFrom::Start(0)).expect("the state file");
        state
            .write_all(format!("{iteration:020}\n").as_bytes())
            .expect("the state file");
        let started = Instant::now();
        let len = source.run(seed, iteration, resizes);
        let took = started.elapsed();
        if took > SLOW {
            slow += 1;
            let ms = took.as_millis();
            let line = format!("SLOW iteration {iteration} seed {seed}: {ms} ms, {len} bytes\n");
            report.write_all(line.as_bytes()).expect("the report");
        }
        bytes += len as u64;
        iteration += 1;
    }
    let (damaged, taken) = (damaged + source.damaged, taken + source.taken);
    let done = format!(
        "done: iterations {first}..{iteration}, {} MB restored from, {damaged} damaged \
         ({taken} restored), {slow} slow, seed {seed}\n",
        bytes / 1_000_000
    );
    report.write_all(done.as_bytes()).expect("the report");
}

#[test]
fn the_crc_is_castagnoli() {
    // The check value of CRC-32/ISCSI.
    assert_eq!(crc32c(&[b"1234", b"56789"]), 0xE306_9283);
}

#[test]
fn an_unchanged_snapshot_frames_back_to_itself() {
    let mut engine = Ghostty::new(sized(80, 24), 100, policy::XTVERSION).expect("an engine");
    engine.feed(b"\x1b[1;31mred\x1b[0m\r\nplain\x1b[?1049halt");
    let budget = AtomicUsize::new(usize::MAX / 2);
    let bytes = engine
        .encode(Encoding::Snapshot, &budget)
        .expect("a snapshot");
    let (envelope, records) = parse(&bytes).expect("framed");
    assert!(records.len() > 2);
    assert_eq!(frame(&envelope, &records, None), bytes);
}
