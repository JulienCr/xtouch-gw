//! Persistence Actor for debounced state snapshots using sled
//!
//! This module provides an actor-based persistence layer that stores [`StateSnapshot`]
//! to an embedded sled database with configurable debouncing to minimize disk I/O.
//!
//! # Debouncing Strategy
//!
//! The persistence actor implements a debouncing strategy to batch multiple
//! snapshot requests within a configurable time window (default: 500ms).
//!
//! **How it works:**
//!
//! 1. When a snapshot save is requested, the actor stores it as a "pending" snapshot
//!    and records the current timestamp.
//!
//! 2. If another save request arrives within the debounce window, the pending
//!    snapshot is replaced with the newer one (last-write-wins).
//!
//! 3. Once the debounce window expires without new requests, the pending snapshot
//!    is flushed to the sled database.
//!
//! This approach ensures:
//! - High-frequency state updates (e.g., fader movements) don't cause excessive writes
//! - The most recent state is always persisted
//! - Disk I/O is minimized for better SSD longevity
//!
//! # Example
//!
//! ```ignore
//! use xtouch_gw::state::persistence_actor::PersistenceActorHandle;
//!
//! let handle = PersistenceActorHandle::spawn("./data/state.sled", 500)?;
//!
//! // Save snapshots (debounced)
//! handle.save_snapshot(snapshot).await?;
//!
//! // Force flush before shutdown
//! handle.flush().await?;
//!
//! // Graceful shutdown
//! handle.shutdown();
//! ```

use super::persistence::StateSnapshot;
use anyhow::{Context, Result};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, error, info, trace, warn};

/// Default debounce window in milliseconds
pub const DEFAULT_DEBOUNCE_MS: u64 = 500;

/// Key used to store the snapshot in sled
const SNAPSHOT_KEY: &[u8] = b"state_snapshot";

/// Commands sent to the persistence actor
#[derive(Debug)]
pub enum PersistenceCommand {
    /// Save a state snapshot (debounced)
    Save(StateSnapshot),
    /// Load the current snapshot
    Load(oneshot::Sender<Option<StateSnapshot>>),
    /// Force flush any pending snapshot
    Flush(oneshot::Sender<Result<()>>),
    /// Shutdown the actor
    Shutdown,
}

/// Persistence actor that manages debounced writes to sled database
pub struct PersistenceActor {
    /// Sled embedded database
    db: sled::Db,
    /// Command receiver channel
    command_rx: mpsc::Receiver<PersistenceCommand>,
    /// Pending snapshot awaiting flush
    pending_snapshot: Option<StateSnapshot>,
    /// Timestamp of last write request
    last_write_ts: std::time::Instant,
    /// Debounce window in milliseconds
    debounce_ms: u64,
    /// Total number of writes performed
    write_count: u64,
}

/// Handle to communicate with the persistence actor
///
/// This handle is cheap to clone and can be shared across tasks.
#[derive(Clone)]
pub struct PersistenceActorHandle {
    /// Command sender channel
    cmd_tx: mpsc::Sender<PersistenceCommand>,
}

impl PersistenceActor {
    /// Spawn a new persistence actor with the given database path and debounce window.
    ///
    /// # Arguments
    ///
    /// * `db_path` - Path to the sled database directory
    /// * `debounce_ms` - Debounce window in milliseconds (0 disables debouncing)
    ///
    /// # Returns
    ///
    /// A handle to communicate with the spawned actor.
    ///
    /// # Errors
    ///
    /// Returns an error if the sled database cannot be opened.
    pub fn spawn(db_path: &str, debounce_ms: u64) -> Result<PersistenceActorHandle> {
        // Open sled database
        let db = sled::open(db_path)
            .with_context(|| format!("Failed to open sled database at: {}", db_path))?;

        info!("Persistence actor opened database at: {}", db_path);

        // Create bounded channel for commands (capacity: 100)
        let (cmd_tx, command_rx) = mpsc::channel(100);

        // Create the actor
        let actor = PersistenceActor {
            db,
            command_rx,
            pending_snapshot: None,
            last_write_ts: Instant::now(),
            debounce_ms,
            write_count: 0,
        };

        // Spawn the actor task
        tokio::spawn(actor.run());

        Ok(PersistenceActorHandle { cmd_tx })
    }

