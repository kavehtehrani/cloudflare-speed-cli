use crate::model::RunResult;
use anyhow::{Context, Result};
use std::sync::mpsc as std_mpsc;
use std::sync::OnceLock;
use std::time::Duration;

use super::state::UiState;

// Global clipboard manager channel - initialized once on first use
static CLIPBOARD_SENDER: OnceLock<std_mpsc::Sender<String>> = OnceLock::new();

/// Enrich RunResult with network information from UiState.
/// This uses the shared enrichment function and then adds TUI-specific state (IP, colo, etc.)
pub fn enrich_result_with_network_info(r: &RunResult, state: &UiState) -> RunResult {
    // Create NetworkInfo from UiState
    let network_info = crate::network::NetworkInfo {
        interface_name: state.network.interface_name.clone(),
        network_name: state.network.network_name.clone(),
        is_wireless: state.network.is_wireless,
        interface_mac: state.network.interface_mac.clone(),
        local_ipv4: state.network.local_ipv4.clone(),
        local_ipv6: state.network.local_ipv6.clone(),
    };

    // Use shared enrichment function
    let mut enriched = crate::network::enrich_result(r, &network_info);

    // Override with TUI state values (which may have been updated from meta)
    enriched.ip = state.network.ip.clone();
    enriched.colo = state.network.colo.clone();
    enriched.asn = state.network.asn.clone();
    enriched.as_org = state.network.as_org.clone();

    // Server might already be set, but update from state if available
    if enriched.server.is_none() {
        enriched.server = state.network.server.clone();
    }
    enriched
}

/// Save JSON to the default auto-save location.
pub fn save_result_json(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::save_run(&enriched)
}

/// Save result and update state.info with the saved path message.
pub fn save_and_show_path(r: &RunResult, state: &mut UiState) {
    match save_result_json(r, state) {
        Ok(path) => {
            // Update last_result to the enriched version that was saved
            // This ensures the path computation matches
            let enriched = enrich_result_with_network_info(r, state);
            state.last_result = Some(enriched);
            // Verify file exists before showing path
            if path.exists() {
                let line = crate::event_format::format_saved_line(&path);
                state.info = line.clone();
                // Mirror text mode: log the same line to the Test Activity panel.
                UiState::push_log_line(&mut state.text_log, line);
            } else {
                state.info = format!("Saved (verifying): {}", path.display());
            }
        }
        Err(e) => {
            state.info = format!("Save failed: {e:#}");
        }
    }
}

/// Export JSON to a user-specified file location.
/// Returns the absolute path of the exported file.
pub fn export_result_json(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.json",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );

    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::export_json(&path, &enriched)?;
    Ok(path)
}

/// Export CSV to a user-specified file location.
/// Returns the absolute path of the exported file.
pub fn export_result_csv(r: &RunResult, state: &UiState) -> Result<std::path::PathBuf> {
    // Generate a default filename based on timestamp
    let default_name = format!(
        "cloudflare-speed-{}-{}.csv",
        r.timestamp_utc.replace(':', "-").replace('T', "_"),
        &r.meas_id[..8.min(r.meas_id.len())]
    );

    // Get absolute path from current directory
    let current_dir = std::env::current_dir().context("get current directory")?;
    let path = current_dir.join(default_name);
    let enriched = enrich_result_with_network_info(r, state);
    crate::storage::export_csv(&path, &enriched)?;
    Ok(path)
}

/// Initialize the clipboard manager thread if not already initialized.
/// This creates a background thread that processes clipboard operations sequentially,
/// keeping each clipboard instance alive for a sufficient duration.
fn init_clipboard_manager() -> Result<&'static std_mpsc::Sender<String>> {
    CLIPBOARD_SENDER.get_or_init(|| {
        let (tx, rx) = std_mpsc::channel::<String>();

        // Spawn a dedicated thread to manage clipboard operations
        std::thread::spawn(move || {
            #[cfg(not(target_os = "android"))]
            use arboard::Clipboard;

            #[cfg(not(target_os = "android"))]
            for text in rx {
                if let Ok(mut clipboard) = Clipboard::new() {
                    if clipboard.set_text(&text).is_ok() {
                        std::thread::sleep(Duration::from_secs(2));
                    }
                }
            }

            #[cfg(target_os = "android")]
            for _ in rx {
                // Clipboard not supported on Android.
             }
        }
        );

        tx
    });

    CLIPBOARD_SENDER
        .get()
        .ok_or_else(|| anyhow::anyhow!("Failed to initialize clipboard manager"))
}

/// Copy text to clipboard.
/// Uses a background thread manager to keep clipboard instances alive for a sufficient duration
/// to ensure clipboard managers have time to read the contents on Linux.
/// Returns immediately after queuing the clipboard operation, without blocking the main thread.
pub fn copy_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "android")]
    {
        let _ = text;
        return Err(anyhow::anyhow!(
            "Clipboard is not supported on Android"
        ));
    }

    #[cfg(not(target_os = "android"))]
    {
        let sender = init_clipboard_manager()?;
        sender
            .send(text.to_string())
            .map_err(|_| anyhow::anyhow!("Clipboard manager channel closed"))?;
        Ok(())
    }
}
