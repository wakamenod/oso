//! Compares a result with the baseline from `main` and decides whether it
//! fails (ADR 0024, ADR 0025, ADR 0044). The thresholds live here.
//!
//! Must not depend on: `app` or `bench`; it works on results alone, so it can
//! be tested with made-up numbers.

use crate::report::{Gpu, Report};

/// When a metric counts as regressed: it must grow by this ratio and by this
/// many milliseconds. The floor keeps noise on very small numbers from
/// failing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threshold {
    pub ratio: f64,
    pub floor_ms: f64,
}

/// The metrics that can fail, and their thresholds. Anything else is only
/// reported: single worst samples are too noisy to judge on.
const GATED: [(&str, Threshold); 8] = [
    (
        "key_p50_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 1.0,
        },
    ),
    (
        "key_p99_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 2.0,
        },
    ),
    (
        "startup_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 50.0,
        },
    ),
    (
        "open_first_screen_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 20.0,
        },
    ),
    (
        "open_full_load_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 200.0,
        },
    ),
    (
        "big_load_ms",
        Threshold {
            ratio: 1.5,
            floor_ms: 1000.0,
        },
    ),
    (
        "big_keys_p99_ms",
        Threshold {
            ratio: 2.0,
            floor_ms: 5.0,
        },
    ),
    (
        "dev_build_ms",
        Threshold {
            ratio: 2.0,
            floor_ms: 5000.0,
        },
    ),
];

/// The p99 key latency budget, on a real GPU only (ADR 0025).
const KEY_P99_BUDGET_MS: f64 = 8.0;

/// One line of the comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub base: Option<f64>,
    pub new: f64,
    pub status: Status,
}

/// How a metric fared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Ok,
    /// Reported, never fails.
    Info,
    Regressed,
    OverBudget,
}

/// The outcome of a comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Comparison {
    pub rows: Vec<Row>,
    /// Why some or all of it could not be compared.
    pub notes: Vec<String>,
}

impl Comparison {
    pub fn failed(&self) -> bool {
        self.rows
            .iter()
            .any(|row| matches!(row.status, Status::Regressed | Status::OverBudget))
    }

    /// The comparison as Markdown, for the job summary.
    pub fn markdown(&self, new: &Report) -> String {
        let gpu = new.gpu.as_ref().map_or("no GPU".to_owned(), describe);
        let text = if new.text { "" } else { ", no text backend" };
        let mut out = format!("### Performance: {} {}, {gpu}{text}\n\n", new.os, new.arch);
        for note in &self.notes {
            out += &format!("> {note}\n\n");
        }
        out += "| Metric | Baseline (ms) | This run (ms) | Change | Status |\n";
        out += "|---|--:|--:|--:|---|\n";
        for row in &self.rows {
            let base = row.base.map_or("-".to_owned(), |base| format!("{base:.2}"));
            let change = row
                .base
                .filter(|base| *base > 0.0)
                .map_or("-".to_owned(), |base| {
                    format!("{:+.0}%", (row.new / base - 1.0) * 100.0)
                });
            let status = match row.status {
                Status::Ok => "ok",
                Status::Info => "",
                Status::Regressed => "**regressed**",
                Status::OverBudget => "**over budget**",
            };
            out += &format!(
                "| {} | {base} | {:.2} | {change} | {status} |\n",
                row.name, row.new
            );
        }
        out
    }
}

fn describe(gpu: &Gpu) -> String {
    format!("{} ({})", gpu.name, gpu.kind)
}

fn threshold(name: &str) -> Option<Threshold> {
    GATED
        .iter()
        .find(|(gated, _)| *gated == name)
        .map(|(_, threshold)| *threshold)
}

fn regressed(threshold: Threshold, base: f64, new: f64) -> bool {
    new > base * threshold.ratio && new - base > threshold.floor_ms
}

/// Why `base` cannot be compared with `new`, if so. Numbers from another
/// machine type say nothing about the code: the next run on `main` sets a
/// new baseline.
fn mismatch(base: &Report, new: &Report) -> Option<String> {
    if (base.os.as_str(), base.arch.as_str()) != (new.os.as_str(), new.arch.as_str()) {
        return Some(format!(
            "The baseline is from {} {}, not {} {}: not compared.",
            base.os, base.arch, new.os, new.arch
        ));
    }
    if let (Some(base), Some(new)) = (&base.gpu, &new.gpu)
        && base != new
    {
        return Some(format!(
            "The baseline ran on {}, this run on {}: not compared.",
            describe(base),
            describe(new)
        ));
    }
    None
}

/// Compares `new` with `base`. With no usable baseline nothing can regress,
/// but the absolute budget still applies.
pub fn compare(base: Option<&Report>, new: &Report) -> Comparison {
    let mut notes = Vec::new();
    let usable = match base {
        None => {
            notes.push("No baseline from `main` yet: nothing to compare with.".to_owned());
            None
        }
        Some(base) => match mismatch(base, new) {
            Some(note) => {
                notes.push(note);
                None
            }
            None => {
                if let Some(commit) = &base.commit {
                    notes.push(format!("Baseline: `main` at {commit}."));
                }
                Some(base)
            }
        },
    };
    let hardware = new.gpu.as_ref().is_some_and(Gpu::is_hardware);
    if !hardware && new.metrics.contains_key("key_p99_ms") {
        notes.push(format!(
            "No hardware GPU: the {KEY_P99_BUDGET_MS} ms budget is not checked here."
        ));
    }
    let rows = new
        .metrics
        .iter()
        .map(|(name, metric)| {
            let old = usable
                .and_then(|base| base.metrics.get(name))
                .map(|m| m.value);
            let gate = threshold(name);
            let status = if name == "key_p99_ms" && hardware && metric.value > KEY_P99_BUDGET_MS {
                Status::OverBudget
            } else if let (Some(gate), Some(old)) = (gate, old)
                && regressed(gate, old, metric.value)
            {
                Status::Regressed
            } else if gate.is_some() {
                Status::Ok
            } else {
                Status::Info
            };
            Row {
                name: name.clone(),
                base: old,
                new: metric.value,
                status,
            }
        })
        .collect();
    Comparison { rows, notes }
}