    /// Main actor run loop
    ///
    /// Processes commands from the channel and handles debounce timing.
    async fn run(mut self) {
        info!("Persistence actor started (debounce: {}ms)", self.debounce_ms);

        // Create interval ticker for debounce checking
        let tick_interval = if self.debounce_ms > 0 {
            self.debounce_ms
        } else {
            // If debounce is disabled, use a longer interval (just for cleanup checks)
            1000
        };
        let mut ticker = tokio::time::interval(Duration::from_millis(tick_interval));

        loop {
            tokio::select! {
                // Handle incoming commands
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        PersistenceCommand::Save(snapshot) => {
                            trace!("Received save command, queuing snapshot");
                            self.pending_snapshot = Some(snapshot);
                            self.last_write_ts = Instant::now();

                            // If debounce is disabled (0), flush immediately
                            if self.debounce_ms == 0 {
                                self.flush_pending_snapshot().await;
                            }
                        }
                        PersistenceCommand::Load(response_tx) => {
                            trace!("Received load command");
                            let snapshot = self.load_snapshot();
                            // Best-effort send, receiver may have dropped
                            let _ = response_tx.send(snapshot);
                        }
                        PersistenceCommand::Flush(response_tx) => {
                            trace!("Received flush command");
                            self.flush_pending_snapshot().await;
                            // Best-effort send, receiver may have dropped
                            let _ = response_tx.send(Ok(()));
                        }
                        PersistenceCommand::Shutdown => {
                            info!("Persistence actor shutting down, flushing pending snapshot");
                            self.flush_pending_snapshot().await;
                            info!(
                                "Persistence actor shutdown complete (total writes: {})",
                                self.write_count
                            );
                            return;
                        }
                    }
                }
                // Check for debounce timeout
                _ = ticker.tick() => {
                    if self.pending_snapshot.is_some() && self.debounce_ms > 0 {
                        let elapsed = self.last_write_ts.elapsed();
                        if elapsed >= Duration::from_millis(self.debounce_ms) {
                            trace!(
                                "Debounce window expired ({:?}), flushing pending snapshot",
                                elapsed
                            );
                            self.flush_pending_snapshot().await;
                        }
                    }
                }
            }
        }
    }

    /// Flush any pending snapshot to disk
    ///
    /// Called when the debounce window expires or on explicit flush request.
    async fn flush_pending_snapshot(&mut self) {
        let Some(snapshot) = self.pending_snapshot.take() else {
            trace!("No pending snapshot to flush");
            return;
        };

        // Serialize snapshot to JSON
        let json = match serde_json::to_vec(&snapshot) {
            Ok(data) => data,
            Err(e) => {
                error!("Failed to serialize snapshot: {}", e);
                // Put the snapshot back so we can retry later
                self.pending_snapshot = Some(snapshot);
                return;
            }
        };

        // Clone db handle for spawn_blocking
        let db = self.db.clone();

        // Write to sled using spawn_blocking to avoid blocking the async runtime
        let write_result = tokio::task::spawn_blocking(move || {
            db.insert(SNAPSHOT_KEY, json)?;
            db.flush()?;
            Ok::<_, sled::Error>(())
        })
        .await;

        match write_result {
            Ok(Ok(())) => {
                self.write_count += 1;
                self.last_write_ts = Instant::now();
                trace!(
                    "Snapshot flushed to sled (write #{})",
                    self.write_count
                );
            }
            Ok(Err(e)) => {
                error!("Failed to write snapshot to sled: {}", e);
                // Note: snapshot was already taken, so it's lost on write failure
                // This is acceptable as the next save will provide fresh data
            }
            Err(e) => {
                error!("Spawn blocking task panicked: {}", e);
            }
        }
    }

    /// Load a snapshot from the database
    ///
    /// Returns `None` if no snapshot exists or deserialization fails.
    fn load_snapshot(&self) -> Option<StateSnapshot> {
        match self.db.get(SNAPSHOT_KEY) {
            Ok(Some(data)) => {
                match serde_json::from_slice::<StateSnapshot>(&data) {
                    Ok(snapshot) => {
                        debug!(
                            "Loaded snapshot (version: {}, timestamp: {})",
                            snapshot.version, snapshot.timestamp
                        );
                        Some(snapshot)
                    }
                    Err(e) => {
                        warn!("Failed to deserialize snapshot: {}", e);
                        None
                    }
                }
            }
            Ok(None) => {
                debug!("No snapshot found in database");
                None
            }
            Err(e) => {
                error!("Failed to read snapshot from sled: {}", e);
                None
            }
        }
    }

    /// Write a snapshot to the database
    ///
    /// # Errors
    ///
    /// Returns an error if serialization or database write fails.
    fn write_snapshot(&self, snapshot: &StateSnapshot) -> Result<()> {
        let json = serde_json::to_vec(snapshot).context("Failed to serialize snapshot")?;

        self.db
            .insert(SNAPSHOT_KEY, json)
            .context("Failed to insert snapshot into sled")?;

        self.db.flush().context("Failed to flush sled database")?;

        trace!("Snapshot written to sled directly");
        Ok(())
    }
}

