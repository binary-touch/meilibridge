use lazy_static::lazy_static;
use prometheus::{
    CounterVec, Encoder, GaugeVec, HistogramVec, TextEncoder, register_counter_vec,
    register_gauge_vec, register_histogram_vec,
};
use std::time::Duration;

lazy_static! {
    /// Total number of CDC events received
    pub static ref CDC_EVENTS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_cdc_events_total",
        "Total number of CDC events received",
        &["table", "event_type"]
    ).unwrap();

    /// Total number of CDC events processed successfully
    pub static ref CDC_EVENTS_PROCESSED: CounterVec = register_counter_vec!(
        "meilibridge_cdc_events_processed_total",
        "Total number of CDC events processed successfully",
        &["table", "event_type"]
    ).unwrap();

    /// Total number of CDC events failed
    pub static ref CDC_EVENTS_FAILED: CounterVec = register_counter_vec!(
        "meilibridge_cdc_events_failed_total",
        "Total number of CDC events that failed processing",
        &["table", "event_type", "error_type"]
    ).unwrap();

    /// CDC lag in seconds (difference between event timestamp and processing time)
    pub static ref CDC_LAG_SECONDS: GaugeVec = register_gauge_vec!(
        "meilibridge_cdc_lag_seconds",
        "CDC lag in seconds",
        &["table"]
    ).unwrap();

    /// Current LSN position
    pub static ref REPLICATION_LSN: GaugeVec = register_gauge_vec!(
        "meilibridge_replication_lsn",
        "Current replication LSN position",
        &["slot_name"]
    ).unwrap();

    /// Meilisearch sync latency
    pub static ref MEILISEARCH_SYNC_LATENCY: HistogramVec = register_histogram_vec!(
        "meilibridge_meilisearch_sync_duration_seconds",
        "Time taken to sync data to Meilisearch",
        &["index", "operation"],
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 10.0]
    ).unwrap();

    /// Batch size histogram
    pub static ref BATCH_SIZE: HistogramVec = register_histogram_vec!(
        "meilibridge_batch_size",
        "Number of events in each batch",
        &["table"],
        vec![1.0, 5.0, 10.0, 50.0, 100.0, 500.0, 1000.0]
    ).unwrap();

    /// Active sync tasks
    pub static ref ACTIVE_SYNC_TASKS: GaugeVec = register_gauge_vec!(
        "meilibridge_active_sync_tasks",
        "Number of active sync tasks",
        &["status"]
    ).unwrap();

    /// Connection pool metrics
    pub static ref CONNECTION_POOL_SIZE: GaugeVec = register_gauge_vec!(
        "meilibridge_connection_pool_size",
        "Number of connections in the pool",
        &["pool_name", "state"]
    ).unwrap();

    /// API request metrics
    pub static ref API_REQUESTS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_api_requests_total",
        "Total number of API requests",
        &["method", "endpoint", "status"]
    ).unwrap();

    /// API request duration
    pub static ref API_REQUEST_DURATION: HistogramVec = register_histogram_vec!(
        "meilibridge_api_request_duration_seconds",
        "API request duration in seconds",
        &["method", "endpoint"],
        vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0]
    ).unwrap();

    /// Dead letter queue events
    pub static ref DEAD_LETTER_EVENTS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_dead_letter_events_total",
        "Total number of events sent to dead letter queue",
        &["task_id"]
    ).unwrap();

    /// Dead letter queue size
    pub static ref DEAD_LETTER_QUEUE_SIZE: GaugeVec = register_gauge_vec!(
        "meilibridge_dead_letter_queue_size",
        "Current number of events in dead letter queue",
        &["task_id"]
    ).unwrap();

    /// Dead letter reprocess attempts
    pub static ref DEAD_LETTER_REPROCESS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_dead_letter_reprocess_total",
        "Total number of dead letter reprocess attempts",
        &["task_id", "status"]
    ).unwrap();

    /// CDC pause/resume operations
    pub static ref CDC_CONTROL_OPERATIONS: CounterVec = register_counter_vec!(
        "meilibridge_cdc_control_operations_total",
        "Total number of CDC control operations (pause/resume)",
        &["operation", "scope", "table"]
    ).unwrap();

    /// Parallel processing metrics
    pub static ref PARALLEL_WORKER_EVENTS: CounterVec = register_counter_vec!(
        "meilibridge_parallel_worker_events_total",
        "Total number of events processed by parallel workers",
        &["table", "worker_id"]
    ).unwrap();

    /// Parallel queue size
    pub static ref PARALLEL_QUEUE_SIZE: GaugeVec = register_gauge_vec!(
        "meilibridge_parallel_queue_size",
        "Current number of events in parallel processing queue",
        &["table"]
    ).unwrap();

    /// Work stealing operations
    pub static ref WORK_STEALING_OPERATIONS: CounterVec = register_counter_vec!(
        "meilibridge_work_stealing_operations_total",
        "Total number of work stealing operations",
        &["from_table", "to_table"]
    ).unwrap();

    /// Statement cache size
    pub static ref STATEMENT_CACHE_SIZE: prometheus::IntGauge = prometheus::register_int_gauge!(
        "meilibridge_statement_cache_size",
        "Current number of cached prepared statements"
    ).unwrap();

    /// Statement cache hits
    pub static ref STATEMENT_CACHE_HITS: prometheus::Counter = prometheus::register_counter!(
        "meilibridge_statement_cache_hits_total",
        "Total number of statement cache hits"
    ).unwrap();

    /// Statement cache misses
    pub static ref STATEMENT_CACHE_MISSES: prometheus::Counter = prometheus::register_counter!(
        "meilibridge_statement_cache_misses_total",
        "Total number of statement cache misses"
    ).unwrap();

    /// Statement cache evictions
    pub static ref STATEMENT_CACHE_EVICTIONS: prometheus::Counter = prometheus::register_counter!(
        "meilibridge_statement_cache_evictions_total",
        "Total number of statement cache evictions"
    ).unwrap();

    /// Statement cache hit rate
    pub static ref STATEMENT_CACHE_HIT_RATE: prometheus::Gauge = prometheus::register_gauge!(
        "meilibridge_statement_cache_hit_rate",
        "Statement cache hit rate (0.0 to 1.0)"
    ).unwrap();

    /// Circuit breaker calls
    pub static ref CIRCUIT_BREAKER_CALLS: CounterVec = register_counter_vec!(
        "meilibridge_circuit_breaker_calls_total",
        "Total number of circuit breaker calls",
        &["name", "result"]
    ).unwrap();

    /// Circuit breaker state changes
    pub static ref CIRCUIT_BREAKER_STATE_CHANGES: CounterVec = register_counter_vec!(
        "meilibridge_circuit_breaker_state_changes_total",
        "Total number of circuit breaker state changes",
        &["name", "from_state", "to_state"]
    ).unwrap();

    /// Circuit breaker current state
    pub static ref CIRCUIT_BREAKER_STATE: GaugeVec = register_gauge_vec!(
        "meilibridge_circuit_breaker_state",
        "Current circuit breaker state (0=closed, 1=open, 2=half-open)",
        &["name"]
    ).unwrap();

    // === Phase 3.1 Production Monitoring Metrics ===

    // Data Integrity Metrics
    /// At-least-once delivery violations (duplicate events detected)
    pub static ref AT_LEAST_ONCE_VIOLATIONS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_at_least_once_violations_total",
        "Total number of duplicate events detected in at-least-once delivery",
        &["table", "violation_type"]
    ).unwrap();

    /// Checkpoint lag in seconds
    pub static ref CHECKPOINT_LAG_SECONDS: GaugeVec = register_gauge_vec!(
        "meilibridge_checkpoint_lag_seconds",
        "Checkpoint lag behind current position in seconds",
        &["source", "table"]
    ).unwrap();

    /// Event deduplication hits
    pub static ref DEDUPLICATION_HITS_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_deduplication_hits_total",
        "Total number of duplicate events detected and skipped",
        &["table", "dedup_reason"]
    ).unwrap();

    // Performance Metrics
    /// Processing latency histogram with finer buckets
    pub static ref PROCESSING_LATENCY_HISTOGRAM: HistogramVec = register_histogram_vec!(
        "meilibridge_processing_latency_seconds",
        "Event processing latency from receipt to acknowledgment",
        &["table", "event_type", "stage"],
        vec![0.0001, 0.0005, 0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
    ).unwrap();

    /// Memory usage in bytes
    pub static ref MEMORY_USAGE_BYTES: GaugeVec = register_gauge_vec!(
        "meilibridge_memory_usage_bytes",
        "Current memory usage in bytes",
        &["component", "type"]
    ).unwrap();

    /// Connection pool saturation
    pub static ref CONNECTION_POOL_SATURATION: GaugeVec = register_gauge_vec!(
        "meilibridge_connection_pool_saturation_ratio",
        "Connection pool saturation (used/total connections)",
        &["pool_name", "source"]
    ).unwrap();

    // Business Metrics
    /// Documents synced with detailed status
    pub static ref DOCUMENTS_SYNCED_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_documents_synced_total",
        "Total number of documents synced to Meilisearch",
        &["table", "status", "operation"]
    ).unwrap();

    /// Sync lag by table
    pub static ref SYNC_LAG_BY_TABLE_SECONDS: GaugeVec = register_gauge_vec!(
        "meilibridge_sync_lag_by_table_seconds",
        "Current sync lag per table in seconds",
        &["source", "table", "index"]
    ).unwrap();

    /// Soft deletes processed
    pub static ref SOFT_DELETES_PROCESSED_TOTAL: CounterVec = register_counter_vec!(
        "meilibridge_soft_deletes_processed_total",
        "Total number of soft delete events processed",
        &["table", "field", "value"]
    ).unwrap();

    // Additional Production Metrics
    /// Transaction coordinator state
    pub static ref TRANSACTION_COORDINATOR_STATE: GaugeVec = register_gauge_vec!(
        "meilibridge_transaction_coordinator_state",
        "Transaction coordinator state (0=idle, 1=preparing, 2=committing)",
        &["coordinator_id"]
    ).unwrap();

    /// Adaptive batch size adjustments
    pub static ref ADAPTIVE_BATCH_ADJUSTMENTS: CounterVec = register_counter_vec!(
        "meilibridge_adaptive_batch_adjustments_total",
        "Total number of adaptive batch size adjustments",
        &["table", "direction"]
    ).unwrap();

    /// Current adaptive batch size
    pub static ref CURRENT_ADAPTIVE_BATCH_SIZE: GaugeVec = register_gauge_vec!(
        "meilibridge_current_adaptive_batch_size",
        "Current adaptive batch size per table",
        &["table"]
    ).unwrap();

    /// Memory pressure events
    pub static ref MEMORY_PRESSURE_EVENTS: CounterVec = register_counter_vec!(
        "meilibridge_memory_pressure_events_total",
        "Total number of memory pressure events",
        &["severity", "component"]
    ).unwrap();

    /// Pipeline stage duration
    pub static ref PIPELINE_STAGE_DURATION: HistogramVec = register_histogram_vec!(
        "meilibridge_pipeline_stage_duration_seconds",
        "Duration of each pipeline stage",
        &["stage", "table"],
        vec![0.0001, 0.001, 0.01, 0.05, 0.1, 0.5, 1.0, 5.0]
    ).unwrap();
}

