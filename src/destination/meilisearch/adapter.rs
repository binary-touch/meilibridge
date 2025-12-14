use crate::config::{MeilisearchConfig, PerformanceConfig};
use crate::destination::adapter::{DestinationAdapter, SyncResult};
use crate::destination::meilisearch::{
    batch_processor::BatchProcessor, client::convert_error,
    protected_client::ProtectedMeilisearchClient,
};
use crate::error::{MeiliBridgeError, Result};
use crate::models::stream_event::Event;
use crate::pipeline::{AdaptiveBatchingManager, BatchMetrics};
use async_trait::async_trait;
use meilisearch_sdk::indexes::Index;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::time::Instant;
use tracing::{debug, error, info};

pub struct MeilisearchAdapter {
    config: MeilisearchConfig,
    client: Option<ProtectedMeilisearchClient>,
    batch_processor: BatchProcessor,
    table_primary_keys: HashMap<String, String>,
    table_to_index: HashMap<String, String>,
    adaptive_batching: Option<Arc<AdaptiveBatchingManager>>,
    performance_config: PerformanceConfig,
}

impl MeilisearchAdapter {
    pub fn new(config: MeilisearchConfig, table_to_index: HashMap<String, String>) -> Self {
        let primary_key = config.primary_key.clone();

        // Initialize default primary keys for known tables
        let mut table_primary_keys = HashMap::new();
        table_primary_keys.insert("users".to_string(), "id".to_string());
        table_primary_keys.insert("products".to_string(), "sku".to_string());
        table_primary_keys.insert("orders".to_string(), "id".to_string());

        Self {
            config,
            client: None,
            batch_processor: BatchProcessor::new(primary_key),
            table_primary_keys,
            table_to_index,
            adaptive_batching: None,
            performance_config: PerformanceConfig::default(),
        }
    }

    pub fn with_adaptive_batching(
        mut self,
        manager: Arc<AdaptiveBatchingManager>,
        perf_config: PerformanceConfig,
    ) -> Self {
        self.adaptive_batching = Some(manager);
        self.performance_config = perf_config;
        self
    }

    fn get_client(&self) -> Result<&ProtectedMeilisearchClient> {
        self.client.as_ref().ok_or_else(|| {
            MeiliBridgeError::Meilisearch("Not connected to Meilisearch".to_string())
        })
    }

    fn resolve_index_name(&self, schema: Option<&str>, table: &str, fallback: &str) -> String {
        if let Some(schema) = schema {
            let key = Self::canonical_table_key(Some(schema), table);
            if let Some(index) = self.table_to_index.get(&key) {
                return index.clone();
            }
        }

        let table_key = Self::canonical_table_key(None, table);
        if let Some(index) = self.table_to_index.get(&table_key) {
            return index.clone();
        }

        let (fallback_schema, fallback_table) = Self::split_table_identifier(fallback);
        if let Some(schema) = fallback_schema.as_deref() {
            let fb_key = Self::canonical_table_key(Some(schema), &fallback_table);
            if let Some(index) = self.table_to_index.get(&fb_key) {
                return index.clone();
            }
        }

        let fb_table_key = Self::canonical_table_key(None, &fallback_table);
        if let Some(index) = self.table_to_index.get(&fb_table_key) {
            return index.clone();
        }

        fallback.to_string()
    }

    fn split_table_identifier(table: &str) -> (Option<String>, String) {
        let trimmed = table.trim();
        if let Some((schema, name)) = trimmed.split_once('.') {
            let schema_clean = Self::sanitize_identifier(schema);
            let table_clean = Self::sanitize_identifier(name);
            (Some(schema_clean), table_clean)
        } else {
            (None, Self::sanitize_identifier(trimmed))
        }
    }

    fn sanitize_identifier(identifier: &str) -> String {
        identifier.trim().trim_matches('"').to_string()
    }

    fn canonical_table_key(schema: Option<&str>, table: &str) -> String {
        let table_part = table.trim().trim_matches('"').to_lowercase();
        match schema {
            Some(schema) if !schema.trim().is_empty() => format!(
                "{}.{}",
                schema.trim().trim_matches('"').to_lowercase(),
                table_part
            ),
            _ => table_part,
        }
    }

    async fn get_or_create_index(
        &self,
        index_name: &str,
        primary_key: Option<&str>,
    ) -> Result<Index> {
        let client = self.get_client()?;
        let pk = primary_key.or(self.config.primary_key.as_deref());

        if self.config.auto_create_index {
            client.get_or_create_index(index_name, pk).await
        } else {
            // Try to get index only
            let meilisearch = client.inner().client();
            meilisearch
                .get_index(index_name)
                .await
                .map_err(convert_error)
        }
    }

