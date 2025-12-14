use crate::api::ApiState;
use crate::models::Position;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{debug, info};

/// Pipeline diagnostics for a specific table
#[derive(Debug, Serialize)]
pub struct PipelineDiagnostics {
    pub table: String,
    pub status: PipelineStatus,
    pub checkpoint: CheckpointInfo,
    pub performance: PerformanceMetrics,
    pub queue_stats: QueueStatistics,
    pub error_summary: ErrorSummary,
}

#[derive(Debug, Serialize)]
pub struct PipelineStatus {
    pub is_active: bool,
    pub is_paused: bool,
    pub last_event_at: Option<String>,
    pub current_lag_seconds: f64,
}

#[derive(Debug, Serialize)]
pub struct CheckpointInfo {
    pub position: Option<Position>,
    pub lag_seconds: f64,
    pub last_saved_at: Option<String>,
    pub save_count: u64,
}

#[derive(Debug, Serialize)]
pub struct PerformanceMetrics {
    pub events_per_second: f64,
    pub avg_latency_ms: f64,
    pub p95_latency_ms: f64,
    pub p99_latency_ms: f64,
    pub current_batch_size: usize,
}

#[derive(Debug, Serialize)]
pub struct QueueStatistics {
    pub pending_events: usize,
    pub processing_events: usize,
    pub dead_letter_count: usize,
    pub memory_usage_mb: f64,
}

#[derive(Debug, Serialize)]
pub struct ErrorSummary {
    pub total_errors: u64,
    pub errors_last_hour: u64,
    pub recent_errors: Vec<RecentError>,
}

#[derive(Debug, Serialize)]
pub struct RecentError {
    pub timestamp: String,
    pub error_type: String,
    pub message: String,
    pub count: u64,
}

/// All checkpoints diagnostic response
#[derive(Debug, Serialize)]
pub struct CheckpointsDiagnostics {
    pub total_checkpoints: usize,
    pub checkpoints: Vec<CheckpointDetail>,
}

#[derive(Debug, Serialize)]
pub struct CheckpointDetail {
    pub source: String,
    pub table: String,
    pub position: Position,
    pub lag_seconds: f64,
    pub last_updated: String,
}

/// Connection pool diagnostics
#[derive(Debug, Serialize)]
pub struct ConnectionsDiagnostics {
    pub pools: Vec<PoolDiagnostics>,
    pub total_connections: usize,
    pub total_active: usize,
    pub total_idle: usize,
}

#[derive(Debug, Serialize)]
pub struct PoolDiagnostics {
    pub name: String,
    pub source: String,
    pub max_connections: usize,
    pub active_connections: usize,
    pub idle_connections: usize,
    pub wait_queue_size: usize,
    pub saturation_ratio: f64,
    pub health_status: String,
}