/// Record a CDC event received
pub fn record_cdc_event_received(table: &str, event_type: &str) {
    CDC_EVENTS_TOTAL
        .with_label_values(&[table, event_type])
        .inc();
}

/// Record a CDC event processed successfully
pub fn record_cdc_event_processed(table: &str, event_type: &str) {
    CDC_EVENTS_PROCESSED
        .with_label_values(&[table, event_type])
        .inc();
}

/// Record a CDC event failure
pub fn record_cdc_event_failed(table: &str, event_type: &str, error_type: &str) {
    CDC_EVENTS_FAILED
        .with_label_values(&[table, event_type, error_type])
        .inc();
}

/// Update CDC lag for a table
pub fn update_cdc_lag(table: &str, lag_seconds: f64) {
    CDC_LAG_SECONDS.with_label_values(&[table]).set(lag_seconds);
}

/// Update replication LSN position
pub fn update_replication_lsn(slot_name: &str, lsn: u64) {
    REPLICATION_LSN
        .with_label_values(&[slot_name])
        .set(lsn as f64);
}

/// Record Meilisearch sync operation
pub fn record_meilisearch_sync(index: &str, operation: &str, duration: Duration) {
    MEILISEARCH_SYNC_LATENCY
        .with_label_values(&[index, operation])
        .observe(duration.as_secs_f64());
}

