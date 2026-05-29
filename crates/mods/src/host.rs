//! The QuickJS mod host.
//!
//! QuickJS contexts are single-threaded, so the runtime lives on its own OS
//! thread. The engine talks to it over channels: [`ModRuntime::fire`] pushes
//! [`ModEvent`]s in, and resulting [`ModAction`]s come back out on a Tokio
//! channel the engine drains and applies.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

use rquickjs::{Context, Runtime};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tracing::{error, info, warn};

use crate::event::{ModAction, ModEvent};

const PRELUDE: &str = include_str!("prelude.js");

/// Messages sent to the mod thread.
enum Msg {
    Event(ModEvent),
    Shutdown,
}

/// A handle the engine uses to drive the mod runtime.
#[derive(Clone)]
pub struct ModRuntime {
    tx: Sender<Msg>,
    mods: Vec<String>,
}

impl ModRuntime {
    /// Load every `*.js` file in `dir` (if it exists) and start the runtime.
    /// Returns the handle plus the receiver of actions mods emit.
    pub fn spawn(dir: impl AsRef<Path>) -> (ModRuntime, UnboundedReceiver<ModAction>) {
        let dir = dir.as_ref().to_path_buf();
        let scripts = load_scripts(&dir);
        let names: Vec<String> = scripts.iter().map(|(n, _)| n.clone()).collect();

        let (tx, rx) = mpsc::channel::<Msg>();
        let (action_tx, action_rx) = unbounded_channel::<ModAction>();

        thread::Builder::new()
            .name("cubeplane-mods".into())
            .spawn(move || run_thread(scripts, rx, action_tx))
            .expect("spawn mod thread");

        (ModRuntime { tx, mods: names }, action_rx)
    }

    /// Names of successfully-discovered mod files.
    pub fn loaded(&self) -> &[String] {
        &self.mods
    }

    /// Fire an event into the runtime (non-blocking, best-effort).
    pub fn fire(&self, event: ModEvent) {
        let _ = self.tx.send(Msg::Event(event));
    }

    /// Ask the runtime thread to stop.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
    }
}

fn load_scripts(dir: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "js").unwrap_or(false))
        .collect();
    paths.sort();
    for path in paths {
        match std::fs::read_to_string(&path) {
            Ok(src) => {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("mod.js")
                    .to_string();
                out.push((name, src));
            }
            Err(e) => warn!("could not read mod {}: {e}", path.display()),
        }
    }
    out
}

fn run_thread(scripts: Vec<(String, String)>, rx: mpsc::Receiver<Msg>, actions: UnboundedSender<ModAction>) {
    let runtime = match Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            error!("failed to create QuickJS runtime: {e}");
            return;
        }
    };
    // 64 MiB is plenty for gameplay mods and bounds runaway scripts.
    runtime.set_memory_limit(64 * 1024 * 1024);

    let ctx = match Context::full(&runtime) {
        Ok(c) => c,
        Err(e) => {
            error!("failed to create QuickJS context: {e}");
            return;
        }
    };

    ctx.with(|ctx| {
        if let Err(e) = ctx.eval::<(), _>(PRELUDE) {
            error!("mod prelude failed to load: {e}");
            return;
        }
        for (name, src) in &scripts {
            // Run each mod in an isolated, error-trapped IIFE so one bad mod
            // can't abort loading of the others.
            let wrapped = format!(
                "try {{ (function(){{ 'use strict';\n{src}\n}})(); }} \
                 catch (e) {{ cubeplane.log('[{name}] load error: ' + e); }}"
            );
            if let Err(e) = ctx.eval::<(), _>(wrapped) {
                error!("mod {name} failed to evaluate: {e}");
            }
        }
        // Flush any actions produced during load (mostly log lines).
        drain_into(&ctx, &actions);
    });

    info!("mod runtime ready with {} mod(s)", scripts.len());

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Shutdown => break,
            Msg::Event(event) => {
                ctx.with(|ctx| {
                    dispatch(&ctx, &event);
                    drain_into(&ctx, &actions);
                });
            }
        }
    }
}

/// Run the appropriate JS dispatch for an event.
fn dispatch(ctx: &rquickjs::Ctx, event: &ModEvent) {
    let (name, data) = event.to_js();
    let data_json = serde_json::to_string(&data).unwrap_or_else(|_| "{}".into());
    let code = if event.is_command() {
        format!("__cubeplane.runCommand({data_json});")
    } else {
        let name_lit = serde_json::to_string(name).unwrap();
        format!("__cubeplane.dispatch({name_lit}, {data_json});")
    };
    if let Err(e) = ctx.eval::<(), _>(code) {
        error!("mod dispatch for '{name}' failed: {e}");
    }
}

/// Drain queued actions out of JS and forward them to the engine.
fn drain_into(ctx: &rquickjs::Ctx, actions: &UnboundedSender<ModAction>) {
    let drained: Result<String, _> = ctx.eval("__cubeplane.drain()");
    let json = match drained {
        Ok(s) => s,
        Err(e) => {
            error!("failed to drain mod actions: {e}");
            return;
        }
    };
    let parsed: Vec<ModAction> = serde_json::from_str(&json).unwrap_or_default();
    for action in parsed {
        if matches!(action, ModAction::Unknown) {
            continue;
        }
        let _ = actions.send(action);
    }
}
