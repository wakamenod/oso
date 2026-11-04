//! `just build-time`: changes one file of the tip crate and times the
//! incremental dev build, several times (ADR 0024).
//!
//! Must not depend on: the app or the control socket; it only runs cargo and
//! git, and edits that one file.

use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::process;
use crate::report::Report;

/// The file changed, relative to the workspace root.
const PROBE: &str = "crates/oso/src/commands.rs";

/// Builds timed, by default.
pub const RUNS: usize = 5;

/// Puts the file back, whatever happens to the builds.
struct Restore<'a> {
    path: &'a Path,
    text: &'a str,
}

impl Drop for Restore<'_> {
    fn drop(&mut self) {
        if let Err(e) = fs::write(self.path, self.text) {
            // Rare, and visible in `git status`.
            log_failure(self.path, &e);
        }
    }
}

#[expect(clippy::disallowed_macros, reason = "the harness reports to stderr")]
fn log_failure(path: &Path, error: &std::io::Error) {
    eprintln!("perf: cannot restore {}: {error}", path.display());
}

/// `text` with a new item after its `//!` header. This moves every line
/// below it, as a real edit does, so more than the changed item is redone.
fn with_probe(text: &str, n: usize) -> String {
    let header = text
        .split_inclusive('\n')
        .take_while(|line| line.starts_with("//!"))
        .map(str::len)
        .sum::<usize>();
    format!(
        "{}const _: u32 = {n};\n{}",
        &text[..header],
        &text[header..]
    )
}

fn cargo_build() -> Result<(), String> {
    let status = process::command("cargo")
        .args([
            "build",
            "--quiet",
            "--locked",
            "-p",
            "oso",
            "--features",
            "dev",
        ])
        .status()
        .map_err(|e| format!("cannot run cargo: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("cargo build failed: {status}"))
    }
}

/// Fails if the file has changes, which restoring it must not touch.
fn check_clean(path: &str) -> Result<(), String> {
    let output = process::command("git")
        .args(["status", "--porcelain", "--", path])
        .output()
        .map_err(|e| format!("cannot run git: {e}"))?;
    if !output.status.success() {
        return Err(format!("git status failed: {}", output.status));
    }
    if !output.stdout.is_empty() {
        return Err(format!("{path} has uncommitted changes; commit them first"));
    }
    Ok(())
}

/// Times `runs` builds, each after a new change to the probe file.
pub fn run(runs: usize) -> Result<Report, String> {
    if runs == 0 {
        return Err("--runs must be at least 1".into());
    }
    let path = Path::new(PROBE);
    check_clean(PROBE)?;
    let text = fs::read_to_string(path).map_err(|e| format!("{PROBE}: {e}"))?;

    // Everything else is up to date first, so that only the change is timed.
    cargo_build()?;
    let mut times = Vec::with_capacity(runs);
    let _restore = Restore { path, text: &text };
    for n in 0..runs {
        fs::write(path, with_probe(&text, n)).map_err(|e| format!("{PROBE}: {e}"))?;
        let started = Instant::now();
        cargo_build()?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let mut report = Report::new();
    report.add("dev_build_ms", times);
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::with_probe;

    #[test]
    fn the_probe_goes_after_the_header() {
        let text = "//! Header.\n//!\n//! More.\n\nuse a;\n";
        assert_eq!(
            with_probe(text, 3),
            "//! Header.\n//!\n//! More.\nconst _: u32 = 3;\n\nuse a;\n"
        );
    }

    #[test]
    fn the_probe_handles_crlf_checkouts() {
        assert_eq!(
            with_probe("//! H\r\nuse a;\r\n", 1),
            "//! H\r\nconst _: u32 = 1;\nuse a;\r\n"
        );
    }

    #[test]
    fn each_probe_differs() {
        assert_ne!(with_probe("//! H\n", 0), with_probe("//! H\n", 1));
    }
}
