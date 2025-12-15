// Advanced unit tests for dead letter queue

use chrono::Utc;
use meilibridge::dlq::{DeadLetterQueue, DlqStorage, InMemoryDlqStorage};
use meilibridge::error::MeiliBridgeError;
use meilibridge::models::event::{
    Event, EventData, EventId, EventMetadata, EventSource, EventType,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

#[cfg(test)]
mod dlq_tests {
    use super::*;

    fn create_test_event(table: &str, id: &str) -> Event {
        let mut new_data = HashMap::new();
        new_data.insert("id".to_string(), serde_json::json!(id));
        new_data.insert("name".to_string(), serde_json::json!("Test"));

        Event {
            id: EventId::new(),
            event_type: EventType::Create,
            source: EventSource {
                database: "test_db".to_string(),
                schema: "public".to_string(),
                table: table.to_string(),
            },
            data: EventData {
                key: serde_json::json!(id),
                old: None,
                new: Some(new_data),
            },
            metadata: EventMetadata {
                transaction_id: Some("100".to_string()),
                position: format!("0/{id}"),
                custom: HashMap::new(),
            },
            timestamp: Utc::now(),
        }
    }

    fn create_test_error(msg: &str) -> MeiliBridgeError {
        MeiliBridgeError::Pipeline(msg.to_string())
    }

    #[tokio::test]
    async fn test_dlq_add_and_retrieve() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        let event = create_test_event("users", "1");
        let error = create_test_error("Connection timeout");

        // Add failed event
        dlq.add_failed_event("task1".to_string(), event.clone(), error, 0)
            .await
            .unwrap();

        // Retrieve by task
        let entries = storage.get_by_task("task1", None).await.unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].task_id, "task1");
        assert_eq!(entries[0].event.source.table, "users");
        assert_eq!(entries[0].retry_count, 0);
        assert!(entries[0].error.contains("Connection timeout"));
    }

    #[tokio::test]
    async fn test_dlq_multiple_tasks() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        // Add events for different tasks
        for task_num in 1..=3 {
            for event_num in 1..=5 {
                let event = create_test_event("users", &format!("{task_num}-{event_num}"));
                let error = create_test_error(&format!("Error for task{task_num}"));
                dlq.add_failed_event(format!("task{task_num}"), event, error, 0)
                    .await
                    .unwrap();
            }
        }

        // Verify each task has correct number of entries
        for task_num in 1..=3 {
            let entries = storage
                .get_by_task(&format!("task{task_num}"), None)
                .await
                .unwrap();
            assert_eq!(entries.len(), 5);

            // All should belong to the same task
            for entry in entries {
                assert_eq!(entry.task_id, format!("task{task_num}"));
            }
        }
    }

    #[tokio::test]
    async fn test_dlq_statistics() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        // Add various failed events
        let errors = &[
            "Connection timeout",
            "Connection timeout",
            "Index not found",
            "Authentication failed",
            "Authentication failed",
            "Authentication failed",
        ];

        for (i, error_msg) in errors.iter().enumerate() {
            let event = create_test_event("users", &i.to_string());
            let error = create_test_error(error_msg);
            let task_id = if i < 3 { "task1" } else { "task2" };
            dlq.add_failed_event(task_id.to_string(), event, error, i as u32)
                .await
                .unwrap();
        }

        let stats = dlq.get_statistics().await.unwrap();

        assert_eq!(stats.total_entries, 6);
        assert_eq!(stats.entries_by_task.get("task1"), Some(&3));
        assert_eq!(stats.entries_by_task.get("task2"), Some(&3));
        // Check that error types are tracked (exact key may vary based on error formatting)
        assert!(!stats.entries_by_error.is_empty());
        // The error count should match our test data
        let total_errors: usize = stats.entries_by_error.values().sum();
        assert_eq!(total_errors, 6);
        assert!(stats.oldest_entry.is_some());
        assert!(stats.newest_entry.is_some());
    }

    #[tokio::test]
    async fn test_dlq_limit_retrieval() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        // Add 10 events
        for i in 0..10 {
            let event = create_test_event("users", &i.to_string());
            let error = create_test_error("Test error");
            dlq.add_failed_event("task1".to_string(), event, error, 0)
                .await
                .unwrap();
        }

        // Retrieve with limit
        let entries = storage.get_by_task("task1", Some(5)).await.unwrap();
        assert_eq!(entries.len(), 5);

        // Should be ordered by created_at (oldest first)
        for i in 1..entries.len() {
            assert!(entries[i - 1].created_at <= entries[i].created_at);
        }
    }

    #[tokio::test]
    async fn test_dlq_update_entry() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        let event = create_test_event("users", "1");
        let error = create_test_error("Initial error");
        dlq.add_failed_event("task1".to_string(), event, error, 0)
            .await
            .unwrap();

        // Get the entry
        let entries = storage.get_by_task("task1", None).await.unwrap();
        let mut entry = entries[0].clone();

        // Update retry count and last retry time
        entry.retry_count = 5;
        entry.last_retry_at = Some(Utc::now());
        entry
            .metadata
            .insert("updated".to_string(), "true".to_string());

        storage.update(entry.clone()).await.unwrap();

        // Verify update
        let updated = storage.get(&entry.id).await.unwrap().unwrap();
        assert_eq!(updated.retry_count, 5);
        assert!(updated.last_retry_at.is_some());
        assert_eq!(updated.metadata.get("updated"), Some(&"true".to_string()));
    }

    #[tokio::test]
    async fn test_dlq_clear_task() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        // Add events for multiple tasks
        for i in 0..5 {
            let event1 = create_test_event("users", &format!("task1-{i}"));
            let event2 = create_test_event("users", &format!("task2-{i}"));
            dlq.add_failed_event("task1".to_string(), event1, create_test_error("Error"), 0)
                .await
                .unwrap();
            dlq.add_failed_event("task2".to_string(), event2, create_test_error("Error"), 0)
                .await
                .unwrap();
        }

        // Clear task1
        let removed = dlq.clear_task("task1").await.unwrap();
        assert_eq!(removed, 5);

        // Verify task1 entries are gone
        let task1_entries = storage.get_by_task("task1", None).await.unwrap();
        assert_eq!(task1_entries.len(), 0);

        // Verify task2 entries remain
        let task2_entries = storage.get_by_task("task2", None).await.unwrap();
        assert_eq!(task2_entries.len(), 5);
    }

    #[tokio::test]
    async fn test_dlq_reprocess_entries() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let mut dlq = DeadLetterQueue::new(storage.clone());

        // Set up reprocess channel
        let (tx, mut rx) = mpsc::channel(10);
        dlq.set_reprocess_channel(tx);

        // Add some failed events
        for i in 0..3 {
            let event = create_test_event("users", &i.to_string());
            let error = create_test_error("Temporary failure");
            dlq.add_failed_event("task1".to_string(), event, error, i)
                .await
                .unwrap();
        }

        // Reprocess entries
        let count = dlq.reprocess_entries("task1", None).await.unwrap();
        assert_eq!(count, 3);

        // Verify entries were sent for reprocessing
        let mut received = vec![];
        while let Ok(entry) = rx.try_recv() {
            received.push(entry);
        }
        assert_eq!(received.len(), 3);

        // Verify retry counts were incremented
        for (i, entry) in received.iter().enumerate() {
            assert_eq!(entry.retry_count, (i + 1) as u32);
            assert!(entry.last_retry_at.is_some());
        }

        // Verify entries were removed from DLQ after reprocessing
        let remaining = storage.get_by_task("task1", None).await.unwrap();
        assert_eq!(remaining.len(), 0);
    }

    #[tokio::test]
    async fn test_dlq_reprocess_with_limit() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let mut dlq = DeadLetterQueue::new(storage.clone());

        let (tx, mut rx) = mpsc::channel(10);
        dlq.set_reprocess_channel(tx);

        // Add 10 failed events
        for i in 0..10 {
            let event = create_test_event("users", &i.to_string());
            let error = create_test_error("Error");
            dlq.add_failed_event("task1".to_string(), event, error, 0)
                .await
                .unwrap();
        }

        // Reprocess only 5
        let count = dlq.reprocess_entries("task1", Some(5)).await.unwrap();
        assert_eq!(count, 5);

        // Verify only 5 were sent
        let mut received = 0;
        while rx.try_recv().is_ok() {
            received += 1;
        }
        assert_eq!(received, 5);

        // Verify 5 remain in DLQ
        let remaining = storage.get_by_task("task1", None).await.unwrap();
        assert_eq!(remaining.len(), 5);
    }

    #[tokio::test]
    async fn test_dlq_concurrent_operations() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = Arc::new(DeadLetterQueue::new(storage.clone()));

        // Spawn multiple tasks adding entries concurrently
        let mut handles = vec![];

        for task_num in 0..5 {
            for event_num in 0..10 {
                let dlq_clone = dlq.clone();
                let handle = tokio::spawn(async move {
                    let event = create_test_event("users", &format!("{task_num}-{event_num}"));
                    let error = create_test_error("Concurrent error");
                    dlq_clone
                        .add_failed_event(format!("task{task_num}"), event, error, 0)
                        .await
                        .unwrap();
                });
                handles.push(handle);
            }
        }

        // Wait for all operations
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all entries were added
        let stats = dlq.get_statistics().await.unwrap();
        assert_eq!(stats.total_entries, 50); // 5 tasks * 10 events

        // Verify each task has correct count
        for task_num in 0..5 {
            let entries = storage
                .get_by_task(&format!("task{task_num}"), None)
                .await
                .unwrap();
            assert_eq!(entries.len(), 10);
        }
    }

    #[tokio::test]
    async fn test_dlq_entry_metadata() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        let event = create_test_event("users", "1");
        let error = create_test_error("Test error");

        // Add event with retry count
        dlq.add_failed_event("task1".to_string(), event, error, 3)
            .await
            .unwrap();

        let entries = storage.get_by_task("task1", None).await.unwrap();
        let entry = &entries[0];

        // Verify metadata
        assert_eq!(entry.retry_count, 3);
        assert!(entry.last_retry_at.is_some()); // Should be set when retry_count > 0
        assert!(entry.created_at <= Utc::now());
        assert!(!entry.id.is_empty());
    }

    #[tokio::test]
    async fn test_dlq_statistics_edge_cases() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage);

        // Empty DLQ statistics
        let stats = dlq.get_statistics().await.unwrap();
        assert_eq!(stats.total_entries, 0);
        assert!(stats.entries_by_task.is_empty());
        assert!(stats.entries_by_error.is_empty());
        assert!(stats.oldest_entry.is_none());
        assert!(stats.newest_entry.is_none());

        // Add single entry
        let event = create_test_event("users", "1");
        let error = create_test_error("Single error");
        dlq.add_failed_event("task1".to_string(), event, error, 0)
            .await
            .unwrap();

        let stats = dlq.get_statistics().await.unwrap();
        assert_eq!(stats.total_entries, 1);
        assert_eq!(stats.oldest_entry, stats.newest_entry);
    }

    #[tokio::test]
    async fn test_dlq_remove_entry() {
        let storage = Arc::new(InMemoryDlqStorage::new());
        let dlq = DeadLetterQueue::new(storage.clone());

        // Add an entry
        let event = create_test_event("users", "1");
        dlq.add_failed_event("task1".to_string(), event, create_test_error("Error"), 0)
            .await
            .unwrap();

        // Get the entry ID
        let entries = storage.get_by_task("task1", None).await.unwrap();
        let entry_id = entries[0].id.clone();

        // Remove it
        storage.remove(&entry_id).await.unwrap();

        // Verify it's gone
        let result = storage.get(&entry_id).await.unwrap();
        assert!(result.is_none());

        // Verify task is empty
        let entries = storage.get_by_task("task1", None).await.unwrap();
        assert_eq!(entries.len(), 0);
    }
}
