use crate::error::Result;
use crate::models::{Position, stream_event::Event};
use async_trait::async_trait;
use serde_json::Value;
use std::collections::HashMap;

/// Result of a sync operation
#[derive(Debug, Clone)]
pub struct SyncResult {
    /// Number of documents successfully synced
    pub success_count: usize,
    /// Number of documents that failed to sync
    pub failed_count: usize,
    /// Last position processed
    pub last_position: Option<Position>,
    /// Errors encountered during sync
    pub errors: Vec<String>,
}

impl SyncResult {
    pub fn new() -> Self {
        Self {
            success_count: 0,
            failed_count: 0,
            last_position: None,
            errors: Vec::new(),
        }
    }

    pub fn add_success(&mut self) {
        self.success_count += 1;
    }

    pub fn add_failure(&mut self, error: String) {
        self.failed_count += 1;
        self.errors.push(error);
    }

    pub fn is_successful(&self) -> bool {
        self.failed_count == 0
    }
}

/// Trait for destination adapters (Meilisearch, Elasticsearch, etc.)
#[async_trait]
pub trait DestinationAdapter: Send + Sync {
    /// Connect to the destination system
    async fn connect(&mut self) -> Result<()>;

    /// Process a batch of events
    async fn process_events(&mut self, events: Vec<Event>) -> Result<SyncResult>;

    /// Create or update an index
    async fn ensure_index(
        &mut self,
        index_name: &str,
        schema: Option<HashMap<String, Value>>,
    ) -> Result<()>;

    /// Perform a full data import
    async fn import_data(
        &mut self,
        index_name: &str,
        documents: Vec<Value>,
        primary_key: Option<&str>,
    ) -> Result<SyncResult>;

    /// Swap indexes atomically (for zero-downtime updates)
    async fn swap_indexes(&mut self, from: &str, to: &str) -> Result<()>;

    /// Delete an index
    async fn delete_index(&mut self, index_name: &str) -> Result<()>;

    /// Get index statistics
    async fn get_index_stats(&self, index_name: &str) -> Result<Value>;

    /// Check if the destination is healthy
    async fn health_check(&self) -> Result<bool>;

    /// Check if the destination is healthy (alias for health_check)
    async fn is_healthy(&self) -> bool {
        self.health_check().await.unwrap_or(false)
    }

    /// Disconnect from the destination
    async fn disconnect(&mut self) -> Result<()>;
}

impl Default for SyncResult {
    fn default() -> Self {
        Self::new()
    }
}
