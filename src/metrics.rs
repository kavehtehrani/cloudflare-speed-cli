//! Statistical helpers for latency and throughput samples.

/// Summary statistics computed from a sample set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SampleMetrics {
    pub mean: f64,
    pub median: f64,
    pub p25: f64,
    pub p75: f64,
    pub p95: f64,
    pub p99: f64,
}

/// Linear interpolation percentile on a **sorted** slice.
///
/// Uses the common `(n - 1) * p` index method so even-length medians are the
/// average of the two central values (e.g. `[1,2,3,4]` → median `2.5`).
fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    let n = sorted.len();
    debug_assert!(!sorted.is_empty());
    if n == 1 {
        return sorted[0];
    }
    let p = p.clamp(0.0, 1.0);
    let pos = p * (n - 1) as f64;
    let lower = pos.floor() as usize;
    let upper = pos.ceil() as usize;
    if lower == upper {
        sorted[lower]
    } else {
        let frac = pos - lower as f64;
        sorted[lower] + frac * (sorted[upper] - sorted[lower])
    }
}

/// Compute metrics (mean, median, p25, p75, p95, p99) from samples.
/// Sorts a temporary copy internally.
pub fn compute_sample_metrics(samples: &[f64]) -> Option<SampleMetrics> {
    if samples.is_empty() {
        return None;
    }
    let n = samples.len();
    let mean = samples.iter().sum::<f64>() / n as f64;

    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    Some(SampleMetrics {
        mean,
        median: percentile_sorted(&sorted, 0.5),
        p25: percentile_sorted(&sorted, 0.25),
        p75: percentile_sorted(&sorted, 0.75),
        p95: percentile_sorted(&sorted, 0.95),
        p99: percentile_sorted(&sorted, 0.99),
    })
}

/// Compute a single percentile from unsorted samples.
pub fn percentile(samples: &[f64], p: f64) -> Option<f64> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    Some(percentile_sorted(&sorted, p))
}

/// Backward-compatible tuple return for call sites that only need quartiles.
#[allow(dead_code)]
pub fn compute_metrics(samples: &[f64]) -> Option<(f64, f64, f64, f64)> {
    compute_sample_metrics(samples).map(|m| (m.mean, m.median, m.p25, m.p75))
}

/// Compute jitter (sample standard deviation) from latency samples.
pub fn compute_jitter(samples: &[f64]) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    let variance = samples.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some(variance.sqrt())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_metrics_basic() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let m = compute_sample_metrics(&samples).unwrap();
        assert!((m.mean - 3.0).abs() < 0.001);
        assert!((m.median - 3.0).abs() < 0.001);
        assert!((m.p25 - 2.0).abs() < 0.001);
        assert!((m.p75 - 4.0).abs() < 0.001);
        assert!((m.p95 - 4.8).abs() < 0.001);
        assert!((m.p99 - 4.96).abs() < 0.01);
    }

    #[test]
    fn test_median_even_length() {
        // [1,2,3,4] → median 2.5 (not 3)
        let samples = vec![1.0, 2.0, 3.0, 4.0];
        let m = compute_sample_metrics(&samples).unwrap();
        assert!((m.median - 2.5).abs() < 0.001);
        assert!((m.p25 - 1.75).abs() < 0.001);
        assert!((m.p75 - 3.25).abs() < 0.001);
    }

    #[test]
    fn test_compute_metrics_empty() {
        assert!(compute_sample_metrics(&[]).is_none());
        assert!(compute_metrics(&[]).is_none());
    }

    #[test]
    fn test_compute_metrics_single_sample() {
        let m = compute_sample_metrics(&[42.0]).unwrap();
        assert!((m.mean - 42.0).abs() < 0.001);
        assert!((m.median - 42.0).abs() < 0.001);
        assert!((m.p25 - 42.0).abs() < 0.001);
        assert!((m.p75 - 42.0).abs() < 0.001);
        assert!((m.p95 - 42.0).abs() < 0.001);
        assert!((m.p99 - 42.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_metrics_two_samples() {
        let m = compute_sample_metrics(&[10.0, 20.0]).unwrap();
        assert!((m.mean - 15.0).abs() < 0.001);
        assert!((m.median - 15.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_metrics_unsorted_input() {
        let samples = vec![5.0, 1.0, 3.0, 2.0, 4.0];
        let m = compute_sample_metrics(&samples).unwrap();
        assert!((m.mean - 3.0).abs() < 0.001);
        assert!((m.median - 3.0).abs() < 0.001);
        assert!((m.p25 - 2.0).abs() < 0.001);
        assert!((m.p75 - 4.0).abs() < 0.001);
    }

    #[test]
    fn test_compute_jitter_basic() {
        let samples = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let jitter = compute_jitter(&samples).unwrap();
        assert!((jitter - 1.5811).abs() < 0.001);
    }

    #[test]
    fn test_compute_jitter_insufficient_samples() {
        assert!(compute_jitter(&[1.0]).is_none());
        assert!(compute_jitter(&[]).is_none());
    }
}
