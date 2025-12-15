// Advanced unit tests for event deduplication

use meilibridge::delivery::deduplication::{DeduplicationKey, EventDeduplicator, compare_lsn};

#[cfg(test)]
mod deduplication_tests {
    use super::*;

    #[test]
    fn test_deduplication_key_equality() {
        let key1 = DeduplicationKey::new("0/1000".to_string(), Some(100), "users".to_string());
        let key2 = DeduplicationKey::new("0/1000".to_string(), Some(100), "users".to_string());
        let key3 = DeduplicationKey::new("0/1001".to_string(), Some(100), "users".to_string());

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_deduplication_key_with_primary_key() {
        let key1 = DeduplicationKey::new("0/1000".to_string(), Some(100), "users".to_string())
            .with_primary_key("user_1".to_string());
        let key2 = DeduplicationKey::new("0/1000".to_string(), Some(100), "users".to_string())
            .with_primary_key("user_2".to_string());

        // Same LSN/XID but different primary keys should be different
        assert_ne!(key1, key2);
    }

    #[test]
    fn test_event_deduplicator_basic() {
        let mut dedup = EventDeduplicator::new(10);

        let key = DeduplicationKey::new("0/1000".to_string(), Some(100), "events".to_string());

        // First occurrence
        assert!(!dedup.contains(&key));
        dedup.add(key.clone());

        // Should now be detected as duplicate
        assert!(dedup.contains(&key));

        // Stats should reflect this
        assert_eq!(dedup.stats().total_events, 1);
        assert_eq!(dedup.stats().duplicate_events, 0);

        // Add same key again
        dedup.add(key.clone());
        assert_eq!(dedup.stats().total_events, 2);
        assert_eq!(dedup.stats().duplicate_events, 1);
    }

    #[test]
    fn test_deduplicator_window_eviction() {
        let mut dedup = EventDeduplicator::new(5);

        // Fill the window
        for i in 0..5 {
            let key = DeduplicationKey::new(format!("0/{i}"), None, "test".to_string());
            dedup.add(key);
        }

        assert_eq!(dedup.current_size(), 5);

        // Add one more - should evict the oldest
        let new_key = DeduplicationKey::new("0/5".to_string(), None, "test".to_string());
        dedup.add(new_key);

        // First key should be evicted
        let first_key = DeduplicationKey::new("0/0".to_string(), None, "test".to_string());
        assert!(!dedup.contains(&first_key));

        // Others should still be present
        for i in 1..6 {
            let key = DeduplicationKey::new(format!("0/{i}"), None, "test".to_string());
            assert!(dedup.contains(&key));
        }

        assert_eq!(dedup.stats().window_evictions, 1);
    }

    #[test]
    fn test_deduplicator_duplicate_counting() {
        let mut dedup = EventDeduplicator::new(100);

        // Add same key multiple times
        let key = DeduplicationKey::new("0/1000".to_string(), Some(100), "orders".to_string());

        for _ in 0..5 {
            dedup.add(key.clone());
        }

        assert_eq!(dedup.stats().total_events, 5);
        assert_eq!(dedup.stats().duplicate_events, 4); // First one is not duplicate
        assert!(dedup.duplicate_rate() > 0.79 && dedup.duplicate_rate() < 0.81);
        // ~80%
    }

    #[test]
    fn test_deduplicator_clear() {
        let mut dedup = EventDeduplicator::new(50);

        // Add some keys
        for i in 0..10 {
            let key = DeduplicationKey::new(format!("0/{i}"), Some(i as u32), "test".to_string());
            dedup.add(key);
        }

        assert_eq!(dedup.current_size(), 10);

        // Clear all
        dedup.clear();

        assert_eq!(dedup.current_size(), 0);

        // Previously added keys should not be duplicates anymore
        for i in 0..10 {
            let key = DeduplicationKey::new(format!("0/{i}"), Some(i as u32), "test".to_string());
            assert!(!dedup.contains(&key));
        }
    }

    #[test]
    fn test_lsn_comparison() {
        // Test basic LSN comparison
        assert_eq!(
            compare_lsn("0/1000000", "0/1000000"),
            std::cmp::Ordering::Equal
        );
        assert_eq!(
            compare_lsn("0/1000000", "0/2000000"),
            std::cmp::Ordering::Less
        );
        assert_eq!(
            compare_lsn("0/2000000", "0/1000000"),
            std::cmp::Ordering::Greater
        );

        // Test with different high parts
        assert_eq!(
            compare_lsn("1/0", "0/FFFFFFFF"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(compare_lsn("A/1000", "9/2000"), std::cmp::Ordering::Greater);

        // Test edge cases
        assert_eq!(compare_lsn("0/0", "0/1"), std::cmp::Ordering::Less);
        assert_eq!(
            compare_lsn("FFFFFFFF/FFFFFFFF", "FFFFFFFF/FFFFFFFF"),
            std::cmp::Ordering::Equal
        );

        // Test invalid format (fallback to string comparison)
        assert_eq!(compare_lsn("invalid", "invalid"), std::cmp::Ordering::Equal);
        assert_eq!(compare_lsn("0/1000", "invalid"), std::cmp::Ordering::Less); // '0' < 'i'
    }

    #[test]
    fn test_deduplicator_with_mixed_keys() {
        let mut dedup = EventDeduplicator::new(100);

        // Add keys from different tables
        let tables = vec!["users", "orders", "products"];
        let mut total_keys = 0;

        for table in &tables {
            for i in 0..10 {
                let key =
                    DeduplicationKey::new(format!("0/{i:04x}"), Some(i as u32), table.to_string());
                dedup.add(key);
                total_keys += 1;
            }
        }

        assert_eq!(dedup.current_size(), total_keys);
        assert_eq!(dedup.stats().total_events, total_keys as u64);

        // Verify all keys are tracked
        for table in &tables {
            for i in 0..10 {
                let key =
                    DeduplicationKey::new(format!("0/{i:04x}"), Some(i as u32), table.to_string());
                assert!(dedup.contains(&key));
            }
        }
    }

    #[test]
    fn test_deduplicator_performance_characteristics() {
        let window_sizes = vec![10, 100, 1000, 10000];

        for window_size in window_sizes {
            let mut dedup = EventDeduplicator::new(window_size);

            // Fill to capacity
            for i in 0..window_size {
                let key = DeduplicationKey::new(
                    format!("0/{i:08x}"),
                    Some(i as u32),
                    "perf_test".to_string(),
                );
                dedup.add(key);
            }

            // Verify size
            assert_eq!(dedup.current_size(), window_size);

            // Add more to trigger evictions
            for i in window_size..(window_size * 2) {
                let key = DeduplicationKey::new(
                    format!("0/{i:08x}"),
                    Some(i as u32),
                    "perf_test".to_string(),
                );
                dedup.add(key);
            }

            // Window should still be at capacity
            assert_eq!(dedup.current_size(), window_size);
            assert_eq!(dedup.stats().window_evictions, window_size as u64);
        }
    }

    #[test]
    fn test_deduplicator_edge_cases() {
        // Test with window size of 1
        let mut dedup = EventDeduplicator::new(1);

        let key1 = DeduplicationKey::new("0/1".to_string(), None, "test".to_string());
        let key2 = DeduplicationKey::new("0/2".to_string(), None, "test".to_string());

        dedup.add(key1.clone());
        assert!(dedup.contains(&key1));

        dedup.add(key2.clone());
        assert!(!dedup.contains(&key1)); // Should be evicted
        assert!(dedup.contains(&key2));

        // Test with empty deduplicator
        let dedup_empty = EventDeduplicator::new(100);
        assert_eq!(dedup_empty.current_size(), 0);
        assert_eq!(dedup_empty.duplicate_rate(), 0.0);

        // Test with keys having no XID
        let mut dedup_no_xid = EventDeduplicator::new(10);
        let key_no_xid = DeduplicationKey::new("0/1000".to_string(), None, "test".to_string());
        dedup_no_xid.add(key_no_xid.clone());
        assert!(dedup_no_xid.contains(&key_no_xid));
    }
}
