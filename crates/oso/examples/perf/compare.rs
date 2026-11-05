//! Compares a result with the baseline from `main` and decides whether it
//! fails (ADR 0024, ADR 0025, ADR 0046). The thresholds live here.
//!
//! Must not depend on: `app` or `bench`; it works on results alone, so it can
//! be tested with made-up numbers.

use crate::report::{BUDGETS, Baseline, Gpu, Report};

/// When a metric counts as regressed: it must grow by this ratio and by this
/// many milliseconds. The floor keeps noise on very small numbers from
/// failing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Threshold {
    pub ratio: f64,
    pub floor_ms: f64,
}

const fn gate(ratio: f64, floor_ms: f64) -> Threshold {
    Threshold { ratio, floor_ms }
}

/// A slowdown this large is a freeze, wherever it is measured. Worst keys on
/// big files, moving or typing, stayed under 30 ms in every run on the runners.
const FREEZE: Threshold = gate(3.0, 100.0);

/// The metrics that can fail, and their thresholds, on each kind of runner.
/// Anything else is only reported. The code ran 6 times on each runner:
/// a threshold is at least twice the spread seen there, as a ratio over 1 and
/// as a difference (ADR 0046). A typing metric takes the wider of its own spread
/// and that of the motion metric beside it in the same runs, since both draw the
/// same frames (ADR 0056).
const LINUX: [(&str, Threshold); 18] = [
    ("key_p50_ms", gate(3.5, 0.75)),
    ("key_p99_ms", gate(5.5, 2.5)),
    ("type_p99_ms", gate(2.0, 0.75)),
    ("startup_ms", gate(2.25, 55.0)),
    ("open_first_screen_ms", gate(2.25, 4.75)),
    ("open_full_load_ms", gate(3.0, 370.0)),
    ("big_load_ms", gate(3.0, 3750.0)),
    ("big_keys_p99_ms", gate(5.0, 2.5)),
    ("big_keys_max_ms", FREEZE),
    ("big_type_p99_ms", gate(5.5, 3.25)),
    ("big_type_max_ms", FREEZE),
    ("line_load_ms", gate(3.25, 235.0)),
    ("line_keys_p99_ms", gate(5.0, 2.5)),
    ("line_keys_max_ms", FREEZE),
    ("line_type_p99_ms", gate(5.0, 2.5)),
    ("line_type_max_ms", FREEZE),
    ("big_trip_max_ms", FREEZE),
    ("dev_build_ms", gate(2.0, 1400.0)),
];
const MACOS: [(&str, Threshold); 18] = [
    ("key_p50_ms", gate(3.0, 1.5)),
    ("key_p99_ms", gate(6.5, 12.0)),
    ("type_p99_ms", gate(3.0, 4.0)),
    ("startup_ms", gate(4.25, 1030.0)),
    ("open_first_screen_ms", gate(4.75, 44.0)),
    ("open_full_load_ms", gate(4.0, 530.0)),
    ("big_load_ms", gate(3.5, 5100.0)),
    ("big_keys_p99_ms", gate(4.5, 12.0)),
    ("big_keys_max_ms", FREEZE),
    ("big_type_p99_ms", gate(4.5, 12.0)),
    ("big_type_max_ms", FREEZE),
    ("line_load_ms", gate(2.25, 235.0)),
    ("line_keys_p99_ms", gate(6.75, 18.0)),
    ("line_keys_max_ms", FREEZE),
    ("line_type_p99_ms", gate(6.75, 18.0)),
    ("line_type_max_ms", FREEZE),
    ("big_trip_max_ms", FREEZE),
    ("dev_build_ms", gate(4.25, 3500.0)),
];
/// Windows has no control socket yet (ADR 0032), so only the build is timed.
const WINDOWS: [(&str, Threshold); 1] = [("dev_build_ms", gate(2.25, 2775.0))];

/// The one budget CI enforces, on a real GPU only (ADR 0025).
const KEY_P99: &str = "key_p99_ms";

