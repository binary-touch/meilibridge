use crate::api::{ApiState, cache_handlers, diagnostics, handlers, middleware};
use crate::config::Config;
use crate::error::{MeiliBridgeError, Result};
use axum::{
    Router, middleware as axum_middleware,
    routing::{delete, get, post, put},
};
use std::net::SocketAddr;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

/// API server for MeiliBridge
pub struct ApiServer {
    config: Config,
    state: ApiState,
}

impl ApiServer {
    pub fn new(config: Config, state: ApiState) -> Self {
        Self { config, state }
    }

    /// Build the router with all routes
    fn build_router(&self) -> Router {
        Router::new()
            // Health check
            .route("/health", get(handlers::health))
            .route("/health/{component}", get(handlers::get_component_health))
            // Task management
            .route("/tasks", get(handlers::get_tasks))
            .route("/tasks", post(handlers::create_task))
            .route("/tasks/{id}", get(handlers::get_task))
            .route("/tasks/{id}", delete(handlers::delete_task))
            .route("/tasks/{id}/pause", put(handlers::pause_task))
            .route("/tasks/{id}/resume", put(handlers::resume_task))
            .route("/tasks/{id}/full-sync", post(handlers::full_sync_task))
            // CDC control
            .route("/cdc/pause", put(handlers::pause_cdc))
            .route("/cdc/resume", put(handlers::resume_cdc))
            .route("/cdc/status", get(handlers::get_cdc_status))
            // Parallel processing status
            .route("/parallel/status", get(handlers::get_parallel_status))
            .route("/parallel/queues", get(handlers::get_parallel_queues))
            // Dead letter queue
            .route("/dead-letters", get(handlers::get_dead_letter_stats))
            .route(
                "/dead-letters/{task_id}/reprocess",
                post(handlers::reprocess_dead_letters),
            )
            // Metrics endpoint
            .route("/metrics", get(handlers::get_metrics))
            // Statement cache management
            .route("/cache/stats", get(cache_handlers::get_cache_stats))
            .route("/cache/clear", post(cache_handlers::clear_cache))
            // Diagnostic endpoints
            .route(
                "/diagnostics/pipeline/{table}",
                get(diagnostics::get_pipeline_diagnostics),
            )
            .route(
                "/diagnostics/checkpoints",
                get(diagnostics::get_checkpoints_diagnostics),
            )
            .route(
                "/diagnostics/connections",
                get(diagnostics::get_connections_diagnostics),
            )
            .route(
                "/diagnostics/trace/{event_id}",
                get(diagnostics::get_event_trace),
            )
            .route(
                "/diagnostics/replay/{table}",
                post(diagnostics::replay_events),
            )
            .route(
                "/diagnostics/heap-dump",
                post(diagnostics::create_heap_dump),
            )
            // Add middleware
            .layer(axum_middleware::from_fn(middleware::track_metrics))
            .layer(TraceLayer::new_for_http())
            .layer(CorsLayer::permissive())
            .with_state(self.state.clone())
    }

    /// Start the API server
    pub async fn start(self) -> Result<()> {
        let app = self.build_router();

        // Parse API configuration
        let host = &self.config.api.host;
        let port = self.config.api.port;

        let addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .map_err(|e| MeiliBridgeError::Configuration(format!("Invalid API address: {}", e)))?;

        info!("Starting API server on {}", addr);

        // Try to bind with retries
        let mut retry_count = 0;
        let max_retries = 3;
        let listener = loop {
            match tokio::net::TcpListener::bind(addr).await {
                Ok(listener) => break listener,
                Err(e) if retry_count < max_retries => {
                    warn!(
                        "Failed to bind to {}: {}. Retrying in 2 seconds... ({}/{})",
                        addr,
                        e,
                        retry_count + 1,
                        max_retries
                    );
                    retry_count += 1;
                    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                }
                Err(e) => {
                    error!(
                        "Failed to bind to {} after {} retries: {}",
                        addr, max_retries, e
                    );
                    return Err(MeiliBridgeError::Io(e));
                }
            }
        };

        info!("Successfully bound to {}", addr);

        // Start the server with graceful shutdown
        let server = axum::serve(listener, app).with_graceful_shutdown(shutdown_signal());

        info!("API server is ready to accept connections");

        server.await.map_err(MeiliBridgeError::Io)?;

        info!("API server stopped gracefully");
        Ok(())
    }
}

/// Graceful shutdown signal handler
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }

    info!("Received shutdown signal");
}
