use crate::model::LatencySummary;

#[derive(Debug, Default, Clone)]
pub struct OnlineStats {
    n: u64,
    mean: f64,
    m2: f64,
}

impl OnlineStats {
    pub fn push(&mut self, x: f64) {
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / (self.n as f64);
        let delta2 = x - self.mean;
        self.m2 += delta * delta2;
    }

    pub fn stddev(&self) -> Option<f64> {
        if self.n < 2 {
            None
        } else {
            Some((self.m2 / ((self.n - 1) as f64)).sqrt())
        }
    }
}

pub fn latency_summary_from_samples(
    sent: u64,
    received: u64,
    samples_ms: &[f64],
    jitter_ms: Option<f64>,
) -> LatencySummary {
    let loss = if sent == 0 {
        0.0
    } else {
        ((sent - received) as f64) / (sent as f64)
    };

    if samples_ms.is_empty() {
        return LatencySummary {
            sent,
            received,
            loss,
            jitter_ms,
            ..Default::default()
        };
    }

    // Use the same calculation method as metrics.rs for consistency
    let mut sorted = samples_ms.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = sorted.len();

    let min_ms = Some(sorted[0]);
    let max_ms = Some(sorted[n - 1]);

    // Compute metrics using the same method as metrics.rs
    if let Some(m) = crate::metrics::compute_sample_metrics(samples_ms) {
        // Use provided jitter or compute from samples using shared function
        let jitter = jitter_ms.or_else(|| crate::metrics::compute_jitter(samples_ms));

        LatencySummary {
            sent,
            received,
            loss,
            min_ms,
            mean_ms: Some(m.mean),
            median_ms: Some(m.median),
            p25_ms: Some(m.p25),
            p75_ms: Some(m.p75),
            p95_ms: Some(m.p95),
            p99_ms: Some(m.p99),
            max_ms,
            jitter_ms: jitter,
        }
    } else {
        LatencySummary {
            sent,
            received,
            loss,
            jitter_ms,
            ..Default::default()
        }
    }
}
