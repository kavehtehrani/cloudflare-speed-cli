use std::time::Duration;

const GITHUB_RELEASE_URL: &str =
    "https://api.github.com/repos/kavehtehrani/cloudflare-speed-cli/releases/latest";
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Check GitHub for a newer release.
/// Returns Some(Some(version)) if update available, Some(None) if on latest.
/// Returns None on any error (network, parse, timeout, etc.) - fails silently.
pub async fn check_for_update() -> Option<Option<String>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let resp = client
        .get(GITHUB_RELEASE_URL)
        .header("User-Agent", "cloudflare-speed-cli")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .ok()?;

    let json: serde_json::Value = resp.json().await.ok()?;
    let tag = json.get("tag_name")?.as_str()?;
    let latest = tag.trim_start_matches('v');

    if is_newer(latest, CURRENT_VERSION) {
        Some(Some(latest.to_string()))
    } else {
        Some(None)
    }
}

/// Simple semver comparison (major.minor.patch)
fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> (u32, u32, u32) {
        let parts: Vec<u32> = s.split('.').filter_map(|p| p.parse().ok()).collect();
        (
            parts.first().copied().unwrap_or(0),
            parts.get(1).copied().unwrap_or(0),
            parts.get(2).copied().unwrap_or(0),
        )
    };
    parse(latest) > parse(current)
}
