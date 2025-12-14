mod redis_storage;

pub use redis_storage::RedisDlqStorage;

use crate::error::{MeiliBridgeError, Result};
use crate::models::event::Event;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tracing::{error, info, warn};

/// Dead letter queue entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    /// Unique identifier for this entry
    pub id: String,

    /// The task ID this entry belongs to
    pub task_id: String,

    /// The original event that failed
    pub event: Event,

    /// The error that caused the failure
    pub error: String,

    /// Number of retry attempts
    pub retry_count: u32,

    /// When this entry was first created
    pub created_at: DateTime<Utc>,

    /// When this entry was last retried
    pub last_retry_at: Option<DateTime<Utc>>,

    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Dead letter queue statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DlqStatistics {
    /// Total number of entries
    pub total_entries: usize,

    /// Number of entries per task
    pub entries_by_task: HashMap<String, usize>,

    /// Number of entries by error type
    pub entries_by_error: HashMap<String, usize>,

    /// Oldest entry timestamp
    pub oldest_entry: Option<DateTime<Utc>>,

    /// Newest entry timestamp
    pub newest_entry: Option<DateTime<Utc>>,
}

/// Dead letter queue storage backend trait
#[async_trait::async_trait]
pub trait DlqStorage: Send + Sync {
    /// Store a new dead letter entry
    async fn store(&self, entry: DeadLetterEntry) -> Result<()>;

    /// Retrieve entries for a specific task
    async fn get_by_task(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<DeadLetterEntry>>;

    /// Retrieve a specific entry
    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>>;

    /// Remove an entry
    async fn remove(&self, id: &str) -> Result<()>;

    /// Update an entry (e.g., after retry)
    async fn update(&self, entry: DeadLetterEntry) -> Result<()>;

    /// Get statistics
    async fn get_statistics(&self) -> Result<DlqStatistics>;

    /// Clear all entries for a task
    async fn clear_task(&self, task_id: &str) -> Result<usize>;
}

/// In-memory dead letter queue storage
pub struct InMemoryDlqStorage {
    entries: Arc<RwLock<HashMap<String, DeadLetterEntry>>>,
}

impl InMemoryDlqStorage {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

impl Default for InMemoryDlqStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl DlqStorage for InMemoryDlqStorage {
    async fn store(&self, entry: DeadLetterEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn get_by_task(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<DeadLetterEntry>> {
        let entries = self.entries.read().await;
        let mut task_entries: Vec<_> = entries
            .values()
            .filter(|e| e.task_id == task_id)
            .cloned()
            .collect();

        // Sort by created_at (oldest first)
        task_entries.sort_by(|a, b| a.created_at.cmp(&b.created_at));

        if let Some(limit) = limit {
            task_entries.truncate(limit);
        }

        Ok(task_entries)
    }

    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>> {
        let entries = self.entries.read().await;
        Ok(entries.get(id).cloned())
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.remove(id);
        Ok(())
    }

    async fn update(&self, entry: DeadLetterEntry) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.insert(entry.id.clone(), entry);
        Ok(())
    }

    async fn get_statistics(&self) -> Result<DlqStatistics> {
        let entries = self.entries.read().await;

        let mut entries_by_task = HashMap::new();
        let mut entries_by_error = HashMap::new();
        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        for entry in entries.values() {
            // Count by task
            *entries_by_task.entry(entry.task_id.clone()).or_insert(0) += 1;

            // Count by error type (extract first line)
            let error_type = entry.error.lines().next().unwrap_or("Unknown").to_string();
            *entries_by_error.entry(error_type).or_insert(0) += 1;

            // Track oldest/newest
            match oldest {
                None => oldest = Some(entry.created_at),
                Some(old) if entry.created_at < old => oldest = Some(entry.created_at),
                _ => {}
            }

            match newest {
                None => newest = Some(entry.created_at),
                Some(new) if entry.created_at > new => newest = Some(entry.created_at),
                _ => {}
            }
        }

        Ok(DlqStatistics {
            total_entries: entries.len(),
            entries_by_task,
            entries_by_error,
            oldest_entry: oldest,
            newest_entry: newest,
        })
    }

    async fn clear_task(&self, task_id: &str) -> Result<usize> {
        let mut entries = self.entries.write().await;
        let before_count = entries.len();
        entries.retain(|_, entry| entry.task_id != task_id);
        let removed_count = before_count - entries.len();
        Ok(removed_count)
    }
}

/// Dead letter queue manager
pub struct DeadLetterQueue {
    storage: Arc<dyn DlqStorage>,
    reprocess_tx: Option<mpsc::Sender<DeadLetterEntry>>,
}

impl DeadLetterQueue {
    pub fn new(storage: Arc<dyn DlqStorage>) -> Self {
        Self {
            storage,
            reprocess_tx: None,
        }
    }