/// Event trace for debugging
#[derive(Debug, Serialize)]
pub struct EventTrace {
    pub event_id: String,
    pub table: String,
    pub trace_points: Vec<TracePoint>,
    pub total_duration_ms: f64,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct TracePoint {
    pub stage: String,
    pub timestamp: String,
    pub duration_ms: f64,
    pub details: HashMap<String, String>,
}

/// Query parameters for event replay
#[derive(Debug, Deserialize)]
pub struct ReplayQuery {
    pub from_lsn: Option<String>,
    pub to_lsn: Option<String>,
    pub dry_run: Option<bool>,
    pub limit: Option<usize>,
}

/// Event replay response
#[derive(Debug, Serialize)]
pub struct ReplayResponse {
    pub mode: String,
    pub events_replayed: usize,
    pub events_skipped: usize,
    pub errors: Vec<String>,
    pub estimated_duration_seconds: f64,
}

/// Get pipeline diagnostics for a specific table
pub async fn get_pipeline_diagnostics(
    State(state): State<ApiState>,
    Path(table): Path<String>,
) -> Result<Json<PipelineDiagnostics>, StatusCode> {
    debug!("Getting pipeline diagnostics for table: {}", table);

    // Get orchestrator reference
    let orchestrator = state.orchestrator.read().await;

    // Get CDC coordinator
    let cdc_coordinator = orchestrator
        .cdc_coordinator
        .as_ref()
        .ok_or(StatusCode::SERVICE_UNAVAILABLE)?;
    let cdc = cdc_coordinator.read().await;

    // Check if table is active
    // Note: CdcCoordinator tracks pause state globally, not per-table
    let is_paused = cdc.is_paused().await;
    let is_active = !is_paused;

    // Get performance metrics from Prometheus (mock for now)
    let performance = PerformanceMetrics {
        events_per_second: 0.0,
        avg_latency_ms: 0.0,
        p95_latency_ms: 0.0,
        p99_latency_ms: 0.0,
        current_batch_size: 100,
    };

    // Get queue statistics
    let queue_stats = QueueStatistics {
        pending_events: 0,
        processing_events: 0,
        dead_letter_count: 0,
        memory_usage_mb: 0.0,
    };

    // Build response
    let diagnostics = PipelineDiagnostics {
        table: table.clone(),
        status: PipelineStatus {
            is_active,
            is_paused,
            last_event_at: None,
            current_lag_seconds: 0.0,
        },
        checkpoint: CheckpointInfo {
            position: None,
            lag_seconds: 0.0,
            last_saved_at: None,
            save_count: 0,
        },
        performance,
        queue_stats,
        error_summary: ErrorSummary {
            total_errors: 0,
            errors_last_hour: 0,
            recent_errors: vec![],
        },
    };

    Ok(Json(diagnostics))
}

/// Get all checkpoints diagnostics
pub async fn get_checkpoints_diagnostics(
    State(_state): State<ApiState>,
) -> Result<Json<CheckpointsDiagnostics>, StatusCode> {
    debug!("Getting checkpoints diagnostics");

    // For now, return empty checkpoints since we can't access private field
    // In real implementation, this would need a public method on orchestrator
    let checkpoint_details = vec![];

    Ok(Json(CheckpointsDiagnostics {
        total_checkpoints: checkpoint_details.len(),
        checkpoints: checkpoint_details,
    }))
}

/// Get connection pool diagnostics
pub async fn get_connections_diagnostics(
    State(_state): State<ApiState>,
) -> Result<Json<ConnectionsDiagnostics>, StatusCode> {
    debug!("Getting connections diagnostics");

    // Mock implementation - in real implementation, query actual connection pools
    let pools = vec![
        PoolDiagnostics {
            name: "cdc_pool".to_string(),
            source: "primary".to_string(),
            max_connections: 5,
            active_connections: 2,
            idle_connections: 3,
            wait_queue_size: 0,
            saturation_ratio: 0.4,
            health_status: "healthy".to_string(),
        },
        PoolDiagnostics {
            name: "sync_pool".to_string(),
            source: "primary".to_string(),
            max_connections: 10,
            active_connections: 1,
            idle_connections: 9,
            wait_queue_size: 0,
            saturation_ratio: 0.1,
            health_status: "healthy".to_string(),
        },
    ];

    let total_active: usize = pools.iter().map(|p| p.active_connections).sum();
    let total_idle: usize = pools.iter().map(|p| p.idle_connections).sum();

    Ok(Json(ConnectionsDiagnostics {
        total_connections: total_active + total_idle,
        total_active,
        total_idle,
        pools,
    }))
}

/// Get event trace for debugging
pub async fn get_event_trace(
    State(_state): State<ApiState>,
    Path(event_id): Path<String>,
) -> Result<Json<EventTrace>, StatusCode> {
    debug!("Getting event trace for: {}", event_id);

    // Mock implementation - in real implementation, query trace storage
    let trace = EventTrace {
        event_id: event_id.clone(),
        table: "users".to_string(),
        trace_points: vec![
            TracePoint {
                stage: "cdc_capture".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                duration_ms: 1.5,
                details: HashMap::from([
                    ("lsn".to_string(), "0/16B3F48".to_string()),
                    ("xid".to_string(), "1234".to_string()),
                ]),
            },
            TracePoint {
                stage: "deduplication".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                duration_ms: 0.3,
                details: HashMap::from([("result".to_string(), "unique".to_string())]),
            },
            TracePoint {
                stage: "transformation".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                duration_ms: 2.1,
                details: HashMap::from([("fields_mapped".to_string(), "5".to_string())]),
            },
            TracePoint {
                stage: "meilisearch_sync".to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
                duration_ms: 15.7,
                details: HashMap::from([
                    ("index".to_string(), "users".to_string()),
                    ("operation".to_string(), "update".to_string()),
                ]),
            },
        ],
        total_duration_ms: 19.6,
        status: "success".to_string(),
    };

    Ok(Json(trace))
}

/// Replay events from a specific position
pub async fn replay_events(
    State(_state): State<ApiState>,
    Path(table): Path<String>,
    Query(params): Query<ReplayQuery>,
) -> Result<Json<ReplayResponse>, StatusCode> {
    info!(
        "Replay events requested for table: {} with params: {:?}",
        table, params
    );

    let dry_run = params.dry_run.unwrap_or(true);
    let _limit = params.limit.unwrap_or(1000);

    if !dry_run {
        // Only allow dry-run in this implementation for safety
        return Err(StatusCode::FORBIDDEN);
    }

    // Mock implementation
    let response = ReplayResponse {
        mode: "dry_run".to_string(),
        events_replayed: 0,
        events_skipped: 0,
        errors: vec![],
        estimated_duration_seconds: 0.0,
    };

    Ok(Json(response))
}

/// Get memory heap dump (returns path to dump file)
#[derive(Debug, Serialize)]
pub struct HeapDumpResponse {
    pub path: String,
    pub size_bytes: u64,
    pub timestamp: String,
}

pub async fn create_heap_dump(
    State(_state): State<ApiState>,
) -> Result<Json<HeapDumpResponse>, StatusCode> {
    info!("Heap dump requested");

    // In a real implementation, this would use jemalloc or similar to create a heap dump
    // For now, return a mock response
    Err(StatusCode::NOT_IMPLEMENTED)
}
