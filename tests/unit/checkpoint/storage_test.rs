// Checkpoint storage tests

use meilibridge::checkpoint::{CheckpointStorage, MemoryStorage};
use meilibridge::models::progress::{Checkpoint, Position};

#[cfg(test)]
mod checkpoint_storage_tests {
    use super::*;

    fn create_test_checkpoint(task_id: &str) -> Checkpoint {
        let mut checkpoint =
            Checkpoint::new(task_id.to_string(), Position::postgresql("0/1234567"));
        checkpoint.metadata = serde_json::json!({
            "table": "users",
            "version": "1.0",
        });
        checkpoint.stats.events_processed = 100;
        checkpoint
    }

    #[tokio::test]
    async fn test_memory_storage_basic_operations() {
        let storage = MemoryStorage::new();

        // Test save
        let checkpoint = create_test_checkpoint("task1");
        storage.save(&checkpoint).await.unwrap();

        // Test load
        let loaded = storage.load("task1").await.unwrap();
        assert!(loaded.is_some());
        let loaded_checkpoint = loaded.unwrap();
        assert_eq!(loaded_checkpoint.task_id, checkpoint.task_id);
        assert_eq!(loaded_checkpoint.stats.events_processed, 100);

        // Test non-existent key
        let not_found = storage.load("nonexistent").await.unwrap();
        assert!(not_found.is_none());
    }

    #[tokio::test]
    async fn test_memory_storage_update() {
        let storage = MemoryStorage::new();

        // Save initial checkpoint
        let mut checkpoint = create_test_checkpoint("task1");
        storage.save(&checkpoint).await.unwrap();

        // Update checkpoint
        checkpoint.position = Position::postgresql("0/2345678");
        checkpoint.stats.events_processed = 200;
        storage.save(&checkpoint).await.unwrap();

        // Verify update
        let loaded = storage.load("task1").await.unwrap().unwrap();
        match &loaded.position {
            Position::PostgreSQL { lsn } => assert_eq!(lsn, "0/2345678"),
            _ => panic!("Expected PostgreSQL position"),
        }
        assert_eq!(loaded.stats.events_processed, 200);
    }

    #[tokio::test]
    async fn test_memory_storage_delete() {
        let storage = MemoryStorage::new();

        // Save checkpoint
        let checkpoint = create_test_checkpoint("task1");
        storage.save(&checkpoint).await.unwrap();

        // Verify it exists
        assert!(storage.load("task1").await.unwrap().is_some());

        // Delete checkpoint
        storage.delete("task1").await.unwrap();

        // Verify it's gone
        assert!(storage.load("task1").await.unwrap().is_none());

        // Delete non-existent should not error
        storage.delete("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_memory_storage_list() {
        let storage = MemoryStorage::new();

        // Save multiple checkpoints
        for i in 1..=5 {
            let checkpoint = create_test_checkpoint(&format!("task{i}"));
            storage.save(&checkpoint).await.unwrap();
        }

        // List all
        let checkpoints = storage.list().await.unwrap();
        assert_eq!(checkpoints.len(), 5);

        // Verify all task IDs
        let task_ids: Vec<String> = checkpoints.iter().map(|c| c.task_id.clone()).collect();
        for i in 1..=5 {
            assert!(task_ids.contains(&format!("task{i}")));
        }
    }

    #[tokio::test]
    async fn test_memory_storage_concurrent_access() {
        use std::sync::Arc;

        let storage = Arc::new(MemoryStorage::new());

        // Concurrent writes
        let mut handles = vec![];
        for i in 0..10 {
            let storage_clone = storage.clone();
            let handle = tokio::spawn(async move {
                let checkpoint = create_test_checkpoint(&format!("task{i}"));
                storage_clone.save(&checkpoint).await.unwrap();
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all writes succeeded
        let checkpoints = storage.list().await.unwrap();
        assert_eq!(checkpoints.len(), 10);

        // Concurrent reads
        let mut handles = vec![];
        for i in 0..10 {
            let storage_clone = storage.clone();
            let handle = tokio::spawn(async move {
                let loaded = storage_clone.load(&format!("task{i}")).await.unwrap();
                assert!(loaded.is_some());
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_checkpoint_serialization() {
        let storage = MemoryStorage::new();

        // Create checkpoint with all fields populated
        let mut checkpoint = create_test_checkpoint("serialize_test");
        checkpoint.metadata = serde_json::json!({
            "complex_value": {"nested": true},
            "special_chars": "test\n\t\r\"'",
            "unicode": "测试 🎉"
        });

        // Save and load
        storage.save(&checkpoint).await.unwrap();
        let loaded = storage.load("serialize_test").await.unwrap().unwrap();

        // Verify all fields preserved correctly
        assert_eq!(loaded.task_id, checkpoint.task_id);
        assert_eq!(loaded.metadata["complex_value"]["nested"], true);
        assert_eq!(loaded.metadata["special_chars"], "test\n\t\r\"'");
        assert_eq!(loaded.metadata["unicode"], "测试 🎉");
    }

    #[tokio::test]
    async fn test_different_position_types() {
        let storage = MemoryStorage::new();

        // PostgreSQL position
        let pg_checkpoint =
            Checkpoint::new("pg_task".to_string(), Position::postgresql("0/DEADBEEF"));
        storage.save(&pg_checkpoint).await.unwrap();

        // MySQL position
        let mysql_checkpoint = Checkpoint::new(
            "mysql_task".to_string(),
            Position::mysql("mysql-bin.000123", 456789),
        );
        storage.save(&mysql_checkpoint).await.unwrap();

        // MongoDB position
        let mongo_checkpoint = Checkpoint::new(
            "mongo_task".to_string(),
            Position::mongodb("82648290000000000000000001"),
        );
        storage.save(&mongo_checkpoint).await.unwrap();

        // Verify all positions preserved correctly
        let pg_loaded = storage.load("pg_task").await.unwrap().unwrap();
        match &pg_loaded.position {
            Position::PostgreSQL { lsn } => assert_eq!(lsn, "0/DEADBEEF"),
            _ => panic!("Wrong position type"),
        }

        let mysql_loaded = storage.load("mysql_task").await.unwrap().unwrap();
        match &mysql_loaded.position {
            Position::MySQL { file, position } => {
                assert_eq!(file, "mysql-bin.000123");
                assert_eq!(*position, 456789);
            }
            _ => panic!("Wrong position type"),
        }

        let mongo_loaded = storage.load("mongo_task").await.unwrap().unwrap();
        match &mongo_loaded.position {
            Position::MongoDB { resume_token } => {
                assert_eq!(resume_token, "82648290000000000000000001");
            }
            _ => panic!("Wrong position type"),
        }
    }

    #[tokio::test]
    async fn test_storage_capacity() {
        let storage = MemoryStorage::new();

        // Store many checkpoints
        for i in 0..1000 {
            let checkpoint = create_test_checkpoint(&format!("bulk_task_{i}"));
            storage.save(&checkpoint).await.unwrap();
        }

        // Verify all stored
        let checkpoints = storage.list().await.unwrap();
        assert_eq!(checkpoints.len(), 1000);

        // Random access should still be fast
        let start = std::time::Instant::now();
        for i in (0..1000).step_by(100) {
            storage.load(&format!("bulk_task_{i}")).await.unwrap();
        }
        let duration = start.elapsed();
        assert!(duration.as_millis() < 100); // Should be very fast
    }
}
