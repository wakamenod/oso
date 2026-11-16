//! The search of past conversations' benchmark (ADR 0145, step 2), over a tree of transcripts
//! of its own, 1,000 of 512 KiB each, about 1% of them prompts and replies, the rest tool
//! results, as on the author's machine. `conversation_search_first_rows_ms`: from the key
//! that starts the first search, cold, to its first rows. `conversation_search_cold_ms`: to
//! its end, every transcript read. `conversation_search_warm_ms`: a search's end once the
//! text is held. `conversation_search_key_p99_ms`: key to presented frame while typing into
//! the float. Recorded, with no threshold yet.
//!
//! Must not depend on: anything but the harness's own pacing, from the module above, and its
//! temporary directories, which are removed however the phase ends.

use std::fs::File;
use std::io::{BufWriter, Write as _};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::{Lcg, log_step, paced_keys, settle};
use crate::app::App;
use crate::files::TempDir;
use crate::report::{Report, percentile};

/// How many transcripts the tree holds.
const TRANSCRIPTS: usize = 1_000;

/// About how many bytes each transcript holds.
const TRANSCRIPT_BYTES: usize = 512 * 1024;

/// How many prompts and replies each holds, half of each.
const MESSAGES: usize = 40;

/// The bytes of a tool result, the bulk of a transcript.
const TOOL_RESULT: usize = 8 * 1024;

/// Words the text is made of; `trust` is in some messages only.
const WORDS: [&str; 12] = [
    "worktree", "session", "buffer", "render", "trust", "window", "search", "float", "agent",
    "review", "commit", "space",
];

/// How long a search may take to end.
const WITHIN: Duration = Duration::from_secs(120);

/// How often to look whether rows came.
const POLL: Duration = Duration::from_millis(5);

/// What is typed into the float, once the text is held: each key searches again.
const KEYS: [&str; 6] = ["e", "s", "DEL", "DEL", "x", "DEL"];

/// The searches timed warm in each run.
const WARM: usize = 10;

/// Runs the phase `runs` times, each in a new process over the same tree.
pub(super) fn conversations(
    exe: &Path,
    runs: usize,
    count: usize,
    report: &mut Report,
) -> Result<(), String> {
    log_step("searching 1,000 transcripts of past conversations");
    let dir = TempDir::new("conversations")
        .map_err(|e| format!("cannot create a temporary directory: {e}"))?;
    let claude = dir.file("claude");
    let started = Instant::now();
    write_tree(&claude).map_err(|e| format!("cannot write the transcripts: {e}"))?;
    log_step(&format!(
        "wrote {TRANSCRIPTS} transcripts in {:.1} s",
        started.elapsed().as_secs_f64()
    ));
    let (mut first, mut cold, mut warm, mut keys) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for run in 0..runs {
        let env = [("CLAUDE_CONFIG_DIR", claude.as_path())];
        let mut app = App::start_with_env(exe, None, None, &env)?;
        settle(&mut app)?;
        app.send_keys("C-c a /")?;
        app.send_keys("t r")?;
        let typed = Instant::now();
        let mut first_rows = None;
        loop {
            let completion = completion(&mut app)?;
            if first_rows.is_none() && completion["count"].as_u64() > Some(0) {
                first_rows = Some(typed.elapsed());
            }
            if completion["pending"] == false {
                break;
            }
            if typed.elapsed() > WITHIN {
                return Err(format!("the search did not end within {WITHIN:?}"));
            }
            thread::sleep(POLL);
        }
        cold.push(typed.elapsed().as_secs_f64() * 1000.0);
        let first_rows = first_rows.ok_or("the search found no rows")?;
        first.push(first_rows.as_secs_f64() * 1000.0);
        let mut times = Vec::new();
        for index in 0..WARM {
            app.send_keys(if index % 2 == 0 { "u" } else { "DEL" })?;
            let sent = Instant::now();
            ended(&mut app, sent)?;
            times.push(sent.elapsed().as_secs_f64() * 1000.0);
        }
        warm.push(percentile(&times, 50.0));
        settle(&mut app)?;
        let mut rng = Lcg(run as u64 + 1);
        let samples = paced_keys(&mut app, &KEYS, count & !7, &mut rng)?;
        keys.push(percentile(&samples, 99.0));
        app.send_keys("C-g")?;
        settle(&mut app)?;
        app.stop();
    }
    report.add("conversation_search_first_rows_ms", first);
    report.add("conversation_search_cold_ms", cold);
    report.add("conversation_search_warm_ms", warm);
    report.add("conversation_search_key_p99_ms", keys);
    Ok(())
}

/// What dev state says of the open float's rows.
fn completion(app: &mut App) -> Result<Value, String> {
    let state = app.call("get-state", None)?;
    Ok(state["prompts"][0]["completion"].clone())
}

/// Waits until the search a key started has ended.
fn ended(app: &mut App, sent: Instant) -> Result<(), String> {
    loop {
        if completion(app)?["pending"] == false {
            return Ok(());
        }
        if sent.elapsed() > WITHIN {
            return Err(format!("a warm search did not end within {WITHIN:?}"));
        }
        thread::sleep(POLL);
    }
}

/// Writes the tree under `claude`: `projects/<slug>/<uuid>.jsonl`, and an empty `sessions`.
fn write_tree(claude: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(claude.join("sessions"))?;
    let projects = claude.join("projects");
    let mut rng = Lcg(7);
    for index in 0..TRANSCRIPTS {
        let directory = format!("/work/project-{}/src", index % 50);
        let slug: String = directory
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let folder = projects.join(&slug);
        std::fs::create_dir_all(&folder)?;
        let id = format!("{index:08x}-0000-4000-8000-000000000000");
        let mut out = BufWriter::new(File::create(folder.join(format!("{id}.jsonl")))?);
        let mut written = 0;
        for message in 0..MESSAGES {
            let words = 20 + rng.below(40);
            let text = sentence(&mut rng, words);
            let line = if message % 2 == 0 {
                json!({"type": "user", "cwd": directory, "message": {"role": "user", "content": text}})
            } else {
                json!({"type": "assistant", "message": {"content": [{"type": "text", "text": text}]}})
            };
            written += writeln_counted(&mut out, &line)?;
            let result = sentence(&mut rng, TOOL_RESULT / 7);
            let tool = json!({"type": "user", "message": {"content": [{"type": "tool_result", "content": result}]}});
            while written < TRANSCRIPT_BYTES * (message + 1) / MESSAGES {
                written += writeln_counted(&mut out, &tool)?;
            }
        }
        out.flush()?;
    }
    Ok(())
}

/// Writes `line` and a line break, and gives the bytes written.
fn writeln_counted(out: &mut impl std::io::Write, line: &Value) -> std::io::Result<usize> {
    let text = line.to_string();
    writeln!(out, "{text}")?;
    Ok(text.len() + 1)
}

/// `words` words of [`WORDS`], spaced.
fn sentence(rng: &mut Lcg, words: usize) -> String {
    let mut text = String::with_capacity(words * 7);
    for index in 0..words {
        if index > 0 {
            text.push(' ');
        }
        text.push_str(WORDS[rng.below(WORDS.len())]);
    }
    text
}
