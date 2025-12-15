use crate::checkpoint::storage::CheckpointStorage;
use crate::error::{MeiliBridgeError, Result};
use crate::models::{Checkpoint, Position};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

/// Commands for checkpoint management
#[derive(Debug)]
pub enum CheckpointCommand {
    Save {
        task_id: String,
        position: Position,
    },
    Load {
        task_id: String,
        response: mpsc::Sender<Option<Checkpoint>>,
    },
    Delete {
        task_id: String,
    },
    List {
        response: mpsc::Sender<Vec<Checkpoint>>,
    },
    Flush,
}

/// Manages checkpoints for sync tasks
pub struct CheckpointManager {
    storage: Arc<dyn CheckpointStorage>,
    command_tx: Option<mpsc::Sender<CheckpointCommand>>,
    pending_checkpoints: Arc<RwLock<HashMap<String, Checkpoint>>>,
    flush_interval: Duration,
    batch_size: usize,
    shutdown_tx: Option<watch::Sender<bool>>,
    task_handle: Option<tokio::task::JoinHandle<()>>,
}

impl CheckpointManager {
    pub fn new(
        storage: Arc<dyn CheckpointStorage>,
        flush_interval: Duration,
        batch_size: usize,
    ) -> Self {
        Self {
            storage,
            command_tx: None,
            pending_checkpoints: Arc::new(RwLock::new(HashMap::new())),
            flush_interval,
            batch_size,
            shutdown_tx: None,
            task_handle: None,
        }
    }

    /// Start the checkpoint manager
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting checkpoint manager");

        // Create command channel
        let (tx, rx) = mpsc::channel(1000);
        self.command_tx = Some(tx);

        // Create shutdown channel
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        self.shutdown_tx = Some(shutdown_tx);

        // Start background processor
        let storage = self.storage.clone();
        let pending = self.pending_checkpoints.clone();
        let flush_interval = self.flush_interval;
        let batch_size = self.batch_size;

        let handle = tokio::spawn(async move {
            Self::process_commands(
                rx,
                storage,
                pending,
                flush_interval,
                batch_size,
                shutdown_rx,
            )
            .await;
        });

        self.task_handle = Some(handle);

