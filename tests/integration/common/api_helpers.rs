// Common API testing helpers

use axum::Router;
use axum::routing::{delete, get, post, put};
use meilibridge::api::{ApiState, handlers};
use meilibridge::config::Config;
use meilibridge::health::HealthRegistry;
use meilibridge::pipeline::PipelineOrchestrator;
use meilibridge::sync::SyncTaskManager;
use reqwest::Client;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

/// API test server wrapper
pub struct TestApiServer {
    pub addr: SocketAddr,
    pub client: Client,
    pub handle: tokio::task::JoinHandle<()>,
    base_url: String,
}

impl TestApiServer {
    /// Create and start a test API server
    pub async fn new(config: Config) -> Self {
        let state = create_api_state(config).await;

        // Start the task manager
        {
            let mut task_manager = state.task_manager.write().await;
            let _ = task_manager.start().await;
        }

        let app = create_api_router(state);

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("Failed to bind test server");
        let addr = listener.local_addr().unwrap();

        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Wait for server to start
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

        let base_url = format!("http://{}", addr);
        let client = Client::new();

        Self {
            addr,
            client,
            handle,
            base_url,
        }
    }

    /// Get the base URL for the test server
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Make a GET request
    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{}", self.base_url, path))
            .send()
            .await
            .expect("Failed to send GET request")
    }

    /// Make a POST request with JSON body
    pub async fn post_json<T: serde::Serialize>(&self, path: &str, json: &T) -> reqwest::Response {
        self.client
            .post(format!("{}{}", self.base_url, path))
            .json(json)
            .send()
            .await
            .expect("Failed to send POST request")
    }

    /// Make a PUT request with JSON body
    pub async fn put_json<T: serde::Serialize>(&self, path: &str, json: &T) -> reqwest::Response {
        self.client
            .put(format!("{}{}", self.base_url, path))
            .json(json)
            .send()
            .await
            .expect("Failed to send PUT request")
    }

    /// Make a DELETE request
    pub async fn delete(&self, path: &str) -> reqwest::Response {
        self.client
            .delete(format!("{}{}", self.base_url, path))
            .send()
            .await
            .expect("Failed to send DELETE request")
    }

    /// Shutdown the test server
    pub fn shutdown(self) {
        self.handle.abort();
    }
}

/// Create an API state for testing
pub async fn create_api_state(config: Config) -> ApiState {
    let orchestrator =
        PipelineOrchestrator::new(config.clone()).expect("Failed to create orchestrator");
    let task_manager = SyncTaskManager::new(config);

    let state = ApiState::new(
        Arc::new(RwLock::new(orchestrator)),
        Arc::new(RwLock::new(task_manager)),
    );

    // Add health registry
    let health_registry = Arc::new(HealthRegistry::new());
    state.with_health_registry(health_registry)
}

/// Create the API router with all routes
pub fn create_api_router(state: ApiState) -> Router {
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
        // CDC control
        .route("/cdc/status", get(handlers::get_cdc_status))
        // Metrics
        .route("/metrics", get(handlers::get_metrics))
        // Cache control
        .route(
            "/cache/clear",
            post(meilibridge::api::cache_handlers::clear_cache),
        )
        .with_state(state)
}

/// Common API response assertions
pub mod assertions {
    use reqwest::{Response, StatusCode};
    use serde_json::Value;

    pub async fn assert_status(response: Response, expected: StatusCode) -> Response {
        let status = response.status();
        assert_eq!(
            status, expected,
            "Expected status {} but got {}",
            expected, status
        );
        response
    }

    pub async fn assert_json_field(response: Response, field: &str, expected: &Value) -> Value {
        let json: Value = response
            .json()
            .await
            .expect("Failed to parse JSON response");
        let actual = json
            .get(field)
            .unwrap_or_else(|| panic!("Field '{}' not found in response", field));
        assert_eq!(
            actual, expected,
            "Field '{}' expected to be {:?} but was {:?}",
            field, expected, actual
        );
        // Return the parsed JSON for further assertions
        json
    }

    pub async fn get_json(response: Response) -> Value {
        response
            .json()
            .await
            .expect("Failed to parse JSON response")
    }
}
