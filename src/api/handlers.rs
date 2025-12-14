use crate::api::ApiState;
use crate::config::SyncTaskConfig;
use crate::error::MeiliBridgeError;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::info;

/// Health check response
#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
    pub uptime_seconds: u64,
}

/// Task status response
#[derive(Debug, Serialize)]
pub struct TaskStatusResponse {
    pub id: String,
    pub status: String,
    pub table: String,
    pub events_processed: u64,
    pub last_error: Option<String>,
    pub last_sync_at: Option<String>,
}

/// Error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

impl From<MeiliBridgeError> for ErrorResponse {
    fn from(err: MeiliBridgeError) -> Self {
        Self {
            error: format!("{:?}", err),
            message: err.to_string(),
        }
    }
}

/// Convert error to response
impl IntoResponse for MeiliBridgeError {
    fn into_response(self) -> axum::response::Response {
        let error_response = ErrorResponse::from(self);
        (StatusCode::INTERNAL_SERVER_ERROR, Json(error_response)).into_response()
    }
}

/// Health check endpoint
pub async fn health(State(state): State<ApiState>) -> Result<impl IntoResponse, MeiliBridgeError> {
    if let Some(registry) = state.health_registry() {
        let system_health = registry.get_system_health().await;

        let status_code = match system_health.status {
            crate::health::HealthStatus::Healthy => StatusCode::OK,
            crate::health::HealthStatus::Degraded => StatusCode::OK,
            crate::health::HealthStatus::Unhealthy => StatusCode::SERVICE_UNAVAILABLE,
        };

        Ok((status_code, Json(system_health)))
    } else {
        // Fallback if health registry is not available
        Ok((
            StatusCode::OK,
            Json(crate::health::SystemHealth {
                status: crate::health::HealthStatus::Healthy,
                components: HashMap::new(),
                version: env!("CARGO_PKG_VERSION").to_string(),
                uptime_seconds: 0,
            }),
        ))
    }
}

/// Get all sync tasks status
pub async fn get_tasks(
    State(state): State<ApiState>,
) -> Result<Json<Vec<TaskStatusResponse>>, MeiliBridgeError> {
    let task_manager = state.task_manager.read().await;
    let statuses = task_manager.get_all_task_statuses().await;

    let responses: Vec<TaskStatusResponse> = statuses
        .into_iter()
        .map(|status| TaskStatusResponse {
            id: status.task_id,
            status: format!("{:?}", status.state),
            table: status.table,
            events_processed: status.processed_count,
            last_error: status.last_error,
            last_sync_at: Some(status.last_updated.to_rfc3339()),
        })
        .collect();

    Ok(Json(responses))
}

/// Get specific task status
pub async fn get_task(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
) -> Result<Json<TaskStatusResponse>, MeiliBridgeError> {
    let task_manager = state.task_manager.read().await;

    if let Some(status) = task_manager.get_task_status(&task_id).await {
        Ok(Json(TaskStatusResponse {
            id: status.task_id,
            status: format!("{:?}", status.state),
            table: status.table,
            events_processed: status.processed_count,
            last_error: status.last_error,
            last_sync_at: Some(status.last_updated.to_rfc3339()),
        }))
    } else {
        Err(MeiliBridgeError::NotFound(format!(
            "Task '{}' not found",
            task_id
        )))
    }
}

