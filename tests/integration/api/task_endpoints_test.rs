// Task management API endpoints integration tests

use crate::common::fixtures::*;
use meilibridge::config::SyncTaskConfig;
use meilibridge::pipeline::orchestrator::PipelineOrchestrator;
use meilibridge::sync::task_manager::{SyncTaskManager, TaskState, TaskStatus};

#[cfg(test)]
mod task_endpoints_tests {
    use super::*;

    #[tokio::test]
    async fn test_task_status_structure() {
        // Test TaskStatus structure without needing API

        // Create a sample task status
        let status = TaskStatus {
            task_id: "test_task".to_string(),
            table: "test_table".to_string(),
            state: TaskState::Idle,
            last_position: None,
            processed_count: 100,
            error_count: 5,
            last_error: Some("Test error".to_string()),
            started_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
        };

        // Convert to response format
        let response = meilibridge::api::handlers::TaskStatusResponse {
            id: status.task_id.clone(),
            status: format!("{:?}", status.state),
            table: status.table.clone(),
            events_processed: status.processed_count,
            last_error: status.last_error.clone(),
            last_sync_at: Some(status.last_updated.to_rfc3339()),
        };

        assert_eq!(response.id, "test_task");
        assert_eq!(response.status, "Idle");
        assert_eq!(response.table, "test_table");
        assert_eq!(response.events_processed, 100);
        assert_eq!(response.last_error, Some("Test error".to_string()));
        assert!(response.last_sync_at.is_some());
    }

    #[tokio::test]
    async fn test_create_task_config() {
        let task_config = SyncTaskConfig {
            id: "new_task".to_string(),
            source_name: "primary".to_string(),
            table: "products".to_string(),
            index: "products_index".to_string(),
            primary_key: "id".to_string(),
            filter: None,
            transform: None,
            mapping: None,
            auto_start: Some(true),
            full_sync_on_start: Some(true),
            soft_delete: None,
            options: Default::default(),
        };

        assert_eq!(task_config.id, "new_task");
        assert_eq!(task_config.table, "products");
        assert_eq!(task_config.index, "products_index");
        assert_eq!(task_config.primary_key, "id");
        assert_eq!(task_config.auto_start, Some(true));
        assert_eq!(task_config.full_sync_on_start, Some(true));
    }

    #[tokio::test]
    async fn test_task_state_transitions() {
        use meilibridge::sync::task_manager::TaskState;

        // Test all task state variants
        let states = vec![
            TaskState::Idle,
            TaskState::Running,
            TaskState::Failed,
            TaskState::Paused,
            TaskState::Completed,
        ];

        for state in states {
            let state_str = format!("{state:?}");
            assert!(!state_str.is_empty());
        }
    }

    #[tokio::test]
    async fn test_task_manager_initialization() {
        let config = create_test_config(
            "postgres://localhost:5432/test",
            "http://localhost:7700",
            "redis://localhost:6379",
        );
        let task_manager = SyncTaskManager::new(config);

        // Task manager should start without command channel
        assert!(task_manager.command_tx.is_none());
    }

    #[tokio::test]
    async fn test_orchestrator_initialization() {
        let config = create_test_config(
            "postgres://localhost:5432/test",
            "http://localhost:7700",
            "redis://localhost:6379",
        );
        let orchestrator = PipelineOrchestrator::new(config);

        assert!(orchestrator.is_ok());
        let _orchestrator = orchestrator.unwrap();

        // Orchestrator created successfully - test passes if we reach here
    }
}