#[cfg(test)]
mod tests {
    use super::{Status, Threshold, compare, regressed};
    use crate::report::{Gpu, Report};

    fn report(metrics: &[(&str, f64)], gpu: (&str, &str)) -> Report {
        let mut report = Report::new();
        report.gpu = Some(Gpu {
            name: gpu.0.into(),
            kind: gpu.1.into(),
        });
        for (name, value) in metrics {
            report.add(name, vec![*value]);
        }
        report
    }

    const LAVAPIPE: (&str, &str) = ("llvmpipe", "Cpu");
    const MAC: (&str, &str) = ("Apple M5 Pro", "IntegratedGpu");

    fn status(base: &Report, new: &Report, name: &str) -> Status {
        let comparison = compare(Some(base), new);
        let row = comparison.rows.iter().find(|row| row.name == name);
        row.unwrap().status
    }

    #[test]
    fn a_regression_needs_both_the_ratio_and_the_floor() {
        let gate = Threshold {
            ratio: 1.5,
            floor_ms: 10.0,
        };
        assert!(regressed(gate, 100.0, 151.0));
        assert!(!regressed(gate, 100.0, 150.0), "exactly the ratio");
        assert!(!regressed(gate, 1.0, 10.0), "a big ratio under the floor");
        assert!(!regressed(gate, 100.0, 105.0));
        assert!(regressed(gate, 0.0, 11.0));
    }

    #[test]
    fn a_large_regression_fails_and_noise_passes() {
        let base = report(&[("key_p99_ms", 10.0)], LAVAPIPE);
        let same = report(&[("key_p99_ms", 12.0)], LAVAPIPE);
        assert_eq!(status(&base, &same, "key_p99_ms"), Status::Ok);
        assert!(!compare(Some(&base), &same).failed());
        let slow = report(&[("key_p99_ms", 30.0)], LAVAPIPE);
        assert_eq!(status(&base, &slow, "key_p99_ms"), Status::Regressed);
        assert!(compare(Some(&base), &slow).failed());
    }

    #[test]
    fn metrics_without_a_threshold_never_fail() {
        let base = report(&[("key_max_ms", 1.0)], LAVAPIPE);
        let new = report(&[("key_max_ms", 500.0)], LAVAPIPE);
        assert_eq!(status(&base, &new, "key_max_ms"), Status::Info);
        assert!(!compare(Some(&base), &new).failed());
    }

    #[test]
    fn no_baseline_passes_with_a_note() {
        let new = report(&[("key_p99_ms", 100.0)], LAVAPIPE);
        let comparison = compare(None, &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("No baseline"));
        assert!(comparison.markdown(&new).contains("No baseline"));
    }

    #[test]
    fn a_new_metric_passes() {
        let base = report(&[], LAVAPIPE);
        let new = report(&[("key_p99_ms", 5.0)], LAVAPIPE);
        assert_eq!(status(&base, &new, "key_p99_ms"), Status::Ok);
    }

    #[test]
    fn the_budget_applies_only_on_a_real_gpu() {
        let over = |gpu| report(&[("key_p99_ms", 9.0)], gpu);
        assert!(compare(None, &over(MAC)).failed());
        let software = compare(None, &over(LAVAPIPE));
        assert!(!software.failed());
        assert!(
            software
                .notes
                .iter()
                .any(|note| note.contains("No hardware GPU"))
        );
        let virtual_mac = over(("Apple Paravirtual device", "IntegratedGpu"));
        assert!(!compare(None, &virtual_mac).failed());
        assert!(!compare(None, &report(&[("key_p99_ms", 8.0)], MAC)).failed());
    }

    #[test]
    fn another_machine_type_is_not_compared() {
        let base = report(&[("key_p99_ms", 1.0)], ("Old GPU", "Cpu"));
        let new = report(&[("key_p99_ms", 50.0)], LAVAPIPE);
        let comparison = compare(Some(&base), &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("not compared"));
        assert_eq!(comparison.rows[0].base, None);

        let mut other_os = base.clone();
        other_os.os = "plan9".into();
        let comparison = compare(Some(&other_os), &new);
        assert!(comparison.notes[0].contains("plan9"));
        assert!(!comparison.failed());
    }

    #[test]
    fn the_summary_shows_each_status() {
        let base = report(&[("key_p50_ms", 1.0), ("key_p99_ms", 2.0)], MAC);
        let new = report(
            &[
                ("key_p50_ms", 9.0),
                ("key_p99_ms", 9.0),
                ("key_max_ms", 1.0),
            ],
            MAC,
        );
        let markdown = compare(Some(&base), &new).markdown(&new);
        assert!(
            markdown.contains("| key_p50_ms | 1.00 | 9.00 | +800% | **regressed** |"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| key_p99_ms | 2.00 | 9.00 | +350% | **over budget** |"),
            "{markdown}"
        );
        assert!(
            markdown.contains("| key_max_ms | - | 1.00 | - |  |"),
            "{markdown}"
        );
    }
}
