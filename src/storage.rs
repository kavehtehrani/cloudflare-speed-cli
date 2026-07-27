use crate::model::RunResult;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// Get the base directory for storing application data.
fn base_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("cloudflare-speed-cli")
}

/// Get the directory for storing test run results.
fn runs_dir() -> PathBuf {
    base_dir().join("runs")
}

pub fn save_run(result: &RunResult) -> Result<PathBuf> {
    save_run_to(&runs_dir(), result)
}

/// Save a run into `dir` (the runs directory). The write is atomic (write to
/// a temp file, then rename) so a crash or full disk mid-write can never
/// leave a truncated run file behind.
fn save_run_to(dir: &Path, result: &RunResult) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).context("create runs dir")?;
    let path = run_path_in(dir, result);
    let data = serde_json::to_vec_pretty(result)?;
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, data) {
        // Don't leave a partial temp file behind (e.g. disk full mid-write).
        let _ = std::fs::remove_file(&tmp);
        return Err(e).context("write run json");
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).context("finalize run json");
    }
    Ok(path)
}

fn run_path_in(dir: &Path, result: &RunResult) -> PathBuf {
    let ts = &result.timestamp_utc;
    let safe_ts = ts.replace(':', "-").replace('T', "_");
    dir.join(format!("run-{safe_ts}-{}.json", result.meas_id))
}

pub fn get_run_path(result: &RunResult) -> Result<PathBuf> {
    Ok(run_path_in(&runs_dir(), result))
}

pub fn delete_run(result: &RunResult) -> Result<()> {
    let path = get_run_path(result)?;
    if path.exists() {
        std::fs::remove_file(&path).context("delete run file")?;
    }
    Ok(())
}

pub fn export_json(path: &Path, result: &RunResult) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create export directory")?;
    }
    let data = serde_json::to_vec_pretty(result)?;
    std::fs::write(path, data).context("write export json")?;
    Ok(())
}

const CSV_HEADER: &str = "timestamp_utc,base_url,meas_id,comments,server,download_mbps,upload_mbps,idle_mean_ms,idle_median_ms,idle_p25_ms,idle_p75_ms,idle_p95_ms,idle_p99_ms,idle_loss,dl_loaded_mean_ms,dl_loaded_median_ms,dl_loaded_p25_ms,dl_loaded_p75_ms,dl_loaded_p95_ms,dl_loaded_p99_ms,dl_loaded_loss,ul_loaded_mean_ms,ul_loaded_median_ms,ul_loaded_p25_ms,ul_loaded_p75_ms,ul_loaded_p95_ms,ul_loaded_p99_ms,ul_loaded_loss,ip,colo,asn,as_org,interface_name,network_name,is_wireless,interface_mac,local_ipv4,local_ipv6,external_ipv4,external_ipv6,dns_resolution_ms,dns_ipv4_count,dns_ipv6_count,dns_servers,tls_handshake_ms,tls_protocol,tls_cipher,ipv4_download_mbps,ipv4_upload_mbps,ipv4_latency_ms,ipv6_download_mbps,ipv6_upload_mbps,ipv6_latency_ms,traceroute_hops,bufferbloat_grade,bufferbloat_ms,stability_grade,stability_cv_pct,stability_cv_download_pct,stability_cv_upload_pct\n";

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn opt_f64(v: Option<f64>) -> String {
    v.map(|x| format!("{:.3}", x)).unwrap_or_default()
}

