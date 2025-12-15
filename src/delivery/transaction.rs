//! Two-phase commit protocol implementation

use crate::error::{MeiliBridgeError, Result};
use crate::models::Position;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::time::{Duration, timeout};
use tracing::{debug, error, info, warn};

/// Transaction state in two-phase commit
#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    /// Transaction started
    Started,
    /// Prepare phase completed
    Prepared,
    /// Transaction committed
    Committed,
    /// Transaction aborted
    Aborted,
}

/// Transaction information
#[derive(Debug, Clone)]
pub struct Transaction {
    pub id: String,
    pub task_id: String,
    pub state: TransactionState,
    pub position: Option<Position>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub participants: Vec<String>,
}

/// Two-phase commit protocol handler
#[derive(Debug)]
pub struct TwoPhaseCommit {
    /// Transaction timeout
    timeout_secs: u64,

    /// Active transactions
    transactions: Arc<RwLock<HashMap<String, Transaction>>>,
}

impl TwoPhaseCommit {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            timeout_secs,
            transactions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Begin a new transaction
    pub async fn begin(&self, transaction_id: String, task_id: String) -> Result<()> {
        let mut txns = self.transactions.write().await;

        if txns.contains_key(&transaction_id) {
            return Err(MeiliBridgeError::Pipeline(format!(
                "Transaction {transaction_id} already exists",
            )));
        }

        let transaction = Transaction {
            id: transaction_id.clone(),
            task_id,
            state: TransactionState::Started,
            position: None,
            created_at: chrono::Utc::now(),
            participants: vec!["checkpoint".to_string(), "meilisearch".to_string()],
        };

        txns.insert(transaction_id.clone(), transaction);
        info!("Started 2PC transaction: {}", transaction_id);

        Ok(())
    }

    /// Prepare phase - all participants vote
    pub async fn prepare(&self, transaction_id: &str, position: Position) -> Result<bool> {
        let mut txns = self.transactions.write().await;

        let txn = txns.get_mut(transaction_id).ok_or_else(|| {
            MeiliBridgeError::Pipeline(format!("Transaction {transaction_id} not found"))
        })?;

        if txn.state != TransactionState::Started {
            return Err(MeiliBridgeError::Pipeline(format!(
                "Transaction {} in invalid state: {:?}",
                transaction_id, txn.state
            )));
        }

        txn.position = Some(position);
        txn.state = TransactionState::Prepared;

        info!("Prepared 2PC transaction: {}", transaction_id);
        Ok(true)
    }

    /// Commit phase - finalize transaction
    pub async fn commit(&self, transaction_id: &str) -> Result<()> {
        let mut txns = self.transactions.write().await;

        let txn = txns.get_mut(transaction_id).ok_or_else(|| {
            MeiliBridgeError::Pipeline(format!("Transaction {transaction_id} not found"))
        })?;

        if txn.state != TransactionState::Prepared {
            return Err(MeiliBridgeError::Pipeline(format!(
                "Transaction {} not prepared: {:?}",
                transaction_id, txn.state
            )));
        }

        txn.state = TransactionState::Committed;
        info!("Committed 2PC transaction: {}", transaction_id);

        // Remove committed transaction after a delay
        let transaction_id = transaction_id.to_string();
        let transactions = self.transactions.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let mut txns = transactions.write().await;
            txns.remove(&transaction_id);
        });

        Ok(())
    }

    /// Abort/rollback transaction
    pub async fn abort(&self, transaction_id: &str) -> Result<()> {
        let mut txns = self.transactions.write().await;

        if let Some(txn) = txns.get_mut(transaction_id) {
            txn.state = TransactionState::Aborted;
            warn!("Aborted 2PC transaction: {}", transaction_id);
        }

        // Remove aborted transaction
        txns.remove(transaction_id);
        Ok(())
    }

    /// Clean up stale transactions
    pub async fn cleanup_stale_transactions(&self) -> Result<()> {
        let mut txns = self.transactions.write().await;
        let now = chrono::Utc::now();
        let timeout = chrono::Duration::seconds(self.timeout_secs as i64);

        let stale_txns: Vec<String> = txns
            .iter()
            .filter(|(_, txn)| {
                now - txn.created_at > timeout && txn.state != TransactionState::Committed
            })
            .map(|(id, _)| id.clone())
            .collect();

        for txn_id in stale_txns {
            error!("Aborting stale transaction: {}", txn_id);
            txns.remove(&txn_id);
        }

        Ok(())
    }
}

