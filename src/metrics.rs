/// Compute metrics (mean, median, 25th percentile, 75th percentile) from samples
pub fn compute_metrics(mut samples: Vec<f64>) -> Option<(f64, f64, f64, f64)> {
    if samples.len() < 2 {
        return None;
    }
    let n = samples.len();
    let mean = samples.iter().sum::<f64>() / n as f64;
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = samples[n / 2];
    let p25 = samples[n / 4];
    let p75 = samples[3 * n / 4];
    Some((mean, median, p25, p75))
}
