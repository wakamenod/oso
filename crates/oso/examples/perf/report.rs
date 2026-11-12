//! Benchmark results: statistics over samples, and the JSON that CI keeps
//! and compares (ADR 0046).
//!
//! Must not depend on: the app or the control socket; results are plain data.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, json};

/// Bumped when a result can no longer be compared with older ones.
pub const SCHEMA: u64 = 1;

/// The value of the sample at percentile `p` (nearest rank).
///
/// # Panics
///
/// If `samples` is empty: the callers check first.
pub fn percentile(samples: &[f64], p: f64) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = (p / 100.0 * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

/// The median of `samples`: the lower middle one of an even count.
pub fn median(samples: &[f64]) -> f64 {
    percentile(samples, 50.0)
}

/// The largest of `samples`.
pub fn max(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(f64::NEG_INFINITY, f64::max)
}

/// One measured number in milliseconds: the median over repeated runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Metric {
    pub value: f64,
    pub runs: Vec<f64>,
}

/// The GPU a result was measured on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    pub kind: String,
}

impl Gpu {
    /// Whether this is a real GPU, the only place where the absolute latency
    /// budget applies (ADR 0025). GitHub's macOS runners report a
    /// paravirtualized device as an integrated GPU.
    pub fn is_hardware(&self) -> bool {
        matches!(self.kind.as_str(), "integrated" | "discrete")
            && !self.name.contains("Paravirtual")
    }
}

/// The budgets in `docs/dev/roadmap.md`, with "no freeze" as 100 ms. Each key's
/// latency, as a 99th percentile, fails over its budget on a real GPU; the rest are
/// read by hand on the author's Mac (ADR 0025, ADR 0046, ADR 0123).
pub const BUDGETS: [(&str, f64); 27] = [
    ("key_p99_ms", 8.0),
    ("type_p99_ms", 8.0),
    ("del_p99_ms", 8.0),
    ("ime_preedit_p99_ms", 8.0),
    ("prompt_type_p99_ms", 8.0),
    ("mx_type_p99_ms", 8.0),
    ("file_prompt_type_p99_ms", 8.0),
    ("win4_key_p99_ms", 8.0),
    ("win4_type_p99_ms", 8.0),
    ("win16_key_p99_ms", 8.0),
    ("win16_type_p99_ms", 8.0),
    ("win16_other_p99_ms", 8.0),
    ("syntax_type_p99_ms", 8.0),
    ("indent_ret_p99_ms", 8.0),
    ("big_keys_p99_ms", 8.0),
    ("big_type_p99_ms", 8.0),
    ("big_del_p99_ms", 8.0),
    ("line_keys_p99_ms", 8.0),
    ("line_type_p99_ms", 8.0),
    ("line_del_p99_ms", 8.0),
    ("startup_ms", 100.0),
    ("open_first_screen_ms", 50.0),
    ("big_keys_max_ms", 100.0),
    ("line_keys_max_ms", 100.0),
    ("big_type_max_ms", 100.0),
    ("line_type_max_ms", 100.0),
    ("dev_build_ms", 2000.0),
];

/// A baseline to compare with, as far as there is one.
#[derive(Debug, Clone, PartialEq)]
pub enum Baseline {
    None,
    /// From an older or newer harness, whose numbers mean something else.
    /// The next run on `main` sets a new one.
    OtherSchema(Option<u64>),
    Report(Report),
}

impl Baseline {
    /// Parses a baseline file. Only broken files are errors.
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
        let schema = value["schema"].as_u64();
        if schema == Some(SCHEMA) {
            Report::parse(json).map(Self::Report)
        } else {
            Ok(Self::OtherSchema(schema))
        }
    }
}

/// One result file.
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub os: String,
    pub arch: String,
    pub commit: Option<String>,
    pub gpu: Option<Gpu>,
    /// Whether this OS has a text backend. Without one, frames have no
    /// glyphs to draw (ADR 0036).
    pub has_text_backend: bool,
    pub metrics: BTreeMap<String, Metric>,
}

impl Report {
    /// An empty result for this machine.
    pub fn new() -> Self {
        Self {
            os: std::env::consts::OS.to_owned(),
            arch: std::env::consts::ARCH.to_owned(),
            commit: None,
            gpu: None,
            has_text_backend: cfg!(target_os = "macos"),
            metrics: BTreeMap::new(),
        }
    }

    /// Adds a number from repeated runs, as their median.
    pub fn add(&mut self, name: &str, runs: Vec<f64>) {
        if runs.is_empty() {
            return;
        }
        let value = median(&runs);
        self.metrics.insert(name.to_owned(), Metric { value, runs });
    }

    pub fn to_json(&self) -> String {
        let metrics: serde_json::Map<String, Value> = self
            .metrics
            .iter()
            .map(|(name, metric)| {
                let metric = json!({ "value": metric.value, "runs": metric.runs });
                (name.clone(), metric)
            })
            .collect();
        let report = json!({
            "schema": SCHEMA,
            "os": self.os,
            "arch": self.arch,
            "commit": self.commit,
            "gpu": self.gpu.as_ref().map(|gpu| json!({ "name": gpu.name, "kind": gpu.kind })),
            "has_text_backend": self.has_text_backend,
            "metrics": metrics,
        });
        format!("{report:#}\n")
    }