/// Record batch size
pub fn record_batch_size(table: &str, size: usize) {
    BATCH_SIZE.with_label_values(&[table]).observe(size as f64);
}

/// Update active sync tasks count
pub fn update_active_sync_tasks(status: &str, count: i64) {
    ACTIVE_SYNC_TASKS
        .with_label_values(&[status])
        .set(count as f64);
}

/// Update connection pool metrics
pub fn update_connection_pool_size(pool_name: &str, state: &str, size: usize) {
    CONNECTION_POOL_SIZE
        .with_label_values(&[pool_name, state])
        .set(size as f64);
}

/// Record API request
pub fn record_api_request(method: &str, endpoint: &str, status: u16, duration: Duration) {
    let status_str = status.to_string();
    API_REQUESTS_TOTAL
        .with_label_values(&[method, endpoint, &status_str])
        .inc();
    API_REQUEST_DURATION
        .with_label_values(&[method, endpoint])
        .observe(duration.as_secs_f64());
}

// === Helper functions for new production metrics ===

/// Record an at-least-once delivery violation (duplicate event)
pub fn record_at_least_once_violation(table: &str, violation_type: &str) {
    AT_LEAST_ONCE_VIOLATIONS_TOTAL
        .with_label_values(&[table, violation_type])
        .inc();
}