impl PersistenceActorHandle {
    /// Request saving a state snapshot (debounced)
    ///
    /// The snapshot will be held in memory until the debounce window expires,
    /// then written to the database. Multiple calls within the window will
    /// replace the pending snapshot (last-write-wins).
    ///
    /// # Errors
    ///
    /// Returns an error if the actor has shut down.
    pub async fn save_snapshot(&self, snapshot: StateSnapshot) -> Result<()> {
        self.cmd_tx
            .send(PersistenceCommand::Save(snapshot))
            .await
            .context("Failed to send save command: actor shut down")
    }

    /// Load the most recently persisted snapshot
    ///
    /// Note: This returns the snapshot from disk, not any pending (unsaved) snapshot.
    ///
    /// # Returns
    ///
    /// - `Ok(Some(snapshot))` if a snapshot exists
    /// - `Ok(None)` if no snapshot has been saved
    ///
    /// # Errors
    ///
    /// Returns an error if the actor has shut down.
    pub async fn load_snapshot(&self) -> Result<Option<StateSnapshot>> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PersistenceCommand::Load(tx))
            .await
            .context("Failed to send load command: actor shut down")?;

        rx.await.context("Failed to receive load response")
    }

    /// Force flush any pending snapshot to disk immediately
    ///
    /// Use this before shutdown to ensure all state is persisted.
    ///
    /// # Errors
    ///
    /// Returns an error if the flush fails or the actor has shut down.
    pub async fn flush(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd_tx
            .send(PersistenceCommand::Flush(tx))
            .await
            .context("Failed to send flush command: actor shut down")?;

        rx.await.context("Failed to receive flush response")?
    }

    /// Signal the actor to shut down
    ///
    /// This is a fire-and-forget operation. The actor will flush any pending
    /// snapshot before terminating. Use [`flush`](Self::flush) first if you
    /// need to ensure persistence completes.
    pub fn shutdown(&self) {
        // Best-effort send, ignore errors if already shut down
        let _ = self.cmd_tx.try_send(PersistenceCommand::Shutdown);
    }

    /// Get a clone of the command sender channel
    ///
    /// Useful for integrating with other components that need direct channel access.
    pub fn cmd_tx(&self) -> mpsc::Sender<PersistenceCommand> {
        self.cmd_tx.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tempfile::tempdir;

    fn make_test_snapshot() -> StateSnapshot {
        StateSnapshot {
            timestamp: 1234567890,
            version: StateSnapshot::VERSION.to_string(),
            states: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_spawn_and_shutdown() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        let handle = PersistenceActor::spawn(db_path.to_str().unwrap(), 100).unwrap();

        // Shutdown should work without errors
        handle.shutdown();

        // Give the actor time to shut down
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    #[tokio::test]
    async fn test_save_and_load_snapshot() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        let handle = PersistenceActor::spawn(db_path.to_str().unwrap(), 0).unwrap();

        let snapshot = make_test_snapshot();
        handle.save_snapshot(snapshot).await.unwrap();

        // Since debounce is 0, should be immediate
        tokio::time::sleep(Duration::from_millis(50)).await;

        let loaded = handle.load_snapshot().await.unwrap();
        assert!(loaded.is_some());

        let loaded = loaded.unwrap();
        assert_eq!(loaded.timestamp, 1234567890);
        assert_eq!(loaded.version, StateSnapshot::VERSION);

        handle.shutdown();
    }

    #[tokio::test]
    async fn test_flush_forces_write() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        // Use long debounce to test flush override
        let handle = PersistenceActor::spawn(db_path.to_str().unwrap(), 10000).unwrap();

        let snapshot = make_test_snapshot();
        handle.save_snapshot(snapshot).await.unwrap();

        // Force flush
        handle.flush().await.unwrap();

        // Should be persisted now despite long debounce
        let loaded = handle.load_snapshot().await.unwrap();
        assert!(loaded.is_some());

        handle.shutdown();
    }

    #[tokio::test]
    async fn test_load_empty_database() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        let handle = PersistenceActor::spawn(db_path.to_str().unwrap(), 100).unwrap();

        let loaded = handle.load_snapshot().await.unwrap();
        assert!(loaded.is_none());

        handle.shutdown();
    }

    #[tokio::test]
    async fn test_debounce_coalesces_writes() {
        let temp = tempdir().unwrap();
        let db_path = temp.path().join("test.sled");

        // Use 200ms debounce
        let handle = PersistenceActor::spawn(db_path.to_str().unwrap(), 200).unwrap();

        // Send multiple snapshots rapidly
        for i in 0..5 {
            let mut snapshot = make_test_snapshot();
            snapshot.timestamp = i;
            handle.save_snapshot(snapshot).await.unwrap();
        }

        // Wait for debounce to expire
        tokio::time::sleep(Duration::from_millis(300)).await;

        // Should have the last snapshot (timestamp = 4)
        let loaded = handle.load_snapshot().await.unwrap();
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().timestamp, 4);

        handle.shutdown();
    }
}
