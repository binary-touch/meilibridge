use crate::config::AdaptiveBatchingConfig;
use crate::error::Result;
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use sysinfo::{MemoryRefreshKind, RefreshKind, System};
use tokio::sync::RwLock;
use tokio::time::{Duration, Instant};

/// Metrics for batch processing
#[derive(Debug, Clone)]
pub struct BatchMetrics {
    pub batch_size: usize,
    pub processing_time_ms: u64,
    pub documents_per_second: f64,
    pub memory_usage_mb: f64,
    pub timestamp: Instant,
}

/// Adaptive batch size calculator
#[derive(Clone)]
pub struct AdaptiveBatchingManager {
    config: AdaptiveBatchingConfig,
    // Per-table metrics and batch sizes
    table_metrics: Arc<RwLock<HashMap<String, TableBatchState>>>,
    // Global memory monitor
    memory_monitor: Arc<MemoryMonitor>,
}

#[derive(Debug)]
struct TableBatchState {
    current_batch_size: usize,
    metrics_window: VecDeque<BatchMetrics>,
    last_adjustment: Instant,
    consecutive_successes: u32,
    consecutive_failures: u32,
}

impl TableBatchState {
    fn new(initial_size: usize) -> Self {
        Self {
            current_batch_size: initial_size,
            metrics_window: VecDeque::new(),
            last_adjustment: Instant::now(),
            consecutive_successes: 0,
            consecutive_failures: 0,
        }
    }
}

#[derive(Clone)]
struct MemoryMonitor {
    #[allow(dead_code)]
    threshold_percentage: f64,
    system: Arc<RwLock<System>>,
}

impl MemoryMonitor {
    fn new(threshold_percentage: f64) -> Self {
        let mut system = System::new_with_specifics(
            RefreshKind::nothing().with_memory(MemoryRefreshKind::everything()),
        );
        system.refresh_memory();

        Self {
            threshold_percentage,
            system: Arc::new(RwLock::new(system)),
        }
    }

    async fn get_memory_pressure(&self) -> f64 {
        // Refresh memory information
        let mut system = self.system.write().await;
        system.refresh_memory();

        let total_memory = system.total_memory();
        let used_memory = system.used_memory();

        if total_memory > 0 {
            (used_memory as f64 / total_memory as f64) * 100.0
        } else {
            0.0
        }
    }

    #[allow(dead_code)]
    async fn get_available_memory_mb(&self) -> f64 {
        let system = self.system.read().await;
        let available_memory = system.available_memory();
        // Convert from KB to MB
        available_memory as f64 / 1024.0
    }
}

impl AdaptiveBatchingManager {
    pub fn new(config: AdaptiveBatchingConfig) -> Self {
        let memory_monitor = Arc::new(MemoryMonitor::new(config.memory_pressure_threshold));

        Self {
            config,
            table_metrics: Arc::new(RwLock::new(HashMap::new())),
            memory_monitor,
        }
    }

    /// Get the optimal batch size for a table
    pub async fn get_batch_size(
        &self,
        table_name: &str,
        default_size: usize,
        min_size: usize,
        max_size: usize,
    ) -> Result<usize> {
        if !self.config.per_table_optimization {
            return Ok(default_size);
        }

        let mut metrics = self.table_metrics.write().await;
        let state = metrics
            .entry(table_name.to_string())
            .or_insert_with(|| TableBatchState::new(default_size));

        // Check if we should adjust
        let elapsed = state.last_adjustment.elapsed();
        if elapsed < Duration::from_millis(self.config.adjustment_interval_ms) {
            return Ok(state.current_batch_size);
        }

        // Calculate average metrics
        if state.metrics_window.len() >= self.config.metric_window_size / 2 {
            let avg_latency = self.calculate_average_latency(&state.metrics_window);
            let memory_pressure = self.memory_monitor.get_memory_pressure().await;

            // Adjust batch size based on latency and memory
            let new_size = self.calculate_new_batch_size(
                state.current_batch_size,
                avg_latency,
                memory_pressure,
                min_size,
                max_size,
            );

            if new_size != state.current_batch_size {
                tracing::info!(
                    table = table_name,
                    old_size = state.current_batch_size,
                    new_size = new_size,
                    avg_latency_ms = avg_latency,
                    memory_pressure = memory_pressure,
                    "Adjusting batch size"
                );

                state.current_batch_size = new_size;
                state.last_adjustment = Instant::now();
            }
        }

        Ok(state.current_batch_size)
    }

