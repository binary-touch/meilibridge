use crate::metrics;
use crate::models::stream_event::Event;
use crate::pipeline::{
    filter::EventFilter, mapper::FieldMapper, soft_delete::SoftDeleteHandler,
    transformer::EventTransformer,
};
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore, mpsc};
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

/// Configuration for parallel processing
#[derive(Debug, Clone)]
pub struct ParallelConfig {
    /// Number of worker threads per table
    pub workers_per_table: usize,
    /// Maximum events in flight across all workers
    pub max_concurrent_events: usize,
    /// Enable work stealing between tables
    pub work_stealing: bool,
    /// Work stealing check interval
    pub steal_interval_ms: u64,
}

impl Default for ParallelConfig {
    fn default() -> Self {
        Self {
            workers_per_table: 4,
            max_concurrent_events: 1000,
            work_stealing: true,
            steal_interval_ms: 100,
        }
    }
}

/// Parameters for worker loop
struct WorkerParams {
    worker_id: usize,
    table_name: String,
    event_queue: Arc<RwLock<Vec<Event>>>,
    semaphore: Arc<Semaphore>,
    filter: Option<EventFilter>,
    transformer: Option<EventTransformer>,
    mapper: Option<FieldMapper>,
    soft_delete_handler: Option<SoftDeleteHandler>,
    destination_tx: mpsc::Sender<Vec<Event>>,
}

/// Manages parallel event processing for a table
pub struct ParallelTableProcessor {
    table_name: String,
    config: ParallelConfig,
    /// Shared queue for events waiting to be processed
    event_queue: Arc<RwLock<Vec<Event>>>,
    /// Semaphore to limit concurrent processing
    processing_semaphore: Arc<Semaphore>,
    /// Worker handles
    worker_handles: Vec<tokio::task::JoinHandle<()>>,
}

impl ParallelTableProcessor {
    pub fn new(table_name: String, config: ParallelConfig) -> Self {
        let processing_semaphore = Arc::new(Semaphore::new(config.max_concurrent_events));

        Self {
            table_name,
            config,
            event_queue: Arc::new(RwLock::new(Vec::new())),
            processing_semaphore,
            worker_handles: Vec::new(),
        }
    }

    /// Start parallel workers for this table
    pub fn start_workers(
        &mut self,
        filter: Option<EventFilter>,
        transformer: Option<EventTransformer>,
        mapper: Option<FieldMapper>,
        soft_delete_handler: Option<SoftDeleteHandler>,
        destination_tx: mpsc::Sender<Vec<Event>>,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        info!(
            "Starting {} parallel workers for table '{}'",
            self.config.workers_per_table, self.table_name
        );

        for worker_id in 0..self.config.workers_per_table {
            let table_name = self.table_name.clone();
            let event_queue = self.event_queue.clone();
            let semaphore = self.processing_semaphore.clone();
            let filter = filter.clone();
            let transformer = transformer.clone();
            let mapper = mapper.clone();
            let soft_delete_handler = soft_delete_handler.clone();
            let destination_tx = destination_tx.clone();
            let mut shutdown_rx = shutdown_rx.clone();

            let handle = tokio::spawn(async move {
                let params = WorkerParams {
                    worker_id,
                    table_name,
                    event_queue,
                    semaphore,
                    filter,
                    transformer,
                    mapper,
                    soft_delete_handler,
                    destination_tx,
                };
                Self::worker_loop(params, &mut shutdown_rx).await;
            });

            self.worker_handles.push(handle);
        }
    }

    /// Add events to the processing queue
    pub async fn enqueue_events(&self, events: Vec<Event>) {
        let mut queue = self.event_queue.write().await;
        let event_count = events.len();
        queue.extend(events);
        let queue_size = queue.len();
        debug!(
            "Table '{}': Enqueued {} events, queue size: {}",
            self.table_name, event_count, queue_size
        );

        // Update metrics
        metrics::PARALLEL_QUEUE_SIZE
            .with_label_values(&[&self.table_name])
            .set(queue_size as f64);
    }

    /// Get current queue size
    pub async fn queue_size(&self) -> usize {
        self.event_queue.read().await.len()
    }

    /// Steal events from this processor's queue
    pub async fn steal_events(&self, count: usize) -> Vec<Event> {
        let mut queue = self.event_queue.write().await;
        let steal_count = count.min(queue.len() / 2); // Only steal up to half

        if steal_count > 0 {
            let mut stolen = Vec::with_capacity(steal_count);
            for _ in 0..steal_count {
                if let Some(event) = queue.pop() {
                    stolen.push(event);
                }
            }
            debug!(
                "Table '{}': {} events stolen from queue",
                self.table_name,
                stolen.len()
            );
            stolen
        } else {
            Vec::new()
        }
    }

