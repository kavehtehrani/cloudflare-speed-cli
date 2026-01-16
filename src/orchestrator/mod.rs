//! Application-level orchestration utilities.
//!
//! This module owns run lifecycle control (start/stop/restart) and post-run processing
//! such as enrichment, auto-save, exports, and history refresh. UI/CLI layers call into
//! this module to keep responsibilities separated.

mod controller;
mod post_process;

pub(crate) use controller::{run_controller, UiCommand};
pub(crate) use post_process::process_run_completion;
