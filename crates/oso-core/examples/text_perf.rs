//! Informal timings for the rope and the display model (M1-4).
//!
//! Must not depend on: other workspace crates or crate internals; it uses
//! only the public API.
//!
//! Run with `cargo run --release -p oso-core --example text_perf`. Shows that
//! per-query costs do not grow with file size or line length. The benchmarks
//! that CI checks against the budgets are in `crates/oso/examples/perf`.

// A benchmark that reports to stdout, not editor code (ADR 0038).
#![expect(clippy::disallowed_macros, reason = "the benchmark prints its timings")]

use std::hint::black_box;
use std::time::{Duration, Instant};

use oso_core::buffer::{Buffer, Replacement};
use oso_core::display_map::{DisplayMap, DisplayPoint, TabWidth};
use oso_core::editor::Editor;
use oso_core::motion::WalkBudget;
use oso_core::rope::{Point, Rope};
use oso_core::selection::{Selection, SelectionSet};
use oso_core::sum_tree::Bias;
use oso_core::word;

const MB: usize = 1024 * 1024;
const QUERIES: usize = 100_000;

/// A small deterministic pseudo-random generator, to avoid a dependency.
struct Lcg(u64);

impl Lcg {
    fn below(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 33) as usize) % bound.max(1)
    }
}

/// About `bytes` of source-like text: mostly ASCII lines with tabs, some
/// Japanese, emoji and CRLF.
fn file_text(bytes: usize) -> String {
    let lines = [
        "fn main() {\n",
        "\tlet value = compute(42); // 計算する\n",
        "\t\tprintln!(\"{value} 👍\");\r\n",
        "}\n",
        "日本語のテキストと English text が混ざった行です。\n",
    ];
    let mut text = String::with_capacity(bytes + 64);
    let mut index = 0;
    while text.len() < bytes {
        text.push_str(lines[index % lines.len()]);
        index += 1;
    }
    text
}

/// About `bytes` of one line, with tabs and wide characters.
fn single_line(bytes: usize) -> String {
    let piece = "abc\tdef 日本語 x\u{301} 👍 ";
    piece.repeat(bytes / piece.len() + 1)
}