        info!("Checkpoint manager started");
        Ok(())
    }

    /// Stop the checkpoint manager
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping checkpoint manager");

        // Flush pending checkpoints
        if let Some(tx) = &self.command_tx {
            let _ = tx.send(CheckpointCommand::Flush).await;
        }

        // Send shutdown signal
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(true);
        }

        // Wait for task to complete
        if let Some(handle) = self.task_handle.take() {
            let _ = handle.await;
        }

        info!("Checkpoint manager stopped");
        Ok(())
    }

    /// Save a checkpoint
    pub async fn save_checkpoint(&self, task_id: String, position: Position) -> Result<()> {
        if let Some(tx) = &self.command_tx {
            tx.send(CheckpointCommand::Save { task_id, position })
                .await
                .map_err(|_| MeiliBridgeError::ChannelSend)?;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Checkpoint manager not started".to_string(),
            ))
        }
    }

    /// Clean up old checkpoints based on retention policy
    pub async fn cleanup_checkpoints(
        &self,
        active_task_ids: Vec<String>,
        max_checkpoints_per_task: usize,
    ) -> Result<()> {
        info!("Starting checkpoint cleanup");

        // Get all checkpoints
        let checkpoints = self.list_checkpoints().await?;

        // Group checkpoints by task
        let mut checkpoints_by_task: std::collections::HashMap<String, Vec<Checkpoint>> =
            std::collections::HashMap::new();

        for checkpoint in checkpoints {
            checkpoints_by_task
                .entry(checkpoint.task_id.clone())
                .or_default()
                .push(checkpoint);
        }

        let mut removed_count = 0;

        // Clean up orphaned checkpoints (tasks that no longer exist)
        for (task_id, checkpoints) in &checkpoints_by_task {
            if !active_task_ids.contains(task_id) {
                info!(
                    "Removing {} orphaned checkpoints for inactive task '{}'",
                    checkpoints.len(),
                    task_id
                );
                for _ in checkpoints {
                    if let Err(e) = self.delete_checkpoint(task_id).await {
                        warn!("Failed to delete checkpoint for task '{}': {}", task_id, e);
                    } else {
                        removed_count += 1;
                    }
                }
            }
        }

        // Keep only the most recent N checkpoints per active task
        for task_id in &active_task_ids {
            if let Some(checkpoints) = checkpoints_by_task.get_mut(task_id) {
                // Sort by creation time (newest first)
                checkpoints.sort_by(|a, b| b.created_at.cmp(&a.created_at));

                // Remove old checkpoints beyond the limit
                if checkpoints.len() > max_checkpoints_per_task {
                    let to_remove = checkpoints.split_off(max_checkpoints_per_task);
                    info!(
                        "Removing {} old checkpoints for task '{}'",
                        to_remove.len(),
                        task_id
                    );

                    for checkpoint in to_remove {
                        if let Err(e) = self.delete_checkpoint(&checkpoint.task_id).await {
                            warn!("Failed to delete old checkpoint: {}", e);
                        } else {
                            removed_count += 1;
                        }
                    }
                }
            }
        }

        info!(
            "Checkpoint cleanup completed. Removed {} checkpoints",
            removed_count
        );
        Ok(())
    }

    /// Load a checkpoint
    pub async fn load_checkpoint(&self, task_id: &str) -> Result<Option<Checkpoint>> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            tx.send(CheckpointCommand::Load {
                task_id: task_id.to_string(),
                response: resp_tx,
            })
            .await
            .map_err(|_| MeiliBridgeError::ChannelSend)?;

            resp_rx.recv().await.ok_or(MeiliBridgeError::ChannelReceive)
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Checkpoint manager not started".to_string(),
            ))
        }
    }

    /// Delete a checkpoint
    pub async fn delete_checkpoint(&self, task_id: &str) -> Result<()> {
        if let Some(tx) = &self.command_tx {
            tx.send(CheckpointCommand::Delete {
                task_id: task_id.to_string(),
            })
            .await
            .map_err(|_| MeiliBridgeError::ChannelSend)?;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Checkpoint manager not started".to_string(),
            ))
        }
    }

    /// List all checkpoints
    pub async fn list_checkpoints(&self) -> Result<Vec<Checkpoint>> {
        if let Some(tx) = &self.command_tx {
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            tx.send(CheckpointCommand::List { response: resp_tx })
                .await
                .map_err(|_| MeiliBridgeError::ChannelSend)?;

            resp_rx.recv().await.ok_or(MeiliBridgeError::ChannelReceive)
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Checkpoint manager not started".to_string(),
            ))
        }
    }

    /// Process commands in the background
    async fn process_commands(
        mut rx: mpsc::Receiver<CheckpointCommand>,
        storage: Arc<dyn CheckpointStorage>,
        pending: Arc<RwLock<HashMap<String, Checkpoint>>>,
        flush_interval: Duration,
        batch_size: usize,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut flush_timer = interval(flush_interval);
        flush_timer.reset();

        loop {
            tokio::select! {
                Some(cmd) = rx.recv() => {
                    match cmd {
                        CheckpointCommand::Save { task_id, position } => {
                            let checkpoint = Checkpoint {
                                id: uuid::Uuid::new_v4().to_string(),
                                task_id: task_id.clone(),
                                position,
                                created_at: chrono::Utc::now(),
                                stats: crate::models::ProgressStats::new(),
                                metadata: serde_json::Value::Null,
                            };

                            let mut pending_map = pending.write().await;
                            pending_map.insert(task_id, checkpoint);

                            // Flush if batch size reached
                            if pending_map.len() >= batch_size {
                                drop(pending_map);
                                Self::flush_pending(&storage, &pending).await;
                            }
                        }

                        CheckpointCommand::Load { task_id, response } => {
                            // Check pending first
                            let pending_map = pending.read().await;
                            if let Some(checkpoint) = pending_map.get(&task_id) {
                                let _ = response.send(Some(checkpoint.clone())).await;
                            } else {
                                drop(pending_map);
                                // Load from storage
                                match storage.load(&task_id).await {
                                    Ok(checkpoint) => {
                                        let _ = response.send(checkpoint).await;
                                    }
                                    Err(e) => {
                                        error!("Failed to load checkpoint for '{}': {}", task_id, e);
                                        let _ = response.send(None).await;
                                    }
                                }
                            }
                        }

                        CheckpointCommand::Delete { task_id } => {
                            // Remove from pending
                            let mut pending_map = pending.write().await;
                            pending_map.remove(&task_id);
                            drop(pending_map);

                            // Delete from storage
                            if let Err(e) = storage.delete(&task_id).await {
                                error!("Failed to delete checkpoint for '{}': {}", task_id, e);
                            }
                        }

                        CheckpointCommand::List { response } => {
                            // Get all checkpoints (pending + stored)
                            let mut all_checkpoints = Vec::new();

                            // Add pending checkpoints
                            let pending_map = pending.read().await;
                            all_checkpoints.extend(pending_map.values().cloned());
                            drop(pending_map);

                            // Add stored checkpoints
                            match storage.list().await {
                                Ok(stored) => {
                                    for checkpoint in stored {
                                        if !all_checkpoints.iter().any(|c| c.task_id == checkpoint.task_id) {
                                            all_checkpoints.push(checkpoint);
                                        }
                                    }
                                }
                                Err(e) => {
                                    error!("Failed to list checkpoints from storage: {}", e);
                                }
                            }

                            let _ = response.send(all_checkpoints).await;
                        }

                        CheckpointCommand::Flush => {
                            Self::flush_pending(&storage, &pending).await;
                        }
                    }
                }

                _ = flush_timer.tick() => {
                    Self::flush_pending(&storage, &pending).await;
                }

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down checkpoint manager");
                        // Final flush
                        Self::flush_pending(&storage, &pending).await;
                        break;
                    }
                }
            }
        }
    }

    /// Flush pending checkpoints to storage
    async fn flush_pending(
        storage: &Arc<dyn CheckpointStorage>,
        pending: &Arc<RwLock<HashMap<String, Checkpoint>>>,
    ) {
        let mut pending_map = pending.write().await;

        if pending_map.is_empty() {
            return;
        }

        debug!("Flushing {} pending checkpoints", pending_map.len());

        let checkpoints: Vec<Checkpoint> = pending_map.values().cloned().collect();
        let mut failed_tasks = Vec::new();

        for checkpoint in checkpoints {
            match storage.save(&checkpoint).await {
                Ok(_) => {
                    debug!("Saved checkpoint for task '{}'", checkpoint.task_id);
                }
                Err(e) => {
                    error!(
                        "Failed to save checkpoint for task '{}': {}",
                        checkpoint.task_id, e
                    );
                    failed_tasks.push(checkpoint.task_id.clone());
                }
            }
        }

        // Remove successfully saved checkpoints
        pending_map.retain(|task_id, _| failed_tasks.contains(task_id));

        if !failed_tasks.is_empty() {
            warn!(
                "{} checkpoints failed to save and will be retried",
                failed_tasks.len()
            );
        }
    }
}

