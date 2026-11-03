//! Informal timings for the rope and the display model (M1-4).
//!
//! Run with `cargo run --release -p oso-core --example text_perf`. Shows that
//! per-query costs do not grow with file size or line length. CI benchmarks
//! against the budgets arrive in M1-8.

use std::hint::black_box;
use std::time::{Duration, Instant};

use oso_core::display_map::{DisplayMap, DisplayPoint, TabWidth};
use oso_core::rope::{Point, Rope};
use oso_core::sum_tree::Bias;

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

fn main() {
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
