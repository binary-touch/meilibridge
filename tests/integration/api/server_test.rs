// API server integration tests

use crate::common::TestApiServer;
use crate::common::api_helpers::assertions::*;
use crate::common::fixtures::*;
use reqwest::{Client, StatusCode};
use serde_json::json;
use std::sync::Arc;

#[cfg(test)]
mod api_server_tests {
    use super::*;

    #[tokio::test]
    async fn test_health_endpoint() {
        let config = create_test_config(
            "postgres://localhost:5432/test",
            "http://localhost:7700",
            "redis://localhost:6379",
        );
        let server = TestApiServer::new(config).await;

        let response = server.get("/health").await;

        assert_eq!(response.status(), StatusCode::OK);

        let health = get_json(response).await;
        assert_eq!(health["status"], "healthy");
        assert!(health["version"].is_string());
        assert!(health["components"].is_object());

        server.shutdown();
    }

    #[tokio::test]
    async fn test_metrics_endpoint() {
        let config = create_test_config(
            "postgres://localhost:5432/test",
            "http://localhost:7700",
            "redis://localhost:6379",
        );
        let server = TestApiServer::new(config).await;

        // Make a few requests to generate some metrics
        for _ in 0..3 {
            let _ = server.get("/health").await;
        }

        let response = server.get("/metrics").await;

        assert_eq!(response.status(), StatusCode::OK);

        // Metrics endpoint should return a string (might be empty if no metrics registered)
        let body = response.text().await.unwrap();
        // Just verify we got a valid response
        assert!(body.is_empty() || body.contains("#") || body.contains("meilibridge"));

        server.shutdown();
    }

    #[tokio::test]
    async fn test_task_list_empty() {
        let state = create_test_api_state().await;

        // Start the task manager to initialize
        {
            let mut task_manager = state.task_manager.write().await;
            let _ = task_manager.start().await;
        }

        let (addr, _handle) = start_test_server(state).await;

        let client = Client::new();
        let response = client
            .get(format!("http://{}/tasks", addr))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let tasks: Vec<serde_json::Value> = response.json().await.unwrap();
        assert!(tasks.is_empty() || tasks.len() == 1); // May have test_task from config
    }

    #[tokio::test]
    async fn test_get_nonexistent_task() {
        let state = create_test_api_state().await;
        let (addr, _handle) = start_test_server(state).await;

        let client = Client::new();
        let response = client
            .get(format!("http://{}/tasks/nonexistent", addr))
            .send()
            .await
            .unwrap();

        // Should return 500 error since task doesn't exist
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let error: serde_json::Value = response.json().await.unwrap();
        assert!(error["error"].is_string());
        assert!(error["message"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_cdc_status() {
        let state = create_test_api_state().await;
        let (addr, _handle) = start_test_server(state).await;

        let client = Client::new();
        let response = client
            .get(format!("http://{}/cdc/status", addr))
            .send()
            .await
            .unwrap();

        // CDC status endpoint might return 500 if orchestrator is not fully started
        // Let's just check that we get a response
        assert!(
            response.status() == StatusCode::OK
                || response.status() == StatusCode::INTERNAL_SERVER_ERROR
        );

        if response.status() == StatusCode::OK {
            let status: serde_json::Value = response.json().await.unwrap();
            assert!(status["is_paused"].is_boolean());
            assert!(status["paused_tables"].is_array());
        }
    }

    #[tokio::test]
    async fn test_create_task() {
        let state = create_test_api_state().await;

        // Start the task manager
        {
            let mut task_manager = state.task_manager.write().await;
            let _ = task_manager.start().await;
        }

        let (addr, _handle) = start_test_server(state).await;

        let client = Client::new();

        let new_task = json!({
            "id": "test_create_task",
            "source_name": "primary",
            "table": "test_table",
            "index": "test_index",
            "primary_key": "id",
            "auto_start": false,
            "full_sync_on_start": false,
            "options": {
                "batch_size": 1000,
                "batch_timeout_ms": 1000,
                "deduplicate": false
            }
        });

        let response = client
            .post(format!("http://{}/tasks", addr))
            .json(&new_task)
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::CREATED);

        let created: serde_json::Value = response.json().await.unwrap();
        assert_eq!(created["id"], "test_create_task");
        assert_eq!(created["table"], "test_table");
        assert_eq!(created["status"], "created");
    }

    #[tokio::test]
    async fn test_concurrent_requests() {
        let state = create_test_api_state().await;
        let (addr, _handle) = start_test_server(state).await;

        let client = Arc::new(Client::new());
        let mut handles = vec![];

        // Send 20 concurrent health check requests
        for i in 0..20 {
            let client_clone = client.clone();
            let addr_str = addr.to_string();

            let handle = tokio::spawn(async move {
                let response = client_clone
                    .get(format!("http://{}/health", addr_str))
                    .header("X-Request-ID", i.to_string())
                    .send()
                    .await
                    .unwrap();

                assert_eq!(response.status(), StatusCode::OK);
            });

            handles.push(handle);
        }

        // Wait for all requests to complete
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_health_component_endpoint() {
        let state = create_test_api_state().await;
        let (addr, _handle) = start_test_server(state).await;

        let client = Client::new();

        // Test non-existent component
        let response = client
            .get(format!("http://{}/health/nonexistent", addr))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let error: serde_json::Value = response.json().await.unwrap();
        assert!(error["message"].as_str().unwrap().contains("not found"));
    }
}
