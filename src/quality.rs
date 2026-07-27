//! Pure functions that turn measured numbers into Connection Quality grades.
//! No I/O, no global state. See docs/superpowers/specs/2026-05-24-connection-quality-design.md.

use crate::constants::{
    BUFFERBLOAT_THRESHOLDS, GRADE_UNAVAILABLE, MIN_STABILITY_SAMPLES, STABILITY_THRESHOLDS,
    STEADY_STATE_MIN_RAMP, STEADY_STATE_RAMP_FRACTION,
};
use crate::model::{ConnectionQuality, RunResult};

/// Map a latency-increase-under-load value (ms) to a Waveform grade.
/// Negative inputs are clamped to 0 (treated as "no measurable bloat").
pub fn bufferbloat_grade(bloat_ms: f64) -> &'static str {
    let v = bloat_ms.max(0.0);
    for (threshold, grade) in BUFFERBLOAT_THRESHOLDS {
        if v <= *threshold {
            return grade;
        }
    }
    // Unreachable: the last row is f64::INFINITY.
    "F"
}

/// Map a coefficient of variation (already in percent units, e.g. 8.2) to a stability grade.
pub fn stability_grade(cv_pct: f64) -> &'static str {
    for (threshold, grade) in STABILITY_THRESHOLDS {
        if cv_pct <= *threshold {
            return grade;
        }
    }
    "F"
}

/// Coefficient of variation of `samples`, expressed as a percentage of the mean.
/// Returns `None` if fewer than `MIN_STABILITY_SAMPLES` samples are provided,
/// or if the mean is zero (CV undefined).
pub fn cv_percent(samples: &[f64]) -> Option<f64> {
    if samples.len() < MIN_STABILITY_SAMPLES {
        return None;
    }
    let n = samples.len() as f64;
    let mean = samples.iter().sum::<f64>() / n;
    if mean.abs() < f64::EPSILON {
        return None;
    }
    // Sample stddev (n-1 denominator), matching `metrics::compute_jitter`.
    let var = samples.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n - 1.0);
    Some(var.sqrt() / mean * 100.0)
}

/// Drops the ramp-up prefix from a throughput point series `(t_secs, mbps)`:
/// everything within max(`STEADY_STATE_RAMP_FRACTION` x span,
/// `STEADY_STATE_MIN_RAMP`) of the first point, so TCP slow start does not
/// read as instability.
fn steady_points(points: &[(f64, f64)]) -> &[(f64, f64)] {
    let (Some((t_first, _)), Some((t_last, _))) = (points.first(), points.last()) else {
        return points;
    };
    let span = t_last - t_first;
    let cutoff =
        t_first + (span * STEADY_STATE_RAMP_FRACTION).max(STEADY_STATE_MIN_RAMP.as_secs_f64());
    match points.iter().position(|(t, _)| *t >= cutoff) {
        Some(i) => &points[i..],
        None => &[],
    }
}

