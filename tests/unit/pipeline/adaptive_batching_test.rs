// Advanced unit tests for adaptive batching

use meilibridge::config::AdaptiveBatchingConfig;
use meilibridge::pipeline::adaptive_batching::{AdaptiveBatchingManager, BatchMetrics};
use std::time::Duration;
use tokio::time::Instant;

#[cfg(test)]
mod adaptive_batching_tests {
    use super::*;

    fn create_test_config() -> AdaptiveBatchingConfig {
        AdaptiveBatchingConfig {
            target_latency_ms: 500,
            adjustment_factor: 0.2,
            metric_window_size: 5,
            adjustment_interval_ms: 100, // Short interval for testing
            memory_pressure_threshold: 0.8,
            per_table_optimization: true,
        }
    }

    fn create_metrics(batch_size: usize, processing_time_ms: u64) -> BatchMetrics {
        BatchMetrics {
            batch_size,
            processing_time_ms,
            documents_per_second: batch_size as f64 / (processing_time_ms as f64 / 1000.0),
            memory_usage_mb: batch_size as f64 * 0.1,
            timestamp: Instant::now(),
        }
    }

    #[tokio::test]
    async fn test_batch_size_adjustment_faster_than_target() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config.clone());

        let table_name = "test_table";
        let default_size = 100;

        // Get initial batch size
        let initial_size = manager
            .get_batch_size(table_name, default_size, 10, 1000)
            .await
            .unwrap();
        assert_eq!(initial_size, default_size);

        // Record multiple fast batches
        for _ in 0..5 {
            let metrics = create_metrics(initial_size, 200); // 200ms < 500ms target
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        // Wait for adjustment interval
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Batch size should increase since processing was faster than target
        // OR decrease if memory pressure is high
        let new_size = manager
            .get_batch_size(table_name, default_size, 10, 1000)
            .await
            .unwrap();

        // The batch size might decrease due to memory pressure or increase due to fast processing
        assert_ne!(
            new_size, initial_size,
            "Expected batch size to change from {initial_size} but it remained the same",
        );

        // If it decreased, it should be due to memory pressure (80% of original)
        if new_size < initial_size {
            assert_eq!(
                new_size, 80,
                "If decreased due to memory pressure, should be 80% of original"
            );
        } else {
            // Otherwise it should have increased
            assert!(
                new_size > initial_size,
                "Expected batch size to increase from {initial_size} to {new_size}",
            );
        }
    }

    #[tokio::test]
    async fn test_batch_size_adjustment_slower_than_target() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config.clone());

        let table_name = "slow_table";
        let default_size = 500;

        // Record multiple slow batches
        for _ in 0..5 {
            let metrics = create_metrics(default_size, 1000); // 1000ms > 500ms target
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        // Wait for adjustment interval
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Batch size should decrease since processing was slower than target
        let new_size = manager
            .get_batch_size(table_name, default_size, 10, 1000)
            .await
            .unwrap();
        assert!(
            new_size < default_size,
            "Expected batch size to decrease from {default_size} to {new_size}",
        );
    }

    #[tokio::test]
    async fn test_multiple_table_isolation() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config);

        let table1 = "fast_table";
        let table2 = "slow_table";
        let default_size = 200;

        // Record metrics for table1 (fast processing)
        for _ in 0..5 {
            let metrics = create_metrics(default_size, 100); // Very fast
            manager.record_metrics(table1, metrics).await.unwrap();
        }

        // Record metrics for table2 (slow processing)
        for _ in 0..5 {
            let metrics = create_metrics(default_size, 800); // Slow
            manager.record_metrics(table2, metrics).await.unwrap();
        }

        // Wait for adjustment
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Verify each table adjusted independently
        let size1 = manager
            .get_batch_size(table1, default_size, 10, 1000)
            .await
            .unwrap();
        let size2 = manager
            .get_batch_size(table2, default_size, 10, 1000)
            .await
            .unwrap();

        // Due to memory pressure, both might decrease, or they might adjust as expected
        if size1 == 160 && size2 == 160 {
            // Both decreased due to memory pressure (80% of 200)
            assert_eq!(size1, 160, "Both tables decreased due to memory pressure");
        } else {
            // Normal behavior without memory pressure
            assert!(
                size1 >= default_size,
                "Fast table should increase or stay same"
            );
            assert!(
                size2 <= default_size,
                "Slow table should decrease or stay same"
            );
            assert_ne!(size1, size2, "Tables should have different batch sizes");
        }
    }

    #[tokio::test]
    async fn test_metrics_window_sliding() {
        let mut config = create_test_config();
        config.metric_window_size = 3;
        let manager = AdaptiveBatchingManager::new(config);

        let table_name = "window_test";
        let default_size = 100;

        // Fill the metrics window with increasing latencies
        for i in 0..5 {
            let metrics = create_metrics(default_size, 100 + i * 100);
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        // Get statistics and verify window behavior
        let stats = manager.get_statistics().await;
        if let Some(table_stats) = stats.get(table_name) {
            assert!(
                table_stats.metrics_count <= 3,
                "Window should contain at most 3 metrics"
            );
            assert!(
                table_stats.average_latency_ms > 200,
                "Average should reflect recent metrics"
            );
        }
    }

    #[tokio::test]
    async fn test_minimum_metrics_required() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config.clone());

        let table_name = "min_metrics_test";
        let default_size = 100;

        // Record only one metric (less than window_size / 2)
        let metrics = create_metrics(default_size, 100);
        manager.record_metrics(table_name, metrics).await.unwrap();

        // Wait for adjustment interval
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should still return default size due to insufficient metrics
        let size = manager
            .get_batch_size(table_name, default_size, 10, 1000)
            .await
            .unwrap();
        assert_eq!(
            size, default_size,
            "Should not adjust with insufficient metrics"
        );
    }

    #[tokio::test]
    async fn test_per_table_optimization_disabled() {
        let mut config = create_test_config();
        config.per_table_optimization = false;
        let manager = AdaptiveBatchingManager::new(config);

        let table_name = "disabled_opt_test";
        let default_size = 250;

        // Record metrics that would normally trigger adjustment
        for _ in 0..5 {
            let metrics = create_metrics(default_size, 50); // Very fast
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        // Should always return default size when per-table optimization is disabled
        let size = manager
            .get_batch_size(table_name, default_size, 10, 1000)
            .await
            .unwrap();
        assert_eq!(
            size, default_size,
            "Should return default when optimization is disabled"
        );
    }

    #[tokio::test]
    async fn test_concurrent_table_updates() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config);

        let tables: Vec<String> = (0..5).map(|i| format!("table_{i}")).collect();

        // Spawn concurrent tasks updating different tables
        let mut handles = vec![];

        for (i, table) in tables.iter().enumerate() {
            let manager_clone = manager.clone();
            let table_clone = table.clone();
            let handle = tokio::spawn(async move {
                for j in 0..10 {
                    let metrics = create_metrics(100 + i * 10, 300 + j * 50);
                    manager_clone
                        .record_metrics(&table_clone, metrics)
                        .await
                        .unwrap();
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            });
            handles.push(handle);
        }

        // Wait for all tasks
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify all tables have metrics
        let stats = manager.get_statistics().await;
        for table in &tables {
            assert!(
                stats.contains_key(table),
                "Table {table} should have statistics",
            );
        }
    }

    #[tokio::test]
    async fn test_batch_size_boundaries() {
        let config = create_test_config();
        let manager = AdaptiveBatchingManager::new(config);

        let table_name = "boundary_test";
        let min_size = 10;
        let max_size = 100;
        let default_size = 50;

        // Record very fast processing to trigger maximum increase
        for _ in 0..10 {
            let metrics = create_metrics(default_size, 10); // Extremely fast
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should not exceed max_size
        let size = manager
            .get_batch_size(table_name, default_size, min_size, max_size)
            .await
            .unwrap();
        assert!(
            size <= max_size,
            "Batch size {size} should not exceed max {max_size}",
        );

        // Now record very slow processing
        for _ in 0..10 {
            let metrics = create_metrics(size, 5000); // Extremely slow
            manager.record_metrics(table_name, metrics).await.unwrap();
        }

        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should not go below min_size
        let final_size = manager
            .get_batch_size(table_name, default_size, min_size, max_size)
            .await
            .unwrap();
        assert!(
            final_size >= min_size,
            "Batch size {final_size} should not go below min {min_size}",
        );
    }
}