    /// Worker loop for processing events
    async fn worker_loop(
        params: WorkerParams,
        shutdown_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) {
        info!(
            "Worker {} for table '{}' started",
            params.worker_id, params.table_name
        );
        let mut processed_count = 0;
        let mut check_interval = interval(Duration::from_millis(10));

        loop {
            tokio::select! {
                _ = check_interval.tick() => {
                    // Try to get events from queue
                    let events_to_process = {
                        let mut queue = params.event_queue.write().await;
                        let take_count = 10.min(queue.len()); // Process up to 10 at a time
                        queue.drain(..take_count).collect::<Vec<_>>()
                    };

                    if !events_to_process.is_empty() {
                        // Acquire permits for processing
                        let permits = params.semaphore
                            .acquire_many(events_to_process.len() as u32)
                            .await
                            .unwrap();

                        let mut processed_events = Vec::new();

                        for event in events_to_process {
                            if let Some(processed) = Self::process_single_event(
                                event,
                                &params.filter,
                                &params.transformer,
                                &params.mapper,
                                &params.soft_delete_handler,
                            ).await {
                                processed_events.push(processed);
                                processed_count += 1;
                            }
                        }

                        if !processed_events.is_empty() {
                            // Record metrics
                            metrics::PARALLEL_WORKER_EVENTS
                                .with_label_values(&[&params.table_name, &params.worker_id.to_string()])
                                .inc_by(processed_events.len() as f64);

                            if params.destination_tx.send(processed_events).await.is_err() {
                                warn!(
                                    "Worker {} for table '{}': Failed to send to destination",
                                    params.worker_id, params.table_name
                                );
                            }
                        }

                        // Release permits
                        drop(permits);

                        // Update queue size metric
                        let queue_size = params.event_queue.read().await.len();
                        metrics::PARALLEL_QUEUE_SIZE
                            .with_label_values(&[&params.table_name])
                            .set(queue_size as f64);
                    }
                }

                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        info!(
                            "Worker {} for table '{}' shutting down, processed {} events",
                            params.worker_id, params.table_name, processed_count
                        );
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

        // Apply soft delete handler
        if let Some(sdh) = soft_delete_handler {
            match sdh.transform_event(current_event) {
                Some(transformed) => current_event = transformed,
                None => return None, // Soft-deleted record filtered out
            }
        }

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

        Some(current_event)
    }

    /// Shutdown all workers
    pub async fn shutdown(mut self) {
        for handle in self.worker_handles.drain(..) {
            let _ = handle.await;
        }
    }
}

/// Type alias for processor registry
type ProcessorRegistry = Arc<RwLock<Vec<(String, Arc<ParallelTableProcessor>)>>>;

/// Manages work stealing between table processors
pub struct WorkStealingCoordinator {
    processors: ProcessorRegistry,
    config: ParallelConfig,
}

impl WorkStealingCoordinator {
    pub fn new(config: ParallelConfig) -> Self {
        Self {
            processors: Arc::new(RwLock::new(Vec::new())),
            config,
        }
    }

    /// Register a table processor
    pub async fn register_processor(
        &self,
        table_name: String,
        processor: Arc<ParallelTableProcessor>,
    ) {
        let mut processors = self.processors.write().await;
        processors.push((table_name, processor));
    }

    /// Start the work stealing coordinator
    pub fn start(
        &self,
        shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let processors = self.processors.clone();
        let steal_interval = Duration::from_millis(self.config.steal_interval_ms);

        tokio::spawn(async move {
            let mut interval = interval(steal_interval);
            let mut shutdown_rx = shutdown_rx;

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        Self::balance_work(&processors).await;
                    }

                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Work stealing coordinator shutting down");
                            break;
                        }
                    }
                }
            }
        })
    }

    /// Balance work between processors
    async fn balance_work(processors: &ProcessorRegistry) {
        let processors = processors.read().await;
        if processors.len() < 2 {
            return; // Need at least 2 processors for work stealing
        }

        // Find queue sizes
        let mut queue_info = Vec::new();
        for (table_name, processor) in processors.iter() {
            let size = processor.queue_size().await;
            queue_info.push((table_name.clone(), processor.clone(), size));
        }

        // Sort by queue size
        queue_info.sort_by_key(|&(_, _, size)| size);

        // Steal from largest to smallest
        if let (
            Some((small_table, small_proc, small_size)),
            Some((large_table, large_proc, large_size)),
        ) = (queue_info.first(), queue_info.last())
            && *large_size > *small_size + 100
        {
            // Only steal if significant imbalance
            debug!(
                "Attempting work stealing: {} ({}) -> {} ({})",
                large_table, large_size, small_table, small_size
            );

            // Try to steal a batch from the largest queue
            let stolen = large_proc.steal_events(50).await;
            if !stolen.is_empty() {
                debug!("Stole {} events from {}", stolen.len(), large_table);
                small_proc.enqueue_events(stolen).await;
            }
        }
    }
}