/// Helper functions for working with position types
impl Position {
    /// Compare two positions (for the same source type)
    pub fn is_after(&self, other: &Position) -> bool {
        match (self, other) {
            (Position::PostgreSQL { lsn: lsn1 }, Position::PostgreSQL { lsn: lsn2 }) => {
                // Parse LSN format (e.g., "0/1234567")
                let parts1: Vec<&str> = lsn1.split('/').collect();
                let parts2: Vec<&str> = lsn2.split('/').collect();

                if parts1.len() == 2 && parts2.len() == 2 {
                    let (hi1, lo1) = (
                        u64::from_str_radix(parts1[0], 16).unwrap_or(0),
                        u64::from_str_radix(parts1[1], 16).unwrap_or(0),
                    );
                    let (hi2, lo2) = (
                        u64::from_str_radix(parts2[0], 16).unwrap_or(0),
                        u64::from_str_radix(parts2[1], 16).unwrap_or(0),
                    );

                    hi1 > hi2 || (hi1 == hi2 && lo1 > lo2)
                } else {
                    false
                }
            }
            (
                Position::MySQL {
                    file: f1,
                    position: p1,
                },
                Position::MySQL {
                    file: f2,
                    position: p2,
                },
            ) => f1 > f2 || (f1 == f2 && p1 > p2),
            _ => false, // Different types or unsupported comparison
        }
    }
}