    /// Record batch processing metrics
    pub async fn record_metrics(&self, table_name: &str, metrics: BatchMetrics) -> Result<()> {
        let mut table_metrics = self.table_metrics.write().await;
        let state = table_metrics
            .entry(table_name.to_string())
            .or_insert_with(|| TableBatchState::new(metrics.batch_size));

        // Add to sliding window
        state.metrics_window.push_back(metrics.clone());
        if state.metrics_window.len() > self.config.metric_window_size {
            state.metrics_window.pop_front();
        }

        // Track success/failure patterns
        if metrics.processing_time_ms < self.config.target_latency_ms {
            state.consecutive_successes += 1;
            state.consecutive_failures = 0;
        } else {
            state.consecutive_failures += 1;
            state.consecutive_successes = 0;
        }

        Ok(())
    }

    /// Calculate average latency from metrics window
    fn calculate_average_latency(&self, window: &VecDeque<BatchMetrics>) -> u64 {
        if window.is_empty() {
            return 0;
        }

        let sum: u64 = window.iter().map(|m| m.processing_time_ms).sum();
        sum / window.len() as u64
    }

    /// Calculate new batch size based on performance metrics
    fn calculate_new_batch_size(
        &self,
        current_size: usize,
        avg_latency_ms: u64,
        memory_pressure: f64,
        min_size: usize,
        max_size: usize,
    ) -> usize {
        let mut new_size = current_size;

        // Memory pressure check first
        if memory_pressure > 0.8 {
            // High memory pressure - reduce batch size
            new_size = (current_size as f64 * 0.8) as usize;
        } else {
            // Adjust based on latency
            let latency_ratio = self.config.target_latency_ms as f64 / avg_latency_ms as f64;

            if latency_ratio > 1.2 {
                // We're faster than target - can increase batch size
                let increase_factor =
                    1.0 + (self.config.adjustment_factor * f64::min(latency_ratio - 1.0, 1.0));
                new_size = (current_size as f64 * increase_factor) as usize;
            } else if latency_ratio < 0.8 {
                // We're slower than target - decrease batch size
                let decrease_factor =
                    1.0 - (self.config.adjustment_factor * f64::min(1.0 - latency_ratio, 0.5));
                new_size = (current_size as f64 * decrease_factor) as usize;
            }
        }

        // Apply bounds
        new_size.max(min_size).min(max_size)
    }

    /// Get performance statistics for monitoring
    pub async fn get_statistics(&self) -> HashMap<String, TableBatchStatistics> {
        let metrics = self.table_metrics.read().await;
        let mut stats = HashMap::new();

        for (table, state) in metrics.iter() {
            let avg_latency = self.calculate_average_latency(&state.metrics_window);
            let avg_throughput = if !state.metrics_window.is_empty() {
                state
                    .metrics_window
                    .iter()
                    .map(|m| m.documents_per_second)
                    .sum::<f64>()
                    / state.metrics_window.len() as f64
            } else {
                0.0
            };

            stats.insert(
                table.clone(),
                TableBatchStatistics {
                    current_batch_size: state.current_batch_size,
                    average_latency_ms: avg_latency,
                    average_throughput: avg_throughput,
                    consecutive_successes: state.consecutive_successes,
                    consecutive_failures: state.consecutive_failures,
                    metrics_count: state.metrics_window.len(),
                },
            );
        }

        stats
    }
}

#[derive(Debug, Clone)]
pub struct TableBatchStatistics {
    pub current_batch_size: usize,
    pub average_latency_ms: u64,
    pub average_throughput: f64,
    pub consecutive_successes: u32,
    pub consecutive_failures: u32,
    pub metrics_count: usize,
}