fn csv_row(result: &RunResult) -> String {
    let dns_resolution_ms = result.dns.as_ref().map(|d| d.resolution_time_ms);
    let dns_ipv4_count = result.dns.as_ref().map(|d| d.ipv4_count);
    let dns_ipv6_count = result.dns.as_ref().map(|d| d.ipv6_count);
    let dns_servers = result
        .dns
        .as_ref()
        .map(|d| d.dns_servers.join("; "))
        .unwrap_or_default();
    let tls_handshake_ms = result.tls.as_ref().map(|t| t.handshake_time_ms);
    let tls_protocol = result.tls.as_ref().and_then(|t| t.protocol_version.clone());
    let tls_cipher = result.tls.as_ref().and_then(|t| t.cipher_suite.clone());

    let ipv4_download = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv4_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.download_mbps);
    let ipv4_upload = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv4_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.upload_mbps);
    let ipv4_latency = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv4_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.latency_ms);

    let ipv6_download = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv6_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.download_mbps);
    let ipv6_upload = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv6_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.upload_mbps);
    let ipv6_latency = result
        .ip_comparison
        .as_ref()
        .and_then(|c| c.ipv6_result.as_ref())
        .filter(|r| r.available)
        .map(|r| r.latency_ms);

    let traceroute_hops = result.traceroute.as_ref().map(|t| t.hops.len());

    let (cq_bloat_grade, cq_bloat_ms, cq_stab_grade, cq_stab_cv, cq_stab_cv_dl, cq_stab_cv_ul) =
        match result.connection_quality.as_ref() {
            Some(cq) => (
                cq.bufferbloat_grade.clone(),
                cq.bufferbloat_ms
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_default(),
                cq.stability_grade.clone(),
                cq.stability_cv_pct
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_default(),
                cq.stability_cv_download_pct
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_default(),
                cq.stability_cv_upload_pct
                    .map(|v| format!("{:.3}", v))
                    .unwrap_or_default(),
            ),
            None => (
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
                String::new(),
            ),
        };

    format!(
        "{},{},{},{},{},{:.3},{:.3},{},{},{},{},{},{},{:.6},{},{},{},{},{},{},{:.6},{},{},{},{},{},{},{:.6},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}\n",
        csv_escape(&result.timestamp_utc),
        csv_escape(&result.base_url),
        csv_escape(&result.meas_id),
        csv_escape(result.comments.as_deref().unwrap_or("")),
        csv_escape(result.server.as_deref().unwrap_or("")),
        result.download.mbps,
        result.upload.mbps,
        opt_f64(result.idle_latency.mean_ms),
        opt_f64(result.idle_latency.median_ms),
        opt_f64(result.idle_latency.p25_ms),
        opt_f64(result.idle_latency.p75_ms),
        opt_f64(result.idle_latency.p95_ms),
        opt_f64(result.idle_latency.p99_ms),
        result.idle_latency.loss,
        opt_f64(result.loaded_latency_download.mean_ms),
        opt_f64(result.loaded_latency_download.median_ms),
        opt_f64(result.loaded_latency_download.p25_ms),
        opt_f64(result.loaded_latency_download.p75_ms),
        opt_f64(result.loaded_latency_download.p95_ms),
        opt_f64(result.loaded_latency_download.p99_ms),
        result.loaded_latency_download.loss,
        opt_f64(result.loaded_latency_upload.mean_ms),
        opt_f64(result.loaded_latency_upload.median_ms),
        opt_f64(result.loaded_latency_upload.p25_ms),
        opt_f64(result.loaded_latency_upload.p75_ms),
        opt_f64(result.loaded_latency_upload.p95_ms),
        opt_f64(result.loaded_latency_upload.p99_ms),
        result.loaded_latency_upload.loss,
        csv_escape(result.ip.as_deref().unwrap_or("")),
        csv_escape(result.colo.as_deref().unwrap_or("")),
        csv_escape(result.asn.as_deref().unwrap_or("")),
        csv_escape(result.as_org.as_deref().unwrap_or("")),
        csv_escape(result.interface_name.as_deref().unwrap_or("")),
        csv_escape(result.network_name.as_deref().unwrap_or("")),
        result
            .is_wireless
            .map(|w| if w { "true" } else { "false" })
            .unwrap_or(""),
        csv_escape(result.interface_mac.as_deref().unwrap_or("")),
        csv_escape(result.local_ipv4.as_deref().unwrap_or("")),
        csv_escape(result.local_ipv6.as_deref().unwrap_or("")),
        csv_escape(result.external_ipv4.as_deref().unwrap_or("")),
        csv_escape(result.external_ipv6.as_deref().unwrap_or("")),
        dns_resolution_ms.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        dns_ipv4_count.map(|v| v.to_string()).unwrap_or_default(),
        dns_ipv6_count.map(|v| v.to_string()).unwrap_or_default(),
        csv_escape(&dns_servers),
        tls_handshake_ms.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        csv_escape(tls_protocol.as_deref().unwrap_or("")),
        csv_escape(tls_cipher.as_deref().unwrap_or("")),
        ipv4_download.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        ipv4_upload.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        ipv4_latency.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        ipv6_download.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        ipv6_upload.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        ipv6_latency.map(|v| format!("{:.3}", v)).unwrap_or_default(),
        traceroute_hops.map(|v| v.to_string()).unwrap_or_default(),
        csv_escape(&cq_bloat_grade),
        cq_bloat_ms,
        csv_escape(&cq_stab_grade),
        cq_stab_cv,
        cq_stab_cv_dl,
        cq_stab_cv_ul,
    )
}

pub fn export_csv(path: &Path, result: &RunResult) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create export directory")?;
    }
    let mut out = String::from(CSV_HEADER);
    out.push_str(&csv_row(result));
    std::fs::write(path, out).context("write export csv")?;
    Ok(())
}

/// Export multiple runs as a single CSV file (one header row, one row per run).
pub fn export_all_csv(path: &Path, results: &[RunResult]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("create export directory")?;
    }
    let mut out = String::from(CSV_HEADER);
    for result in results {
        out.push_str(&csv_row(result));
    }
    std::fs::write(path, out).context("write export csv")?;
    Ok(())
}

/// List run JSON paths in `dir` newest-first without reading file contents.
/// A missing directory is treated as an empty history.
fn list_run_paths_in(dir: &Path, limit: usize) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<PathBuf> = Vec::new();
    for e in std::fs::read_dir(dir).context("read runs dir")? {
        let e = e?;
        let p = e.path();
        if p.extension().and_then(|e| e.to_str()) == Some("json") {
            entries.push(p);
        }
    }
    entries.sort_by(|a, b| {
        let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
        bn.cmp(an)
    });
    entries.truncate(limit);
    Ok(entries)
}