    /// Parses a result. A file from another schema is an error, which the
    /// caller reports, rather than a comparison of unlike numbers.
    pub fn parse(json: &str) -> Result<Self, String> {
        let value: Value = serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
        let schema = value["schema"].as_u64();
        if schema != Some(SCHEMA) {
            return Err(format!("schema {schema:?}, expected {SCHEMA}"));
        }
        let text = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("no string {key}"))
        };
        let gpu = value["gpu"].as_object().map(|gpu| Gpu {
            name: gpu["name"].as_str().unwrap_or_default().to_owned(),
            kind: gpu["kind"].as_str().unwrap_or_default().to_owned(),
        });
        let metrics = value["metrics"]
            .as_object()
            .ok_or("no metrics")?
            .iter()
            .map(|(name, metric)| {
                let value = metric["value"]
                    .as_f64()
                    .ok_or_else(|| format!("{name} has no value"))?;
                let runs = metric["runs"]
                    .as_array()
                    .map(|runs| runs.iter().filter_map(Value::as_f64).collect())
                    .unwrap_or_default();
                Ok((name.clone(), Metric { value, runs }))
            })
            .collect::<Result<_, String>>()?;
        Ok(Self {
            os: text("os")?,
            arch: text("arch")?,
            commit: value["commit"].as_str().map(str::to_owned),
            gpu,
            has_text_backend: value["has_text_backend"].as_bool().unwrap_or(false),
            metrics,
        })
    }

    pub fn read(path: &Path) -> Result<Self, String> {
        let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&json).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The results as a plain text table.
    pub fn table(&self) -> String {
        let gpu = self.gpu.as_ref().map_or("no GPU".to_owned(), |gpu| {
            format!("{} ({})", gpu.name, gpu.kind)
        });
        let mut table = format!("{} {}, {gpu}\n", self.os, self.arch);
        for (name, metric) in &self.metrics {
            table += &format!("{name:<36} {:>10.2} ms", metric.value);
            if let Some((_, budget)) = BUDGETS.iter().find(|(budgeted, _)| budgeted == name) {
                table += &format!("  (budget {budget} ms)");
            }
            table += "\n";
        }
        table
    }
}

#[cfg(test)]
mod tests {
    use super::{Baseline, Gpu, Report, SCHEMA, max, median, percentile};

    #[test]
    fn percentiles_use_the_nearest_rank() {
        let samples: Vec<f64> = (1..=100).map(f64::from).rev().collect();
        assert_eq!(percentile(&samples, 50.0), 50.0);
        assert_eq!(percentile(&samples, 99.0), 99.0);
        assert_eq!(percentile(&samples, 100.0), 100.0);
        assert_eq!(percentile(&[7.0], 99.0), 7.0);
        assert_eq!(percentile(&[1.0, 2.0], 0.0), 1.0);
    }

    #[test]
    fn median_and_max() {
        assert_eq!(median(&[3.0, 1.0, 2.0]), 2.0);
        assert_eq!(median(&[4.0, 1.0, 2.0, 3.0]), 2.0);
        assert_eq!(max(&[3.0, 9.0, 2.0]), 9.0);
    }

    #[test]
    fn only_real_gpus_count_as_hardware() {
        let gpu = |name: &str, kind: &str| Gpu {
            name: name.into(),
            kind: kind.into(),
        };
        assert!(gpu("Some Chip", "integrated").is_hardware());
        assert!(gpu("Some Card", "discrete").is_hardware());
        assert!(!gpu("llvmpipe (LLVM 20)", "cpu").is_hardware());
        assert!(!gpu("Apple Paravirtual device", "integrated").is_hardware());
        assert!(!gpu("Virtio", "virtual").is_hardware());
    }

    #[test]
    fn results_round_trip_through_json() {
        let mut report = Report::new();
        report.commit = Some("abc".into());
        report.gpu = Some(Gpu {
            name: "Test".into(),
            kind: "cpu".into(),
        });
        report.add("a_ms", vec![1.0, 3.0, 2.0]);
        report.add("empty_ms", vec![]);
        let parsed = Report::parse(&report.to_json()).unwrap();
        assert_eq!(parsed, report);
        assert_eq!(parsed.metrics["a_ms"].value, 2.0);
        assert!(!parsed.metrics.contains_key("empty_ms"));
    }

    #[test]
    fn a_baseline_of_another_schema_is_not_an_error() {
        let other = format!(r#"{{"schema": {}}}"#, SCHEMA + 1);
        assert_eq!(
            Baseline::parse(&other).unwrap(),
            Baseline::OtherSchema(Some(SCHEMA + 1))
        );
        assert_eq!(Baseline::parse("{}").unwrap(), Baseline::OtherSchema(None));
        assert!(Baseline::parse("not json").is_err());
        let report = Report::new();
        assert_eq!(
            Baseline::parse(&report.to_json()).unwrap(),
            Baseline::Report(report)
        );
    }

    #[test]
    fn the_table_shows_budgets() {
        let mut report = Report::new();
        report.add("startup_ms", vec![120.0]);
        report.add("key_max_ms", vec![1.0]);
        let table = report.table();
        assert!(table.contains("120.00 ms  (budget 100 ms)"), "{table}");
        let unbudgeted = table.lines().find(|line| line.starts_with("key_max_ms"));
        assert!(!unbudgeted.unwrap().contains("budget"), "{table}");
    }

    #[test]
    fn rejects_other_schemas_and_bad_files() {
        assert!(
            Report::parse("not json")
                .unwrap_err()
                .contains("invalid JSON")
        );
        let other = format!(r#"{{"schema": {}}}"#, SCHEMA + 1);
        assert!(Report::parse(&other).unwrap_err().contains("schema"));
        let no_metrics = format!(r#"{{"schema": {SCHEMA}, "os": "x", "arch": "y"}}"#);
        assert!(Report::parse(&no_metrics).unwrap_err().contains("metrics"));
    }
}
