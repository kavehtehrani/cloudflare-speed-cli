//! Run lifecycle controller.
//!
//! Owns start/stop/restart orchestration and emits events for presentation layers.

use crate::cli::{build_config, Cli};
use crate::engine::{EngineControl, TestEngine};
use crate::model::{InfoEvent, RunConfig, RunResult, TestEvent};
use anyhow::Result;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::time::Duration;

/// Commands emitted by UI layers to control the running test.
#[derive(Debug, Clone)]
pub(crate) enum UiCommand {
    Pause(bool),
    Restart,
    Quit,
}

/// Internal handle for a running test task.
struct RunCtx {
    ctrl_tx: UnboundedSender<EngineControl>,
    handle: Option<tokio::task::JoinHandle<Result<RunResult>>>,
}

/// Spawn a new test run and return its control handle.
fn start_run(args: &Cli, event_tx: UnboundedSender<TestEvent>) -> RunCtx {
    let cfg: RunConfig = build_config(args);
    let (ctrl_tx, ctrl_rx) = tokio::sync::mpsc::unbounded_channel::<EngineControl>();
    let engine = TestEngine::new(cfg);
    let handle = tokio::spawn(async move { engine.run(event_tx, ctrl_rx).await });
    RunCtx {
        ctrl_tx,
        handle: Some(handle),
    }
}

/// Orchestrate test runs based on UI commands and emit events back to presentation layers.
pub(crate) async fn run_controller(
    args: &Cli,
    event_tx: UnboundedSender<TestEvent>,
    mut cmd_rx: UnboundedReceiver<UiCommand>,
) -> Result<()> {
    let mut run_ctx = if args.test_on_launch {
        Some(start_run(args, event_tx.clone()))
    } else {
        None
    };
    let mut restart_pending = false;
    let mut quit_pending = false;
    // Cancel watchdog: if a cancel takes too long, emit a status message to keep UI feedback alive.
    let mut cancel_deadline: Option<tokio::time::Instant> = None;
    let mut watchdog = tokio::time::interval(Duration::from_millis(500));

    let res = loop {
        tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(UiCommand::Pause(p)) => {
                        if let Some(ctx) = &run_ctx {
                            let _ = ctx.ctrl_tx.send(EngineControl::Pause(p));
                        }
                    }
                    Some(UiCommand::Restart) => {
                        // Restart is serialized: cancel the active run first, then start a new one
                        // once we observe completion. This avoids overlapping test runs.
                        restart_pending = true;
                        if let Some(ctx) = &run_ctx {
                            let _ = ctx.ctrl_tx.send(EngineControl::Cancel);
                            let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(
                                "Cancelling…".into(),
                            )));
                            cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                        } else {
                            run_ctx = Some(start_run(args, event_tx.clone()));
                            restart_pending = false;
                            let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(
                                "Restarting…".into(),
                            )));
                        }
                    }
                    Some(UiCommand::Quit) => {
                        // Quit waits for the current run to finish so we can cleanly finalize UI state.
                        quit_pending = true;
                        if let Some(ctx) = &run_ctx {
                            let _ = ctx.ctrl_tx.send(EngineControl::Cancel);
                            let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(
                                "Cancelling…".into(),
                            )));
                            cancel_deadline = Some(tokio::time::Instant::now() + Duration::from_secs(3));
                        } else {
                            break Ok(());
                        }
                    }
                    None => {
                        quit_pending = true;
                        if let Some(ctx) = &run_ctx {
                            let _ = ctx.ctrl_tx.send(EngineControl::Cancel);
                        } else {
                            break Ok(());
                        }
                    }
                }
            }
            // Do not take the JoinHandle before this branch wins; otherwise it can be dropped
            // if another select branch is chosen, and we'll never observe completion.
            maybe_done = async {
                if let Some(ctx) = &mut run_ctx {
                    if let Some(h) = ctx.handle.as_mut() {
                        return Some(h.await);
                    }
                }
                futures::future::pending().await
            } => {
                if let Some(join_res) = maybe_done {
                    if let Some(ctx) = &mut run_ctx {
                        ctx.handle.take();
                    }
                    match join_res {
                        Ok(Ok(r)) => {
                            let _ = event_tx.send(TestEvent::RunCompleted { result: Box::new(r) });
                        }
                        Ok(Err(e)) => {
                            let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(format!(
                                "Run failed: {e:#}"
                            ))));
                        }
                        Err(e) => {
                            let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(format!(
                                "Run join failed: {e}"
                            ))));
                        }
                    }
                    run_ctx = None;
                    cancel_deadline = None;
                    if quit_pending {
                        break Ok(());
                    }
                    if restart_pending {
                        run_ctx = Some(start_run(args, event_tx.clone()));
                        restart_pending = false;
                    }
                }
            }
            // If cancel stalls (e.g., network op in flight), keep the user informed.
            _ = watchdog.tick() => {
                if let Some(deadline) = cancel_deadline {
                    if tokio::time::Instant::now() >= deadline && run_ctx.is_some() {
                        let _ = event_tx.send(TestEvent::Info(InfoEvent::Message(
                            "Still cancelling…".into(),
                        )));
                        cancel_deadline = None;
                    }
                }
            }
        }
    };

    res
}
