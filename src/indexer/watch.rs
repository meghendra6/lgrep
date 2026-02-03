// SPDX-License-Identifier: MIT OR Apache-2.0

//! File watcher for incremental index updates with debouncing

use anyhow::Result;
use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher, Event};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::{Duration, Instant};
use colored::Colorize;

use crate::indexer::IndexBuilder;

/// Default debounce interval in seconds
const DEFAULT_DEBOUNCE_SECS: u64 = 2;

/// Minimum time between reindex operations
const MIN_REINDEX_INTERVAL_SECS: u64 = 5;

/// File system watcher with debouncing
pub struct Watcher {
    root: PathBuf,
    debounce_duration: Duration,
    min_reindex_interval: Duration,
}

impl Watcher {
    pub fn new(root: impl AsRef<Path>) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            debounce_duration: Duration::from_secs(DEFAULT_DEBOUNCE_SECS),
            min_reindex_interval: Duration::from_secs(MIN_REINDEX_INTERVAL_SECS),
        }
    }

    /// Create watcher with custom debounce interval
    pub fn with_debounce(root: impl AsRef<Path>, debounce_secs: u64) -> Self {
        Self {
            root: root.as_ref().to_path_buf(),
            debounce_duration: Duration::from_secs(debounce_secs),
            min_reindex_interval: Duration::from_secs(MIN_REINDEX_INTERVAL_SECS.max(debounce_secs)),
        }
    }

    /// Start watching for file changes with debouncing
    pub fn watch(&self) -> Result<()> {
        let (tx, rx) = channel();

        let config = NotifyConfig::default()
            .with_poll_interval(Duration::from_secs(2));

        let mut watcher = RecommendedWatcher::new(tx, config)?;
        watcher.watch(&self.root, RecursiveMode::Recursive)?;

        println!("{} Watching {} for changes...", "👁".cyan(), self.root.display());
        println!("  Debounce: {}s, Min interval: {}s", 
                 self.debounce_duration.as_secs(),
                 self.min_reindex_interval.as_secs());
        println!("Press Ctrl+C to stop\n");

        let builder = IndexBuilder::new(&self.root)?;
        
        // Track pending changes and last reindex time
        let mut pending_paths: HashSet<PathBuf> = HashSet::new();
        let mut last_event_time: Option<Instant> = None;
        let mut last_reindex_time: Option<Instant> = None;

        loop {
            // Use timeout to implement debouncing
            let timeout = if pending_paths.is_empty() {
                Duration::from_secs(60) // Long timeout when idle
            } else {
                self.debounce_duration
            };

            match rx.recv_timeout(timeout) {
                Ok(Ok(event)) => {
                    if should_reindex(&event) {
                        // Collect changed paths
                        for path in &event.paths {
                            // Skip .cgrep directory
                            if path.to_string_lossy().contains(".cgrep") {
                                continue;
                            }
                            pending_paths.insert(path.clone());
                        }
                        last_event_time = Some(Instant::now());
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("{} Watch error: {}", "✗".red(), e);
                }
                Err(RecvTimeoutError::Timeout) => {
                    // Check if we should flush pending changes
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break;
                }
            }

            // Check if we should trigger reindex
            if !pending_paths.is_empty() {
                let should_reindex = if let Some(last_event) = last_event_time {
                    // Debounce: wait for debounce_duration since last event
                    last_event.elapsed() >= self.debounce_duration
                } else {
                    false
                };

                let can_reindex = if let Some(last_reindex) = last_reindex_time {
                    // Rate limit: ensure minimum interval between reindexes
                    last_reindex.elapsed() >= self.min_reindex_interval
                } else {
                    true
                };

                if should_reindex && can_reindex {
                    let num_changes = pending_paths.len();
                    println!("{} {} file(s) changed, reindexing...", "🔄".yellow(), num_changes);
                    
                    // Clear pending before reindex to capture new events during reindex
                    pending_paths.clear();
                    last_event_time = None;
                    
                    let start = Instant::now();
                    if let Err(e) = builder.build(false) {
                        eprintln!("{} Reindex failed: {}", "✗".red(), e);
                    } else {
                        println!("{} Reindex complete in {:.1}s", "✓".green(), start.elapsed().as_secs_f64());
                    }
                    
                    last_reindex_time = Some(Instant::now());
                }
            }
        }

        Ok(())
    }
}

/// Check if event should trigger reindex
fn should_reindex(event: &Event) -> bool {
    use notify::EventKind::*;
    matches!(event.kind, Create(_) | Modify(_) | Remove(_))
}

/// Run the watch command
pub fn run(path: Option<&str>, debounce_secs: Option<u64>) -> Result<()> {
    let root = path.map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(|| anyhow::anyhow!("Cannot determine current directory"))?;

    // Build initial index
    let builder = IndexBuilder::new(&root)?;
    builder.build(false)?;

    // Start watching with optional custom debounce
    let watcher = match debounce_secs {
        Some(secs) => Watcher::with_debounce(&root, secs),
        None => Watcher::new(&root),
    };
    watcher.watch()
}