/// Update checkpoint lag
pub fn update_checkpoint_lag(source: &str, table: &str, lag_seconds: f64) {
    CHECKPOINT_LAG_SECONDS
        .with_label_values(&[source, table])
        .set(lag_seconds);
}

/// Record a deduplication hit
pub fn record_deduplication_hit(table: &str, reason: &str) {
    DEDUPLICATION_HITS_TOTAL
        .with_label_values(&[table, reason])
        .inc();
}

/// Record processing latency for a specific stage
pub fn record_processing_latency(table: &str, event_type: &str, stage: &str, duration: Duration) {
    PROCESSING_LATENCY_HISTOGRAM
        .with_label_values(&[table, event_type, stage])
        .observe(duration.as_secs_f64());
}

/// Update memory usage
pub fn update_memory_usage(component: &str, memory_type: &str, bytes: u64) {
    MEMORY_USAGE_BYTES
        .with_label_values(&[component, memory_type])
        .set(bytes as f64);
}

/// Update connection pool saturation
pub fn update_connection_pool_saturation(pool_name: &str, source: &str, ratio: f64) {
    CONNECTION_POOL_SATURATION
        .with_label_values(&[pool_name, source])
        .set(ratio);
}

/// Record documents synced
pub fn record_documents_synced(table: &str, status: &str, operation: &str, count: u64) {
    DOCUMENTS_SYNCED_TOTAL
        .with_label_values(&[table, status, operation])
        .inc_by(count as f64);
}

/// Update sync lag by table
pub fn update_sync_lag_by_table(source: &str, table: &str, index: &str, lag_seconds: f64) {
    SYNC_LAG_BY_TABLE_SECONDS
        .with_label_values(&[source, table, index])
        .set(lag_seconds);
}

/// Record soft delete processed
pub fn record_soft_delete_processed(table: &str, field: &str, value: &str) {
    SOFT_DELETES_PROCESSED_TOTAL
        .with_label_values(&[table, field, value])
        .inc();
}

/// Update transaction coordinator state
pub fn update_transaction_coordinator_state(coordinator_id: &str, state: i32) {
    TRANSACTION_COORDINATOR_STATE
        .with_label_values(&[coordinator_id])
        .set(state as f64);
}

/// Record adaptive batch adjustment
pub fn record_adaptive_batch_adjustment(table: &str, direction: &str) {
    ADAPTIVE_BATCH_ADJUSTMENTS
        .with_label_values(&[table, direction])
        .inc();
}

/// Update current adaptive batch size
pub fn update_adaptive_batch_size(table: &str, size: usize) {
    CURRENT_ADAPTIVE_BATCH_SIZE
        .with_label_values(&[table])
        .set(size as f64);
}

/// Record memory pressure event
pub fn record_memory_pressure_event(severity: &str, component: &str) {
    MEMORY_PRESSURE_EVENTS
        .with_label_values(&[severity, component])
        .inc();
}

/// Record pipeline stage duration
pub fn record_pipeline_stage_duration(stage: &str, table: &str, duration: Duration) {
    PIPELINE_STAGE_DURATION
        .with_label_values(&[stage, table])
        .observe(duration.as_secs_f64());
}

/// Export metrics in Prometheus format
pub fn export_metrics() -> Result<String, prometheus::Error> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder.encode(&metric_families, &mut buffer)?;
    String::from_utf8(buffer).map_err(|e| prometheus::Error::Msg(e.to_string()))
}

/// Initialize process metrics
pub fn init_process_metrics() {
    // Process metrics are automatically registered by prometheus crate
    // when using the "process" feature flag
}
