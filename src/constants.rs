//! Project-wide constants. Centralizes magic numbers so they are not
//! duplicated across modules.

use std::time::Duration;

/// Timeout for non-critical startup requests (/meta, /locations, /cdn-cgi/trace).
pub const META_FETCH_TIMEOUT: Duration = Duration::from_secs(5);

/// Per-connection TCP connect timeout for the speed-test HTTP client. Kept
/// short so that when a family is pinned via `--ipv4-only` / `--ipv6-only` and
/// that family has no working route, connects fail fast instead of stalling on
/// the overall request timeout.
pub const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Waveform-style bufferbloat thresholds: (max latency increase in ms, grade).
/// The first row whose threshold the measured value is <= to wins.
/// `f64::INFINITY` serves as the catch-all for F.
pub const BUFFERBLOAT_THRESHOLDS: &[(f64, &str)] = &[
    (5.0, "A+"),
    (30.0, "A"),
    (60.0, "B"),
    (200.0, "C"),
    (400.0, "D"),
    (f64::INFINITY, "F"),
];

/// Stability thresholds: (max coefficient of variation in percent, grade).
/// First matching row wins. Calibrated against typical observed CV ranges
/// (wired fiber ~2-4%, decent Wi-Fi ~8-15%, congested ~20%+).
pub const STABILITY_THRESHOLDS: &[(f64, &str)] = &[
    (5.0, "A"),
    (10.0, "B"),
    (20.0, "C"),
    (35.0, "D"),
    (f64::INFINITY, "F"),
];

/// Sentinel grade used in `ConnectionQuality` when one half (bufferbloat
/// or stability) cannot be computed but the other can. Single-character
/// hyphen, never the em dash.
pub const GRADE_UNAVAILABLE: &str = "-";

/// Minimum number of throughput samples required to compute a CV
/// (stddev of fewer than 3 samples is meaningless).
pub const MIN_STABILITY_SAMPLES: usize = 3;

/// Cadence at which the download/upload loops sample cumulative bytes and emit
/// a `ThroughputTick`. Also serves as the minimum steady-state window length:
/// a window shorter than one full interval carries too little signal to report.
pub const THROUGHPUT_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

/// Minimum number of additional history entries fetched per lazy-load request
/// when the History tab selection nears the end of the loaded window.
pub const HISTORY_LOAD_CHUNK_MIN: usize = 20;

/// Trigger history lazy-loading when the selection is within this many rows of
/// the end of the visible (filtered) list.
pub const HISTORY_LAZY_LOAD_THRESHOLD: usize = 10;

/// Rows PageUp/PageDown jump by in the History tab.
pub const HISTORY_PAGE_SIZE: usize = 20;
