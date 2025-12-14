//! Event deduplication for at-least-once delivery

use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use tracing::debug;

/// Key for event deduplication using LSN + Transaction ID
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeduplicationKey {
    /// PostgreSQL Log Sequence Number
    pub lsn: String,

    /// Transaction ID (XID)
    pub xid: Option<u32>,

    /// Table name
    pub table: String,

    /// Optional primary key for row-level deduplication
    pub primary_key: Option<String>,
}

impl DeduplicationKey {
    pub fn new(lsn: String, xid: Option<u32>, table: String) -> Self {
        Self {
            lsn,
            xid,
            table,
            primary_key: None,
        }
    }

    pub fn with_primary_key(mut self, pk: String) -> Self {
        self.primary_key = Some(pk);
        self
    }
}

/// Sliding window event deduplicator
pub struct EventDeduplicator {
    /// Maximum window size
    window_size: usize,

    /// Ordered queue of keys (oldest first)
    key_queue: VecDeque<DeduplicationKey>,

    /// Hash set for O(1) lookups
    key_set: HashMap<DeduplicationKey, usize>,

    /// Statistics
    stats: DeduplicationStats,
}

#[derive(Debug, Default)]
pub struct DeduplicationStats {
    pub total_events: u64,
    pub duplicate_events: u64,
    pub window_evictions: u64,
}

impl EventDeduplicator {
    pub fn new(window_size: usize) -> Self {
        Self {
            window_size,
            key_queue: VecDeque::with_capacity(window_size),
            key_set: HashMap::with_capacity(window_size),
            stats: DeduplicationStats::default(),
        }
    }

    /// Check if an event has been seen before
    pub fn contains(&self, key: &DeduplicationKey) -> bool {
        self.key_set.contains_key(key)
    }

    /// Add an event to the deduplication window
    pub fn add(&mut self, key: DeduplicationKey) {
        self.stats.total_events += 1;

        // Check if already exists
        if let Some(count) = self.key_set.get_mut(&key) {
            *count += 1;
            self.stats.duplicate_events += 1;
            debug!("Duplicate event detected: {:?}", key);
            return;
        }

        // Evict oldest if at capacity
        if self.key_queue.len() >= self.window_size {
            if let Some(oldest_key) = self.key_queue.pop_front() {
                if let Some(count) = self.key_set.get(&oldest_key) {
                    if *count == 1 {
                        self.key_set.remove(&oldest_key);
                    } else {
                        self.key_set.insert(oldest_key, count - 1);
                    }
                }
                self.stats.window_evictions += 1;
            }
        }

        // Add new key
        self.key_queue.push_back(key.clone());
        self.key_set.insert(key, 1);
    }

    /// Clear the deduplication window
    pub fn clear(&mut self) {
        self.key_queue.clear();
        self.key_set.clear();
    }

    /// Get deduplication statistics
    pub fn stats(&self) -> &DeduplicationStats {
        &self.stats
    }

    /// Get current window size
    pub fn current_size(&self) -> usize {
        self.key_queue.len()
    }

    /// Get duplicate rate
    pub fn duplicate_rate(&self) -> f64 {
        if self.stats.total_events == 0 {
            0.0
        } else {
            self.stats.duplicate_events as f64 / self.stats.total_events as f64
        }
    }
}

/// LSN comparison for PostgreSQL
pub fn compare_lsn(lsn1: &str, lsn2: &str) -> std::cmp::Ordering {
    // Parse LSN format (e.g., "0/1234567")
    let parts1: Vec<&str> = lsn1.split('/').collect();
    let parts2: Vec<&str> = lsn2.split('/').collect();

    if parts1.len() == 2 && parts2.len() == 2 {
        let (hi1, lo1) = (
            u64::from_str_radix(parts1[0], 16).unwrap_or(0),
            u64::from_str_radix(parts1[1], 16).unwrap_or(0),
        );
        let (hi2, lo2) = (
            u64::from_str_radix(parts2[0], 16).unwrap_or(0),
            u64::from_str_radix(parts2[1], 16).unwrap_or(0),
        );

        match hi1.cmp(&hi2) {
            std::cmp::Ordering::Equal => lo1.cmp(&lo2),
            other => other,
        }
    } else {
        lsn1.cmp(lsn2)
    }
}
