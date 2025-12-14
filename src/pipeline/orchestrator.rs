use crate::checkpoint::{CheckpointManager, CheckpointStorage, MemoryStorage, RedisStorage};
use crate::config::{Config, SyncTaskConfig};
use crate::destination::adapter::DestinationAdapter;
use crate::destination::meilisearch::MeilisearchAdapter;
use crate::dlq::{DeadLetterQueue, DlqStorage, InMemoryDlqStorage, RedisDlqStorage};
use crate::error::{MeiliBridgeError, Result};
use crate::metrics;
use crate::models::event::{
    Event as ModelsEvent, EventData as ModelsEventData, EventType as ModelsEventType,
};
use crate::models::{EventId, EventMetadata, EventSource};
use crate::models::{Position, stream_event::Event};
use crate::pipeline::{AdaptiveBatchingManager, MemoryMonitor};
use crate::pipeline::{
    BackpressureConfig, BackpressureEvent, BackpressureManager, CdcCoordinator,
    ParallelTableProcessor, WorkStealingCoordinator, filter::EventFilter, mapper::FieldMapper,
    soft_delete::SoftDeleteHandler, transformer::EventTransformer,
};
use crate::source::adapter::SourceAdapter;
use crate::source::postgres::PostgresAdapter;
use chrono::Utc;
use futures::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc, watch};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, trace, warn};

/// Parameters for batch processing with at-least-once delivery
struct AtLeastOnceBatchParams<'a> {
    destination_tx: &'a mpsc::Sender<(
        DestinationCommand,
        mpsc::Sender<Result<DestinationResponse>>,
    )>,
    checkpoints: &'a Arc<RwLock<HashMap<String, Position>>>,
    table_name: &'a str,
    dead_letter_queue: Option<&'a Arc<DeadLetterQueue>>,
    task_id: &'a str,
    at_least_once_manager: &'a Arc<crate::delivery::AtLeastOnceManager>,
    transactional_checkpoint: &'a Arc<crate::delivery::TransactionalCheckpoint>,
}

/// Parameters for batch processing
struct BatchProcessParams<'a> {
    destination_tx: &'a mpsc::Sender<(
        DestinationCommand,
        mpsc::Sender<Result<DestinationResponse>>,
    )>,
    checkpoints: &'a Arc<RwLock<HashMap<String, Position>>>,
    table_name: &'a str,
    dead_letter_queue: Option<&'a Arc<DeadLetterQueue>>,
    task_id: &'a str,
    at_least_once_manager: Option<&'a Arc<crate::delivery::AtLeastOnceManager>>,
    transactional_checkpoint: Option<&'a Arc<crate::delivery::TransactionalCheckpoint>>,
}

/// Parameters for event stream processing
struct EventStreamParams {
    filter: Option<EventFilter>,
    transformer: Option<EventTransformer>,
    mapper: Option<FieldMapper>,
    soft_delete_handler: Option<SoftDeleteHandler>,
    destination_tx: mpsc::Sender<(
        DestinationCommand,
        mpsc::Sender<Result<DestinationResponse>>,
    )>,
    checkpoints: Arc<RwLock<HashMap<String, Position>>>,
    table_name: String,
    batch_size: usize,
    batch_timeout: Duration,
    dead_letter_queue: Option<Arc<DeadLetterQueue>>,
    task_id: String,
    at_least_once_manager: Option<Arc<crate::delivery::AtLeastOnceManager>>,
    transactional_checkpoint: Option<Arc<crate::delivery::TransactionalCheckpoint>>,
}

/// Convert stream event to models event for dead letter queue
fn convert_to_models_event(event: Event) -> ModelsEvent {
    let (event_type, source, data) = match event {
        Event::Cdc(cdc_event) => {
            let key = cdc_event
                .data
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            (
                cdc_event.event_type,
                EventSource {
                    database: String::new(),
                    schema: cdc_event.schema,
                    table: cdc_event.table,
                },
                ModelsEventData {
                    key,
                    old: None,
                    new: Some(cdc_event.data),
                },
            )
        }
        Event::Insert {
            table,
            new_data,
            position: _,
        } => {
            let key = new_data
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            (
                ModelsEventType::Create,
                EventSource {
                    database: String::new(),
                    schema: "public".to_string(),
                    table: table.clone(),
                },
                ModelsEventData {
                    key,
                    old: None,
                    new: Some(new_data.as_object().unwrap().clone().into_iter().collect()),
                },
            )
        }
        Event::Update {
            table,
            old_data,
            new_data,
            position: _,
        } => {
            let key = new_data
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            (
                ModelsEventType::Update,
                EventSource {
                    database: String::new(),
                    schema: "public".to_string(),
                    table: table.clone(),
                },
                ModelsEventData {
                    key,
                    old: old_data
                        .as_object()
                        .map(|o| o.clone().into_iter().collect()),
                    new: Some(new_data.as_object().unwrap().clone().into_iter().collect()),
                },
            )
        }
        Event::Delete {
            table,
            old_data,
            position: _,
        } => {
            let key = old_data
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            (
                ModelsEventType::Delete,
                EventSource {
                    database: String::new(),
                    schema: "public".to_string(),
                    table: table.clone(),
                },
                ModelsEventData {
                    key,
                    old: Some(old_data.as_object().unwrap().clone().into_iter().collect()),
                    new: None,
                },
            )
        }
        Event::FullSync { table, data } => {
            let key = data.get("id").cloned().unwrap_or(serde_json::Value::Null);
            (
                ModelsEventType::Create,
                EventSource {
                    database: String::new(),
                    schema: "public".to_string(),
                    table: table.clone(),
                },
                ModelsEventData {
                    key,
                    old: None,
                    new: Some(data.as_object().unwrap().clone().into_iter().collect()),
                },
            )
        }
        Event::Checkpoint(_) | Event::Heartbeat => {
            // These are control events, not data events
            // Create a placeholder event for the dead letter queue
            (
                ModelsEventType::Create,
                EventSource {
                    database: String::new(),
                    schema: "system".to_string(),
                    table: "control".to_string(),
                },
                ModelsEventData {
                    key: serde_json::Value::Null,
                    old: None,
                    new: None,
                },
            )
        }
    };

    ModelsEvent {
        id: EventId::new(),
        event_type,
        source,
        data,
        metadata: EventMetadata {
            transaction_id: None,
            position: String::new(),
            custom: HashMap::new(),
        },
        timestamp: Utc::now(),
    }
}

/// Commands for the destination processor
#[derive(Debug, Clone)]
#[allow(dead_code)]
enum DestinationCommand {
    ProcessEvents(Vec<Event>),
    ImportData {
        index: String,
        documents: Vec<Value>,
        primary_key: Option<String>,
    },
    EnsureIndex {
        index_name: String,
        primary_key: Option<String>,
    },
    HealthCheck,
    Shutdown,
}

/// Response from destination processor
#[derive(Debug)]
struct DestinationResponse {
    success_count: usize,
    failed_count: usize,
    last_position: Option<Position>,
    errors: Vec<String>,
}

/// Manages the data pipeline from source to destination
pub struct PipelineOrchestrator {
    pub(crate) config: Config,
    source_adapters: HashMap<String, Box<dyn SourceAdapter>>,
    destination_tx: Option<
        mpsc::Sender<(
            DestinationCommand,
            mpsc::Sender<Result<DestinationResponse>>,
        )>,
    >,
    filters: HashMap<String, EventFilter>,
    transformers: HashMap<String, EventTransformer>,
    mappers: HashMap<String, FieldMapper>,
    soft_delete_handlers: HashMap<String, SoftDeleteHandler>,
    shutdown_tx: Option<watch::Sender<bool>>,
    task_handles: Vec<tokio::task::JoinHandle<()>>,
    checkpoints: Arc<RwLock<HashMap<String, Position>>>,
    checkpoint_manager: Option<Arc<CheckpointManager>>,
    dead_letter_queue: Option<Arc<DeadLetterQueue>>,
    dlq_reprocess_rx: Option<mpsc::Receiver<crate::dlq::DeadLetterEntry>>,
    pub(crate) cdc_coordinator: Option<Arc<RwLock<CdcCoordinator>>>,
    pub(crate) parallel_processors: HashMap<String, Arc<ParallelTableProcessor>>,
    work_stealing_coordinator: Option<WorkStealingCoordinator>,
    // At-least-once delivery components
    at_least_once_manager: Option<Arc<crate::delivery::AtLeastOnceManager>>,
    transactional_checkpoint: Option<Arc<crate::delivery::TransactionalCheckpoint>>,
    // Adaptive batching
    adaptive_batching_manager: Option<Arc<AdaptiveBatchingManager>>,
    // Memory monitoring
    memory_monitor: Option<Arc<MemoryMonitor>>,
    // Backpressure management
    backpressure_manager: Option<Arc<BackpressureManager>>,
}

