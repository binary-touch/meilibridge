// Checkpoint manager tests

use chrono::Utc;
use meilibridge::checkpoint::CheckpointStorage;
use meilibridge::models::progress::{Checkpoint, Position};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[cfg(test)]
mod checkpoint_manager_tests {
    use super::*;

    // Mock storage for testing
    struct MockStorage {
        data: Arc<RwLock<HashMap<String, Checkpoint>>>,
    }

    impl MockStorage {
        fn new() -> Self {
            Self {
                data: Arc::new(RwLock::new(HashMap::new())),
            }
        }
    }

    #[async_trait::async_trait]
    impl CheckpointStorage for MockStorage {
        async fn save(&self, checkpoint: &Checkpoint) -> meilibridge::error::Result<()> {
            let mut data = self.data.write().await;
            data.insert(checkpoint.task_id.clone(), checkpoint.clone());
            Ok(())
        }

        async fn load(&self, task_id: &str) -> meilibridge::error::Result<Option<Checkpoint>> {
            let data = self.data.read().await;
            Ok(data.get(task_id).cloned())
        }

        async fn delete(&self, task_id: &str) -> meilibridge::error::Result<()> {
            let mut data = self.data.write().await;
            data.remove(task_id);
            Ok(())
        }

        async fn list(&self) -> meilibridge::error::Result<Vec<Checkpoint>> {
            let data = self.data.read().await;
            Ok(data.values().cloned().collect())
        }

        async fn is_healthy(&self) -> bool {
            true
        }
    }

    fn create_test_checkpoint(task_id: &str) -> Checkpoint {
        Checkpoint::new(task_id.to_string(), Position::postgresql("0/1234567"))
    }

    #[tokio::test]
    async fn test_checkpoint_save_and_load() {
        let storage = Arc::new(MockStorage::new());
        let checkpoint = create_test_checkpoint("test_task");

        // Save checkpoint
        storage.save(&checkpoint).await.unwrap();

        // Load checkpoint
        let loaded = storage.load("test_task").await.unwrap();
        assert!(loaded.is_some());

        let loaded_checkpoint = loaded.unwrap();
        assert_eq!(loaded_checkpoint.task_id, checkpoint.task_id);
        match &loaded_checkpoint.position {
            Position::PostgreSQL { lsn } => assert_eq!(lsn, "0/1234567"),
            _ => panic!("Expected PostgreSQL position"),
        }
    }

    #[tokio::test]
    async fn test_checkpoint_update() {
        let storage = Arc::new(MockStorage::new());

        // Create initial checkpoint
        let mut checkpoint = create_test_checkpoint("test_task");
        storage.save(&checkpoint).await.unwrap();

        // Update checkpoint
        checkpoint.position = Position::postgresql("0/2345678");
        checkpoint.stats.events_processed = 200;

        storage.save(&checkpoint).await.unwrap();

        // Verify update
        let loaded = storage.load("test_task").await.unwrap().unwrap();
        match &loaded.position {
            Position::PostgreSQL { lsn } => assert_eq!(lsn, "0/2345678"),
            _ => panic!("Expected PostgreSQL position"),
        }
        assert_eq!(loaded.stats.events_processed, 200);
    }

