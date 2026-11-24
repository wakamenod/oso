//! What a request's parts cost, printed to be read: run by hand, in release
//! (`cargo test -p oso --release sessions::native::request::measure_tests -- --ignored --nocapture`).
//!
//! Must not depend on: the app; the tests give what the call says.

use serde_json::json;

use super::line::{Says, line};
use super::reading::{READ_AFTER, Reading, Row, View};
use super::tests::block;
use super::*;

/// The middle and the 98th hundredth of 200 runs of `run`.
fn times(mut run: impl FnMut()) -> String {
    let mut took: Vec<Duration> = (0..200)
        .map(|_| {
            let started = Instant::now();
            run();
            started.elapsed()
        })
        .collect();
    took.sort_unstable();
    format!("p50 {:?}, p98 {:?}", took[100], took[196])
}

/// What indents to over a hundred times its length.
fn deep() -> Value {
    let mut value = json!(vec![1; 143]);
    for _ in 0..120 {
        value = json!([value]);
    }
    value
}

fn view(block: &Block, from: usize) -> View {
    let mut start = 0;
    let lines = block.body().text.split_inclusive('\n');
    let rows = lines.enumerate().map(|(at, line)| {
        let bytes = start..start + line.len();
        start = bytes.end;
        (at, bytes)
    });
    View {
        window: None,
        columns: 100,
        wraps: true,
        attended: true,
        rows: (rows.skip(from).take(62))
            .map(|(at, bytes)| Row {
                start: bytes.start,
                end: bytes.end,
                at,
                cells: 40,
            })
            .collect(),
    }
}

/// Calls of each size and some made to cost much: what each is, its tool and its input.
fn calls() -> Vec<(&'static str, &'static str, Value)> {
    let code = "    let target = redirect(&req).location();\n";
    let old: String = (0..1200).map(|n| format!("{n}{}", &code[..20])).collect();
    vec![
        (
            "8 lines",
            "Bash",
            json!({"command": "cargo test\n".repeat(7), "timeout": 1}),
        ),
        ("200 lines", "Bash", json!({"command": code.repeat(199)})),
        (
            "2000 lines",
            "Bash",
            json!({"command": "let target = 1;\n".repeat(1999)}),
        ),
        (
            "one line of 64 KiB",
            "Bash",
            json!({"command": "x".repeat((64 << 10) - 20)}),
        ),
        (
            "marks",
            "Bash",
            json!({"command": "\u{301}\n".repeat(12 << 10)}),
        ),
        ("a name", "X", json!({"\u{202e}".repeat(20 << 10): 1})),
        ("nested", "X", json!({"x": vec![json!([[[[1]]]]); 5000]})),
        (
            "nested fields",
            "X",
            (0..120).map(|at| (format!("a{at}"), deep())).collect(),
        ),
        ("a name of DEL", "X", json!({"\u{7f}".repeat(65_000): 1})),
        ("DEL", "Bash", json!({"command": "\u{7f}".repeat(65_000)})),
        (
            "small edits",
            "MultiEdit",
            json!({"edits": vec![json!({"old_string": "", "new_string": ""}); 1500]}),
        ),
        (
            "edit of 64 KiB",
            "Edit",
            json!({"file_path": "a", "old_string": old, "new_string": code.repeat(200)}),
        ),
    ]
}

#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(clippy::disallowed_macros, reason = "the timing is printed to be read")]
fn what_a_block_costs() {
    for (what, tool, input) in &calls() {
        let made = times(|| drop(block(tool, input)));
        let shown = block(tool, input);
        let body = shown.body();
        eprintln!(
            "{what}: {} bytes, {} lines, made in {made}",
            body.text.len(),
            body.text.lines().count()
        );
        eprintln!("  runs {}", times(|| drop(body.runs())));
        eprintln!("  title {}", times(|| drop(shown.title((1, 2), 100))));
        let mut reading = Reading::new(&shown);
        eprintln!("  reading made in {}", times(|| drop(Reading::new(&shown))));
        // Every other page, so that what was read is many ranges.
        let now = Instant::now();
        for (at, from) in (0..2000).step_by(2).enumerate() {
            let mut view = view(&shown, from);
            view.rows.truncate(1);
            let at = now + READ_AFTER * u32::try_from(at).unwrap();
            reading.look(&view, true, at);
            reading.look(&view, false, at + READ_AFTER);
        }
        let (a, b) = (view(&shown, 0), view(&shown, 1));
        let mut at = now + READ_AFTER * 4000;
        eprintln!(
            "  a look at another view {}",
            times(|| {
                reading.look(&a, true, at);
                reading.look(&b, true, at);
            })
        );
        eprintln!(
            "  a look that reads {}",
            times(|| {
                let mut reading = reading.clone();
                reading.look(&b, true, at);
                at += READ_AFTER;
                reading.look(&b, false, at);
            })
        );
        for width in [20, 60, 100, 120] {
            let says = Says {
                here: true,
                answers: true,
            };
            let made = times(|| drop(line(&shown, &reading, says, width)));
            eprintln!("  line, {width} cells {made}");
        }
    }
}

#[test]
#[ignore = "a timing to read, run by hand in release"]
#[expect(clippy::disallowed_macros, reason = "the timing is printed to be read")]
fn what_a_crafted_change_costs() {
    let text = |lines: usize, of: fn(usize) -> String| -> String { (0..lines).map(of).collect() };
    let cases = [
        (
            "apart",
            text(4500, |n| format!("{n:x}\n")),
            text(4500, |n| format!("{:x}\n", n * 7 + 1)),
        ),
        (
            "two letters",
            text(10_000, |n| ["a\n", "b\n"][n * n % 7 % 2].into()),
            text(10_000, |n| ["b\n", "a\n"][n * n % 11 % 2].into()),
        ),
        (
            "backwards",
            text(5000, |n| format!("{n}\n")),
            text(5000, |n| format!("{}\n", 5000 - n)),
        ),
        ("empty lines", "\n".repeat(16_000), "a\n".repeat(10_000)),
        (
            "periods",
            ("a\n".repeat(20) + "x\n").repeat(480),
            ("a\n".repeat(19) + "y\na\n").repeat(480),
        ),
        (
            "every other",
            text(8000, |n| format!("{}\n", n % 10)),
            text(8000, |n| format!("{}\n", n % 2 * n % 10)),
        ),
    ];
    for (what, old, new) in &cases {
        let input = json!({"file_path": "a", "old_string": old, "new_string": new});
        for budget in [
            Duration::from_millis(5),
            COMPARE_WITHIN,
            COMPARE_WITHIN * 1000,
        ] {
            let mut worst = Duration::ZERO;
            for _ in 0..5 {
                let started = Instant::now();
                let deadline = started + budget;
                let block = Block::call("Edit", Some(&input), deadline);
                worst = worst.max(started.elapsed());
                assert_ne!(block.hidden(), Hidden::All, "{what}");
            }
            eprintln!("{what}: within {budget:?}, {worst:?}");
        }
    }
}
