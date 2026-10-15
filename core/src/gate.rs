//! Gates: correctness is part of the API.
//!
//! A gate compares two independent implementations (or one implementation and a fixture) item by item over a corpus
//! and reports how many agree exactly, how many within a stated tolerance, and which differ. Every decoder in the
//! workspace has one; every reader has a census over a game's real data. See CONTRIBUTING.md "Gates".

use std::fmt;
use std::path::PathBuf;

/// How far two outputs may differ and still pass.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tolerance {
    /// Bit for bit.
    Exact,
    /// Largest absolute difference allowed per element (e.g. 1 grey level against Pillow).
    Abs(f64),
}

/// One item that failed a gate.
#[derive(Clone, Debug, PartialEq)]
pub struct GateFailure {
    /// Which item (fixture name, file path, texture key).
    pub item: String,
    pub detail: String,
}

/// The outcome of a gate over a corpus.
#[derive(Clone, Debug, PartialEq)]
pub struct GateReport {
    pub name: String,
    pub tolerance: Tolerance,
    pub checked: u64,
    pub exact: u64,
    /// Not exact but within the tolerance.
    pub within: u64,
    pub failures: Vec<GateFailure>,
}

impl GateReport {
    pub fn new(name: impl Into<String>, tolerance: Tolerance) -> Self {
        GateReport {
            name: name.into(),
            tolerance,
            checked: 0,
            exact: 0,
            within: 0,
            failures: Vec::new(),
        }
    }

    /// Record an item whose largest element difference is `max_diff` (0 for exact).
    pub fn record(&mut self, item: impl Into<String>, max_diff: f64) {
        self.checked += 1;
        if max_diff == 0.0 {
            self.exact += 1;
            return;
        }
        match self.tolerance {
            Tolerance::Abs(t) if max_diff <= t => self.within += 1,
            _ => self.failures.push(GateFailure {
                item: item.into(),
                detail: format!("max difference {max_diff}"),
            }),
        }
    }

    /// Record an item that failed outright (could not be decoded, sizes differ, ...).
    pub fn fail(&mut self, item: impl Into<String>, detail: impl Into<String>) {
        self.checked += 1;
        self.failures.push(GateFailure {
            item: item.into(),
            detail: detail.into(),
        });
    }

    /// Compare two byte buffers element by element.
    pub fn compare_bytes(&mut self, item: impl Into<String>, a: &[u8], b: &[u8]) {
        if a.len() != b.len() {
            self.fail(item, format!("lengths differ: {} vs {}", a.len(), b.len()));
            return;
        }
        let max = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y)).max().unwrap_or(0);
        self.record(item, f64::from(max));
    }

    pub fn passed(&self) -> bool {
        self.failures.is_empty() && self.checked > 0
    }

    /// Panic with the report if the gate did not pass (for tests).
    #[track_caller]
    pub fn assert_passed(&self) {
        assert!(self.passed(), "{self}");
    }
}

impl fmt::Display for GateReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "gate {}: {} checked, {} exact, {} within {:?}, {} failed",
            self.name,
            self.checked,
            self.exact,
            self.within,
            self.tolerance,
            self.failures.len()
        )?;
        for fail in self.failures.iter().take(20) {
            write!(f, "\n  {}: {}", fail.item, fail.detail)?;
        }
        if self.failures.len() > 20 {
            write!(f, "\n  ... {} more", self.failures.len() - 20)?;
        }
        Ok(())
    }
}

/// Real-data location from an environment variable, for tests that read the user's installs or work folders.
/// Absent or missing on disk: prints a skip line and returns `None`, so the test passes as skipped. The variables
/// are listed in CONTRIBUTING.md "Real data".
pub fn real_data(var: &str) -> Option<PathBuf> {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(p) if p.exists() => Some(p),
        Some(p) => {
            eprintln!("SKIPPED: {var}={} does not exist", p.display());
            None
        }
        None => {
            eprintln!("SKIPPED: {var} not set");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_counts_exact_within_and_failed() {
        let mut r = GateReport::new("t", Tolerance::Abs(1.0));
        r.compare_bytes("a", &[1, 2, 3], &[1, 2, 3]);
        r.compare_bytes("b", &[1, 2, 3], &[1, 3, 3]);
        r.compare_bytes("c", &[1, 2, 3], &[1, 5, 3]);
        r.compare_bytes("d", &[1], &[1, 2]);
        assert_eq!((r.checked, r.exact, r.within, r.failures.len()), (4, 1, 1, 2));
        assert!(!r.passed());
        assert!(
            !GateReport::new("empty", Tolerance::Exact).passed(),
            "a gate that checked nothing did not pass"
        );
    }
}