fn floor_boundary(text: &str, offset: usize) -> usize {
    let mut offset = offset.min(text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

fn time<R>(f: impl FnOnce() -> R) -> (R, Duration) {
    let start = Instant::now();
    let result = f();
    (result, start.elapsed())
}

fn per_query(total: Duration) -> String {
    format!("{:>7.0} ns/query", total.as_nanos() as f64 / QUERIES as f64)
}

/// p50 and p99 of single-character edits plus the display map sync, while
/// a snapshot is held, so every edit copies its path.
fn edit_costs(name: &str, mut rope: Rope, map: &mut DisplayMap, rng: &mut Lcg) {
    let snapshot = (rope.clone(), map.snapshot());
    let mut samples = Vec::with_capacity(QUERIES);
    for index in 0..QUERIES {
        let offset = rope.clip_offset(rng.below(rope.len()), Bias::Left);
        let start = Instant::now();
        let edit = if index % 2 == 0 {
            rope.insert(offset, "x")
        } else {
            let end = rope.clip_offset(offset + 1, Bias::Right);
            rope.delete(offset..end)
        };
        map.sync(&rope, &edit);
        samples.push(start.elapsed());
    }
    samples.sort();
    println!(
        "{name:<27} p50 {:>7.2?}  p99 {:>7.2?}",
        samples[samples.len() / 2],
        samples[samples.len() * 99 / 100]
    );
    drop(snapshot);
}

/// An editor showing `text`, with `cursors` cursors spread over it.
fn editor_with_cursors(text: &str, cursors: usize) -> Editor {
    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    let stride = text.len() / cursors;
    let parts = (0..cursors)
        .map(|index| Selection::at(floor_boundary(text, index * stride)))
        .collect();
    editor.current_mut().0.selections =
        SelectionSet::from_parts(parts, 0).expect("there is a cursor");
    editor
}

/// One command that types "x" at every cursor, as the dispatcher will run it.
fn type_at_cursors(editor: &mut Editor, join: bool) {
    let id = editor.window().buffer();
    let edits: Vec<_> = editor
        .window()
        .selections
        .iter()
        .map(|selection| Replacement {
            range: selection.cursor..selection.cursor,
            text: "x",
        })
        .collect();
    editor.begin_undo_group();
    if join {
        editor.allow_undo_join();
    }
    editor
        .apply_edits(id, &edits)
        .expect("the buffer is editable");
    editor.end_undo_group();
}

fn percentiles(samples: &mut [Duration]) -> (Duration, Duration) {
    samples.sort();
    (
        samples[samples.len() / 2],
        samples[samples.len() * 99 / 100],
    )
}

/// p50 and p99 of a self-insert at each of `cursors` scattered cursors, and of
/// undoing a command that typed at them, through the [`Editor`].
fn typing_costs(label: &str, text: &str, cursors: usize) {
    let commands = (20_000 / cursors).clamp(200, 10_000);
    let mut editor = editor_with_cursors(text, cursors);
    let mut typed = Vec::with_capacity(commands);
    for _ in 0..commands {
        let start = Instant::now();
        type_at_cursors(&mut editor, true);
        typed.push(start.elapsed());
    }
    let mut editor = editor_with_cursors(text, cursors);
    for _ in 0..commands {
        type_at_cursors(&mut editor, false);
    }
    let mut undone = Vec::with_capacity(commands);
    for _ in 0..commands {
        let start = Instant::now();
        editor.undo().expect("a command to undo");
        undone.push(start.elapsed());
    }
    let (typed_50, typed_99) = percentiles(&mut typed);
    let (undone_50, undone_99) = percentiles(&mut undone);
    println!(
        "{label:<12} {cursors:>5} cursors  type p50 {typed_50:>9.2?} p99 {typed_99:>9.2?}   undo p50 {undone_50:>9.2?} p99 {undone_99:>9.2?}"
    );
}

/// Killing `killed` bytes of `text`, undoing and redoing it, and what freeing
/// the kill's trees costs when the history drops them on the next edit.
fn large_deletion_costs(text: &str, killed: usize) {
    let start = floor_boundary(text, (text.len() - killed) / 2);
    let end = floor_boundary(text, start + killed);
    let label = format!("{} MB of {} MB", (end - start) / MB, text.len() / MB);
    let kill = [Replacement {
        range: start..end,
        text: "",
    }];
    let next = [Replacement {
        range: 0..0,
        text: "x",
    }];

    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    let id = editor.window().buffer();
    let (result, killing) = time(|| editor.apply_edits(id, &kill));
    result.expect("the buffer is editable");
    let (result, evicting) = time(|| editor.apply_edits(id, &next));
    result.expect("the buffer is editable");
    println!("{label:<20} kill {killing:>9.2?}  next edit {evicting:>9.2?}");

    let mut editor = Editor::with_buffer(Buffer::from_text("file", text));
    let id = editor.window().buffer();
    editor
        .apply_edits(id, &kill)
        .expect("the buffer is editable");
    let (result, undo) = time(|| editor.undo());
    result.expect("the kill to undo");
    let (result, redo) = time(|| editor.redo());
    result.expect("the kill to redo");
    let (result, evicting) = time(|| editor.apply_edits(id, &next));
    result.expect("the buffer is editable");
    let (_, dropped) = time(|| drop(editor));
    println!(
        "{label:<20} undo {undo:>9.2?}  redo {redo:>9.2?}  next edit {evicting:>9.2?}  drop the editor {dropped:>9.2?}"
    );
}

/// Pasting `text` into a small buffer, undoing it, then typing, which discards
/// the redo group and frees the paste.
fn undone_paste_costs(text: &str) {
    let mut editor = Editor::with_buffer(Buffer::from_text("file", "start"));
    let id = editor.window().buffer();
    let paste = [Replacement { range: 0..0, text }];
    let (result, pasting) = time(|| editor.apply_edits(id, &paste));
    result.expect("the buffer is editable");
    let (result, undo) = time(|| editor.undo());
    result.expect("the paste to undo");
    let next = [Replacement {
        range: 0..0,
        text: "x",
    }];
    let (result, typing) = time(|| editor.apply_edits(id, &next));
    result.expect("the buffer is editable");
    println!(
        "{:>3} MB  paste {pasting:>9.2?}  undo {undo:>9.2?}  type, dropping the redo {typing:>9.2?}",
        text.len() / MB
    );
}

/// The parts of a large deletion, and what freeing a killed region costs alone.
fn large_deletion_parts(text: &str, killed: usize) {
    let start = floor_boundary(text, (text.len() - killed) / 2);
    let end = floor_boundary(text, start + killed);
    let label = format!("{} MB of {} MB", (end - start) / MB, text.len() / MB);
    let mut rope = Rope::from(text);
    let mut map = DisplayMap::new(&rope, TabWidth::default());
    let (slice, sliced) = time(|| rope.slice(start..end));
    let mut old_rope = rope.clone();
    let mut old_map = map.clone();
    let (edit, deleted) = time(|| rope.delete(start..end));
    let (_, synced) = time(|| map.sync(&rope, &edit));
    // The old display map's items of the region go at once; the rope's stay
    // alive through the slice, until the history drops it.
    let (_, old_map_freed) = time(|| drop(old_map_slot(&mut old_map)));
    let (_, old_rope_freed) = time(|| drop(std::mem::take(&mut old_rope)));
    let (_, slice_freed) = time(|| drop(slice));
    println!(
        "{label:<20} slice {sliced:>8.2?}  delete {deleted:>8.2?}  sync {synced:>8.2?}  free: \
         old display {old_map_freed:>8.2?}  old rope {old_rope_freed:>8.2?}  slice {slice_freed:>8.2?}"
    );
}

/// Lets `std::mem::take` free the old display map as the buffer's swap does.
fn old_map_slot(map: &mut DisplayMap) -> DisplayMap {
    std::mem::replace(map, DisplayMap::new(&Rope::new(), TabWidth::default()))
}

/// The longest and the middle of some keys' worth of word motion from `start`, each
/// way, with a new budget for each key, as commands have.
fn word_costs(label: &str, text: &str) {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    let times = |forward: bool| {
        let mut rng = Lcg(5);
        let mut samples: Vec<Duration> = (0..200)
            .map(|_| {
                let at = floor_boundary(text, rng.below(text.len()));
                let at = display
                    .display_point_to_offset(display.offset_to_display_point(at), Bias::Left);
                let mut budget = WalkBudget::new();
                time(|| {
                    black_box(if forward {
                        word::forward_word(&display, at, &mut budget)
                    } else {
                        word::backward_word(&display, at, &mut budget)
                    })
                })
                .1
            })
            .collect();
        percentiles(&mut samples)
    };
    let (forward_p50, forward_p99) = times(true);
    let (backward_p50, backward_p99) = times(false);
    println!(
        "{label:<28} M-f p50 {forward_p50:>9.2?} p99 {forward_p99:>9.2?}   M-b p50 {backward_p50:>9.2?} p99 {backward_p99:>9.2?}"
    );
}

/// A count that reads all a command may, each way.
fn word_count_costs(label: &str, text: &str) {
    let display = DisplayMap::new(&Rope::from(text), TabWidth::default()).snapshot();
    let (forward, forward_time) = time(|| {
        let mut budget = WalkBudget::new();
        word::move_words(&display, 0, isize::MAX, &mut budget)
    });
    let (backward, backward_time) = time(|| {
        let mut budget = WalkBudget::new();
        word::move_words(&display, text.len(), isize::MIN, &mut budget)
    });
    println!(
        "{label:<28} huge count: forward {forward_time:>9.2?} to {forward}, backward {backward_time:>9.2?} to {backward}"
    );
}

/// What word motion costs, on ordinary text and on runs with no boundary. Run with
/// the argument `words` for this alone.
fn word_motion_costs() {
    println!("== Word motion, one key at a time");
    word_costs("100 MB file", &file_text(100 * MB));
    word_costs("10 MB line", &single_line(10 * MB));
    word_costs("10 MB line of x", &"x".repeat(10 * MB));
    word_costs("10 MB line of 語", &"語".repeat(10 * MB / 3));
    word_costs("10 MB line of spaces", &" ".repeat(10 * MB));
    word_costs("10 MB of empty lines", &"\n".repeat(10 * MB));
    word_costs(
        "10 MB of marks",
        &("e".to_owned() + &"\u{301}".repeat(5 * MB)),
    );
    println!();
    println!("== Word motion with a count past what a command reads");
    word_count_costs("100 MB file", &file_text(100 * MB));
    word_count_costs("10 MB of empty lines", &"\n".repeat(10 * MB));
    word_count_costs("10 MB of `a `", &"a ".repeat(5 * MB));
}

#[expect(
    clippy::too_many_lines,
    reason = "a linear script of timed sections, one after another"
)]
fn main() {
    if std::env::args().any(|arg| arg == "words") {
        word_motion_costs();
        return;
    }
    println!("== Building (100 MB, source-like text)");
    let text = file_text(100 * MB);
    let (rope, built) = time(|| Rope::from(text.as_str()));
    println!("Rope::from                  {built:>10.2?}");

    let (appended, elapsed) = time(|| {
        let mut rope = Rope::new();
        let mut start = 0;
        while start < text.len() {
            let end = floor_boundary(&text, start + 64 * 1024);
            rope.push_str(&text[start..end]);
            start = end;
        }
        rope
    });
    println!(
        "push_str in 64 KB pieces    {elapsed:>10.2?} ({} MB appended)",
        appended.len() / MB
    );
    drop(appended);

    let first = &text[..floor_boundary(&text, 64 * 1024)];
    let (_, elapsed) = time(|| {
        let rope = Rope::from(first);
        let snapshot = DisplayMap::new(&rope, TabWidth::default()).snapshot();
        for row in 0..60 {
            black_box(
                snapshot
                    .clusters(DisplayPoint::new(row, 0))
                    .take(200)
                    .count(),
            );
        }
    });
    println!("first screen (64 KB, 60 rows) {elapsed:>8.2?}");

    let (mut map, elapsed) = time(|| DisplayMap::new(&rope, TabWidth::default()));
    println!("DisplayMap::new (100 MB)    {elapsed:>10.2?}");
    let (_, elapsed) = time(|| map.set_tab_width(&rope, TabWidth::new(4)));
    println!("set_tab_width (100 MB)      {elapsed:>10.2?}");

    let (snapshot, elapsed) = time(|| (rope.clone(), map.snapshot()));
    println!("clone rope + snapshot       {elapsed:>10.2?}");
    drop(snapshot);

    println!();
    println!("== Offset <-> line (random queries)");
    for size in [MB, 100 * MB] {
        let rope = if size == rope.len() {
            rope.clone()
        } else {
            Rope::from(&text[..size])
        };
        let mut rng = Lcg(7);
        let (_, to_point) = time(|| {
            for _ in 0..QUERIES {
                black_box(rope.offset_to_point(rng.below(rope.len())));
            }
        });
        let rows = rope.line_count();
        let (_, to_offset) = time(|| {
            for _ in 0..QUERIES {
                black_box(rope.point_to_offset(Point::new(rng.below(rows), 10)));
            }
        });
        println!(
            "{:>4} MB  offset_to_point {}   point_to_offset {}",
            rope.len() / MB,
            per_query(to_point),
            per_query(to_offset)
        );
    }

    println!();
    println!("== Rendering 60 rows x 200 clusters from random scroll positions");
    let snapshot = map.snapshot();
    let mut rng = Lcg(5);
    let frames = 1000;
    let (_, elapsed) = time(|| {
        for _ in 0..frames {
            let top = rng.below(snapshot.row_count());
            for row in top..top + 60 {
                black_box(
                    snapshot
                        .clusters(DisplayPoint::new(row, 0))
                        .take(200)
                        .count(),
                );
            }
        }
    });
    println!(
        "100 MB file                 {:>7.0} µs/frame",
        elapsed.as_secs_f64() * 1e6 / f64::from(frames)
    );

    println!();
    println!("== Edits: insert or delete one char, then sync the display map");
    edit_costs("100 MB file", rope.clone(), &mut map, &mut Lcg(3));
    let line = single_line(10 * MB);
    let line_rope = Rope::from(line.as_str());
    let mut line_map = DisplayMap::new(&line_rope, TabWidth::new(4));
    edit_costs("10 MB line", line_rope, &mut line_map, &mut Lcg(4));
    let paste = file_text(10 * MB);
    let mut target = rope.clone();
    let (edit, elapsed) = time(|| target.insert(rope.len() / 2, &paste));
    let (_, synced) = time(|| map.sync(&target, &edit));
    println!("10 MB paste into 100 MB     {elapsed:>10.2?} + sync {synced:.2?}");
    drop((target, map));

    println!();
    println!("== Typing through the Editor: self-insert, and undo");
    typing_costs("1 MB file", &text[..floor_boundary(&text, MB)], 1);
    for cursors in [1, 10, 100, 1000] {
        typing_costs("100 MB file", &text, cursors);
    }
    typing_costs("10 MB line", &line, 1);
    typing_costs("10 MB line", &line, 100);

    println!();
    println!("== Large deletions through the Editor");
    for (size, killed) in [(10 * MB, 9 * MB), (100 * MB, 50 * MB), (100 * MB, 90 * MB)] {
        large_deletion_costs(&text[..floor_boundary(&text, size)], killed);
    }
    println!();
    println!("== Undoing a paste, then typing");
    for size in [10 * MB, 90 * MB] {
        undone_paste_costs(&text[..floor_boundary(&text, size)]);
    }
    println!();
    println!("== Large deletions, part by part");
    for (size, killed) in [(10 * MB, 9 * MB), (100 * MB, 90 * MB)] {
        large_deletion_parts(&text[..floor_boundary(&text, size)], killed);
    }

    println!();
    word_motion_costs();

    println!();
    println!("== Adversarial text: building at 1 MB and 10 MB (should scale ~10x)");
    let flag_run = |bytes: usize| "🇯🇵".repeat(bytes / 8);
    let zalgo = |bytes: usize| "e".to_owned() + &"\u{301}".repeat(bytes / 2);
    let cjk = |bytes: usize| "日本語の文章です。".repeat(bytes / 27);
    let tabs = |bytes: usize| "\t".repeat(bytes);
    let inputs: [(&str, &dyn Fn(usize) -> String); 4] = [
        ("flags", &flag_run),
        ("one huge cluster", &zalgo),
        ("CJK only", &cjk),
        ("tabs only", &tabs),
    ];
    for (name, make) in inputs {
        for size in [MB, 10 * MB] {
            let text = make(size);
            let (rope, built) = time(|| Rope::from(text.as_str()));
            let (_, pushed) = time(|| {
                let mut rope = Rope::new();
                let mut start = 0;
                while start < text.len() {
                    let end = floor_boundary(&text, start + 64 * 1024);
                    rope.push_str(&text[start..end]);
                    start = end;
                }
            });
            let (_, mapped) = time(|| DisplayMap::new(&rope, TabWidth::default()));
            println!(
                "{name:>16} {:>3} MB  from {built:>9.2?}  push_str {pushed:>9.2?}  display {mapped:>9.2?}",
                size / MB
            );
        }
    }

    println!();
    println!("== One long line: column mapping (random queries)");
    for size in [MB, 10 * MB] {
        let line = single_line(size);
        let rope = Rope::from(line.as_str());
        let (map, built) = time(|| DisplayMap::new(&rope, TabWidth::new(4)));
        let snapshot = map.snapshot();
        let width = snapshot.row_cells(0);
        let mut rng = Lcg(11);
        let (_, to_display) = time(|| {
            for _ in 0..QUERIES {
                black_box(snapshot.offset_to_display_point(rng.below(rope.len())));
            }
        });
        let (_, to_buffer) = time(|| {
            for _ in 0..QUERIES {
                let point = DisplayPoint::new(0, rng.below(width));
                black_box(snapshot.display_point_to_offset(point, Bias::Left));
            }
        });
        let (_, point) = time(|| {
            for _ in 0..QUERIES {
                black_box(rope.offset_to_point(rng.below(rope.len())));
            }
        });
        let (_, visible) = time(|| {
            for _ in 0..QUERIES / 100 {
                let start = DisplayPoint::new(0, rng.below(width));
                black_box(snapshot.clusters(start).take(200).count());
            }
        });
        println!(
            "{:>4} MB line ({} cells, built in {built:.2?})",
            rope.len() / MB,
            width
        );
        println!("    offset -> display point  {}", per_query(to_display));
        println!("    display point -> offset  {}", per_query(to_buffer));
        println!("    offset -> point          {}", per_query(point));
        println!(
            "    200 visible clusters     {:>7.0} ns/row",
            visible.as_nanos() as f64 / (QUERIES / 100) as f64
        );
    }
}
