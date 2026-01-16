//! Post-run processing utilities.
//!
//! Handles enrichment, auto-save, exports, and history refresh after a run completes.

use crate::cli::Cli;
use crate::model::RunResult;
use crate::network::{self, NetworkInfo};
use crate::storage;

/// Result of post-run processing, ready for presentation layers.
pub(crate) struct ProcessedRun {
    pub enriched: RunResult,
    pub export_messages: Vec<String>,
    pub history: Vec<RunResult>,
    pub auto_saved_path: Option<std::path::PathBuf>,
}

/// Process a completed run: enrich with network info, auto-save, export, and reload history.
pub(crate) fn process_run_completion(
    args: &Cli,
    network_info: &NetworkInfo,
    history_load: usize,
    auto_save: bool,
    run: &RunResult,
) -> ProcessedRun {
    let mut enriched = network::enrich_result(run, network_info);

    if let Some(meta) = run.meta.as_ref() {
        let extracted = network::extract_metadata(meta);
        enriched.ip = extracted.ip;
        enriched.colo = extracted.colo;
        enriched.asn = extracted.asn;
        enriched.as_org = extracted.as_org;
    }

    if enriched.server.is_none() {
        enriched.server = run.server.clone();
    }

    let auto_saved_path = if auto_save {
        storage::save_run(&enriched).ok()
    } else {
        None
    };

    let mut export_messages = Vec::new();
    if let Some(export_path) = args.export_json.as_deref() {
        match storage::export_json(export_path, &enriched) {
            Ok(_) => export_messages.push(format!("Exported JSON: {}", export_path.display())),
            Err(e) => export_messages.push(format!("Export JSON failed: {e:#}")),
        }
    }
    if let Some(export_path) = args.export_csv.as_deref() {
        match storage::export_csv(export_path, &enriched) {
            Ok(_) => export_messages.push(format!("Exported CSV: {}", export_path.display())),
            Err(e) => export_messages.push(format!("Export CSV failed: {e:#}")),
        }
    }

    let history = storage::load_recent(history_load).unwrap_or_default();

    ProcessedRun {
        enriched,
        export_messages,
        history,
        auto_saved_path,
    }
}