/// One line of the comparison.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub name: String,
    pub base: Option<f64>,
    pub new: Option<f64>,
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
    /// In the baseline, gated, and no longer measured. A check that stops
    /// measuring must not look like a check that passes.
    Missing,
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
        self.rows.iter().any(|row| {
            matches!(
                row.status,
                Status::Regressed | Status::OverBudget | Status::Missing
            )
        })
    }

    /// The comparison as Markdown, for the job summary.
    pub fn markdown(&self, new: &Report) -> String {
        let gpu = new.gpu.as_ref().map_or("no GPU".to_owned(), describe);
        let text = if new.has_text_backend {
            ""
        } else {
            ", no text backend"
        };
        let mut out = format!("### Performance: {} {}, {gpu}{text}\n\n", new.os, new.arch);
        for note in &self.notes {
            out += &format!("> {note}\n\n");
        }
        out += "| Metric | Baseline (ms) | This run (ms) | Change | Status |\n";
        out += "|---|--:|--:|--:|---|\n";
        for row in &self.rows {
            let base = row.base.map_or("-".to_owned(), |base| format!("{base:.2}"));
            let value = row.new.map_or("-".to_owned(), |new| format!("{new:.2}"));
            let change = row
                .base
                .zip(row.new)
                .filter(|(base, _)| *base > 0.0)
                .map_or("-".to_owned(), |(base, new)| {
                    format!("{:+.0}%", (new / base - 1.0) * 100.0)
                });
            let status = match row.status {
                Status::Ok => "ok",
                Status::Info => "",
                Status::Regressed => "**regressed**",
                Status::OverBudget => "**over budget**",
                Status::Missing => "**not measured**",
            };
            out += &format!(
                "| {} | {base} | {value} | {change} | {status} |
",
                row.name
            );
        }
        out
    }
}

fn describe(gpu: &Gpu) -> String {
    format!("{} ({})", gpu.name, gpu.kind)
}

