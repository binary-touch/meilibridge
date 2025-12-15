//! Backpressure management for the pipeline

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{RwLock, mpsc};
use tokio::time::{Duration, interval};
use tracing::{debug, info, warn};

/// Configuration for backpressure management
#[derive(Debug, Clone)]
pub struct BackpressureConfig {
    /// High watermark - pause when channel usage exceeds this percentage
    pub high_watermark: f64,
    /// Low watermark - resume when channel usage drops below this percentage
    pub low_watermark: f64,
    /// How often to check channel capacity (milliseconds)
    pub check_interval_ms: u64,
    /// Channel capacity to monitor
    pub channel_capacity: usize,
}

impl Default for BackpressureConfig {
    fn default() -> Self {
        Self {
            high_watermark: 80.0,
            low_watermark: 60.0,
            check_interval_ms: 100,
            channel_capacity: 1000,
        }
    }
}

/// Tracks channel usage for a specific table
#[derive(Debug, Clone)]
struct ChannelMetrics {
    /// Current number of items in the channel
    current_size: usize,
    /// Whether this channel is currently throttled
    is_throttled: bool,
}

/// Manages backpressure for the pipeline
pub struct BackpressureManager {
    config: BackpressureConfig,
    /// Channel metrics per table
    channel_metrics: Arc<RwLock<HashMap<String, ChannelMetrics>>>,
    /// Sender for backpressure events
    event_tx: Option<mpsc::Sender<BackpressureEvent>>,
}

/// Events emitted by the backpressure manager
#[derive(Debug, Clone)]
pub enum BackpressureEvent {
    /// Pause CDC for a specific source
    PauseSource(String),
    /// Resume CDC for a specific source
    ResumeSource(String),
    /// Pause all sources
    PauseAll,
    /// Resume all sources
    ResumeAll,
}

impl BackpressureManager {
    pub fn new(config: BackpressureConfig) -> Self {
        Self {
            config,
            channel_metrics: Arc::new(RwLock::new(HashMap::new())),
            event_tx: None,
        }
    }

    /// Start monitoring for backpressure
    pub fn start_monitoring(&mut self) -> mpsc::Receiver<BackpressureEvent> {
        let (tx, rx) = mpsc::channel(100);
        self.event_tx = Some(tx.clone());

        let config = self.config.clone();
        let metrics = self.channel_metrics.clone();

        tokio::spawn(async move {
            let mut check_interval = interval(Duration::from_millis(config.check_interval_ms));

            loop {
                check_interval.tick().await;

                let metrics_snapshot = metrics.read().await.clone();
                let mut events = Vec::new();

                for (table, metric) in metrics_snapshot {
                    let usage_percent =
                        (metric.current_size as f64 / config.channel_capacity as f64) * 100.0;

                    if !metric.is_throttled && usage_percent >= config.high_watermark {
                        // Need to throttle
                        warn!(
                            "Table '{}' channel usage at {:.1}% - activating backpressure",
                            table, usage_percent
                        );
                        events.push(BackpressureEvent::PauseSource(table.clone()));

                        // Update metric
                        if let Some(m) = metrics.write().await.get_mut(&table) {
                            m.is_throttled = true;
                        }
                    } else if metric.is_throttled && usage_percent <= config.low_watermark {
                        // Can release throttle
                        info!(
                            "Table '{}' channel usage at {:.1}% - releasing backpressure",
                            table, usage_percent
                        );
                        events.push(BackpressureEvent::ResumeSource(table.clone()));

                        // Update metric
                        if let Some(m) = metrics.write().await.get_mut(&table) {
                            m.is_throttled = false;
                        }
                    }
                }

                // Send events
                for event in events {
                    if tx.send(event).await.is_err() {
                        debug!("Backpressure event receiver dropped");
                        return;
                    }
                }
            }
        });

        rx
    }

    /// Update channel size for a table
    pub async fn update_channel_size(&self, table: &str, size: usize) {
        let mut metrics = self.channel_metrics.write().await;
        metrics
            .entry(table.to_string())
            .and_modify(|m| m.current_size = size)
            .or_insert(ChannelMetrics {
                current_size: size,
                is_throttled: false,
            });
    }

    /// Register a table for monitoring
    pub async fn register_table(&self, table: &str) {
        let mut metrics = self.channel_metrics.write().await;
        metrics.entry(table.to_string()).or_insert(ChannelMetrics {
            current_size: 0,
            is_throttled: false,
        });
    }

    /// Check if a table is currently throttled
    pub async fn is_throttled(&self, table: &str) -> bool {
        self.channel_metrics
            .read()
            .await
            .get(table)
            .map(|m| m.is_throttled)
            .unwrap_or(false)
    }
}
