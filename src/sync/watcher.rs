//! File system watcher — auto-sync when files change.
//!
//! Uses `notify` crate to watch enabled knowledge roots.
//! Debounced: waits for a quiet period before triggering sync.

use anyhow::Result;
use notify::{Event, EventKind, RecursiveMode, Watcher};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Start file watchers for all enabled knowledge roots.
/// Each watcher runs in its own background thread.
/// `on_change` is called after `debounce_ms` of silence.
pub fn watch_all(debounce_ms: u64, on_change: impl Fn() + Send + 'static + Clone) -> Result<()> {
    let bundle_root =
        crate::config::bundle::detect_bundle_root().unwrap_or_else(|_| PathBuf::from("/"));
    let config = crate::config::bundle::load_config(&bundle_root).unwrap_or_default();

    for root in &config.knowledge.roots {
        if !root.enabled {
            continue;
        }

        let root_path = if PathBuf::from(&root.path).is_absolute() {
            PathBuf::from(&root.path)
        } else {
            bundle_root.join(root.path.trim_start_matches("./"))
        };

        let (tx, rx) = mpsc::channel();
        let debounce = Duration::from_millis(debounce_ms);
        let cb = on_change.clone();
        let root_name = root.name.clone();

        let mut watcher = notify::recommended_watcher(move |res: Result<Event, notify::Error>| {
            if let Ok(event) = res {
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                        let _ = tx.send(());
                    }
                    _ => {}
                }
            }
        })?;

        watcher.watch(&root_path, RecursiveMode::Recursive)?;

        tracing::info!(
            root = %root_name,
            path = %root_path.display(),
            debounce_ms = debounce_ms,
            "File watcher started"
        );

        // Spawn debounce thread — keeps watcher alive via move
        std::thread::spawn(move || {
            let _watcher = watcher; // keep alive
            let mut last_event = Instant::now();
            let mut pending = false;

            loop {
                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(()) => {
                        last_event = Instant::now();
                        pending = true;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        if pending && last_event.elapsed() >= debounce {
                            pending = false;
                            tracing::debug!(
                                "File change detected, triggering auto-sync for '{}'",
                                root_name
                            );
                            cb();
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    }

    Ok(())
}