impl PipelineOrchestrator {
    pub fn new(config: Config) -> Result<Self> {
        let mut source_adapters = HashMap::new();

        // Handle backward compatibility - single source configuration
        if let Some(source_config) = &config.source {
            let source_adapter = Self::create_source_adapter(source_config)?;
            source_adapters.insert("primary".to_string(), source_adapter);
        }

        // Handle multiple sources
        for named_source in &config.sources {
            let source_adapter = Self::create_source_adapter(&named_source.config)?;
            source_adapters.insert(named_source.name.clone(), source_adapter);
        }

        // Validate we have at least one source
        if source_adapters.is_empty() {
            return Err(MeiliBridgeError::Configuration(
                "No data sources configured. Use either 'source' or 'sources' configuration"
                    .to_string(),
            ));
        }

        // Create filters, transformers, mappers, and soft delete handlers for each sync task
        let mut filters = HashMap::new();
        let mut transformers = HashMap::new();
        let mut mappers = HashMap::new();
        let mut soft_delete_handlers = HashMap::new();

        for task in &config.sync_tasks {
            if let Some(filter_config) = &task.filter {
                filters.insert(task.table.clone(), EventFilter::new(filter_config.clone()));
            }

            if let Some(transform_config) = &task.transform {
                transformers.insert(
                    task.table.clone(),
                    EventTransformer::new(transform_config.clone()),
                );
            }

            if let Some(mapping_config) = &task.mapping {
                mappers.insert(task.table.clone(), FieldMapper::new(mapping_config.clone()));
            }

            if let Some(soft_delete_config) = &task.soft_delete {
                soft_delete_handlers.insert(
                    task.table.clone(),
                    SoftDeleteHandler::new(soft_delete_config.clone()),
                );
            }
        }

        let (shutdown_tx, _) = watch::channel(false);

        Ok(Self {
            config,
            source_adapters,
            destination_tx: None,
            filters,
            transformers,
            mappers,
            soft_delete_handlers,
            shutdown_tx: Some(shutdown_tx),
            task_handles: Vec::new(),
            checkpoints: Arc::new(RwLock::new(HashMap::new())),
            checkpoint_manager: None,
            dead_letter_queue: None,
            dlq_reprocess_rx: None,
            cdc_coordinator: None,
            parallel_processors: HashMap::new(),
            work_stealing_coordinator: None,
            at_least_once_manager: None,
            transactional_checkpoint: None,
            adaptive_batching_manager: None,
            memory_monitor: None,
            backpressure_manager: None,
        })
    }

    /// Create a source adapter from configuration
    fn create_source_adapter(
        source_config: &crate::config::SourceConfig,
    ) -> Result<Box<dyn SourceAdapter>> {
        match source_config {
            crate::config::SourceConfig::PostgreSQL(pg_config) => {
                Ok(Box::new(PostgresAdapter::new((**pg_config).clone())) as Box<dyn SourceAdapter>)
            }
            crate::config::SourceConfig::MySQL(_) => Err(MeiliBridgeError::Configuration(
                "MySQL source not yet implemented".to_string(),
            )),
            crate::config::SourceConfig::MongoDB(_) => Err(MeiliBridgeError::Configuration(
                "MongoDB source not yet implemented".to_string(),
            )),
        }
    }

    fn build_table_index_map(config: &Config) -> HashMap<String, String> {
        let mut map = HashMap::new();

        for task in &config.sync_tasks {
            if task.index.is_empty() || task.table.is_empty() {
                continue;
            }

            let (schema_opt, table_name) = Self::parse_table_identifier(&task.table);

            if let Some(schema) = schema_opt.as_deref() {
                let key = Self::canonical_table_key(Some(schema), &table_name);
                if !key.is_empty() {
                    map.insert(key, task.index.clone());
                }
            }

            let table_key = Self::canonical_table_key(None, &table_name);
            if !table_key.is_empty() {
                map.insert(table_key, task.index.clone());
            }
        }

        map
    }

    fn parse_table_identifier(table: &str) -> (Option<String>, String) {
        let trimmed = table.trim();
        if trimmed.is_empty() {
            return (None, String::new());
        }

        if let Some((schema, name)) = trimmed.split_once('.') {
            let schema_clean = Self::sanitize_identifier(schema);
            let table_clean = Self::sanitize_identifier(name);
            (
                if schema_clean.is_empty() {
                    None
                } else {
                    Some(schema_clean)
                },
                table_clean,
            )
        } else {
            (None, Self::sanitize_identifier(trimmed))
        }
    }

    fn sanitize_identifier(identifier: &str) -> String {
        identifier.trim().trim_matches('"').to_string()
    }

    fn canonical_table_key(schema: Option<&str>, table: &str) -> String {
        let table_part = table.trim().trim_matches('"').to_lowercase();
        if table_part.is_empty() {
            return String::new();
        }

        match schema {
            Some(schema) if !schema.trim().is_empty() => format!(
                "{}.{}",
                schema.trim().trim_matches('"').to_lowercase(),
                table_part
            ),
            _ => table_part,
        }
    }

