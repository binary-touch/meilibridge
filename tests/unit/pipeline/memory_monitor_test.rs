// Advanced unit tests for memory monitoring

use meilibridge::pipeline::memory_monitor::{MemoryMonitor, MemoryPressureEvent};
use std::sync::Arc;
use tokio::time::{Duration, timeout};

#[cfg(test)]
mod memory_monitor_tests {
    use super::*;

    #[tokio::test]
    async fn test_memory_allocation_tracking() {
        let monitor = MemoryMonitor::new(100, 50); // 100MB queue, 50MB checkpoint

        // Test successful queue allocation
        let alloc1 = monitor.track_event_memory(10 * 1024 * 1024).unwrap(); // 10MB
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 10 * 1024 * 1024);
        assert_eq!(stats.event_count, 1);

        // Test successful checkpoint allocation
        let alloc2 = monitor.track_checkpoint_memory(5 * 1024 * 1024).unwrap(); // 5MB
        let stats = monitor.get_stats();
        assert_eq!(stats.checkpoint_memory_bytes, 5 * 1024 * 1024);

        // Drop allocations and verify memory is released
        drop(alloc1);
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 0);
        assert_eq!(stats.event_count, 0);

        drop(alloc2);
        let stats = monitor.get_stats();
        assert_eq!(stats.checkpoint_memory_bytes, 0);
    }

    #[tokio::test]
    async fn test_memory_limit_enforcement() {
        let monitor = MemoryMonitor::new(10, 5); // 10MB queue, 5MB checkpoint limits

        // Allocate near the limit
        let _alloc1 = monitor.track_event_memory(8 * 1024 * 1024).unwrap(); // 8MB

        // Try to exceed queue limit
        let result = monitor.track_event_memory(3 * 1024 * 1024); // 3MB would exceed
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("Queue memory limit exceeded")),
            Ok(_) => panic!("Should have failed due to memory limit"),
        }

        // Verify allocation was rolled back
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 8 * 1024 * 1024);

        // Test checkpoint limit
        let _alloc2 = monitor.track_checkpoint_memory(4 * 1024 * 1024).unwrap(); // 4MB
        let result = monitor.track_checkpoint_memory(2 * 1024 * 1024); // Would exceed
        assert!(result.is_err());
        match result {
            Err(e) => assert!(e.to_string().contains("Checkpoint memory limit exceeded")),
            Ok(_) => panic!("Should have failed due to memory limit"),
        }
    }

    #[tokio::test]
    async fn test_backpressure_detection() {
        let monitor = MemoryMonitor::new(100, 50); // 100MB queue limit

        // Should not apply backpressure at low usage
        assert!(!monitor.should_apply_backpressure());

        // Allocate 79MB (79% usage - below 80% threshold)
        let _alloc = monitor.track_event_memory(79 * 1024 * 1024).unwrap();
        assert!(!monitor.should_apply_backpressure());

        // Allocate additional 2MB (81% total - above 80% threshold)
        let _alloc2 = monitor.track_event_memory(2 * 1024 * 1024).unwrap();
        assert!(monitor.should_apply_backpressure());
    }

    #[tokio::test]
    async fn test_memory_stats_calculation() {
        let monitor = MemoryMonitor::new(200, 100); // 200MB queue, 100MB checkpoint

        // Allocate some memory
        let _alloc1 = monitor.track_event_memory(50 * 1024 * 1024).unwrap(); // 50MB
        let _alloc2 = monitor.track_checkpoint_memory(25 * 1024 * 1024).unwrap(); // 25MB

        let stats = monitor.get_stats();

        // Verify raw stats
        assert_eq!(stats.queue_memory_bytes, 50 * 1024 * 1024);
        assert_eq!(stats.checkpoint_memory_bytes, 25 * 1024 * 1024);
        assert_eq!(stats.max_queue_memory_bytes, 200 * 1024 * 1024);
        assert_eq!(stats.max_checkpoint_memory_bytes, 100 * 1024 * 1024);
        assert_eq!(stats.event_count, 1);

        // Verify percentage calculations
        assert_eq!(stats.queue_usage_percent(), 25.0);
        assert_eq!(stats.checkpoint_usage_percent(), 25.0);
    }

    #[tokio::test]
    async fn test_memory_monitoring_task() {
        let mut monitor = MemoryMonitor::new(10, 10); // 10MB limits for quick testing

        // Start monitoring
        let mut rx = monitor.start_monitoring();

        // Allocate memory to trigger high usage warning (>90%)
        let _alloc = monitor.track_event_memory(9_500_000).unwrap(); // 9.5MB (95% usage)

        // Wait for pressure event
        let event = timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("Should receive event within timeout")
            .expect("Should receive pressure event");

        match event {
            MemoryPressureEvent::HighQueueMemory(percent) => {
                assert!(percent > 90.0, "Should report high memory usage");
            }
            _ => panic!("Expected HighQueueMemory event"),
        }

        // Stop monitoring
        monitor.stop().await;
    }

    #[tokio::test]
    async fn test_concurrent_allocations() {
        let monitor = Arc::new(MemoryMonitor::new(1000, 500)); // 1GB queue, 500MB checkpoint

        // Spawn multiple tasks allocating memory concurrently
        let mut handles = vec![];

        for i in 0..10 {
            let monitor_clone = monitor.clone();
            let handle = tokio::spawn(async move {
                let mut allocations = vec![];
                for j in 0..5 {
                    // Each task allocates 5 x 2MB = 10MB total
                    let alloc = monitor_clone
                        .track_event_memory(2 * 1024 * 1024)
                        .unwrap_or_else(|_| panic!("Task {i} allocation {j} should succeed"));
                    allocations.push(alloc);
                    tokio::task::yield_now().await;
                }
                allocations // Keep allocations alive
            });
            handles.push(handle);
        }

        // Wait for all tasks and collect allocations
        let mut all_allocations = vec![];
        for handle in handles {
            let allocations = handle.await.unwrap();
            all_allocations.extend(allocations);
        }

        // Verify total allocation (10 tasks * 5 allocations * 2MB = 100MB)
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 100 * 1024 * 1024);
        assert_eq!(stats.event_count, 50);

        // Drop all allocations
        drop(all_allocations);

        // Verify memory is fully released
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 0);
        assert_eq!(stats.event_count, 0);
    }

    #[tokio::test]
    async fn test_mixed_allocation_types() {
        let monitor = MemoryMonitor::new(100, 100); // 100MB each

        // Interleave queue and checkpoint allocations
        let mut allocations = vec![];

        for _i in 0..5 {
            let queue_alloc = monitor.track_event_memory(5 * 1024 * 1024).unwrap();
            let checkpoint_alloc = monitor.track_checkpoint_memory(3 * 1024 * 1024).unwrap();
            allocations.push((queue_alloc, checkpoint_alloc));
        }

        // Verify totals
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 25 * 1024 * 1024); // 5 * 5MB
        assert_eq!(stats.checkpoint_memory_bytes, 15 * 1024 * 1024); // 5 * 3MB
        assert_eq!(stats.event_count, 5);

        // Drop specific allocations and verify
        allocations.truncate(3); // Keep only first 3 pairs

        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 15 * 1024 * 1024); // 3 * 5MB
        assert_eq!(stats.checkpoint_memory_bytes, 9 * 1024 * 1024); // 3 * 3MB
        assert_eq!(stats.event_count, 3);
    }

    #[tokio::test]
    async fn test_memory_pressure_scenarios() {
        let mut monitor = MemoryMonitor::new(50, 50); // 50MB limits
        let mut rx = monitor.start_monitoring();

        // Scenario 1: Queue pressure
        let _alloc1 = monitor.track_event_memory(46 * 1024 * 1024).unwrap(); // 92% queue

        // Collect events with a timeout
        let mut found_queue_pressure = false;
        let mut found_checkpoint_pressure = false;

        // Wait for first event
        if let Ok(Some(MemoryPressureEvent::HighQueueMemory(pct))) =
            timeout(Duration::from_secs(3), rx.recv()).await
        {
            assert!(pct > 90.0);
            found_queue_pressure = true;
        }

        // Scenario 2: Checkpoint pressure
        let _alloc2 = monitor.track_checkpoint_memory(46 * 1024 * 1024).unwrap(); // 92% checkpoint

        // Wait for more events, collecting both types
        let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline
            && (!found_queue_pressure || !found_checkpoint_pressure)
        {
            if let Ok(Some(event)) = timeout(Duration::from_millis(500), rx.recv()).await {
                match event {
                    MemoryPressureEvent::HighQueueMemory(pct) => {
                        assert!(pct > 90.0);
                        found_queue_pressure = true;
                    }
                    MemoryPressureEvent::HighCheckpointMemory(pct) => {
                        assert!(pct > 90.0);
                        found_checkpoint_pressure = true;
                    }
                }
            }
        }

        assert!(
            found_queue_pressure,
            "Should have received queue pressure event"
        );
        assert!(
            found_checkpoint_pressure,
            "Should have received checkpoint pressure event"
        );

        monitor.stop().await;
    }

    #[tokio::test]
    async fn test_allocation_rollback_on_error() {
        let monitor = MemoryMonitor::new(10, 10); // Small limits

        // Fill up queue memory
        let _alloc = monitor.track_event_memory(10 * 1024 * 1024).unwrap();

        // Verify we're at limit
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 10 * 1024 * 1024);

        // Try to allocate more (should fail and rollback)
        for _ in 0..5 {
            let result = monitor.track_event_memory(1024 * 1024); // 1MB
            assert!(result.is_err());
        }

        // Verify memory wasn't increased despite failed attempts
        let stats = monitor.get_stats();
        assert_eq!(stats.queue_memory_bytes, 10 * 1024 * 1024);
        assert_eq!(stats.event_count, 1);
    }

    #[tokio::test]
    async fn test_memory_monitor_shutdown() {
        let mut monitor = MemoryMonitor::new(100, 100);
        let mut rx = monitor.start_monitoring();

        // Stop monitoring
        monitor.stop().await;

        // Channel should be closed
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(rx.recv().await.is_none(), "Channel should be closed");

        // Multiple stops should be safe
        monitor.stop().await;
    }
}