/// Pause a task
pub async fn pause_task(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, MeiliBridgeError> {
    info!("Pausing task '{}'", task_id);

    // First update the task status in the manager
    let task_manager = state.task_manager.read().await;
    if let Some(tx) = task_manager.command_tx.as_ref() {
        let _ = tx
            .send(crate::sync::task_manager::TaskCommand::Pause(
                task_id.clone(),
            ))
            .await;
    }

    // Then pause the CDC processing for this table
    let orchestrator = state.orchestrator.read().await;
    orchestrator.pause_cdc_table(&task_id).await?;

    info!("Successfully paused task '{}'", task_id);
    Ok(StatusCode::OK)
}

/// Resume a task
pub async fn resume_task(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, MeiliBridgeError> {
    info!("Resuming task '{}'", task_id);

    // First update the task status in the manager
    let task_manager = state.task_manager.read().await;
    if let Some(tx) = task_manager.command_tx.as_ref() {
        let _ = tx
            .send(crate::sync::task_manager::TaskCommand::Resume(
                task_id.clone(),
            ))
            .await;
    }

    // Then resume the CDC processing for this table
    let orchestrator = state.orchestrator.read().await;
    orchestrator.resume_cdc_table(&task_id).await?;

    info!("Successfully resumed task '{}'", task_id);
    Ok(StatusCode::OK)
}

/// Trigger full sync for a task
pub async fn full_sync_task(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, MeiliBridgeError> {
    info!("Triggering full sync for task '{}'", task_id);

    // Get a write lock on the orchestrator to call the mutable method
    let mut orchestrator = state.orchestrator.write().await;

    // Trigger the full sync
    orchestrator.trigger_full_sync(&task_id).await?;

    Ok(StatusCode::ACCEPTED)
}

/// Dead letter queue statistics
#[derive(Debug, Serialize)]
pub struct DeadLetterStats {
    pub task_id: String,
    pub count: usize,
}

/// Get dead letter queue statistics
pub async fn get_dead_letter_stats(
    State(state): State<ApiState>,
) -> Result<Json<crate::dlq::DlqStatistics>, MeiliBridgeError> {
    let orchestrator = state.orchestrator.read().await;
    let stats = orchestrator.get_dlq_statistics().await?;
    Ok(Json(stats))
}

/// Reprocess dead letter entries
#[derive(Debug, Deserialize)]
pub struct ReprocessRequest {
    pub limit: Option<usize>,
}

pub async fn reprocess_dead_letters(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
    Json(request): Json<ReprocessRequest>,
) -> Result<StatusCode, MeiliBridgeError> {
    let limit = request.limit.unwrap_or(100);

    info!(
        "Reprocessing up to {} dead letter entries for task '{}'",
        limit, task_id
    );

    let orchestrator = state.orchestrator.read().await;
    let count = orchestrator
        .reprocess_dlq_entries(&task_id, Some(limit))
        .await?;

    info!("Successfully queued {} entries for reprocessing", count);

    // Update metrics
    crate::metrics::DEAD_LETTER_REPROCESS_TOTAL
        .with_label_values(&[task_id.as_str(), "initiated"])
        .inc_by(count as f64);

    Ok(StatusCode::ACCEPTED)
}

/// Create a new sync task
pub async fn create_task(
    State(state): State<ApiState>,
    Json(config): Json<SyncTaskConfig>,
) -> Result<(StatusCode, Json<TaskStatusResponse>), MeiliBridgeError> {
    // Create the task in the task manager
    let task_manager = state.task_manager.read().await;
    task_manager.create_sync_task(config.clone()).await?;

    // Also register the task with the orchestrator for CDC
    let orchestrator = state.orchestrator.read().await;
    if let Some(cdc_coordinator) = &orchestrator.cdc_coordinator {
        let (tx, _rx) = tokio::sync::mpsc::channel(1000);
        cdc_coordinator
            .write()
            .await
            .register_task(config.table.clone(), tx);
    }

    info!("Created new sync task '{}'", config.id);

    Ok((
        StatusCode::CREATED,
        Json(TaskStatusResponse {
            id: config.id.clone(),
            status: "created".to_string(),
            table: config.table.clone(),
            events_processed: 0,
            last_error: None,
            last_sync_at: Some(chrono::Utc::now().to_rfc3339()),
        }),
    ))
}

/// Delete a sync task
pub async fn delete_task(
    State(state): State<ApiState>,
    Path(task_id): Path<String>,
) -> Result<StatusCode, MeiliBridgeError> {
    // Delete the task from the task manager
    let task_manager = state.task_manager.read().await;
    task_manager.delete_sync_task(&task_id).await?;

    // Also unregister the task from the orchestrator
    let orchestrator = state.orchestrator.read().await;
    if let Some(cdc_coordinator) = &orchestrator.cdc_coordinator {
        cdc_coordinator.write().await.unregister_task(&task_id);
    }

    info!("Deleted sync task '{}'", task_id);

    Ok(StatusCode::NO_CONTENT)
}

/// Get Prometheus metrics
pub async fn get_metrics() -> Result<String, MeiliBridgeError> {
    crate::metrics::export_metrics()
        .map_err(|e| MeiliBridgeError::Pipeline(format!("Failed to export metrics: {}", e)))
}

/// Get health check for a specific component
pub async fn get_component_health(
    State(state): State<ApiState>,
    Path(component): Path<String>,
) -> Result<impl IntoResponse, MeiliBridgeError> {
    if let Some(registry) = state.health_registry() {
        if let Some(health) = registry.get_component_health(&component).await {
            Ok((StatusCode::OK, Json(health)))
        } else {
            Err(MeiliBridgeError::NotFound(format!(
                "Component '{}' not found",
                component
            )))
        }
    } else {
        Err(MeiliBridgeError::Pipeline(
            "Health registry not available".to_string(),
        ))
    }
}

/// CDC status response
#[derive(Debug, Serialize)]
pub struct CdcStatusResponse {
    pub is_paused: bool,
    pub paused_tables: Vec<String>,
}

/// Pause all CDC processing
pub async fn pause_cdc(State(state): State<ApiState>) -> Result<StatusCode, MeiliBridgeError> {
    info!("Pausing all CDC processing");

    let orchestrator = state.orchestrator.read().await;
    orchestrator.pause_cdc().await?;

    info!("Successfully paused all CDC processing");
    Ok(StatusCode::OK)
}

/// Resume all CDC processing
pub async fn resume_cdc(State(state): State<ApiState>) -> Result<StatusCode, MeiliBridgeError> {
    info!("Resuming all CDC processing");

    let orchestrator = state.orchestrator.read().await;
    orchestrator.resume_cdc().await?;

    info!("Successfully resumed all CDC processing");
    Ok(StatusCode::OK)
}

/// Get CDC status
pub async fn get_cdc_status(
    State(state): State<ApiState>,
) -> Result<Json<CdcStatusResponse>, MeiliBridgeError> {
    let orchestrator = state.orchestrator.read().await;

    let is_paused = orchestrator.is_cdc_paused().await?;
    let paused_tables = orchestrator.get_paused_tables().await?;

    Ok(Json(CdcStatusResponse {
        is_paused,
        paused_tables,
    }))
}

/// Parallel processing status response
#[derive(Debug, Serialize)]
pub struct ParallelStatusResponse {
    pub enabled: bool,
    pub workers_per_table: usize,
    pub max_concurrent_events: usize,
    pub work_stealing_enabled: bool,
    pub tables: Vec<ParallelTableStatus>,
}

#[derive(Debug, Serialize)]
pub struct ParallelTableStatus {
    pub table_name: String,
    pub queue_size: usize,
    pub workers: usize,
}

/// Get parallel processing status
pub async fn get_parallel_status(
    State(state): State<ApiState>,
) -> Result<Json<ParallelStatusResponse>, MeiliBridgeError> {
    let orchestrator = state.orchestrator.read().await;

    // Get configuration
    let config = &orchestrator.config.performance.parallel_processing;

    // Get table statuses
    let mut tables = Vec::new();
    for (table_name, processor) in orchestrator.parallel_processors.iter() {
        tables.push(ParallelTableStatus {
            table_name: table_name.clone(),
            queue_size: processor.queue_size().await,
            workers: config.workers_per_table,
        });
    }

    Ok(Json(ParallelStatusResponse {
        enabled: config.enabled,
        workers_per_table: config.workers_per_table,
        max_concurrent_events: config.max_concurrent_events,
        work_stealing_enabled: config.work_stealing,
        tables,
    }))
}

/// Queue status response
#[derive(Debug, Serialize)]
pub struct QueueStatusResponse {
    pub queues: HashMap<String, usize>,
    pub total_events: usize,
}

/// Get parallel queue sizes
pub async fn get_parallel_queues(
    State(state): State<ApiState>,
) -> Result<Json<QueueStatusResponse>, MeiliBridgeError> {
    let orchestrator = state.orchestrator.read().await;

    let mut queues = HashMap::new();
    let mut total_events = 0;

    for (table_name, processor) in orchestrator.parallel_processors.iter() {
        let size = processor.queue_size().await;
        queues.insert(table_name.clone(), size);
        total_events += size;
    }

    Ok(Json(QueueStatusResponse {
        queues,
        total_events,
    }))
}
