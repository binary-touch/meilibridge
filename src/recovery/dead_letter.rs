use crate::error::{MeiliBridgeError, Result};
use crate::models::stream_event::Event;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tokio::io::AsyncWriteExt;
use tokio::sync::{RwLock, mpsc};
use tracing::{debug, error, info, warn};

/// Dead letter entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeadLetterEntry {
    pub id: String,
    pub task_id: String,
    pub event: Event,
    pub error: String,
    pub error_count: u32,
    pub first_error_time: DateTime<Utc>,
    pub last_error_time: DateTime<Utc>,
    pub metadata: serde_json::Value,
}

impl DeadLetterEntry {
    pub fn new(task_id: String, event: Event, error: String) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id,
            event,
            error,
            error_count: 1,
            first_error_time: now,
            last_error_time: now,
            metadata: serde_json::Value::Null,
        }
    }

    pub fn increment_error(&mut self, error: String) {
        self.error = error;
        self.error_count += 1;
        self.last_error_time = Utc::now();
    }
}

/// Storage backend for dead letter queue
#[async_trait]
pub trait DeadLetterStorage: Send + Sync {
    /// Store a dead letter entry
    async fn store(&self, entry: &DeadLetterEntry) -> Result<()>;

    /// Retrieve entries for a task
    async fn get_by_task(&self, task_id: &str, limit: usize) -> Result<Vec<DeadLetterEntry>>;

    /// Retrieve a specific entry
    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>>;

    /// Delete an entry
    async fn delete(&self, id: &str) -> Result<()>;

    /// Count entries for a task
    async fn count_by_task(&self, task_id: &str) -> Result<usize>;

    /// List all tasks with dead letters
    async fn list_tasks(&self) -> Result<Vec<String>>;
}

/// In-memory dead letter storage
pub struct MemoryDeadLetterStorage {
    entries: Arc<RwLock<VecDeque<DeadLetterEntry>>>,
    max_entries: usize,
}

impl MemoryDeadLetterStorage {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Arc::new(RwLock::new(VecDeque::new())),
            max_entries,
        }
    }
}

#[async_trait]
impl DeadLetterStorage for MemoryDeadLetterStorage {
    async fn store(&self, entry: &DeadLetterEntry) -> Result<()> {
        let mut entries = self.entries.write().await;

        // Check if we need to evict old entries
        while entries.len() >= self.max_entries {
            if let Some(evicted) = entries.pop_front() {
                warn!(
                    "Evicting dead letter entry {} due to size limit",
                    evicted.id
                );
            }
        }

        entries.push_back(entry.clone());
        debug!(
            "Stored dead letter entry {} for task {}",
            entry.id, entry.task_id
        );
        Ok(())
    }