/// Build `ConnectionQuality` from a finished `RunResult` and the per-second
/// throughput sample vectors recorded during the run. Returns `None` only if
/// neither bufferbloat nor stability can be computed.
pub fn compute(
    result: &RunResult,
    dl_points: &[(f64, f64)],
    ul_points: &[(f64, f64)],
) -> Option<ConnectionQuality> {
    let idle = result.idle_latency.median_ms;
    let dl_med = result.loaded_latency_download.median_ms;
    let ul_med = result.loaded_latency_upload.median_ms;

    // Bufferbloat: max over available directions, clamped at 0.
    let bloat_ms = match idle {
        Some(i) => {
            let deltas: Vec<f64> = [dl_med, ul_med]
                .into_iter()
                .flatten()
                .map(|x| (x - i).max(0.0))
                .collect();
            if deltas.is_empty() {
                None
            } else {
                Some(deltas.into_iter().fold(0.0_f64, f64::max))
            }
        }
        None => None,
    };

    // Stability: worst-of available directions, over steady-state samples only.
    let dl_mbps: Vec<f64> = steady_points(dl_points).iter().map(|(_, m)| *m).collect();
    let ul_mbps: Vec<f64> = steady_points(ul_points).iter().map(|(_, m)| *m).collect();
    let cv_dl = cv_percent(&dl_mbps);
    let cv_ul = cv_percent(&ul_mbps);
    let cv_worst = match (cv_dl, cv_ul) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (Some(a), None) | (None, Some(a)) => Some(a),
        (None, None) => None,
    };

    if bloat_ms.is_none() && cv_worst.is_none() {
        return None;
    }

    let (bufferbloat_grade_str, bufferbloat_ms_field) = match bloat_ms {
        Some(ms) => (bufferbloat_grade(ms).to_string(), Some(ms)),
        None => (GRADE_UNAVAILABLE.to_string(), None),
    };
    let (stability_grade_str, stability_cv_field) = match cv_worst {
        Some(cv) => (stability_grade(cv).to_string(), Some(cv)),
        None => (GRADE_UNAVAILABLE.to_string(), None),
    };

    Some(ConnectionQuality {
        bufferbloat_grade: bufferbloat_grade_str,
        bufferbloat_ms: bufferbloat_ms_field,
        stability_grade: stability_grade_str,
        stability_cv_pct: stability_cv_field,
        stability_cv_download_pct: cv_dl,
        stability_cv_upload_pct: cv_ul,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{empty_run_result, LatencySummary, RunResult};

    fn make_result(idle_med: Option<f64>, dl_med: Option<f64>, ul_med: Option<f64>) -> RunResult {
        let mut r = empty_run_result();
        r.idle_latency = LatencySummary {
            median_ms: idle_med,
            ..Default::default()
        };
        r.loaded_latency_download = LatencySummary {
            median_ms: dl_med,
            ..Default::default()
        };
        r.loaded_latency_upload = LatencySummary {
            median_ms: ul_med,
            ..Default::default()
        };
        r
    }

    fn pts(mbps: &[f64]) -> Vec<(f64, f64)> {
        mbps.iter()
            .enumerate()
            .map(|(i, m)| (i as f64, *m))
            .collect()
    }

    #[test]
    fn cv_percent_handles_steady_signal() {
        let cv = cv_percent(&[100.0, 100.0, 100.0]);
        assert!(cv.is_some());
        assert!(cv.unwrap() < 0.0001);
    }

    #[test]
    fn cv_percent_handles_variation() {
        // mean = 100, stddev ~ 10 (sample), CV ~ 10%
        let cv = cv_percent(&[90.0, 100.0, 110.0]);
        assert!(cv.is_some());
        let v = cv.unwrap();
        assert!((v - 10.0).abs() < 0.5, "got cv={v}");
    }

    #[test]
    fn cv_percent_too_few_samples() {
        assert!(cv_percent(&[]).is_none());
        assert!(cv_percent(&[100.0]).is_none());
        assert!(cv_percent(&[100.0, 100.0]).is_none());
    }

    #[test]
    fn cv_percent_zero_mean() {
        assert!(cv_percent(&[0.0, 0.0, 0.0]).is_none());
    }

    #[test]
    fn compute_full_grades_for_normal_run() {
        // idle 20ms; loaded DL 25ms (+5 -> A+); loaded UL 50ms (+30 -> A).
        // Worst = 30ms = A.
        let r = make_result(Some(20.0), Some(25.0), Some(50.0));
        // Steady ~100Mbps download, ~8.5% CV upload (mean 98, stddev ~8.37).
        let cq = compute(
            &r,
            &pts(&[100.0; 5]),
            &pts(&[90.0, 100.0, 110.0, 100.0, 90.0]),
        )
        .unwrap();
        assert_eq!(cq.bufferbloat_grade, "A");
        assert!((cq.bufferbloat_ms.unwrap() - 30.0).abs() < 0.01);
        assert!(cq.stability_cv_download_pct.unwrap() < 0.01);
        assert!(cq.stability_cv_upload_pct.unwrap() > 5.0);
        // Worst-of stability: upload wins.
        assert!((cq.stability_cv_pct.unwrap() - cq.stability_cv_upload_pct.unwrap()).abs() < 0.01);
        assert_eq!(cq.stability_grade, "B");
    }

    #[test]
    fn stability_ignores_ramp_up_prefix() {
        let r = make_result(None, Some(100.0), Some(100.0));
        // 10 per-second points: TCP ramp for the first 2s, rock-steady after.
        // A stable link must not be graded down for its slow start.
        let mut points = vec![(0.0, 5.0), (1.0, 50.0)];
        points.extend((2..10).map(|i| (i as f64, 100.0)));
        let cq = compute(&r, &points, &points).unwrap();
        assert_eq!(cq.stability_grade, "A", "cv={:?}", cq.stability_cv_pct);
    }

    #[test]
    fn compute_returns_none_when_nothing_computable() {
        let r = make_result(None, None, None);
        assert!(compute(&r, &[], &[]).is_none());
    }

    #[test]
    fn compute_bufferbloat_only_when_throughput_missing() {
        // idle=20, dl_loaded=80 (delta 60), ul_loaded=50 (delta 30). Worst-of = 60 -> "B".
        let r = make_result(Some(20.0), Some(80.0), Some(50.0));
        let cq = compute(&r, &[], &[]).unwrap();
        assert_eq!(cq.bufferbloat_grade, "B");
        assert!((cq.bufferbloat_ms.unwrap() - 60.0).abs() < 0.01);
        assert_eq!(cq.stability_grade, crate::constants::GRADE_UNAVAILABLE);
        assert!(cq.stability_cv_pct.is_none());
        assert!(cq.stability_cv_download_pct.is_none());
        assert!(cq.stability_cv_upload_pct.is_none());
    }

    #[test]
    fn compute_stability_only_when_bufferbloat_missing() {
        let r = make_result(None, Some(100.0), Some(100.0));
        let cq = compute(&r, &pts(&[100.0; 5]), &pts(&[100.0; 5])).unwrap();
        assert_eq!(cq.bufferbloat_grade, crate::constants::GRADE_UNAVAILABLE);
        assert!(cq.bufferbloat_ms.is_none());
        assert_eq!(cq.stability_grade, "A");
    }

    #[test]
    fn compute_single_direction_loaded_latency() {
        // Only download loaded latency available.
        let r = make_result(Some(20.0), Some(220.0), None);
        let cq = compute(&r, &[], &[]).unwrap();
        // 220-20 = 200 -> C
        assert_eq!(cq.bufferbloat_grade, "C");
        assert_eq!(cq.stability_grade, crate::constants::GRADE_UNAVAILABLE);
    }

    #[test]
    fn bufferbloat_boundary_aplus() {
        assert_eq!(bufferbloat_grade(0.0), "A+");
        assert_eq!(bufferbloat_grade(5.0), "A+");
    }

    #[test]
    fn bufferbloat_boundary_a() {
        assert_eq!(bufferbloat_grade(5.0001), "A");
        assert_eq!(bufferbloat_grade(30.0), "A");
    }

    #[test]
    fn bufferbloat_boundary_b() {
        assert_eq!(bufferbloat_grade(30.0001), "B");
        assert_eq!(bufferbloat_grade(60.0), "B");
    }

    #[test]
    fn bufferbloat_boundary_c() {
        assert_eq!(bufferbloat_grade(60.0001), "C");
        assert_eq!(bufferbloat_grade(200.0), "C");
    }

    #[test]
    fn bufferbloat_boundary_d() {
        assert_eq!(bufferbloat_grade(200.0001), "D");
        assert_eq!(bufferbloat_grade(400.0), "D");
    }

    #[test]
    fn bufferbloat_boundary_f() {
        assert_eq!(bufferbloat_grade(400.0001), "F");
        assert_eq!(bufferbloat_grade(10_000.0), "F");
    }

    #[test]
    fn bufferbloat_negative_clamps_to_aplus() {
        assert_eq!(bufferbloat_grade(-5.0), "A+");
        assert_eq!(bufferbloat_grade(-1000.0), "A+");
    }

    #[test]
    fn stability_boundaries() {
        assert_eq!(stability_grade(0.0), "A");
        assert_eq!(stability_grade(5.0), "A");
        assert_eq!(stability_grade(5.0001), "B");
        assert_eq!(stability_grade(10.0), "B");
        assert_eq!(stability_grade(10.0001), "C");
        assert_eq!(stability_grade(20.0), "C");
        assert_eq!(stability_grade(20.0001), "D");
        assert_eq!(stability_grade(35.0), "D");
        assert_eq!(stability_grade(35.0001), "F");
        assert_eq!(stability_grade(1_000.0), "F");
    }
}