/// Coordinates transactions across multiple components
pub struct TransactionCoordinator {
    two_phase_commit: TwoPhaseCommit,
    checkpoint_handler: Arc<RwLock<Option<Arc<super::checkpoint::TransactionalCheckpoint>>>>,
}

impl TransactionCoordinator {
    pub fn new(timeout_secs: u64) -> Self {
        Self {
            two_phase_commit: TwoPhaseCommit::new(timeout_secs),
            checkpoint_handler: Arc::new(RwLock::new(None)),
        }
    }

    /// Set the checkpoint handler
    pub async fn set_checkpoint_handler(
        &self,
        handler: Arc<super::checkpoint::TransactionalCheckpoint>,
    ) {
        let mut checkpoint = self.checkpoint_handler.write().await;
        *checkpoint = Some(handler);
    }

    /// Begin a coordinated transaction
    pub async fn begin(&self, task_id: &str) -> Result<String> {
        let transaction_id = format!("{}-{}", task_id, uuid::Uuid::new_v4());

        // Start 2PC transaction
        self.two_phase_commit
            .begin(transaction_id.clone(), task_id.to_string())
            .await?;

        debug!("Started coordinated transaction: {}", transaction_id);
        Ok(transaction_id)
    }

    /// Prepare phase - coordinate all participants
    pub async fn prepare(&self, transaction_id: &str, position: Position) -> Result<bool> {
        // First, prepare checkpoint
        let checkpoint_handler = self.checkpoint_handler.read().await;
        if let Some(handler) = checkpoint_handler.as_ref() {
            handler
                .begin_transaction(
                    transaction_id.to_string(),
                    transaction_id.split('-').next().unwrap_or("").to_string(),
                    position.clone(),
                )
                .await?;

            // Prepare checkpoint with timeout
            let prepare_future = handler.prepare(transaction_id);
            match timeout(Duration::from_secs(10), prepare_future).await {
                Ok(Ok(true)) => {
                    debug!("Checkpoint prepared for transaction: {}", transaction_id);
                }
                Ok(Ok(false)) => {
                    warn!(
                        "Checkpoint prepare failed for transaction: {}",
                        transaction_id
                    );
                    return Ok(false);
                }
                Ok(Err(e)) => {
                    error!("Checkpoint prepare error: {}", e);
                    return Ok(false);
                }
                Err(_) => {
                    error!("Checkpoint prepare timeout");
                    return Ok(false);
                }
            }
        }

        // Then prepare 2PC
        self.two_phase_commit
            .prepare(transaction_id, position)
            .await?;

        Ok(true)
    }

    /// Commit phase - finalize all participants
    pub async fn commit(&self, transaction_id: &str) -> Result<()> {
        // Commit checkpoint first
        let checkpoint_handler = self.checkpoint_handler.read().await;
        if let Some(handler) = checkpoint_handler.as_ref() {
            handler.commit(transaction_id).await?;
        }

        // Then commit 2PC
        self.two_phase_commit.commit(transaction_id).await?;

        info!("Committed coordinated transaction: {}", transaction_id);
        Ok(())
    }

    /// Rollback transaction
    pub async fn rollback(&self, transaction_id: &str) -> Result<()> {
        // Rollback checkpoint
        let checkpoint_handler = self.checkpoint_handler.read().await;
        if let Some(handler) = checkpoint_handler.as_ref() {
            if let Err(e) = handler.rollback(transaction_id).await {
                error!("Failed to rollback checkpoint: {}", e);
            }
        }

        // Abort 2PC
        self.two_phase_commit.abort(transaction_id).await?;

        warn!("Rolled back coordinated transaction: {}", transaction_id);
        Ok(())
    }

    /// Periodic cleanup task
    pub async fn run_cleanup_task(&self) {
        let mut interval = tokio::time::interval(Duration::from_secs(60));

        loop {
            interval.tick().await;

            if let Err(e) = self.two_phase_commit.cleanup_stale_transactions().await {
                error!("Failed to cleanup stale transactions: {}", e);
            }

            // Also cleanup checkpoint transactions
            let checkpoint_handler = self.checkpoint_handler.read().await;
            if let Some(handler) = checkpoint_handler.as_ref() {
                if let Err(e) = handler
                    .cleanup_stale_transactions(self.two_phase_commit.timeout_secs)
                    .await
                {
                    error!("Failed to cleanup stale checkpoint transactions: {}", e);
                }
            }
        }
    }
}
