//! Aggregations: top-N counts and percentile statistics.

use std::collections::HashMap;

/// Counts occurrences of values and reports the most frequent ones.
#[derive(Debug, Default)]
pub struct TopN {
    counts: HashMap<String, u64>,
    total: u64,
}

impl TopN {
    pub fn add(&mut self, key: &str) {
        self.total += 1;
        if let Some(c) = self.counts.get_mut(key) {
            *c += 1;
        } else {
            self.counts.insert(key.to_string(), 1);
        }
    }

    pub fn total(&self) -> u64 {
        self.total
    }

    /// The `n` most frequent values, ties broken alphabetically for stable output.
    pub fn top(&self, n: usize) -> Vec<(&str, u64)> {
        let mut v: Vec<(&str, u64)> = self.counts.iter().map(|(k, c)| (k.as_str(), *c)).collect();
        v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        v.truncate(n);
        v
    }
}

/// Summary statistics of a numeric field.
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Collects samples; percentiles are exact (samples are kept and sorted once).
#[derive(Debug, Default)]
pub struct Samples(Vec<f64>);

impl Samples {
    pub fn add(&mut self, v: f64) {
        if v.is_finite() {
            self.0.push(v);
        }
    }

    pub fn summary(&mut self) -> Option<Summary> {
        if self.0.is_empty() {
            return None;
        }
        self.0.sort_unstable_by(f64::total_cmp);
        let s = &self.0;
        let sum: f64 = s.iter().sum();
        Some(Summary {
            count: s.len(),
            min: s[0],
            max: s[s.len() - 1],
            mean: sum / s.len() as f64,
            p50: percentile(s, 50.0),
            p90: percentile(s, 90.0),
            p95: percentile(s, 95.0),
            p99: percentile(s, 99.0),
        })
    }
}

/// Percentile with linear interpolation between closest ranks
/// (the same definition as NumPy's default). `sorted` must be non-empty.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    let rank = (p / 100.0) * (sorted.len() - 1) as f64;
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    let frac = rank - lo as f64;
    sorted[lo] + (sorted[hi] - sorted[lo]) * frac
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn top_n_orders_by_count_then_key() {
        let mut t = TopN::default();
        for k in ["b", "a", "c", "a", "b", "a"] {
            t.add(k);
        }
        assert_eq!(t.top(2), vec![("a", 3), ("b", 2)]);
        assert_eq!(t.top(10).len(), 3);
        assert_eq!(t.total(), 6);
    }

    #[test]
    fn percentiles_interpolate() {
        let v: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentile(&v, 50.0), 50.5);
        assert!((percentile(&v, 99.0) - 99.01).abs() < 1e-9);
        assert_eq!(percentile(&v, 0.0), 1.0);
        assert_eq!(percentile(&v, 100.0), 100.0);
        assert_eq!(percentile(&[7.0], 95.0), 7.0);
    }

    #[test]
    fn summary_basic() {
        let mut s = Samples::default();
        for x in [3.0, 1.0, 2.0, f64::NAN, 4.0] {
            s.add(x);
        }
        let sum = s.summary().unwrap();
        assert_eq!((sum.count, sum.min, sum.max, sum.mean), (4, 1.0, 4.0, 2.5));
        assert_eq!(sum.p50, 2.5);
        assert!(Samples::default().summary().is_none());
    }
}