    async fn get_by_task(&self, task_id: &str, limit: usize) -> Result<Vec<DeadLetterEntry>> {
        let entries = self.entries.read().await;
        let result: Vec<DeadLetterEntry> = entries
            .iter()
            .filter(|e| e.task_id == task_id)
            .take(limit)
            .cloned()
            .collect();
        Ok(result)
    }

    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>> {
        let entries = self.entries.read().await;
        Ok(entries.iter().find(|e| e.id == id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<()> {
        let mut entries = self.entries.write().await;
        entries.retain(|e| e.id != id);
        debug!("Deleted dead letter entry {}", id);
        Ok(())
    }

    async fn count_by_task(&self, task_id: &str) -> Result<usize> {
        let entries = self.entries.read().await;
        Ok(entries.iter().filter(|e| e.task_id == task_id).count())
    }

    async fn list_tasks(&self) -> Result<Vec<String>> {
        let entries = self.entries.read().await;
        let mut tasks: Vec<String> = entries.iter().map(|e| e.task_id.clone()).collect();
        tasks.sort();
        tasks.dedup();
        Ok(tasks)
    }
}

/// File-based dead letter storage
pub struct FileDeadLetterStorage {
    base_path: PathBuf,
}

impl FileDeadLetterStorage {
    pub async fn new(base_path: PathBuf) -> Result<Self> {
        // Ensure directory exists
        fs::create_dir_all(&base_path)
            .await
            .map_err(MeiliBridgeError::Io)?;

        Ok(Self { base_path })
    }

    fn get_task_dir(&self, task_id: &str) -> PathBuf {
        self.base_path.join(task_id)
    }

    fn get_entry_path(&self, task_id: &str, entry_id: &str) -> PathBuf {
        self.get_task_dir(task_id).join(format!("{entry_id}.json"))
    }
}

#[async_trait]
impl DeadLetterStorage for FileDeadLetterStorage {
    async fn store(&self, entry: &DeadLetterEntry) -> Result<()> {
        let task_dir = self.get_task_dir(&entry.task_id);
        fs::create_dir_all(&task_dir)
            .await
            .map_err(MeiliBridgeError::Io)?;

        let path = self.get_entry_path(&entry.task_id, &entry.id);
        let json = serde_json::to_string_pretty(entry)?;

        let mut file = fs::File::create(&path)
            .await
            .map_err(MeiliBridgeError::Io)?;
        file.write_all(json.as_bytes())
            .await
            .map_err(MeiliBridgeError::Io)?;

        debug!("Stored dead letter entry {} to {:?}", entry.id, path);
        Ok(())
    }

    async fn get_by_task(&self, task_id: &str, limit: usize) -> Result<Vec<DeadLetterEntry>> {
        let task_dir = self.get_task_dir(task_id);

        if !task_dir.exists() {
            return Ok(Vec::new());
        }

        let mut entries = Vec::new();
        let mut dir = fs::read_dir(&task_dir)
            .await
            .map_err(MeiliBridgeError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(MeiliBridgeError::Io)? {
            if entries.len() >= limit {
                break;
            }

            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                let content = fs::read_to_string(&path)
                    .await
                    .map_err(MeiliBridgeError::Io)?;

                match serde_json::from_str::<DeadLetterEntry>(&content) {
                    Ok(dead_letter) => entries.push(dead_letter),
                    Err(e) => error!("Failed to parse dead letter file {:?}: {}", path, e),
                }
            }
        }

        Ok(entries)
    }

    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>> {
        // We need to search all task directories
        let mut dir = fs::read_dir(&self.base_path)
            .await
            .map_err(MeiliBridgeError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(MeiliBridgeError::Io)? {
            if entry
                .file_type()
                .await
                .map_err(MeiliBridgeError::Io)?
                .is_dir()
            {
                let task_id = entry.file_name();
                let path = self.get_entry_path(task_id.to_str().unwrap_or_default(), id);

                if path.exists() {
                    let content = fs::read_to_string(&path)
                        .await
                        .map_err(MeiliBridgeError::Io)?;
                    let dead_letter = serde_json::from_str(&content)?;
                    return Ok(Some(dead_letter));
                }
            }
        }

        Ok(None)
    }

    async fn delete(&self, id: &str) -> Result<()> {
        // Find and delete the entry
        if let Some(entry) = self.get(id).await? {
            let path = self.get_entry_path(&entry.task_id, id);
            fs::remove_file(&path).await.map_err(MeiliBridgeError::Io)?;
            debug!("Deleted dead letter entry {} from {:?}", id, path);
        }
        Ok(())
    }

    async fn count_by_task(&self, task_id: &str) -> Result<usize> {
        let task_dir = self.get_task_dir(task_id);

        if !task_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        let mut dir = fs::read_dir(&task_dir)
            .await
            .map_err(MeiliBridgeError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(MeiliBridgeError::Io)? {
            if entry.path().extension().is_some_and(|ext| ext == "json") {
                count += 1;
            }
        }

        Ok(count)
    }

    async fn list_tasks(&self) -> Result<Vec<String>> {
        let mut tasks = Vec::new();
        let mut dir = fs::read_dir(&self.base_path)
            .await
            .map_err(MeiliBridgeError::Io)?;

        while let Some(entry) = dir.next_entry().await.map_err(MeiliBridgeError::Io)? {
            if entry
                .file_type()
                .await
                .map_err(MeiliBridgeError::Io)?
                .is_dir()
            {
                if let Some(name) = entry.file_name().to_str() {
                    tasks.push(name.to_string());
                }
            }
        }

        Ok(tasks)
    }
}

/// Dead letter queue manager
pub struct DeadLetterQueue {
    storage: Arc<dyn DeadLetterStorage>,
    _max_retries: u32,
    tx: Option<mpsc::Sender<DeadLetterEntry>>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl DeadLetterQueue {
    pub fn new(storage: Arc<dyn DeadLetterStorage>, max_retries: u32) -> Self {
        Self {
            storage,
            _max_retries: max_retries,
            tx: None,
            handle: None,
        }
    }

    /// Start the dead letter queue processor
    pub fn start(&mut self) -> mpsc::Receiver<DeadLetterEntry> {
        let (tx, rx) = mpsc::channel(1000);
        self.tx = Some(tx);

        let storage = self.storage.clone();
        let handle = tokio::spawn(async move {
            Self::process_entries(storage, rx).await;
        });

        self.handle = Some(handle);

        // Return a receiver for reprocessing entries
        let (_reprocess_tx, reprocess_rx) = mpsc::channel(100);
        reprocess_rx
    }

    /// Send an event to the dead letter queue
    pub async fn send(&self, task_id: String, event: Event, error: String) -> Result<()> {
        if let Some(tx) = &self.tx {
            let entry = DeadLetterEntry::new(task_id, event, error);
            tx.send(entry)
                .await
                .map_err(|_| MeiliBridgeError::ChannelSend)?;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Dead letter queue not started".to_string(),
            ))
        }
    }

    /// Process dead letter entries
    async fn process_entries(
        storage: Arc<dyn DeadLetterStorage>,
        mut rx: mpsc::Receiver<DeadLetterEntry>,
    ) {
        while let Some(entry) = rx.recv().await {
            match storage.store(&entry).await {
                Ok(_) => {
                    info!(
                        "Stored event in dead letter queue for task '{}' (attempt {})",
                        entry.task_id, entry.error_count
                    );
                }
                Err(e) => {
                    error!(
                        "Failed to store dead letter entry for task '{}': {}",
                        entry.task_id, e
                    );
                }
            }
        }
    }

    /// Get statistics for dead letter queue
    pub async fn get_stats(&self) -> Result<Vec<(String, usize)>> {
        let tasks = self.storage.list_tasks().await?;
        let mut stats = Vec::new();

        for task in tasks {
            let count = self.storage.count_by_task(&task).await?;
            stats.push((task, count));
        }

        Ok(stats)
    }

    /// Reprocess entries from dead letter queue
    pub async fn reprocess_task(
        &self,
        task_id: &str,
        limit: usize,
    ) -> Result<Vec<DeadLetterEntry>> {
        let entries = self.storage.get_by_task(task_id, limit).await?;

        // Delete entries that will be reprocessed
        for entry in &entries {
            self.storage.delete(&entry.id).await?;
        }

        info!(
            "Reprocessing {} dead letter entries for task '{}'",
            entries.len(),
            task_id
        );
        Ok(entries)
    }
}
