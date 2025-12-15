use crate::dlq::{DeadLetterEntry, DlqStatistics, DlqStorage};
use crate::error::{MeiliBridgeError, Result};
use chrono::{DateTime, Utc};
use redis::{AsyncCommands, Client};
use std::collections::HashMap;
use tracing::info;

/// Redis-based dead letter queue storage
pub struct RedisDlqStorage {
    client: Client,
    key_prefix: String,
}

impl RedisDlqStorage {
    pub fn new(redis_url: &str, key_prefix: String) -> Result<Self> {
        let client = Client::open(redis_url).map_err(|e| {
            MeiliBridgeError::Configuration(format!("Failed to create Redis client: {e}"))
        })?;

        Ok(Self { client, key_prefix })
    }

    fn entry_key(&self, id: &str) -> String {
        format!("{}:dlq:entry:{}", self.key_prefix, id)
    }

    fn task_index_key(&self, task_id: &str) -> String {
        format!("{}:dlq:task:{}", self.key_prefix, task_id)
    }

    fn all_entries_key(&self) -> String {
        format!("{}:dlq:all", self.key_prefix)
    }

    async fn get_connection(&self) -> Result<redis::aio::MultiplexedConnection> {
        self.client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| MeiliBridgeError::Redis(e.to_string()))
    }
}

#[async_trait::async_trait]
impl DlqStorage for RedisDlqStorage {
    async fn store(&self, entry: DeadLetterEntry) -> Result<()> {
        let mut conn = self.get_connection().await?;

        // Serialize entry
        let entry_data = serde_json::to_string(&entry).map_err(MeiliBridgeError::Serialization)?;

        let entry_key = self.entry_key(&entry.id);
        let task_key = self.task_index_key(&entry.task_id);
        let all_key = self.all_entries_key();

        // Use a transaction to ensure atomicity
        redis::pipe()
            .atomic()
            // Store the entry
            .set(&entry_key, &entry_data)
            // Add to task index with timestamp score
            .zadd(&task_key, &entry.id, entry.created_at.timestamp())
            // Add to all entries index
            .zadd(&all_key, &entry.id, entry.created_at.timestamp())
            .query_async::<()>(&mut conn)
            .await
            .map_err(|e| {
                MeiliBridgeError::Redis(format!("Failed to store dead letter entry: {e}"))
            })?;

        // Update metrics
        self.update_dlq_size_metric(&entry.task_id).await?;

        Ok(())
    }

    async fn get_by_task(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<Vec<DeadLetterEntry>> {
        let mut conn = self.get_connection().await?;
        let task_key = self.task_index_key(task_id);

        // Get entry IDs sorted by timestamp (oldest first)
        let limit = limit.unwrap_or(1000);
        let entry_ids: Vec<String> = conn
            .zrange(&task_key, 0, (limit - 1) as isize)
            .await
            .map_err(|e| MeiliBridgeError::Redis(format!("Failed to get task entries: {e}")))?;

        if entry_ids.is_empty() {
            return Ok(vec![]);
        }

        // Get all entries
        let mut entries = Vec::new();
        for id in &entry_ids {
            if let Some(entry) = self.get(id).await? {
                entries.push(entry);
            }
        }

        Ok(entries)
    }

    async fn get(&self, id: &str) -> Result<Option<DeadLetterEntry>> {
        let mut conn = self.get_connection().await?;
        let entry_key = self.entry_key(id);

        let entry_data: Option<String> = conn
            .get(&entry_key)
            .await
            .map_err(|e| MeiliBridgeError::Redis(format!("Failed to get entry: {e}")))?;

        match entry_data {
            Some(data) => {
                let entry = serde_json::from_str(&data).map_err(MeiliBridgeError::Serialization)?;
                Ok(Some(entry))
            }
            None => Ok(None),
        }
    }

    async fn remove(&self, id: &str) -> Result<()> {
        let mut conn = self.get_connection().await?;

        // Get the entry to find task_id
        if let Some(entry) = self.get(id).await? {
            let entry_key = self.entry_key(id);
            let task_key = self.task_index_key(&entry.task_id);
            let all_key = self.all_entries_key();

            // Remove from all indexes and delete entry
            redis::pipe()
                .atomic()
                .del(&entry_key)
                .zrem(&task_key, id)
                .zrem(&all_key, id)
                .query_async::<()>(&mut conn)
                .await
                .map_err(|e| MeiliBridgeError::Redis(format!("Failed to remove entry: {e}")))?;

            // Update metrics
            self.update_dlq_size_metric(&entry.task_id).await?;
        }

        Ok(())
    }

    async fn update(&self, entry: DeadLetterEntry) -> Result<()> {
        // For Redis, update is the same as store
        self.store(entry).await
    }

    async fn get_statistics(&self) -> Result<DlqStatistics> {
        let mut conn = self.get_connection().await?;
        let all_key = self.all_entries_key();

        // Get all entry IDs
        let entry_ids: Vec<String> = conn
            .zrange(&all_key, 0, -1)
            .await
            .map_err(|e| MeiliBridgeError::Redis(format!("Failed to get all entries: {e}")))?;

        let mut entries_by_task = HashMap::new();
        let mut entries_by_error = HashMap::new();
        let mut oldest: Option<DateTime<Utc>> = None;
        let mut newest: Option<DateTime<Utc>> = None;

        // Get all entries and compute statistics
        for id in &entry_ids {
            if let Some(entry) = self.get(id).await? {
                // Count by task
                *entries_by_task.entry(entry.task_id.clone()).or_insert(0) += 1;

                // Count by error type
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
        }

        Ok(DlqStatistics {
            total_entries: entry_ids.len(),
            entries_by_task,
            entries_by_error,
            oldest_entry: oldest,
            newest_entry: newest,
        })
    }

    async fn clear_task(&self, task_id: &str) -> Result<usize> {
        let mut conn = self.get_connection().await?;
        let task_key = self.task_index_key(task_id);

        // Get all entry IDs for the task
        let entry_ids: Vec<String> = conn
            .zrange(&task_key, 0, -1)
            .await
            .map_err(|e| MeiliBridgeError::Redis(format!("Failed to get task entries: {e}")))?;

        let count = entry_ids.len();

        if count > 0 {
            // Build pipeline to remove all entries
            let mut pipe = redis::pipe();
            pipe.atomic();

            for id in &entry_ids {
                let entry_key = self.entry_key(id);
                pipe.del(entry_key);
                pipe.zrem(self.all_entries_key(), id);
            }

            // Clear the task index
            pipe.del(&task_key);

            pipe.query_async::<()>(&mut conn).await.map_err(|e| {
                MeiliBridgeError::Redis(format!("Failed to clear task entries: {e}"))
            })?;

            info!(
                "Cleared {} dead letter entries for task: {}",
                count, task_id
            );

            // Update metrics
            crate::metrics::DEAD_LETTER_QUEUE_SIZE
                .with_label_values(&[task_id])
                .set(0.0);
        }

        Ok(count)
    }
}

impl RedisDlqStorage {
    async fn update_dlq_size_metric(&self, task_id: &str) -> Result<()> {
        let mut conn = self.get_connection().await?;
        let task_key = self.task_index_key(task_id);

        let size: usize = conn
            .zcard(&task_key)
            .await
            .map_err(|e| MeiliBridgeError::Redis(format!("Failed to get task queue size: {e}")))?;

        crate::metrics::DEAD_LETTER_QUEUE_SIZE
            .with_label_values(&[task_id])
            .set(size as f64);

        Ok(())
    }
}