/// Load up to `limit` runs from `dir` starting `offset` entries into the
/// newest-first ordering, parsing ONLY that slice. Returns the runs that
/// parsed plus the paths of files that could not be read or parsed. A single
/// corrupt file must never make the rest of the history unreachable.
fn load_runs_range_from(
    dir: &Path,
    offset: usize,
    limit: usize,
) -> Result<(Vec<RunResult>, Vec<PathBuf>)> {
    let paths = list_run_paths_in(dir, offset.saturating_add(limit))?;
    let mut out = Vec::with_capacity(limit.min(paths.len()));
    let mut skipped = Vec::new();
    for p in paths.into_iter().skip(offset) {
        match std::fs::read(&p) {
            Ok(data) => match serde_json::from_slice::<RunResult>(&data) {
                Ok(r) => out.push(r),
                Err(_) => skipped.push(p),
            },
            Err(_) => skipped.push(p),
        }
    }
    Ok((out, skipped))
}

fn load_runs_from(dir: &Path, limit: usize) -> Result<(Vec<RunResult>, Vec<PathBuf>)> {
    load_runs_range_from(dir, 0, limit)
}

pub fn load_recent(limit: usize) -> Result<Vec<RunResult>> {
    Ok(load_runs_from(&runs_dir(), limit)?.0)
}

/// Load `limit` runs starting `offset` entries into the newest-first
/// ordering. Lets the History tab's lazy loading parse only the next chunk
/// instead of re-parsing everything already in memory.
pub fn load_recent_range(offset: usize, limit: usize) -> Result<Vec<RunResult>> {
    Ok(load_runs_range_from(&runs_dir(), offset, limit)?.0)
}

/// Load all saved runs newest first, also reporting the files that were
/// skipped as unreadable/corrupt so callers can warn instead of silently
/// under-reporting.
pub fn load_all_with_skipped() -> Result<(Vec<RunResult>, Vec<PathBuf>)> {
    load_runs_from(&runs_dir(), usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::empty_run_result;

    /// Fresh, unique directory under the OS temp dir for storage tests.
    fn test_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "cloudflare-speed-cli-test-{}-{}",
            std::process::id(),
            tag
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn run_at(ts: &str, meas_id: &str) -> crate::model::RunResult {
        let mut r = empty_run_result();
        r.timestamp_utc = ts.into();
        r.meas_id = meas_id.into();
        r.base_url = "https://speed.cloudflare.com".into();
        r
    }

    #[test]
    fn load_runs_from_skips_corrupt_files() {
        let dir = test_dir("skip-corrupt");
        save_run_to(&dir, &run_at("2026-01-01T00:00:00Z", "aaa")).unwrap();
        save_run_to(&dir, &run_at("2026-01-02T00:00:00Z", "bbb")).unwrap();
        // Simulates a file truncated by a crash or full disk mid-write.
        std::fs::write(dir.join("run-2026-01-03_00-00-00Z-ccc.json"), b"{\"trunc").unwrap();

        let (runs, skipped) = load_runs_from(&dir, 10).unwrap();
        assert_eq!(
            runs.iter().map(|r| r.meas_id.as_str()).collect::<Vec<_>>(),
            vec!["bbb", "aaa"]
        );
        assert_eq!(skipped.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_runs_range_parses_only_the_requested_slice() {
        let dir = test_dir("range");
        for (ts, id) in [
            ("2026-01-01T00:00:00Z", "a"),
            ("2026-01-02T00:00:00Z", "b"),
            ("2026-01-03T00:00:00Z", "c"),
            ("2026-01-04T00:00:00Z", "d"),
            ("2026-01-05T00:00:00Z", "e"),
        ] {
            save_run_to(&dir, &run_at(ts, id)).unwrap();
        }
        // Newest-first ordering is e,d,c,b,a; offset 2, limit 2 -> c,b.
        let (runs, skipped) = load_runs_range_from(&dir, 2, 2).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(
            runs.iter().map(|r| r.meas_id.as_str()).collect::<Vec<_>>(),
            vec!["c", "b"]
        );
        // Offset past the end is an empty result, not an error.
        assert!(load_runs_range_from(&dir, 10, 5).unwrap().0.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_run_to_roundtrips_without_leftover_temp_files() {
        let dir = test_dir("roundtrip");
        let r = run_at("2026-01-01T12:34:56Z", "abc123");
        save_run_to(&dir, &r).unwrap();

        // Exactly one file, and it is the final .json (no stray tmp files).
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 1, "unexpected files: {names:?}");
        assert!(names[0].ends_with(".json"), "unexpected files: {names:?}");

        let (runs, skipped) = load_runs_from(&dir, 10).unwrap();
        assert!(skipped.is_empty());
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].meas_id, "abc123");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn csv_row_includes_p95_p99_columns() {
        let mut r = empty_run_result();
        r.timestamp_utc = "2026-01-01T00:00:00Z".into();
        r.meas_id = "123".into();
        r.base_url = "https://speed.cloudflare.com".into();
        r.idle_latency.p95_ms = Some(42.0);
        r.idle_latency.p99_ms = Some(99.0);
        let row = csv_row(&r);
        assert!(row.contains(",42.000,99.000,"));
    }
}
