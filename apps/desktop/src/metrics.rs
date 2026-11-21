//! Benchmark output: `key = value` lines, printed and written to the `--metrics` file.

use std::path::Path;

#[derive(Debug, Default)]
pub struct Metrics {
    lines: Vec<String>,
}

impl Metrics {
    pub fn log(&mut self, key: &str, value: impl std::fmt::Display) {
        let line = format!("{key} = {value}");
        println!("{line}");
        self.lines.push(line);
    }

    pub fn ms(&mut self, key: &str, ms: f64) {
        self.log(key, format!("{ms:.1}"));
    }

    pub fn write(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.lines.join("\n") + "\n")
    }
}

/// `n`, min, median, p95 and max of a sample, in milliseconds.
pub fn summarize(samples: &[f64]) -> String {
    if samples.is_empty() {
        return "no samples".into();
    }
    let mut v = samples.to_vec();
    v.sort_by(f64::total_cmp);
    let at = |q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    format!(
        "n={} min={:.1} median={:.1} p95={:.1} max={:.1}",
        v.len(),
        v[0],
        at(0.5),
        at(0.95),
        v[v.len() - 1]
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_of_samples() {
        assert_eq!(summarize(&[]), "no samples");
        assert_eq!(
            summarize(&[3.0, 1.0, 2.0]),
            "n=3 min=1.0 median=2.0 p95=3.0 max=3.0"
        );
    }
}
