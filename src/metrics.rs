/// Compute latency metrics (mean, median, 25th percentile, 75th percentile) from samples
pub fn compute_latency_metrics(samples: &[f64]) -> Option<(f64, f64, f64, f64)> {
    if samples.len() < 2 {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let mean = samples.iter().sum::<f64>() / samples.len() as f64;
    let median = sorted[n / 2];
    let p25 = sorted[n / 4];
    let p75 = sorted[3 * n / 4];
    Some((mean, median, p25, p75))
}

/// Compute throughput metrics (mean, median, 25th percentile, 75th percentile) from throughput points (time, value pairs)
pub fn compute_throughput_metrics(points: &[(f64, f64)]) -> Option<(f64, f64, f64, f64)> {
    if points.len() < 2 {
        return None;
    }
    // Extract just the throughput values (y-values)
    let values: Vec<f64> = points.iter().map(|(_, y)| *y).collect();
    let mut sorted = values.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = sorted.len();
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let median = sorted[n / 2];
    let p25 = sorted[n / 4];
    let p75 = sorted[3 * n / 4];
    Some((mean, median, p25, p75))
}
