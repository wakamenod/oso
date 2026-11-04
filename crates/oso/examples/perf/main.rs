//! The benchmark harness (ADR 0025, ADR 0044): starts the app, drives it over
//! the control socket, reads back its timings and compares the results with a
//! baseline. Run it through `just perf`, `just perf-compare` and
//! `just build-time`.
//!
//! Must not depend on: winit or the renderer; it sees the app only from
//! outside, through its process and the control socket.

// A tool that reports to stdout and stderr, not editor code (ADR 0038).
#![expect(clippy::disallowed_macros, reason = "the harness prints its results")]

mod app;
mod bench;
mod build_time;
mod compare;
mod files;
mod process;
mod report;

use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
usage:
  perf run --oso PATH [--out FILE] [--quick]
  perf compare BASE NEW [--summary FILE]
  perf build-time [--runs N] [--out FILE]

run         benchmarks the app at PATH, an optimized build with the dev feature
compare     compares two results; a missing BASE passes with a note
build-time  times the incremental dev build of the tip crate, from the
            workspace root; refuses to run if the file it edits has changes";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("run") => run(&args[1..]),
        Some("compare") => compare_results(&args[1..]),
        Some("build-time") => time_build(&args[1..]),
        _ => Err(USAGE.to_owned()),
    };
    match result {
        Ok(code) => code,
        Err(message) => {
            eprintln!("perf: {message}");
            ExitCode::from(2)
        }
    }
}

/// The value after `name` in `args`.
fn option<'a>(args: &'a [String], name: &str) -> Result<Option<&'a str>, String> {
    let Some(index) = args.iter().position(|arg| arg == name) else {
        return Ok(None);
    };
    let value = args.get(index + 1);
    value
        .map(|value| Some(value.as_str()))
        .ok_or_else(|| format!("{name} needs a value"))
}

fn write_out(report: &report::Report, out: Option<&str>) -> Result<(), String> {
    println!("{}", report.table());
    if let Some(out) = out {
        let out = Path::new(out);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(out, report.to_json()).map_err(|e| format!("{}: {e}", out.display()))?;
        println!("wrote {}", out.display());
    }
    Ok(())
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let exe = option(args, "--oso")?.ok_or("run needs --oso PATH")?;
    let exe = std::path::absolute(exe).map_err(|e| format!("{exe}: {e}"))?;
    let scale = if args.iter().any(|arg| arg == "--quick") {
        bench::Scale::QUICK
    } else {
        bench::Scale::FULL
    };
    let report = bench::run_all(&exe, &scale)?;
    write_out(&report, option(args, "--out")?)?;
    Ok(ExitCode::SUCCESS)
}

fn compare_results(args: &[String]) -> Result<ExitCode, String> {
    let (Some(base), Some(new)) = (args.first(), args.get(1)) else {
        return Err("compare needs BASE and NEW".into());
    };
    let new = report::Report::read(Path::new(new))?;
    let base_path = PathBuf::from(base);
    let base = if base_path.exists() {
        Some(report::Report::read(&base_path)?)
    } else {
        None
    };
    let comparison = compare::compare(base.as_ref(), &new);
    let markdown = comparison.markdown(&new);
    println!("{markdown}");
    if let Some(summary) = option(args, "--summary")? {
        std::fs::write(summary, &markdown).map_err(|e| format!("{summary}: {e}"))?;
    }
    Ok(if comparison.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

fn time_build(args: &[String]) -> Result<ExitCode, String> {
    let runs = match option(args, "--runs")? {
        Some(runs) => runs.parse().map_err(|_| format!("not a number: {runs}"))?,
        None => build_time::RUNS,
    };
    let report = build_time::run(runs)?;
    write_out(&report, option(args, "--out")?)?;
    Ok(ExitCode::SUCCESS)
}
