//! Transaction-based checkpointing for at-least-once delivery

use crate::checkpoint::storage::CheckpointStorage;
use crate::error::{MeiliBridgeError, Result};
use crate::models::{Checkpoint, Position};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

/// Represents a checkpoint transaction
#[derive(Debug, Clone)]
pub struct CheckpointTransaction {
    pub transaction_id: String,
    pub task_id: String,
    pub position: Position,
    pub prepared: bool,
    pub committed: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// Manages transactional checkpoints for at-least-once delivery
pub struct TransactionalCheckpoint {
    storage: Arc<dyn CheckpointStorage>,
    transactions: Arc<Mutex<Vec<CheckpointTransaction>>>,
}

impl TransactionalCheckpoint {
    pub fn new(storage: Arc<dyn CheckpointStorage>) -> Self {
        Self {
            storage,
            transactions: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Begin a checkpoint transaction
    pub async fn begin_transaction(
        &self,
        transaction_id: String,
        task_id: String,
        position: Position,
    ) -> Result<()> {
        let mut txns = self.transactions.lock().await;

        // Check if transaction already exists
        if txns.iter().any(|t| t.transaction_id == transaction_id) {
            return Err(MeiliBridgeError::Pipeline(format!(
                "Transaction {} already exists",
                transaction_id
            )));
        }

        let transaction = CheckpointTransaction {
            transaction_id: transaction_id.clone(),
            task_id,
            position,
            prepared: false,
            committed: false,
            created_at: chrono::Utc::now(),
        };

        txns.push(transaction);
        info!("Started checkpoint transaction: {}", transaction_id);

        Ok(())
    }

    /// Prepare phase - save checkpoint atomically BEFORE Meilisearch write
    pub async fn prepare(&self, transaction_id: &str) -> Result<bool> {
        let mut txns = self.transactions.lock().await;

        let txn = txns
            .iter_mut()
            .find(|t| t.transaction_id == transaction_id)
            .ok_or_else(|| {
                MeiliBridgeError::Pipeline(format!("Transaction {} not found", transaction_id))
            })?;

        if txn.prepared {
            debug!("Transaction {} already prepared", transaction_id);
            return Ok(true);
        }

        // Create checkpoint with transaction metadata
        let checkpoint = Checkpoint {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: txn.task_id.clone(),
            position: txn.position.clone(),
            created_at: chrono::Utc::now(),
            stats: crate::models::ProgressStats::new(),
            metadata: serde_json::json!({
                "transaction_id": transaction_id,
                "prepared": true,
                "committed": false,
            }),
        };

        // Save checkpoint atomically
        match self.storage.save(&checkpoint).await {
            Ok(_) => {
                txn.prepared = true;
                info!("Prepared checkpoint transaction: {}", transaction_id);
                Ok(true)
            }
            Err(e) => {
                error!(
                    "Failed to prepare checkpoint transaction {}: {}",
                    transaction_id, e
                );
                Ok(false)
            }
        }
    }

    /// Commit phase - mark checkpoint as committed after successful Meilisearch write
    pub async fn commit(&self, transaction_id: &str) -> Result<()> {
        let mut txns = self.transactions.lock().await;

        let txn_index = txns
            .iter()
            .position(|t| t.transaction_id == transaction_id)
            .ok_or_else(|| {
                MeiliBridgeError::Pipeline(format!("Transaction {} not found", transaction_id))
            })?;

        let txn = &mut txns[txn_index];

        if !txn.prepared {
            return Err(MeiliBridgeError::Pipeline(format!(
                "Transaction {} not prepared",
                transaction_id
            )));
        }

        if txn.committed {
            debug!("Transaction {} already committed", transaction_id);
            return Ok(());
        }

        // Load the checkpoint
        let checkpoint = self.storage.load(&txn.task_id).await?.ok_or_else(|| {
            MeiliBridgeError::Pipeline(format!("Checkpoint not found for task {}", txn.task_id))
        })?;

        // Update checkpoint metadata to mark as committed
        let mut updated_checkpoint = checkpoint;
        updated_checkpoint.metadata = serde_json::json!({
            "transaction_id": transaction_id,
            "prepared": true,
            "committed": true,
            "committed_at": chrono::Utc::now(),
        });

        // Save updated checkpoint
        self.storage.save(&updated_checkpoint).await?;

        // Remove transaction from active list
        txns.remove(txn_index);

        info!("Committed checkpoint transaction: {}", transaction_id);
        Ok(())
    }

    /// Rollback a transaction
    pub async fn rollback(&self, transaction_id: &str) -> Result<()> {
        let mut txns = self.transactions.lock().await;

        let txn_index = txns
            .iter()
            .position(|t| t.transaction_id == transaction_id)
            .ok_or_else(|| {
                MeiliBridgeError::Pipeline(format!("Transaction {} not found", transaction_id))
            })?;

        let txn = &txns[txn_index];

        if txn.prepared && !txn.committed {
            // Delete the prepared checkpoint
            if let Err(e) = self.storage.delete(&txn.task_id).await {
                error!("Failed to delete checkpoint during rollback: {}", e);
            }
        }

        // Remove transaction from active list
        txns.remove(txn_index);

        info!("Rolled back checkpoint transaction: {}", transaction_id);
        Ok(())
    }

    /// Clean up stale transactions
    pub async fn cleanup_stale_transactions(&self, timeout_secs: u64) -> Result<()> {
        let mut txns = self.transactions.lock().await;
        let now = chrono::Utc::now();
        let timeout = chrono::Duration::seconds(timeout_secs as i64);

        let stale_txns: Vec<String> = txns
            .iter()
            .filter(|t| now - t.created_at > timeout)
            .map(|t| t.transaction_id.clone())
            .collect();

        for txn_id in stale_txns {
            if let Some(index) = txns.iter().position(|t| t.transaction_id == txn_id) {
                let txn = &txns[index];
                error!("Cleaning up stale transaction: {}", txn_id);

                // Rollback if prepared but not committed
                if txn.prepared && !txn.committed {
                    if let Err(e) = self.storage.delete(&txn.task_id).await {
                        error!("Failed to cleanup stale checkpoint: {}", e);
                    }
                }

                txns.remove(index);
            }
        }

        Ok(())
    }

    /// Get the latest committed checkpoint for a task
    pub async fn get_latest_committed(&self, task_id: &str) -> Result<Option<Checkpoint>> {
        let checkpoint = self.storage.load(task_id).await?;

        if let Some(cp) = checkpoint {
            // Check if checkpoint is committed
            if let Some(committed) = cp.metadata.get("committed").and_then(|v| v.as_bool())
                && committed
            {
                return Ok(Some(cp));
            }
        }

        Ok(None)
    }
}
