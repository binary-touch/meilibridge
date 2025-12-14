// Advanced unit tests for at-least-once delivery with deduplication

use meilibridge::delivery::{AtLeastOnceConfig, AtLeastOnceManager, DeduplicationKey};
use meilibridge::models::Position;
use std::collections::HashSet;
use std::sync::Arc;
use tokio::time::{Duration, sleep};

#[cfg(test)]
mod at_least_once_tests {
    use super::*;

    fn create_test_config() -> AtLeastOnceConfig {
        AtLeastOnceConfig {
            enabled: true,
            deduplication_window: 100,
            transaction_timeout_secs: 5,
            two_phase_commit: true,
            checkpoint_before_write: true,
        }
    }

    fn create_dedup_key(lsn: &str, xid: u32, table: &str) -> DeduplicationKey {
        DeduplicationKey::new(lsn.to_string(), Some(xid), table.to_string())
    }

    #[tokio::test]
    async fn test_deduplication_basic() {
        let config = create_test_config();
        let manager = AtLeastOnceManager::new(config);

        let key1 = create_dedup_key("0/1000000", 123, "users");

        // First occurrence should not be duplicate
        assert!(!manager.is_duplicate(&key1).await.unwrap());

        // Mark as processed
        manager.mark_processed(key1.clone()).await.unwrap();

        // Now it should be duplicate
        assert!(manager.is_duplicate(&key1).await.unwrap());
    }

    #[tokio::test]
    async fn test_deduplication_window_eviction() {
        let mut config = create_test_config();
        config.deduplication_window = 10; // Small window for testing
        let manager = AtLeastOnceManager::new(config);

        // Add keys until window is full
        for i in 0..10 {
            let key = create_dedup_key(&format!("0/{}", i), i as u32, "events");
            manager.mark_processed(key).await.unwrap();
        }

        // Verify all keys are tracked
        for i in 0..10 {
            let key = create_dedup_key(&format!("0/{}", i), i as u32, "events");
            assert!(manager.is_duplicate(&key).await.unwrap());
        }

        // Add one more key (should evict the oldest)
        let new_key = create_dedup_key("0/10", 10, "events");
        manager.mark_processed(new_key.clone()).await.unwrap();

        // First key should be evicted
        let first_key = create_dedup_key("0/0", 0, "events");
        assert!(!manager.is_duplicate(&first_key).await.unwrap());

        // Last key should still be tracked
        assert!(manager.is_duplicate(&new_key).await.unwrap());
    }

    #[tokio::test]
    async fn test_deduplication_with_primary_key() {
        let config = create_test_config();
        let manager = AtLeastOnceManager::new(config);

        // Same LSN but different primary keys
        let key1 =
            create_dedup_key("0/1000", 100, "products").with_primary_key("prod_1".to_string());
        let key2 =
            create_dedup_key("0/1000", 100, "products").with_primary_key("prod_2".to_string());

        // Both should be unique
        assert!(!manager.is_duplicate(&key1).await.unwrap());
        assert!(!manager.is_duplicate(&key2).await.unwrap());

        // Mark first as processed
        manager.mark_processed(key1.clone()).await.unwrap();

        // First should be duplicate, second should not
        assert!(manager.is_duplicate(&key1).await.unwrap());
        assert!(!manager.is_duplicate(&key2).await.unwrap());
    }