    #[tokio::test]
    async fn test_checkpoint_deletion() {
        let storage = Arc::new(MockStorage::new());

        // Create and save checkpoint
        let checkpoint = create_test_checkpoint("test_task");
        storage.save(&checkpoint).await.unwrap();

        // Verify it exists
        assert!(storage.load("test_task").await.unwrap().is_some());

        // Delete checkpoint
        storage.delete("test_task").await.unwrap();

        // Verify it's gone
        assert!(storage.load("test_task").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_checkpoint_list() {
        let storage = Arc::new(MockStorage::new());

        // Create multiple checkpoints
        for i in 1..=5 {
            let checkpoint = create_test_checkpoint(&format!("task_{i}"));
            storage.save(&checkpoint).await.unwrap();
        }

        // List all checkpoints
        let checkpoints = storage.list().await.unwrap();
        assert_eq!(checkpoints.len(), 5);

        // Verify all task IDs are present
        let task_ids: Vec<String> = checkpoints.iter().map(|c| c.task_id.clone()).collect();
        for i in 1..=5 {
            assert!(task_ids.contains(&format!("task_{i}")));
        }
    }

    #[tokio::test]
    async fn test_concurrent_checkpoint_updates() {
        let storage = Arc::new(MockStorage::new());

        // Create initial checkpoint
        let checkpoint = create_test_checkpoint("test_task");
        storage.save(&checkpoint).await.unwrap();

        // Concurrent updates
        let mut handles = vec![];
        for i in 0..10 {
            let storage_clone = storage.clone();
            let handle = tokio::spawn(async move {
                let mut checkpoint = Checkpoint::new(
                    "test_task".to_string(),
                    Position::postgresql(format!("0/{}", (i + 1) * 1000000)),
                );
                checkpoint.stats.events_processed = (i + 1) * 100;
                storage_clone.save(&checkpoint).await.unwrap();
            });
            handles.push(handle);
        }

        // Wait for all updates
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify checkpoint was updated
        let final_checkpoint = storage.load("test_task").await.unwrap().unwrap();
        assert!(final_checkpoint.stats.events_processed > 0);
    }

    #[tokio::test]
    async fn test_checkpoint_metadata() {
        let storage = Arc::new(MockStorage::new());

        // Create checkpoint with metadata
        let mut checkpoint = create_test_checkpoint("test_task");
        checkpoint.metadata = serde_json::json!({
            "table": "users",
            "version": "1.0"
        });

        storage.save(&checkpoint).await.unwrap();

        // Verify metadata is preserved
        let loaded = storage.load("test_task").await.unwrap().unwrap();
        assert_eq!(loaded.metadata["table"], "users");
        assert_eq!(loaded.metadata["version"], "1.0");
    }

    #[tokio::test]
    async fn test_position_types() {
        let storage = Arc::new(MockStorage::new());

        // Test PostgreSQL position
        let pg_checkpoint =
            Checkpoint::new("pg_task".to_string(), Position::postgresql("0/ABCDEF"));
        storage.save(&pg_checkpoint).await.unwrap();

        // Test MySQL position
        let mysql_checkpoint = Checkpoint::new(
            "mysql_task".to_string(),
            Position::mysql("binlog.001", 12345),
        );
        storage.save(&mysql_checkpoint).await.unwrap();

        // Test MongoDB position
        let mongo_checkpoint = Checkpoint::new(
            "mongo_task".to_string(),
            Position::mongodb("resume_token_abc123"),
        );
        storage.save(&mongo_checkpoint).await.unwrap();

        // Verify all positions
        let checkpoints = storage.list().await.unwrap();
        assert_eq!(checkpoints.len(), 3);
    }

    #[tokio::test]
    async fn test_checkpoint_stats() {
        let storage = Arc::new(MockStorage::new());

        let mut checkpoint = create_test_checkpoint("stats_task");
        checkpoint.stats.events_processed = 1000;
        checkpoint.stats.bytes_processed = 1024 * 1024; // 1MB
        checkpoint.stats.events_failed = 5;
        checkpoint.stats.last_event_at = Some(Utc::now());

        storage.save(&checkpoint).await.unwrap();

        let loaded = storage.load("stats_task").await.unwrap().unwrap();
        assert_eq!(loaded.stats.events_processed, 1000);
        assert_eq!(loaded.stats.bytes_processed, 1024 * 1024);
        assert_eq!(loaded.stats.events_failed, 5);
        assert!(loaded.stats.last_event_at.is_some());

        // Test success rate calculation
        let success_rate = loaded.stats.success_rate();
        assert!(success_rate > 0.99 && success_rate < 1.0); // ~99.5%
    }
}