    /// Set the reprocess channel for sending entries back to the pipeline
    pub fn set_reprocess_channel(&mut self, tx: mpsc::Sender<DeadLetterEntry>) {
        self.reprocess_tx = Some(tx);
    }

    /// Add a failed event to the dead letter queue
    pub async fn add_failed_event(
        &self,
        task_id: String,
        event: Event,
        error: MeiliBridgeError,
        retry_count: u32,
    ) -> Result<()> {
        let entry = DeadLetterEntry {
            id: uuid::Uuid::new_v4().to_string(),
            task_id,
            event,
            error: error.to_string(),
            retry_count,
            created_at: Utc::now(),
            last_retry_at: if retry_count > 0 {
                Some(Utc::now())
            } else {
                None
            },
            metadata: HashMap::new(),
        };

        let task_id = entry.task_id.clone();

        warn!(
            "Adding event to dead letter queue. Task: {}, Error: {}, Retries: {}",
            entry.task_id, entry.error, entry.retry_count
        );

        self.storage.store(entry).await?;

        // Update metrics
        crate::metrics::DEAD_LETTER_EVENTS_TOTAL
            .with_label_values(&[&task_id])
            .inc();

        Ok(())
    }

    /// Reprocess dead letter entries for a task
    pub async fn reprocess_entries(&self, task_id: &str, limit: Option<usize>) -> Result<usize> {
        let entries = self.storage.get_by_task(task_id, limit).await?;
        let count = entries.len();

        if count == 0 {
            info!("No dead letter entries found for task: {}", task_id);
            return Ok(0);
        }

        let reprocess_tx = self.reprocess_tx.as_ref().ok_or_else(|| {
            MeiliBridgeError::Configuration("Reprocess channel not configured".to_string())
        })?;

        info!(
            "Reprocessing {} dead letter entries for task: {}",
            count, task_id
        );

        for mut entry in entries {
            // Update retry information
            entry.retry_count += 1;
            entry.last_retry_at = Some(Utc::now());

            // Update the entry in storage
            self.storage.update(entry.clone()).await?;

            // Send for reprocessing
            if let Err(e) = reprocess_tx.send(entry.clone()).await {
                error!("Failed to send entry for reprocessing: {}", e);
                return Err(MeiliBridgeError::Pipeline(format!(
                    "Failed to reprocess entry: {}",
                    e
                )));
            }

            // Remove from dead letter queue after successful send
            self.storage.remove(&entry.id).await?;
        }

        info!("Successfully queued {} entries for reprocessing", count);
        Ok(count)
    }

    /// Get statistics for the dead letter queue
    pub async fn get_statistics(&self) -> Result<DlqStatistics> {
        self.storage.get_statistics().await
    }

    /// Clear all entries for a task
    pub async fn clear_task(&self, task_id: &str) -> Result<usize> {
        let count = self.storage.clear_task(task_id).await?;
        info!(
            "Cleared {} dead letter entries for task: {}",
            count, task_id
        );
        Ok(count)
    }
}