    /// Start the pipeline
    pub async fn start(&mut self) -> Result<()> {
        info!("Starting pipeline orchestrator");

        // Initialize dead letter queue
        let dlq_storage: Arc<dyn DlqStorage> = if !self.config.redis.url.is_empty() {
            info!("Using Redis-based dead letter queue");
            Arc::new(RedisDlqStorage::new(
                &self.config.redis.url,
                self.config.redis.key_prefix.clone(),
            )?)
        } else {
            info!("Using in-memory dead letter queue");
            Arc::new(InMemoryDlqStorage::new())
        };

        let mut dlq = DeadLetterQueue::new(dlq_storage);

        // Set up reprocess channel
        let (reprocess_tx, reprocess_rx) = mpsc::channel(100);
        dlq.set_reprocess_channel(reprocess_tx);
        self.dlq_reprocess_rx = Some(reprocess_rx);
        self.dead_letter_queue = Some(Arc::new(dlq));

        // Initialize checkpoint manager
        let checkpoint_storage: Arc<dyn CheckpointStorage> = if !self.config.redis.url.is_empty() {
            info!("Using Redis-based checkpoint storage");
            let mut redis_storage = RedisStorage::new(self.config.redis.clone());
            redis_storage.connect().await?;
            Arc::new(redis_storage)
        } else {
            info!("Using in-memory checkpoint storage");
            Arc::new(MemoryStorage::new())
        };

        let flush_interval = Duration::from_secs(30); // Flush checkpoints every 30 seconds
        let batch_size = 10; // Flush after 10 checkpoints
        let mut checkpoint_manager =
            CheckpointManager::new(checkpoint_storage.clone(), flush_interval, batch_size);
        checkpoint_manager.start().await?;
        self.checkpoint_manager = Some(Arc::new(checkpoint_manager));

        // Initialize at-least-once delivery if enabled
        if self.config.at_least_once_delivery.enabled {
            info!("Initializing at-least-once delivery guarantees");

            // Create at-least-once manager
            let at_least_once_config = crate::delivery::AtLeastOnceConfig {
                enabled: self.config.at_least_once_delivery.enabled,
                deduplication_window: self.config.at_least_once_delivery.deduplication_window,
                transaction_timeout_secs: self
                    .config
                    .at_least_once_delivery
                    .transaction_timeout_secs,
                two_phase_commit: self.config.at_least_once_delivery.two_phase_commit,
                checkpoint_before_write: self.config.at_least_once_delivery.checkpoint_before_write,
            };

            let at_least_once_manager = Arc::new(crate::delivery::AtLeastOnceManager::new(
                at_least_once_config,
            ));

            // Create transactional checkpoint
            let transactional_checkpoint = Arc::new(crate::delivery::TransactionalCheckpoint::new(
                checkpoint_storage,
            ));

            // Set up transaction coordinator
            let coordinator = at_least_once_manager.transaction_coordinator.clone();
            let checkpoint_clone = transactional_checkpoint.clone();
            let coordinator_handle = tokio::spawn(async move {
                coordinator.set_checkpoint_handler(checkpoint_clone).await;
                // Run cleanup task
                coordinator.run_cleanup_task().await;
            });
            self.task_handles.push(coordinator_handle);

            self.at_least_once_manager = Some(at_least_once_manager);
            self.transactional_checkpoint = Some(transactional_checkpoint);
        }

        // Connect to source
        for (name, adapter) in &mut self.source_adapters {
            info!("Connecting to source '{}'", name);
            adapter.connect().await?;
        }

        // Create memory monitor if enabled
        if self.config.performance.memory.monitor_memory_pressure {
            let mut monitor = MemoryMonitor::new(
                self.config.performance.memory.max_queue_memory_mb,
                self.config.performance.memory.max_checkpoint_memory_mb,
            );

            // Start monitoring in background
            let memory_pressure_rx = monitor.start_monitoring();
            let monitor_arc = Arc::new(monitor);
            self.memory_monitor = Some(monitor_arc.clone());

            // Handle memory pressure events
            let checkpoint_retention_config = self.config.redis.checkpoint_retention.clone();
            let active_task_ids: Vec<String> = self
                .config
                .sync_tasks
                .iter()
                .map(|task| task.id.clone())
                .collect();
            let checkpoint_manager_ref = self.checkpoint_manager.clone();

            let handle = tokio::spawn(async move {
                let mut rx = memory_pressure_rx;
                while let Some(event) = rx.recv().await {
                    match event {
                        crate::pipeline::MemoryPressureEvent::HighQueueMemory(percent) => {
                            tracing::warn!("High queue memory usage: {:.1}%", percent);

                            // The backpressure manager will handle pausing/resuming sources
                            // based on individual channel metrics
                        }
                        crate::pipeline::MemoryPressureEvent::HighCheckpointMemory(percent) => {
                            tracing::warn!("High checkpoint memory usage: {:.1}%", percent);

                            // Trigger checkpoint cleanup if enabled
                            if checkpoint_retention_config.cleanup_on_memory_pressure
                                && percent >= checkpoint_retention_config.memory_pressure_threshold
                            {
                                tracing::info!(
                                    "Triggering checkpoint cleanup due to memory pressure"
                                );

                                // Trigger cleanup
                                if let Some(checkpoint_manager) = &checkpoint_manager_ref {
                                    if let Err(e) = checkpoint_manager
                                        .cleanup_checkpoints(
                                            active_task_ids.clone(),
                                            checkpoint_retention_config.max_checkpoints_per_task,
                                        )
                                        .await
                                    {
                                        tracing::error!("Failed to cleanup checkpoints: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });
            self.task_handles.push(handle);
        }

        // Create adaptive batching manager if enabled
        if self.config.performance.batch_processing.adaptive_batching {
            let manager = Arc::new(AdaptiveBatchingManager::new(
                self.config
                    .performance
                    .batch_processing
                    .adaptive_config
                    .clone(),
            ));
            self.adaptive_batching_manager = Some(manager.clone());
        }

        // Start destination processor
        let (tx, rx) = mpsc::channel(100);
        self.destination_tx = Some(tx);

        let config = self.config.clone();
        let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();
        let adaptive_manager = self.adaptive_batching_manager.clone();

        let destination_handle = tokio::spawn(async move {
            Self::run_destination_processor(config, rx, shutdown_rx, adaptive_manager).await;
        });

        self.task_handles.push(destination_handle);

        // Group sync tasks by source
        let mut tasks_by_source: HashMap<String, Vec<SyncTaskConfig>> = HashMap::new();
        for task in &self.config.sync_tasks {
            tasks_by_source
                .entry(task.source_name.clone())
                .or_default()
                .push(task.clone());
        }

        // Create CDC coordinators for each source that has sync tasks
        let mut all_task_receivers = Vec::new();
        let mut cdc_coordinators: HashMap<String, Arc<RwLock<CdcCoordinator>>> = HashMap::new();

        for (source_name, tasks) in tasks_by_source {
            // Get the source adapter
            let source_adapter = self.source_adapters.get_mut(&source_name).ok_or_else(|| {
                MeiliBridgeError::Pipeline(format!(
                    "Source adapter '{}' not found for sync tasks",
                    source_name
                ))
            })?;

            // Clone the adapter for CDC coordinator (shares the connection pool)
            let mut cdc_adapter = source_adapter.clone_box();

            // Ensure the cloned adapter is connected
            cdc_adapter.connect().await?;

            // Load checkpoints for all tasks and find the earliest position
            let mut earliest_position: Option<Position> = None;
            if let Some(ref checkpoint_manager) = self.checkpoint_manager {
                for task in &tasks {
                    match checkpoint_manager.load_checkpoint(&task.id).await {
                        Ok(Some(checkpoint)) => {
                            info!(
                                "Loaded checkpoint for task '{}' at position {:?}",
                                task.id, checkpoint.position
                            );
                            // Update in-memory checkpoint
                            let mut checkpoints_map = self.checkpoints.write().await;
                            checkpoints_map.insert(task.id.clone(), checkpoint.position.clone());

                            // Track the earliest position for this source
                            match &earliest_position {
                                None => earliest_position = Some(checkpoint.position),
                                Some(current) => {
                                    // For PostgreSQL, compare LSNs to find the earlier one
                                    if !checkpoint.position.is_after(current) {
                                        earliest_position = Some(checkpoint.position);
                                    }
                                }
                            }
                        }
                        Ok(None) => {
                            info!("No checkpoint found for task '{}', starting fresh", task.id);
                        }
                        Err(e) => {
                            warn!("Failed to load checkpoint for task '{}': {}", task.id, e);
                        }
                    }
                }

                // Set the start position on the CDC adapter if we have one
                if let Some(position) = earliest_position {
                    info!("Setting CDC start position to: {:?}", position);
                    cdc_adapter.set_start_position(position).await?;
                }
            }

            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();
            let mut cdc_coordinator = CdcCoordinator::new(cdc_adapter, shutdown_rx);

            // Register tasks with this coordinator
            let mut task_receivers = Vec::new();
            for task in tasks {
                let (tx, rx) = mpsc::channel(1000);
                cdc_coordinator.register_task(task.table.clone(), tx);
                task_receivers.push((task, rx));
            }

            all_task_receivers.extend(task_receivers);

            // Store the CDC coordinator in an Arc<RwLock> for shared access
            let coordinator_arc = Arc::new(RwLock::new(cdc_coordinator));
            cdc_coordinators.insert(source_name.clone(), coordinator_arc.clone());

            // Start the CDC coordinator
            let coordinator_for_task = coordinator_arc.clone();
            let source_name_for_log = source_name.clone();
            let coordinator_handle = tokio::spawn(async move {
                // We need to hold the write lock for the entire run duration
                // This is okay because other methods only need read access for control operations
                let mut coordinator = coordinator_for_task.write().await;
                if let Err(e) = coordinator.run().await {
                    error!(
                        "CDC coordinator error for source '{}': {}",
                        source_name_for_log, e
                    );
                }
            });
            self.task_handles.push(coordinator_handle);

            info!("Started CDC coordinator for source '{}'", source_name);
        }

        // Store the primary CDC coordinator for backward compatibility
        if let Some(primary_coordinator) = cdc_coordinators.get("primary") {
            self.cdc_coordinator = Some(primary_coordinator.clone());
        }

        // Create backpressure manager
        let mut backpressure_manager = BackpressureManager::new(BackpressureConfig {
            high_watermark: 80.0,
            low_watermark: 60.0,
            check_interval_ms: 100,
            channel_capacity: 1000,
        });

        // Register all tables for monitoring
        for task in &self.config.sync_tasks {
            backpressure_manager.register_table(&task.table).await;
        }

        let backpressure_rx = backpressure_manager.start_monitoring();
        self.backpressure_manager = Some(Arc::new(backpressure_manager));

        // Handle backpressure events
        let cdc_coordinators_ref = cdc_coordinators.clone();
        let handle = tokio::spawn(async move {
            let mut rx = backpressure_rx;
            while let Some(event) = rx.recv().await {
                match event {
                    BackpressureEvent::PauseSource(table) => {
                        info!("Backpressure: Pausing table '{}'", table);
                        // Find which source has this table
                        for coordinator in cdc_coordinators_ref.values() {
                            coordinator.read().await.pause_table(&table).await;
                        }
                    }
                    BackpressureEvent::ResumeSource(table) => {
                        info!("Backpressure: Resuming table '{}'", table);
                        // Find which source has this table
                        for coordinator in cdc_coordinators_ref.values() {
                            coordinator.read().await.resume_table(&table).await;
                        }
                    }
                    BackpressureEvent::PauseAll => {
                        warn!("Backpressure: Pausing all sources");
                        for coordinator in cdc_coordinators_ref.values() {
                            coordinator.read().await.pause_all().await;
                        }
                    }
                    BackpressureEvent::ResumeAll => {
                        info!("Backpressure: Resuming all sources");
                        for coordinator in cdc_coordinators_ref.values() {
                            coordinator.read().await.resume_all().await;
                        }
                    }
                }
            }
        });
        self.task_handles.push(handle);

        // Initialize work stealing coordinator if enabled
        if self.config.performance.parallel_processing.enabled
            && self.config.performance.parallel_processing.work_stealing
        {
            let parallel_config = crate::pipeline::parallel_processor::ParallelConfig {
                workers_per_table: self
                    .config
                    .performance
                    .parallel_processing
                    .workers_per_table,
                max_concurrent_events: self
                    .config
                    .performance
                    .parallel_processing
                    .max_concurrent_events,
                work_stealing: self.config.performance.parallel_processing.work_stealing,
                steal_interval_ms: self
                    .config
                    .performance
                    .parallel_processing
                    .work_steal_interval_ms,
            };

            let work_stealing = WorkStealingCoordinator::new(parallel_config);
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();
            let handle = work_stealing.start(shutdown_rx);
            self.task_handles.push(handle);
            self.work_stealing_coordinator = Some(work_stealing);

            info!("Work stealing coordinator initialized");
        }

        // Start sync tasks with their receivers
        for (task, rx) in all_task_receivers {
            self.start_sync_task_with_receiver(task, rx).await?;
        }

        // Start checkpoint persistence task
        self.start_checkpoint_task().await?;

        // Start periodic checkpoint cleanup task
        if let Some(checkpoint_manager) = &self.checkpoint_manager {
            let checkpoint_manager_ref = checkpoint_manager.clone();
            let active_task_ids: Vec<String> = self
                .config
                .sync_tasks
                .iter()
                .map(|task| task.id.clone())
                .collect();
            let checkpoint_retention_config = self.config.redis.checkpoint_retention.clone();
            let max_checkpoints = checkpoint_retention_config.max_checkpoints_per_task;
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

            let handle = tokio::spawn(async move {
                let mut shutdown_rx = shutdown_rx;
                let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(3600)); // Run every hour

                loop {
                    tokio::select! {
                        _ = interval.tick() => {
                            info!("Running periodic checkpoint cleanup");
                            if let Err(e) = checkpoint_manager_ref.cleanup_checkpoints(
                                active_task_ids.clone(),
                                max_checkpoints,
                            ).await {
                                error!("Periodic checkpoint cleanup failed: {}", e);
                            }
                        }
                        _ = shutdown_rx.changed() => {
                            if *shutdown_rx.borrow() {
                                info!("Stopping periodic checkpoint cleanup task");
                                break;
                            }
                        }
                    }
                }
            });

            self.task_handles.push(handle);
            info!("Started periodic checkpoint cleanup task");
        }

        info!("Pipeline orchestrator started successfully");
        Ok(())
    }

    /// Stop the pipeline
    pub async fn stop(&mut self) -> Result<()> {
        info!("Stopping pipeline orchestrator");

        // Send shutdown signal
        if let Some(tx) = &self.shutdown_tx {
            let _ = tx.send(true);
        }

        // Send shutdown to destination processor
        if let Some(tx) = &self.destination_tx {
            let (resp_tx, _) = mpsc::channel(1);
            let _ = tx.send((DestinationCommand::Shutdown, resp_tx)).await;
        }

        // Wait for tasks to complete with timeout
        let mut remaining_handles = Vec::new();
        for handle in self.task_handles.drain(..) {
            remaining_handles.push(handle);
        }

        // Give tasks 5 seconds to shut down gracefully
        let shutdown_timeout = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            for handle in &mut remaining_handles {
                let _ = handle.await;
            }
        })
        .await;

        if shutdown_timeout.is_err() {
            warn!("Some tasks did not shut down gracefully within timeout, aborting them");
            // Abort remaining tasks
            for handle in remaining_handles {
                handle.abort();
            }
        }

        // Stop memory monitor
        if self.memory_monitor.is_some() {
            info!("Stopping memory monitor");
            // Memory monitor shutdown is handled by the shutdown signal in its background task
            // The task will exit when the channel is closed
        }

        // Stop checkpoint manager
        if let Some(ref _checkpoint_manager) = self.checkpoint_manager {
            info!("Stopping checkpoint manager");
            // Note: CheckpointManager doesn't have mutable stop method
            // The shutdown is handled by the shutdown signal in the background task
        }

        // Disconnect from sources
        for (name, adapter) in &mut self.source_adapters {
            info!("Disconnecting from source '{}'", name);
            adapter.disconnect().await?;
        }

        info!("Pipeline orchestrator stopped");
        Ok(())
    }

    /// Run the destination processor in a separate task
    async fn run_destination_processor(
        config: Config,
        mut rx: mpsc::Receiver<(
            DestinationCommand,
            mpsc::Sender<Result<DestinationResponse>>,
        )>,
        mut shutdown_rx: watch::Receiver<bool>,
        adaptive_batching_manager: Option<Arc<AdaptiveBatchingManager>>,
    ) {
        let table_to_index = Self::build_table_index_map(&config);

        let mut base_adapter = MeilisearchAdapter::new(config.meilisearch.clone(), table_to_index);

        if let Some(manager) = adaptive_batching_manager {
            base_adapter = base_adapter.with_adaptive_batching(manager, config.performance.clone());
        }

        // Create destination adapter with adaptive batching if enabled
        let mut destination_adapter: Box<dyn DestinationAdapter> = Box::new(base_adapter);

        // Connect to destination
        info!("Connecting to destination");
        if let Err(e) = destination_adapter.connect().await {
            error!("Failed to connect to destination: {}", e);
            return;
        }

        loop {
            tokio::select! {
                Some((cmd, resp_tx)) = rx.recv() => {
                    match cmd {
                        DestinationCommand::ProcessEvents(events) => {
                            let result = destination_adapter.process_events(events).await;
                            let response = match result {
                                Ok(sync_result) => Ok(DestinationResponse {
                                    success_count: sync_result.success_count,
                                    failed_count: sync_result.failed_count,
                                    last_position: sync_result.last_position,
                                    errors: sync_result.errors,
                                }),
                                Err(e) => Err(e),
                            };
                            let _ = resp_tx.send(response).await;
                        }
                        DestinationCommand::ImportData { index, documents, primary_key } => {
                            let result = destination_adapter.import_data(&index, documents, primary_key.as_deref()).await;
                            let response = match result {
                                Ok(sync_result) => Ok(DestinationResponse {
                                    success_count: sync_result.success_count,
                                    failed_count: sync_result.failed_count,
                                    last_position: sync_result.last_position,
                                    errors: sync_result.errors,
                                }),
                                Err(e) => Err(e),
                            };
                            let _ = resp_tx.send(response).await;
                        }
                        DestinationCommand::EnsureIndex { index_name, primary_key } => {
                            // Ensure the index exists
                            // Note: The primary key will be handled by the adapter when creating the index
                            let _ = primary_key; // Suppress unused variable warning
                            let result = destination_adapter.ensure_index(&index_name, None).await;
                            let response = match result {
                                Ok(_) => Ok(DestinationResponse {
                                    success_count: 1,
                                    failed_count: 0,
                                    last_position: None,
                                    errors: vec![],
                                }),
                                Err(e) => Err(e),
                            };
                            let _ = resp_tx.send(response).await;
                        }
                        DestinationCommand::HealthCheck => {
                            let result = destination_adapter.is_healthy().await;
                            let response = if result {
                                Ok(DestinationResponse {
                                    success_count: 1,
                                    failed_count: 0,
                                    last_position: None,
                                    errors: vec![],
                                })
                            } else {
                                Ok(DestinationResponse {
                                    success_count: 0,
                                    failed_count: 1,
                                    last_position: None,
                                    errors: vec!["Health check failed".to_string()],
                                })
                            };
                            let _ = resp_tx.send(response).await;
                        }
                        DestinationCommand::Shutdown => {
                            info!("Shutting down destination processor");
                            break;
                        }
                    }
                }
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Received shutdown signal in destination processor");
                        break;
                    }
                }
            }
        }

        // Disconnect from destination
        info!("Disconnecting from destination");
        let _ = destination_adapter.disconnect().await;
    }

    /// Start a sync task with a pre-created event receiver
    async fn start_sync_task_with_receiver(
        &mut self,
        task: SyncTaskConfig,
        rx: mpsc::Receiver<Result<Event>>,
    ) -> Result<()> {
        info!(
            "Starting sync task for table '{}' with CDC receiver",
            task.table
        );

        // Checkpoint loading is now handled in start_sync() before CDC coordinator creation
        // This ensures CDC starts from the correct position

        // Perform initial full sync if configured
        if task.full_sync_on_start.unwrap_or(false) {
            info!("Performing initial full sync for table '{}'", task.table);
            self.perform_full_sync(&task).await?;
        } else {
            // For CDC-only tables, ensure the index exists if auto_create_index is enabled
            if self.config.meilisearch.auto_create_index {
                info!(
                    "Ensuring index '{}' exists for CDC-only table '{}'",
                    task.index, task.table
                );
                // Send a special command to ensure the index exists
                if let Some(tx) = &self.destination_tx {
                    let (resp_tx, mut resp_rx) = mpsc::channel(1);
                    let command = DestinationCommand::EnsureIndex {
                        index_name: task.index.clone(),
                        primary_key: if task.primary_key.is_empty() {
                            self.config.meilisearch.primary_key.clone()
                        } else {
                            Some(task.primary_key.clone())
                        },
                    };

                    if let Err(e) = tx.send((command, resp_tx)).await {
                        warn!(
                            "Failed to ensure index for CDC-only table '{}': {}",
                            task.table, e
                        );
                    } else {
                        // Wait for response
                        if let Some(result) = resp_rx.recv().await {
                            match result {
                                Ok(_) => info!(
                                    "Index '{}' is ready for CDC-only table '{}'",
                                    task.index, task.table
                                ),
                                Err(e) => warn!(
                                    "Failed to ensure index '{}' for table '{}': {}",
                                    task.index, task.table, e
                                ),
                            }
                        }
                    }
                }
            }
        }

        // Check if parallel processing is enabled
        if self.config.performance.parallel_processing.enabled {
            // Use parallel processor
            let parallel_config = crate::pipeline::parallel_processor::ParallelConfig {
                workers_per_table: self
                    .config
                    .performance
                    .parallel_processing
                    .workers_per_table,
                max_concurrent_events: self
                    .config
                    .performance
                    .parallel_processing
                    .max_concurrent_events,
                work_stealing: self.config.performance.parallel_processing.work_stealing,
                steal_interval_ms: self
                    .config
                    .performance
                    .parallel_processing
                    .work_steal_interval_ms,
            };

            let mut processor = ParallelTableProcessor::new(task.table.clone(), parallel_config);

            // Create channel for processed events
            let (processed_tx, processed_rx) = mpsc::channel(100);

            // Start parallel workers
            let filter = self.filters.get(&task.table).cloned();
            let transformer = self.transformers.get(&task.table).cloned();
            let mapper = self.mappers.get(&task.table).cloned();
            let soft_delete_handler = self.soft_delete_handlers.get(&task.table).cloned();
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

            processor.start_workers(
                filter,
                transformer,
                mapper,
                soft_delete_handler,
                processed_tx,
                shutdown_rx,
            );

            // Store processor for work stealing
            let processor_arc = Arc::new(processor);
            self.parallel_processors
                .insert(task.table.clone(), processor_arc.clone());

            // Register with work stealing coordinator
            if let Some(ref coordinator) = self.work_stealing_coordinator {
                coordinator
                    .register_processor(task.table.clone(), processor_arc.clone())
                    .await;
            }

            // Start event distributor task
            let event_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            let processor_for_task = processor_arc.clone();
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

            let distributor_handle = tokio::spawn(async move {
                Self::distribute_events_to_parallel_processor(
                    event_stream,
                    processor_for_task,
                    shutdown_rx,
                )
                .await;
            });
            self.task_handles.push(distributor_handle);

            // Start batch processor for parallel results
            let destination_tx = self
                .destination_tx
                .clone()
                .ok_or_else(|| MeiliBridgeError::Pipeline("Destination not started".to_string()))?;
            let checkpoints = self.checkpoints.clone();
            let table_name = task.table.clone();
            let dlq = self.dead_letter_queue.clone();
            let task_id = task.id.clone();
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

            let batch_handle = tokio::spawn(async move {
                Self::process_parallel_results(
                    processed_rx,
                    destination_tx,
                    checkpoints,
                    table_name,
                    dlq,
                    task_id,
                    shutdown_rx,
                )
                .await;
            });
            self.task_handles.push(batch_handle);

            info!(
                "Started parallel processing for table '{}' with {} workers",
                task.table,
                self.config
                    .performance
                    .parallel_processing
                    .workers_per_table
            );
        } else {
            // Use traditional single-threaded processing
            let event_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

            let filter = self.filters.get(&task.table).cloned();
            let transformer = self.transformers.get(&task.table).cloned();
            let mapper = self.mappers.get(&task.table).cloned();
            let soft_delete_handler = self.soft_delete_handlers.get(&task.table).cloned();
            let destination_tx = self
                .destination_tx
                .clone()
                .ok_or_else(|| MeiliBridgeError::Pipeline("Destination not started".to_string()))?;
            let checkpoints = self.checkpoints.clone();
            let table_name = task.table.clone();
            let batch_size = task.options.batch_size;
            let batch_timeout = Duration::from_millis(task.options.batch_timeout_ms);

            let dlq = self.dead_letter_queue.clone();
            let task_id = task.id.clone();
            let at_least_once = self.at_least_once_manager.clone();
            let transactional_checkpoint = self.transactional_checkpoint.clone();

            let handle = tokio::spawn(async move {
                let params = EventStreamParams {
                    filter,
                    transformer,
                    mapper,
                    soft_delete_handler,
                    destination_tx,
                    checkpoints,
                    table_name,
                    batch_size,
                    batch_timeout,
                    dead_letter_queue: dlq,
                    task_id,
                    at_least_once_manager: at_least_once,
                    transactional_checkpoint,
                };
                Self::process_event_stream(event_stream, shutdown_rx, params).await;
            });

            self.task_handles.push(handle);
        }

        Ok(())
    }

    /// Process event stream
    async fn process_event_stream(
        mut event_stream: impl StreamExt<Item = Result<Event>> + Unpin,
        mut shutdown_rx: watch::Receiver<bool>,
        params: EventStreamParams,
    ) {
        let mut event_batch = Vec::new();
        let mut batch_timer = interval(params.batch_timeout);
        batch_timer.reset();

        loop {
            tokio::select! {
                // Process events from stream
                Some(event_result) = event_stream.next() => {
                    match event_result {
                        Ok(event) => {
                            debug!("Sync task '{}': Received event from coordinator", params.table_name);
                            // Events are pre-filtered by CDC coordinator
                            if let Some(processed) = Self::process_single_event(
                                event,
                                &params.filter,
                                &params.transformer,
                                &params.mapper,
                                &params.soft_delete_handler,
                            ).await {
                                debug!("Sync task '{}': Event processed, adding to batch", params.table_name);
                                event_batch.push(processed);

                                // Process batch if it reaches the size limit
                                if event_batch.len() >= params.batch_size {
                                    info!("Sync task '{}': Batch size {} reached, processing", params.table_name, params.batch_size);
                                    Self::process_batch(
                                        &mut event_batch,
                                        BatchProcessParams {
                                            destination_tx: &params.destination_tx,
                                            checkpoints: &params.checkpoints,
                                            table_name: &params.table_name,
                                            dead_letter_queue: params.dead_letter_queue.as_ref(),
                                            task_id: &params.task_id,
                                            at_least_once_manager: params.at_least_once_manager.as_ref(),
                                            transactional_checkpoint: params.transactional_checkpoint.as_ref(),
                                        },
                                    ).await;
                                }
                            } else {
                                debug!("Sync task '{}': Event filtered out or failed processing", params.table_name);
                            }
                        }
                        Err(e) => {
                            error!("Sync task '{}': Error receiving event: {}", params.table_name, e);
                        }
                    }
                }

                // Process batch on timeout
                _ = batch_timer.tick() => {
                    if !event_batch.is_empty() {
                        info!("Sync task '{}': Batch timeout reached, processing {} events",
                              params.table_name, event_batch.len());
                        Self::process_batch(
                            &mut event_batch,
                            BatchProcessParams {
                                destination_tx: &params.destination_tx,
                                checkpoints: &params.checkpoints,
                                table_name: &params.table_name,
                                dead_letter_queue: params.dead_letter_queue.as_ref(),
                                task_id: &params.task_id,
                                at_least_once_manager: params.at_least_once_manager.as_ref(),
                                transactional_checkpoint: params.transactional_checkpoint.as_ref(),
                            },
                        ).await;
                    } else {
                        trace!("Sync task '{}': Batch timeout but no events to process", params.table_name);
                    }
                }

                // Check for shutdown
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down event stream processor for table '{}'", params.table_name);

                        // Process any remaining events
                        if !event_batch.is_empty() {
                            Self::process_batch(
                                &mut event_batch,
                                BatchProcessParams {
                                    destination_tx: &params.destination_tx,
                                    checkpoints: &params.checkpoints,
                                    table_name: &params.table_name,
                                    dead_letter_queue: params.dead_letter_queue.as_ref(),
                                    task_id: &params.task_id,
                                    at_least_once_manager: params.at_least_once_manager.as_ref(),
                                    transactional_checkpoint: params.transactional_checkpoint.as_ref(),
                                },
                            ).await;
                        }
                        break;
                    }
                }
            }
        }
    }

    /// Process a single event through the pipeline
    async fn process_single_event(
        event: Event,
        filter: &Option<EventFilter>,
        transformer: &Option<EventTransformer>,
        mapper: &Option<FieldMapper>,
        soft_delete_handler: &Option<SoftDeleteHandler>,
    ) -> Option<Event> {
        // Apply filter
        if let Some(f) = filter {
            match f.should_process(&event) {
                Ok(true) => {}
                Ok(false) => return None,
                Err(e) => {
                    warn!("Error in filter: {}", e);
                    return None;
                }
            }
        }

        let mut current_event = event;

        // Apply transformer
        if let Some(t) = transformer {
            match t.transform(current_event) {
                Ok(Some(transformed)) => current_event = transformed,
                Ok(None) => return None,
                Err(e) => {
                    warn!("Error in transformer: {}", e);
                    return None;
                }
            }
        }

        // Apply mapper
        if let Some(m) = mapper {
            match m.map_event(current_event) {
                Ok(mapped) => current_event = mapped,
                Err(e) => {
                    warn!("Error in mapper: {}", e);
                    return None;
                }
            }
        }

        // Apply soft delete handler
        if let Some(sd) = soft_delete_handler {
            match sd.transform_event(current_event) {
                Some(transformed) => current_event = transformed,
                None => return None, // Filtered out by soft delete
            }
        }

        Some(current_event)
    }

    /// Process a batch of events
    async fn process_batch(event_batch: &mut Vec<Event>, params: BatchProcessParams<'_>) {
        if event_batch.is_empty() {
            return;
        }

        debug!(
            "Processing batch of {} events for table '{}'",
            event_batch.len(),
            params.table_name
        );

        // Check if at-least-once delivery is enabled
        if let (Some(at_least_once), Some(transactional_cp)) = (
            params.at_least_once_manager,
            params.transactional_checkpoint,
        ) {
            // Use at-least-once delivery
            if let Err(e) = Self::process_batch_with_at_least_once(
                event_batch,
                AtLeastOnceBatchParams {
                    destination_tx: params.destination_tx,
                    checkpoints: params.checkpoints,
                    table_name: params.table_name,
                    dead_letter_queue: params.dead_letter_queue,
                    task_id: params.task_id,
                    at_least_once_manager: at_least_once,
                    transactional_checkpoint: transactional_cp,
                },
            )
            .await
            {
                error!("Failed to process batch with at-least-once delivery: {}", e);
            }
            return;
        }

        // Standard processing without at-least-once delivery
        let (resp_tx, mut resp_rx) = mpsc::channel(1);
        let cmd = DestinationCommand::ProcessEvents(event_batch.clone());

        match params.destination_tx.send((cmd, resp_tx)).await {
            Ok(_) => {
                // Wait for response
                match resp_rx.recv().await {
                    Some(Ok(response)) => {
                        if response.failed_count == 0 {
                            info!(
                                "Successfully processed {} events for table '{}'",
                                response.success_count, params.table_name
                            );
                        } else {
                            warn!(
                                "Processed batch with {} successes and {} failures for table '{}'",
                                response.success_count, response.failed_count, params.table_name
                            );

                            // Send failed events to dead letter queue
                            if let Some(dlq) = params.dead_letter_queue {
                                for (i, error) in response.errors.iter().enumerate() {
                                    error!("Batch processing error: {}", error);

                                    // Try to identify which event failed (basic heuristic)
                                    if i < event_batch.len() {
                                        let failed_event = &event_batch[i];
                                        if let Err(e) = dlq
                                            .add_failed_event(
                                                params.task_id.to_string(),
                                                convert_to_models_event(failed_event.clone()),
                                                MeiliBridgeError::Pipeline(error.clone()),
                                                0, // Initial retry count
                                            )
                                            .await
                                        {
                                            error!(
                                                "Failed to add event to dead letter queue: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                            }
                        }

                        // Update checkpoint if available
                        if let Some(position) = response.last_position {
                            // Update in-memory checkpoint using task_id
                            // The checkpoint persistence task will handle saving to storage
                            let mut checkpoints_map = params.checkpoints.write().await;
                            checkpoints_map.insert(params.task_id.to_string(), position);
                        }
                    }
                    Some(Err(e)) => {
                        error!(
                            "Failed to process batch for table '{}': {}",
                            params.table_name, e
                        );
                    }
                    None => {
                        error!("No response received from destination processor");
                    }
                }
            }
            Err(e) => {
                error!("Failed to send batch to destination processor: {}", e);
            }
        }

        event_batch.clear();
    }

    /// Process batch with at-least-once delivery guarantees
    async fn process_batch_with_at_least_once(
        event_batch: &mut Vec<Event>,
        params: AtLeastOnceBatchParams<'_>,
    ) -> Result<()> {
        use crate::pipeline::at_least_once_helpers::{
            create_dedup_key_from_event, extract_position_from_event,
        };

        // Extract position from last event
        let last_position = extract_position_from_event(event_batch.last().unwrap());

        if let Some(position) = last_position {
            // Begin transaction
            let transaction_id = params
                .at_least_once_manager
                .begin_transaction(params.task_id)
                .await?;

            // Start transactional checkpoint
            params
                .transactional_checkpoint
                .begin_transaction(
                    transaction_id.clone(),
                    params.task_id.to_string(),
                    position.clone(),
                )
                .await?;

            // Check for duplicates and filter them out
            let mut deduplicated_batch = Vec::new();
            for event in event_batch.iter() {
                let dedup_key = create_dedup_key_from_event(event);

                if !params
                    .at_least_once_manager
                    .is_duplicate(&dedup_key)
                    .await?
                {
                    deduplicated_batch.push(event.clone());
                    params
                        .at_least_once_manager
                        .mark_processed(dedup_key)
                        .await?;
                } else {
                    debug!("Skipping duplicate event: {:?}", dedup_key);
                }
            }

            if deduplicated_batch.is_empty() {
                info!("All events in batch were duplicates, skipping");
                event_batch.clear();
                return Ok(());
            }

            // Prepare phase - save checkpoint atomically BEFORE Meilisearch write
            let prepared = params
                .transactional_checkpoint
                .prepare(&transaction_id)
                .await?;

            if !prepared {
                error!("Failed to prepare transaction {}", transaction_id);
                params
                    .at_least_once_manager
                    .rollback(&transaction_id)
                    .await?;
                return Err(MeiliBridgeError::Pipeline(
                    "Transaction prepare failed".to_string(),
                ));
            }

            // Send to destination
            let (resp_tx, mut resp_rx) = mpsc::channel(1);
            let cmd = DestinationCommand::ProcessEvents(deduplicated_batch.clone());

            match params.destination_tx.send((cmd, resp_tx)).await {
                Ok(_) => {
                    // Wait for response
                    match resp_rx.recv().await {
                        Some(Ok(response)) => {
                            if response.failed_count == 0 {
                                // Success - commit the transaction
                                params
                                    .transactional_checkpoint
                                    .commit(&transaction_id)
                                    .await?;
                                params.at_least_once_manager.commit(&transaction_id).await?;

                                info!(
                                    "Successfully processed {} events for table '{}' with at-least-once delivery",
                                    response.success_count, params.table_name
                                );

                                // Update in-memory checkpoint (for monitoring)
                                let mut checkpoints_map = params.checkpoints.write().await;
                                checkpoints_map.insert(params.task_id.to_string(), position);
                            } else {
                                // Partial failure - rollback
                                warn!(
                                    "Batch processing failed with {} errors, rolling back transaction",
                                    response.failed_count
                                );

                                params
                                    .transactional_checkpoint
                                    .rollback(&transaction_id)
                                    .await?;
                                params
                                    .at_least_once_manager
                                    .rollback(&transaction_id)
                                    .await?;

                                // Send failed events to DLQ
                                if let Some(dlq) = params.dead_letter_queue {
                                    for (i, error) in response.errors.iter().enumerate() {
                                        if i < deduplicated_batch.len() {
                                            let failed_event = &deduplicated_batch[i];
                                            if let Err(e) = dlq
                                                .add_failed_event(
                                                    params.task_id.to_string(),
                                                    convert_to_models_event(failed_event.clone()),
                                                    MeiliBridgeError::Pipeline(error.clone()),
                                                    0,
                                                )
                                                .await
                                            {
                                                error!(
                                                    "Failed to add event to dead letter queue: {}",
                                                    e
                                                );
                                            }
                                        }
                                    }
                                }

                                return Err(MeiliBridgeError::Pipeline(format!(
                                    "Batch processing failed with {} errors",
                                    response.failed_count
                                )));
                            }
                        }
                        Some(Err(e)) => {
                            error!("Failed to process batch: {}, rolling back", e);
                            params
                                .transactional_checkpoint
                                .rollback(&transaction_id)
                                .await?;
                            params
                                .at_least_once_manager
                                .rollback(&transaction_id)
                                .await?;
                            return Err(e);
                        }
                        None => {
                            error!("No response from destination, rolling back");
                            params
                                .transactional_checkpoint
                                .rollback(&transaction_id)
                                .await?;
                            params
                                .at_least_once_manager
                                .rollback(&transaction_id)
                                .await?;
                            return Err(MeiliBridgeError::Pipeline(
                                "No response from destination".to_string(),
                            ));
                        }
                    }
                }
                Err(e) => {
                    error!("Failed to send batch to destination: {}, rolling back", e);
                    params
                        .transactional_checkpoint
                        .rollback(&transaction_id)
                        .await?;
                    params
                        .at_least_once_manager
                        .rollback(&transaction_id)
                        .await?;
                    return Err(MeiliBridgeError::Pipeline(format!("Send failed: {}", e)));
                }
            }
        }

        event_batch.clear();
        Ok(())
    }

    /// Perform full sync for a table
    async fn perform_full_sync(&mut self, task: &SyncTaskConfig) -> Result<()> {
        info!("Starting full sync for table '{}'", task.table);

        let source = self.source_adapters.get(&task.source_name).ok_or_else(|| {
            MeiliBridgeError::Pipeline(format!("Source adapter '{}' not found", task.source_name))
        })?;

        let batch_size = task.options.batch_size;
        let mut data_stream = source.get_full_data(&task.table, batch_size).await?;
        let filter = self.filters.get(&task.table).cloned();
        let transformer = self.transformers.get(&task.table).cloned();
        let mapper = self.mappers.get(&task.table).cloned();
        let soft_delete_handler = self.soft_delete_handlers.get(&task.table).cloned();

        let mut documents = Vec::new();
        let mut total_count = 0;

        while let Some(data_result) = data_stream.next().await {
            match data_result {
                Ok(data) => {
                    // Create a full sync event
                    let event = Event::FullSync {
                        table: task.table.clone(),
                        data,
                    };

                    // Process through pipeline
                    if let Some(Event::FullSync { data, .. }) = Self::process_single_event(
                        event,
                        &filter,
                        &transformer,
                        &mapper,
                        &soft_delete_handler,
                    )
                    .await
                    {
                        documents.push(data);
                        total_count += 1;

                        // Import batch when it reaches the size limit
                        if documents.len() >= batch_size {
                            self.import_documents(
                                &task.index,
                                &mut documents,
                                Some(&task.primary_key),
                            )
                            .await?;
                        }
                    }
                }
                Err(e) => {
                    error!("Error during full sync: {}", e);
                }
            }
        }

        // Import any remaining documents
        if !documents.is_empty() {
            self.import_documents(&task.index, &mut documents, Some(&task.primary_key))
                .await?;
        }

        info!(
            "Full sync completed for table '{}': {} documents",
            task.table, total_count
        );
        Ok(())
    }

    /// Import documents to destination
    async fn import_documents(
        &mut self,
        index: &str,
        documents: &mut Vec<Value>,
        primary_key: Option<&str>,
    ) -> Result<()> {
        let destination_tx = self
            .destination_tx
            .as_ref()
            .ok_or_else(|| MeiliBridgeError::Pipeline("Destination not started".to_string()))?;

        let (resp_tx, mut resp_rx) = mpsc::channel(1);
        let cmd = DestinationCommand::ImportData {
            index: index.to_string(),
            documents: documents.clone(),
            primary_key: primary_key.map(|s| s.to_string()),
        };

        destination_tx
            .send((cmd, resp_tx))
            .await
            .map_err(|_| MeiliBridgeError::Pipeline("Failed to send to destination".to_string()))?;

        match resp_rx.recv().await {
            Some(Ok(response)) => {
                if response.failed_count == 0 {
                    info!(
                        "Imported {} documents to index '{}'",
                        response.success_count, index
                    );
                } else {
                    warn!(
                        "Import completed with {} successes and {} failures",
                        response.success_count, response.failed_count
                    );
                }
            }
            Some(Err(e)) => return Err(e),
            None => {
                return Err(MeiliBridgeError::Pipeline(
                    "No response from destination".to_string(),
                ));
            }
        }

        documents.clear();
        Ok(())
    }

    /// Start checkpoint persistence task
    async fn start_checkpoint_task(&mut self) -> Result<()> {
        info!("Starting checkpoint persistence task");

        let checkpoints = self.checkpoints.clone();
        let checkpoint_manager = self.checkpoint_manager.clone();
        let shutdown_rx = self.shutdown_tx.as_ref().unwrap().subscribe();

        if let Some(manager) = checkpoint_manager {
            let handle = tokio::spawn(async move {
                Self::run_checkpoint_persistence(checkpoints, manager, shutdown_rx).await;
            });
            self.task_handles.push(handle);
        }

        Ok(())
    }

    /// Run checkpoint persistence loop
    async fn run_checkpoint_persistence(
        checkpoints: Arc<RwLock<HashMap<String, Position>>>,
        checkpoint_manager: Arc<CheckpointManager>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut check_interval = interval(Duration::from_secs(10)); // Check every 10 seconds
        check_interval.reset();

        loop {
            tokio::select! {
                _ = check_interval.tick() => {
                    // Get current checkpoints
                    let checkpoints_map = checkpoints.read().await;

                    for (task_id, position) in checkpoints_map.iter() {
                        if let Err(e) = checkpoint_manager.save_checkpoint(
                            task_id.clone(),
                            position.clone()
                        ).await {
                            error!("Failed to save checkpoint for task '{}': {}", task_id, e);
                        }
                    }
                }

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down checkpoint persistence task");

                        // Final save of all checkpoints
                        let checkpoints_map = checkpoints.read().await;
                        for (task_id, position) in checkpoints_map.iter() {
                            if let Err(e) = checkpoint_manager.save_checkpoint(
                                task_id.clone(),
                                position.clone()
                            ).await {
                                error!("Failed to save checkpoint for task '{}': {}", task_id, e);
                            }
                        }

                        break;
                    }
                }
            }
        }
    }

    /// Save checkpoint for a task
    #[allow(dead_code)]
    async fn save_checkpoint(&self, task_id: &str, position: Position) -> Result<()> {
        // Update in-memory checkpoint
        let mut checkpoints_map = self.checkpoints.write().await;
        checkpoints_map.insert(task_id.to_string(), position.clone());

        // Persist to storage if checkpoint manager is available
        if let Some(ref checkpoint_manager) = self.checkpoint_manager {
            checkpoint_manager
                .save_checkpoint(task_id.to_string(), position)
                .await?;
        }

        Ok(())
    }

    /// Get dead letter queue statistics
    pub async fn get_dlq_statistics(&self) -> Result<crate::dlq::DlqStatistics> {
        if let Some(dlq) = &self.dead_letter_queue {
            dlq.get_statistics().await
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Dead letter queue not initialized".to_string(),
            ))
        }
    }

    /// Reprocess dead letter entries for a task
    pub async fn reprocess_dlq_entries(
        &self,
        task_id: &str,
        limit: Option<usize>,
    ) -> Result<usize> {
        if let Some(dlq) = &self.dead_letter_queue {
            dlq.reprocess_entries(task_id, limit).await
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Dead letter queue not initialized".to_string(),
            ))
        }
    }

    /// Clear dead letter entries for a task
    pub async fn clear_dlq_task(&self, task_id: &str) -> Result<usize> {
        if let Some(dlq) = &self.dead_letter_queue {
            dlq.clear_task(task_id).await
        } else {
            Err(MeiliBridgeError::Pipeline(
                "Dead letter queue not initialized".to_string(),
            ))
        }
    }

    /// Pause CDC processing for all tables
    pub async fn pause_cdc(&self) -> Result<()> {
        if let Some(coordinator) = &self.cdc_coordinator {
            coordinator.read().await.pause_all().await;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Get access to the PostgreSQL statement cache if available
    pub fn get_postgres_cache(&self) -> Option<Arc<crate::source::postgres::StatementCache>> {
        // This is a workaround since we can't downcast trait objects
        // For now, return None. In a real implementation, we would need to
        // redesign the architecture to make the cache accessible
        None
    }

    /// Create a PostgreSQL health check if using PostgreSQL source
    pub fn create_postgres_health_check(&self) -> Option<Box<dyn crate::health::HealthCheck>> {
        // Try single source first (backward compatibility)
        if let Some(crate::config::SourceConfig::PostgreSQL(pg_config)) = &self.config.source {
            // Create a new connector for health checks
            let connector = Arc::new(RwLock::new(
                crate::source::postgres::PostgresConnector::new((**pg_config).clone()),
            ));

            return Some(Box::new(crate::health::PostgresHealthCheck::new(connector)));
        }

        // If multiple sources, use the first PostgreSQL source
        for named_source in &self.config.sources {
            if let crate::config::SourceConfig::PostgreSQL(pg_config) = &named_source.config {
                // Create a new connector for health checks
                let connector = Arc::new(RwLock::new(
                    crate::source::postgres::PostgresConnector::new((**pg_config).clone()),
                ));

                return Some(Box::new(crate::health::PostgresHealthCheck::new(connector)));
            }
        }

        None
    }

    /// Resume CDC processing for all tables
    pub async fn resume_cdc(&self) -> Result<()> {
        if let Some(coordinator) = &self.cdc_coordinator {
            coordinator.read().await.resume_all().await;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Check if CDC is paused
    pub async fn is_cdc_paused(&self) -> Result<bool> {
        if let Some(coordinator) = &self.cdc_coordinator {
            Ok(coordinator.read().await.is_paused().await)
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Pause CDC processing for a specific table
    pub async fn pause_cdc_table(&self, table_name: &str) -> Result<()> {
        if let Some(coordinator) = &self.cdc_coordinator {
            coordinator.read().await.pause_table(table_name).await;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Resume CDC processing for a specific table
    pub async fn resume_cdc_table(&self, table_name: &str) -> Result<()> {
        if let Some(coordinator) = &self.cdc_coordinator {
            coordinator.read().await.resume_table(table_name).await;
            Ok(())
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Get list of paused tables
    pub async fn get_paused_tables(&self) -> Result<Vec<String>> {
        if let Some(coordinator) = &self.cdc_coordinator {
            Ok(coordinator.read().await.get_paused_tables().await)
        } else {
            Err(MeiliBridgeError::Pipeline(
                "CDC coordinator not initialized".to_string(),
            ))
        }
    }

    /// Trigger full sync for a specific task
    pub async fn trigger_full_sync(&mut self, task_id: &str) -> Result<()> {
        // Find the task configuration
        let task = self
            .config
            .sync_tasks
            .iter()
            .find(|t| t.id == task_id)
            .ok_or_else(|| MeiliBridgeError::NotFound(format!("Task '{}' not found", task_id)))?
            .clone();

        info!("Triggering full sync for task '{}'", task_id);

        // Pause CDC for this table during full sync
        if let Some(coordinator) = &self.cdc_coordinator {
            coordinator.read().await.pause_table(&task.table).await;
        }

        // Perform the full sync
        match self.perform_full_sync(&task).await {
            Ok(_) => {
                info!("Full sync completed successfully for task '{}'", task_id);

                // Resume CDC for this table
                if let Some(coordinator) = &self.cdc_coordinator {
                    coordinator.read().await.resume_table(&task.table).await;
                }

                Ok(())
            }
            Err(e) => {
                error!("Full sync failed for task '{}': {}", task_id, e);

                // Resume CDC even if full sync failed
                if let Some(coordinator) = &self.cdc_coordinator {
                    coordinator.read().await.resume_table(&task.table).await;
                }

                Err(e)
            }
        }
    }

    /// Distribute events to parallel processor
    async fn distribute_events_to_parallel_processor(
        mut event_stream: impl StreamExt<Item = Result<Event>> + Unpin,
        processor: Arc<ParallelTableProcessor>,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        let mut batch = Vec::new();
        let mut batch_timer = interval(Duration::from_millis(100));

        loop {
            tokio::select! {
                Some(event_result) = event_stream.next() => {
                    match event_result {
                        Ok(event) => {
                            batch.push(event);

                            // Send batch when it reaches a reasonable size
                            if batch.len() >= 50 {
                                processor.enqueue_events(std::mem::take(&mut batch)).await;
                            }
                        }
                        Err(e) => {
                            error!("Error receiving event: {}", e);
                        }
                    }
                }

                _ = batch_timer.tick() => {
                    if !batch.is_empty() {
                        processor.enqueue_events(std::mem::take(&mut batch)).await;
                    }
                }

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        // Process remaining events
                        if !batch.is_empty() {
                            processor.enqueue_events(batch).await;
                        }
                        break;
                    }
                }
            }
        }
    }

    /// Process results from parallel workers
    async fn process_parallel_results(
        mut result_rx: mpsc::Receiver<Vec<Event>>,
        destination_tx: mpsc::Sender<(
            DestinationCommand,
            mpsc::Sender<Result<DestinationResponse>>,
        )>,
        checkpoints: Arc<RwLock<HashMap<String, Position>>>,
        table_name: String,
        dead_letter_queue: Option<Arc<DeadLetterQueue>>,
        task_id: String,
        mut shutdown_rx: watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                Some(events) = result_rx.recv() => {
                    if !events.is_empty() {
                        debug!("Processing {} events from parallel workers for table '{}'",
                               events.len(), table_name);

                        // Record batch size metric
                        metrics::record_batch_size(&table_name, events.len());

                        // Send to destination
                        let (resp_tx, mut resp_rx) = mpsc::channel(1);
                        let command = DestinationCommand::ProcessEvents(events);

                        if let Err(e) = destination_tx.send((command, resp_tx)).await {
                            error!("Failed to send events to destination: {}", e);
                            continue;
                        }

                        // Wait for response
                        if let Some(result) = resp_rx.recv().await {
                            match result {
                                Ok(response) => {
                                    if response.success_count > 0 {
                                        info!("Successfully processed {} events for table '{}'",
                                              response.success_count, table_name);
                                    }

                                    if response.failed_count > 0 {
                                        warn!("Failed to process {} events for table '{}'",
                                              response.failed_count, table_name);

                                        // Handle failed events if DLQ is enabled
                                        if let Some(_dlq) = &dead_letter_queue {
                                            // Note: We'd need to track which specific events failed
                                            // This is a simplified version
                                            warn!("Dead letter queue handling for parallel processing needs implementation");
                                        }
                                    }

                                    // Update checkpoint if we have a position
                                    if let Some(position) = response.last_position {
                                        // Update in-memory checkpoint using task_id
                                        // The checkpoint persistence task will handle saving to storage
                                        let mut checkpoints_map = checkpoints.write().await;
                                        checkpoints_map.insert(task_id.clone(), position);
                                    }
                                }
                                Err(e) => {
                                    error!("Destination error for table '{}': {}", table_name, e);
                                }
                            }
                        }
                    }
                }

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!("Shutting down parallel result processor for table '{}'", table_name);
                        break;
                    }
                }
            }
        }
    }
}

impl Drop for PipelineOrchestrator {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }
    }
}