    async fn apply_batch(
        &mut self,
        index_name: &str,
        primary_key: Option<&str>,
    ) -> Result<SyncResult> {
        let mut result = SyncResult::new();

        if self.batch_processor.is_empty() {
            return Ok(result);
        }

        let client = self.get_client()?;
        let index = self.get_or_create_index(index_name, primary_key).await?;

        // Process upserts
        if !self.batch_processor.documents_to_upsert.is_empty() {
            let doc_count = self.batch_processor.documents_to_upsert.len();
            debug!(
                "Upserting {} documents to index '{}'",
                doc_count, index_name
            );

            match client
                .add_documents(
                    &index,
                    &self.batch_processor.documents_to_upsert,
                    self.config.primary_key.as_deref(),
                )
                .await
            {
                Ok(()) => {
                    result.success_count += doc_count;
                }
                Err(e) => {
                    result.failed_count += doc_count;
                    result.add_failure(format!("Upsert error: {}", e));
                }
            }
        }

        // Process deletes
        if !self.batch_processor.documents_to_delete.is_empty() {
            let delete_count = self.batch_processor.documents_to_delete.len();
            debug!(
                "Deleting {} documents from index '{}'",
                delete_count, index_name
            );

            match client
                .delete_documents(&index, &self.batch_processor.documents_to_delete)
                .await
            {
                Ok(()) => {
                    result.success_count += delete_count;
                }
                Err(e) => {
                    result.failed_count += delete_count;
                    result.add_failure(format!("Delete error: {}", e));
                }
            }
        }

        self.batch_processor.clear();
        Ok(result)
    }
}

#[async_trait]
impl DestinationAdapter for MeilisearchAdapter {
    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Meilisearch...");
        let client = ProtectedMeilisearchClient::new(self.config.clone())?;
        client.test_connection().await?;

        let version = client.get_version().await?;
        info!("Connected to Meilisearch version: {}", version);