    #[tokio::test]
    async fn test_transaction_lifecycle() {
        let config = create_test_config();
        let manager = AtLeastOnceManager::new(config);

        let task_id = "test_task_1";

        // Begin transaction
        let txn_id = manager.begin_transaction(task_id).await.unwrap();
        assert!(!txn_id.is_empty());

        // Prepare with position
        let position = Position::PostgreSQL {
            lsn: "0/2000000".to_string(),
        };
        let prepared = manager.prepare(&txn_id, position).await.unwrap();
        assert!(prepared);

        // Commit transaction
        manager.commit(&txn_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_transaction_rollback() {
        let config = create_test_config();
        let manager = AtLeastOnceManager::new(config);

        let task_id = "rollback_task";

        // Begin transaction
        let txn_id = manager.begin_transaction(task_id).await.unwrap();

        // Prepare
        let position = Position::PostgreSQL {
            lsn: "0/3000000".to_string(),
        };
        manager.prepare(&txn_id, position).await.unwrap();

        // Rollback instead of commit
        manager.rollback(&txn_id).await.unwrap();

        // Transaction should be cleaned up
        // (Would need access to internal state to verify)
    }

    #[tokio::test]
    async fn test_concurrent_transactions() {
        let config = create_test_config();
        let manager = Arc::new(AtLeastOnceManager::new(config));

        let mut handles = vec![];

        // Start multiple concurrent transactions
        for i in 0..5 {
            let manager_clone = manager.clone();
            let task_id = format!("task_{}", i);

            let handle = tokio::spawn(async move {
                // Begin transaction
                let txn_id = manager_clone.begin_transaction(&task_id).await.unwrap();

                // Simulate some processing
                sleep(Duration::from_millis(10)).await;

                // Prepare
                let position = Position::PostgreSQL {
                    lsn: format!("0/{}", 4000000 + i * 1000),
                };
                manager_clone.prepare(&txn_id, position).await.unwrap();

                // Commit
                manager_clone.commit(&txn_id).await.unwrap();

                txn_id
            });

            handles.push(handle);
        }

        // Wait for all transactions
        let mut transaction_ids = HashSet::new();
        for handle in handles {
            let txn_id = handle.await.unwrap();
            transaction_ids.insert(txn_id);
        }

        // All transaction IDs should be unique
        assert_eq!(transaction_ids.len(), 5);
    }

    #[tokio::test]
    async fn test_disabled_at_least_once() {
        let mut config = create_test_config();
        config.enabled = false;
        config.two_phase_commit = false;

        let manager = AtLeastOnceManager::new(config);

        // With disabled at-least-once, nothing should be tracked
        let key = create_dedup_key("0/5000000", 500, "disabled_test");

        // Should never be duplicate
        assert!(!manager.is_duplicate(&key).await.unwrap());
        manager.mark_processed(key.clone()).await.unwrap();
        assert!(!manager.is_duplicate(&key).await.unwrap());

        // Transactions should return empty/success immediately
        let txn_id = manager.begin_transaction("task").await.unwrap();
        assert!(txn_id.is_empty());

        let position = Position::PostgreSQL {
            lsn: "0/6000000".to_string(),
        };
        assert!(manager.prepare(&txn_id, position).await.unwrap());
        manager.commit(&txn_id).await.unwrap();
    }

    #[tokio::test]
    async fn test_deduplication_across_tables() {
        let config = create_test_config();
        let manager = AtLeastOnceManager::new(config);

        // Same LSN and XID but different tables
        let key1 = create_dedup_key("0/7000000", 700, "orders");
        let key2 = create_dedup_key("0/7000000", 700, "customers");

        // Both should be unique (different tables)
        assert!(!manager.is_duplicate(&key1).await.unwrap());
        assert!(!manager.is_duplicate(&key2).await.unwrap());

        // Mark both as processed
        manager.mark_processed(key1.clone()).await.unwrap();
        manager.mark_processed(key2.clone()).await.unwrap();

        // Both should now be duplicates for their respective tables
        assert!(manager.is_duplicate(&key1).await.unwrap());
        assert!(manager.is_duplicate(&key2).await.unwrap());
    }

    #[tokio::test]
    async fn test_transaction_timeout_handling() {
        let mut config = create_test_config();
        config.transaction_timeout_secs = 1; // Very short timeout
        let manager = AtLeastOnceManager::new(config);

        // Begin transaction
        let txn_id = manager.begin_transaction("timeout_test").await.unwrap();

        // Wait longer than timeout
        sleep(Duration::from_secs(2)).await;

        // Transaction operations should handle timeout gracefully
        let position = Position::PostgreSQL {
            lsn: "0/8000000".to_string(),
        };

        // This might fail or succeed depending on implementation
        // The test verifies it doesn't panic
        let _ = manager.prepare(&txn_id, position).await;
        let _ = manager.commit(&txn_id).await;
    }

    #[tokio::test]
    async fn test_high_throughput_deduplication() {
        let mut config = create_test_config();
        config.deduplication_window = 2000; // Large window to track all events
        let manager = Arc::new(AtLeastOnceManager::new(config));

        let num_events = 100;
        let num_duplicates = 20;

        // Process events with intermixed duplicates
        let mut duplicate_count = 0;
        let mut processed_keys = vec![];

        // Process unique events
        for i in 0..num_events {
            let key = create_dedup_key(&format!("0/{:08x}", i), i as u32, "high_throughput");
            assert!(!manager.is_duplicate(&key).await.unwrap());
            manager.mark_processed(key.clone()).await.unwrap();
            processed_keys.push(key);
        }

        // Now process some duplicates from the already processed keys
        for dup_key in processed_keys.iter().take(num_duplicates) {
            if manager.is_duplicate(dup_key).await.unwrap() {
                duplicate_count += 1;
            }
        }

        // Should have detected all duplicates
        assert_eq!(duplicate_count, num_duplicates);
    }
}
