use crate::config::RedisConfig;
use crate::error::{MeiliBridgeError, Result};
use crate::models::Checkpoint;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info};

/// Trait for checkpoint storage backends
#[async_trait]
pub trait CheckpointStorage: Send + Sync {
    /// Save a checkpoint
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()>;

    /// Load a checkpoint for a task
    async fn load(&self, task_id: &str) -> Result<Option<Checkpoint>>;

    /// Delete a checkpoint
    async fn delete(&self, task_id: &str) -> Result<()>;

    /// List all checkpoints
    async fn list(&self) -> Result<Vec<Checkpoint>>;

    /// Check if storage is healthy
    async fn is_healthy(&self) -> bool;
}

/// In-memory checkpoint storage (for testing and single-instance deployments)
pub struct MemoryStorage {
    checkpoints: Arc<RwLock<HashMap<String, Checkpoint>>>,
}

impl MemoryStorage {
    pub fn new() -> Self {
        Self {
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
        }
    }
}

#[async_trait]
impl CheckpointStorage for MemoryStorage {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.insert(checkpoint.task_id.clone(), checkpoint.clone());
        debug!("Saved checkpoint for task '{}'", checkpoint.task_id);
        Ok(())
    }

    async fn load(&self, task_id: &str) -> Result<Option<Checkpoint>> {
        let checkpoints = self.checkpoints.read().await;
        Ok(checkpoints.get(task_id).cloned())
    }

    async fn delete(&self, task_id: &str) -> Result<()> {
        let mut checkpoints = self.checkpoints.write().await;
        checkpoints.remove(task_id);
        debug!("Deleted checkpoint for task '{}'", task_id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Checkpoint>> {
        let checkpoints = self.checkpoints.read().await;
        Ok(checkpoints.values().cloned().collect())
    }

    async fn is_healthy(&self) -> bool {
        true
    }
}

impl Default for MemoryStorage {
    fn default() -> Self {
        Self::new()
    }
}

/// Redis-based checkpoint storage (for distributed deployments)
pub struct RedisStorage {
    config: RedisConfig,
    client: Option<redis::Client>,
    key_prefix: String,
}

impl RedisStorage {
    pub fn new(config: RedisConfig) -> Self {
        let key_prefix = format!("{}:checkpoint:", config.key_prefix);
        Self {
            config,
            client: None,
            key_prefix,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        let client = redis::Client::open(self.config.url.as_str())
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to create Redis client: {e}")))?;

        // Test connection
        let mut con = client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to connect to Redis: {e}")))?;

        redis::cmd("PING")
            .query_async::<String>(&mut con)
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Redis ping failed: {e}")))?;

        self.client = Some(client);
        info!("Connected to Redis for checkpoint storage");
        Ok(())
    }

    fn make_key(&self, task_id: &str) -> String {
        format!("{}{}", self.key_prefix, task_id)
    }

    async fn get_connection(&self) -> Result<redis::aio::MultiplexedConnection> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| MeiliBridgeError::Config("Redis client not initialized".to_string()))?;

        client
            .get_multiplexed_async_connection()
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to get Redis connection: {e}")))
    }
}

#[async_trait]
impl CheckpointStorage for RedisStorage {
    async fn save(&self, checkpoint: &Checkpoint) -> Result<()> {
        let mut con = self.get_connection().await?;
        let key = self.make_key(&checkpoint.task_id);

        // Serialize checkpoint
        let value = serde_json::to_string(checkpoint)?;

        // Save with TTL (7 days)
        let ttl = 604800;
        redis::cmd("SET")
            .arg(&key)
            .arg(&value)
            .arg("EX")
            .arg(ttl)
            .query_async::<()>(&mut con)
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to save checkpoint: {e}")))?;

        debug!(
            "Saved checkpoint for task '{}' to Redis",
            checkpoint.task_id
        );
        Ok(())
    }

    async fn load(&self, task_id: &str) -> Result<Option<Checkpoint>> {
        let mut con = self.get_connection().await?;
        let key = self.make_key(task_id);

        let value: Option<String> = redis::cmd("GET")
            .arg(&key)
            .query_async(&mut con)
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to load checkpoint: {e}")))?;

        match value {
            Some(json) => {
                let checkpoint = serde_json::from_str(&json)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    async fn delete(&self, task_id: &str) -> Result<()> {
        let mut con = self.get_connection().await?;
        let key = self.make_key(task_id);

        redis::cmd("DEL")
            .arg(&key)
            .query_async::<()>(&mut con)
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to delete checkpoint: {e}")))?;

        debug!("Deleted checkpoint for task '{}' from Redis", task_id);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Checkpoint>> {
        let mut con = self.get_connection().await?;
        let pattern = format!("{}*", self.key_prefix);

        // Get all keys matching the pattern
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg(&pattern)
            .query_async(&mut con)
            .await
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to list checkpoints: {e}")))?;

        let mut checkpoints = Vec::new();

        // Load each checkpoint
        for key in keys {
            let value: Option<String> = redis::cmd("GET")
                .arg(&key)
                .query_async(&mut con)
                .await
                .map_err(|e| MeiliBridgeError::Config(format!("Failed to load checkpoint: {e}")))?;

            if let Some(json) = value {
                match serde_json::from_str::<Checkpoint>(&json) {
                    Ok(checkpoint) => checkpoints.push(checkpoint),
                    Err(e) => error!("Failed to deserialize checkpoint from key '{}': {}", key, e),
                }
            }
        }

        Ok(checkpoints)
    }

    async fn is_healthy(&self) -> bool {
        if let Ok(mut con) = self.get_connection().await {
            redis::cmd("PING")
                .query_async::<String>(&mut con)
                .await
                .is_ok()
        } else {
            false
        }
    }
}