fn threshold(os: &str, name: &str) -> Option<Threshold> {
    let gates: &[(&str, Threshold)] = match os {
        "linux" => &LINUX,
        "macos" => &MACOS,
        _ => &WINDOWS,
    };
    gates
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
    if base.has_text_backend != new.has_text_backend {
        return Some("The baseline differs in drawing text: not compared.".to_owned());
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

/// The report to compare with, and a note on it.
fn usable<'a>(base: &'a Baseline, new: &Report) -> (Option<&'a Report>, String) {
    match base {
        Baseline::None => (
            None,
            "No baseline from `main` yet: nothing to compare with.".to_owned(),
        ),
        Baseline::OtherSchema(schema) => (
            None,
            format!("The baseline has schema {schema:?}: not compared."),
        ),
        Baseline::Report(base) => match mismatch(base, new) {
            Some(note) => (None, note),
            None => {
                let commit = base.commit.as_deref().unwrap_or("an unknown commit");
                (Some(base), format!("Baseline: `main` at {commit}."))
            }
        },
    }
}

/// Compares `new` with `base`. With no usable baseline nothing can regress,
/// but the absolute budget still applies.
pub fn compare(base: &Baseline, new: &Report) -> Comparison {
    let (usable, note) = usable(base, new);
    let mut notes = vec![note];
    let hardware = new.gpu.as_ref().is_some_and(Gpu::is_hardware);
    let budget = BUDGETS
        .iter()
        .find(|(name, _)| *name == KEY_P99)
        .map_or(f64::INFINITY, |(_, budget)| *budget);
    if !hardware && new.metrics.contains_key(KEY_P99) {
        notes.push(format!(
            "No hardware GPU: the {budget} ms key latency budget is not checked here."
        ));
    }
    let mut rows: Vec<Row> = new
        .metrics
        .iter()
        .map(|(name, metric)| {
            let old = usable
                .and_then(|base| base.metrics.get(name))
                .map(|m| m.value);
            let gate = threshold(&new.os, name);
            let status = if name == KEY_P99 && hardware && metric.value > budget {
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
                new: Some(metric.value),
                status,
            }
        })
        .collect();
    let gone = usable
        .iter()
        .flat_map(|base| &base.metrics)
        .filter(|(name, _)| threshold(&new.os, name).is_some() && !new.metrics.contains_key(*name));
    rows.extend(gone.map(|(name, metric)| Row {
        name: name.clone(),
        base: Some(metric.value),
        new: None,
        status: Status::Missing,
    }));
    Comparison { rows, notes }
}

#[cfg(test)]
mod tests {
    use super::{Status, Threshold, compare, regressed};
    use crate::report::{Baseline, Gpu, Report};

    fn report(metrics: &[(&str, f64)], gpu: (&str, &str)) -> Report {
        let mut report = Report::new();
        report.os = "linux".into();
        report.gpu = Some(Gpu {
            name: gpu.0.into(),
            kind: gpu.1.into(),
        });
        for (name, value) in metrics {
            report.add(name, vec![*value]);
        }
        report
    }

    const LAVAPIPE: (&str, &str) = ("llvmpipe", "cpu");
    const MAC: (&str, &str) = ("Some Chip", "integrated");

    fn status(base: &Report, new: &Report, name: &str) -> Status {
        let comparison = compare(&Baseline::Report(base.clone()), new);
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
        let same = report(&[("key_p99_ms", 50.0)], LAVAPIPE);
        assert_eq!(status(&base, &same, "key_p99_ms"), Status::Ok);
        assert!(!compare(&Baseline::Report(base.clone()), &same).failed());
        let slow = report(&[("key_p99_ms", 56.0)], LAVAPIPE);
        assert_eq!(status(&base, &slow, "key_p99_ms"), Status::Regressed);
        assert!(compare(&Baseline::Report(base.clone()), &slow).failed());
    }

    #[test]
    fn each_runner_has_its_own_thresholds() {
        let mac = |value| {
            let mut report = report(
                &[("key_p99_ms", value)],
                ("Apple Paravirtual device", "integrated"),
            );
            report.os = "macos".into();
            report
        };
        assert!(!compare(&Baseline::Report(mac(2.0)), &mac(10.0)).failed());
        assert!(compare(&Baseline::Report(mac(2.0)), &mac(20.0)).failed());
        let linux = |value| report(&[("key_p99_ms", value)], LAVAPIPE);
        assert!(compare(&Baseline::Report(linux(2.0)), &linux(12.0)).failed());
    }

    #[test]
    fn the_runners_gate_the_same_metrics() {
        let names = |gates: &[(&'static str, Threshold)]| -> Vec<&'static str> {
            gates.iter().map(|(name, _)| *name).collect()
        };
        assert_eq!(names(&super::LINUX), names(&super::MACOS));
        assert_eq!(names(&super::WINDOWS), ["dev_build_ms"]);
    }

    #[test]
    fn metrics_without_a_threshold_never_fail() {
        let base = report(&[("key_max_ms", 1.0)], LAVAPIPE);
        let new = report(&[("key_max_ms", 500.0)], LAVAPIPE);
        assert_eq!(status(&base, &new, "key_max_ms"), Status::Info);
        assert!(!compare(&Baseline::Report(base.clone()), &new).failed());
    }

    #[test]
    fn no_baseline_passes_with_a_note() {
        let new = report(&[("key_p99_ms", 100.0)], LAVAPIPE);
        let comparison = compare(&Baseline::None, &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("No baseline"));
        assert!(comparison.markdown(&new).contains("No baseline"));
    }

    #[test]
    fn a_baseline_of_another_schema_passes_with_a_note() {
        let new = report(&[("key_p99_ms", 100.0)], LAVAPIPE);
        let comparison = compare(&Baseline::OtherSchema(Some(9)), &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("schema"));
    }

    #[test]
    fn a_gated_metric_that_stops_being_measured_fails() {
        let base = report(&[("key_p99_ms", 1.0), ("key_max_ms", 1.0)], LAVAPIPE);
        let new = report(&[], LAVAPIPE);
        let comparison = compare(&Baseline::Report(base), &new);
        assert!(comparison.failed());
        assert_eq!(comparison.rows.len(), 1, "only gated metrics");
        assert_eq!(comparison.rows[0].status, Status::Missing);
        assert!(comparison.markdown(&new).contains("**not measured**"));
    }

    #[test]
    fn a_change_in_text_drawing_is_not_compared() {
        let base = report(&[("key_p99_ms", 1.0)], LAVAPIPE);
        let mut new = report(&[("key_p99_ms", 50.0)], LAVAPIPE);
        new.has_text_backend = !base.has_text_backend;
        let comparison = compare(&Baseline::Report(base), &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("not compared"));
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
        assert!(compare(&Baseline::None, &over(MAC)).failed());
        let software = compare(&Baseline::None, &over(LAVAPIPE));
        assert!(!software.failed());
        assert!(
            software
                .notes
                .iter()
                .any(|note| note.contains("No hardware GPU"))
        );
        let virtual_mac = over(("Apple Paravirtual device", "integrated"));
        assert!(!compare(&Baseline::None, &virtual_mac).failed());
        assert!(!compare(&Baseline::None, &report(&[("key_p99_ms", 8.0)], MAC)).failed());
    }

    #[test]
    fn another_machine_type_is_not_compared() {
        let base = report(&[("key_p99_ms", 1.0)], ("Old GPU", "cpu"));
        let new = report(&[("key_p99_ms", 50.0)], LAVAPIPE);
        let comparison = compare(&Baseline::Report(base.clone()), &new);
        assert!(!comparison.failed());
        assert!(comparison.notes[0].contains("not compared"));
        assert_eq!(comparison.rows[0].base, None);

        let mut other_os = base.clone();
        other_os.os = "plan9".into();
        let comparison = compare(&Baseline::Report(other_os.clone()), &new);
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
        let markdown = compare(&Baseline::Report(base.clone()), &new).markdown(&new);
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