        self.client = Some(client);
        Ok(())
    }

    async fn process_events(&mut self, events: Vec<Event>) -> Result<SyncResult> {
        let mut result = SyncResult::new();
        let mut last_position = None;

        // Group events by index derived from the configured task mapping
        let mut events_by_index: HashMap<String, Vec<Event>> = HashMap::new();

        for event in events {
            let index_name = match &event {
                Event::Cdc(cdc) => {
                    self.resolve_index_name(Some(&cdc.schema), &cdc.table, &cdc.table)
                }
                Event::FullSync { table, .. } => {
                    let (schema, table_name) = Self::split_table_identifier(table);
                    self.resolve_index_name(schema.as_deref(), &table_name, table)
                }
                _ => continue,
            };

            // Track position
            if let Event::Cdc(cdc) = &event {
                if let Some(pos) = &cdc.position {
                    last_position = Some(pos.clone());
                }
            }

            events_by_index.entry(index_name).or_default().push(event);
        }

        // Process each index's events
        for (index_name, index_events) in events_by_index {
            // Get primary key for this table
            let primary_key_owned = self
                .table_primary_keys
                .get(&index_name)
                .cloned()
                .or(self.config.primary_key.clone());
            let primary_key_str = primary_key_owned.as_deref();

            // Update batch processor with correct primary key
            self.batch_processor = BatchProcessor::new(primary_key_owned.clone());

            debug!(
                "Processing {} events for index '{}'",
                index_events.len(),
                index_name
            );

            // Process events into batch
            for event in index_events {
                if let Err(e) = self.batch_processor.process_event(event) {
                    error!("Failed to process event: {}", e);
                    result.add_failure(e.to_string());
                }
            }

            // Always apply batch if it has any events
            if !self.batch_processor.is_empty() {
                debug!(
                    "Applying batch with {} operations to index '{}'",
                    self.batch_processor.len(),
                    index_name
                );
                let batch_result = self.apply_batch(&index_name, primary_key_str).await?;
                result.success_count += batch_result.success_count;
                result.failed_count += batch_result.failed_count;
                result.errors.extend(batch_result.errors);
            }
        }

        result.last_position = last_position;
        Ok(result)
    }

    async fn ensure_index(
        &mut self,
        index_name: &str,
        _schema: Option<HashMap<String, Value>>,
    ) -> Result<()> {
        let primary_key = self
            .table_primary_keys
            .get(index_name)
            .map(|s| s.as_str())
            .or(self.config.primary_key.as_deref());
        self.get_or_create_index(index_name, primary_key).await?;
        Ok(())
    }

    async fn import_data(
        &mut self,
        index_name: &str,
        documents: Vec<Value>,
        primary_key: Option<&str>,
    ) -> Result<SyncResult> {
        let mut result = SyncResult::new();
        let client = self.get_client()?;
        let pk = primary_key
            .or(self.table_primary_keys.get(index_name).map(|s| s.as_str()))
            .or(self.config.primary_key.as_deref());
        let index = self.get_or_create_index(index_name, pk).await?;

        info!(
            "Importing {} documents to index '{}'",
            documents.len(),
            index_name
        );

        // Get batch size (adaptive or default)
        let batch_size = if let Some(adaptive) = &self.adaptive_batching {
            adaptive
                .get_batch_size(
                    index_name,
                    self.performance_config.batch_processing.default_batch_size,
                    self.performance_config.batch_processing.min_batch_size,
                    self.performance_config.batch_processing.max_batch_size,
                )
                .await
                .unwrap_or(self.config.batch_size)
        } else {
            self.config.batch_size
        };

        // Import in batches
        for chunk in documents.chunks(batch_size) {
            let start_time = Instant::now();

            match client.add_documents(&index, chunk, primary_key).await {
                Ok(()) => {
                    result.success_count += chunk.len();

                    // Record metrics for adaptive batching
                    if let Some(adaptive) = &self.adaptive_batching {
                        let processing_time_ms = start_time.elapsed().as_millis() as u64;
                        let docs_per_second = if processing_time_ms > 0 {
                            (chunk.len() as f64 * 1000.0) / processing_time_ms as f64
                        } else {
                            chunk.len() as f64 * 1000.0
                        };

                        // Estimate memory usage by serializing the batch
                        let memory_usage_mb = if let Ok(serialized) = serde_json::to_vec(&chunk) {
                            serialized.len() as f64 / (1024.0 * 1024.0)
                        } else {
                            // Fallback estimation: assume average 1KB per document
                            chunk.len() as f64 / 1024.0
                        };

                        let metrics = BatchMetrics {
                            batch_size: chunk.len(),
                            processing_time_ms,
                            documents_per_second: docs_per_second,
                            memory_usage_mb,
                            timestamp: start_time,
                        };

                        let _ = adaptive.record_metrics(index_name, metrics).await;
                    }
                }
                Err(e) => {
                    result.failed_count += chunk.len();
                    result.add_failure(format!("Import error: {}", e));
                }
            }
        }

        Ok(result)
    }

    async fn swap_indexes(&mut self, from: &str, to: &str) -> Result<()> {
        // For now, we'll implement this as a delete and rename operation
        // since the swap_indexes API seems to have changed
        info!("Swapping indexes: '{}' -> '{}'", from, to);

        // First, delete the target index if it exists
        match self.delete_index(to).await {
            Ok(_) => debug!("Deleted existing index '{}'", to),
            Err(_) => debug!("Index '{}' doesn't exist, skipping delete", to),
        }

        // Then rename the source index to target
        // Note: Meilisearch doesn't have a rename operation, so we would need to:
        // 1. Create new index with target name
        // 2. Copy all documents
        // 3. Delete source index
        // For now, we'll return an error indicating this needs implementation

        Err(MeiliBridgeError::Meilisearch(
            "Index swap not fully implemented - requires document copying".to_string(),
        ))
    }

    async fn delete_index(&mut self, index_name: &str) -> Result<()> {
        let client = self.get_client()?;
        let meilisearch = client.inner().client();

        info!("Deleting index '{}'", index_name);

        match meilisearch.delete_index(index_name).await {
            Ok(task) => {
                let task_info = meilisearch
                    .wait_for_task(task, None, None)
                    .await
                    .map_err(convert_error)?;

                if task_info.is_success() {
                    Ok(())
                } else {
                    Err(MeiliBridgeError::Meilisearch(format!(
                        "Index deletion failed: {:?}",
                        task_info
                    )))
                }
            }
            Err(e) => Err(convert_error(e)),
        }
    }

    async fn get_index_stats(&self, index_name: &str) -> Result<Value> {
        let index = self.get_or_create_index(index_name, None).await?;
        let stats = index.get_stats().await.map_err(convert_error)?;

        // Convert stats to JSON manually
        Ok(serde_json::json!({
            "number_of_documents": stats.number_of_documents,
            "is_indexing": stats.is_indexing,
            "field_distribution": stats.field_distribution,
        }))
    }

    async fn health_check(&self) -> Result<bool> {
        if let Some(client) = &self.client {
            client.health_check().await
        } else {
            Ok(false)
        }
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("Disconnecting from Meilisearch");
        self.client = None;
        Ok(())
    }
}
