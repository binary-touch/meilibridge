use crate::error::Result;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tokio::sync::mpsc;
use tokio::time::{Duration, interval};
use tracing::{info, warn};

/// Tracks memory usage and enforces limits
#[derive(Clone)]
pub struct MemoryMonitor {
    /// Maximum memory allowed for event queues (bytes)
    max_queue_memory: u64,
    /// Maximum memory for checkpoint cache (bytes)
    max_checkpoint_memory: u64,
    /// Current memory usage for queues
    current_queue_memory: Arc<AtomicU64>,
    /// Current memory usage for checkpoints
    current_checkpoint_memory: Arc<AtomicU64>,
    /// Number of events currently in memory
    event_count: Arc<AtomicUsize>,
    /// Shutdown signal
    shutdown_tx: Option<mpsc::Sender<()>>,
}

impl MemoryMonitor {
    pub fn new(max_queue_memory_mb: usize, max_checkpoint_memory_mb: usize) -> Self {
        Self {
            max_queue_memory: (max_queue_memory_mb * 1024 * 1024) as u64,
            max_checkpoint_memory: (max_checkpoint_memory_mb * 1024 * 1024) as u64,
            current_queue_memory: Arc::new(AtomicU64::new(0)),
            current_checkpoint_memory: Arc::new(AtomicU64::new(0)),
            event_count: Arc::new(AtomicUsize::new(0)),
            shutdown_tx: None,
        }
    }

    /// Stop the memory monitor
    pub async fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(()).await;
        }
    }

    /// Start background monitoring task
    pub fn start_monitoring(&mut self) -> mpsc::Receiver<MemoryPressureEvent> {
        let (tx, rx) = mpsc::channel(10);
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let current_queue = self.current_queue_memory.clone();
        let current_checkpoint = self.current_checkpoint_memory.clone();
        let max_queue = self.max_queue_memory;
        let max_checkpoint = self.max_checkpoint_memory;

        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(1));

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let queue_usage = current_queue.load(Ordering::Relaxed);
                        let checkpoint_usage = current_checkpoint.load(Ordering::Relaxed);

                        let queue_percent = (queue_usage as f64 / max_queue as f64) * 100.0;
                        let checkpoint_percent = (checkpoint_usage as f64 / max_checkpoint as f64) * 100.0;

                        // Check for memory pressure
                        if queue_percent > 90.0 {
                            warn!(
                                "High queue memory usage: {:.1}% ({:.2} MB / {:.2} MB)",
                                queue_percent,
                                queue_usage as f64 / 1_048_576.0,
                                max_queue as f64 / 1_048_576.0
                            );
                            let _ = tx.send(MemoryPressureEvent::HighQueueMemory(queue_percent)).await;
                        }

                        if checkpoint_percent > 90.0 {
                            warn!(
                                "High checkpoint memory usage: {:.1}% ({:.2} MB / {:.2} MB)",
                                checkpoint_percent,
                                checkpoint_usage as f64 / 1_048_576.0,
                                max_checkpoint as f64 / 1_048_576.0
                            );
                            let _ = tx.send(MemoryPressureEvent::HighCheckpointMemory(checkpoint_percent)).await;
                        }
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Memory monitor shutting down");
                        break;
                    }
                }
            }
        });

        rx
    }

    /// Track memory allocation for an event
    pub fn track_event_memory(&self, size_bytes: u64) -> Result<MemoryAllocation> {
        let new_total = self
            .current_queue_memory
            .fetch_add(size_bytes, Ordering::SeqCst)
            + size_bytes;

        if new_total > self.max_queue_memory {
            // Rollback the allocation
            self.current_queue_memory
                .fetch_sub(size_bytes, Ordering::SeqCst);
            return Err(crate::error::MeiliBridgeError::ResourceExhausted(format!(
                "Queue memory limit exceeded: {} MB",
                self.max_queue_memory / 1_048_576
            )));
        }

        self.event_count.fetch_add(1, Ordering::Relaxed);

        Ok(MemoryAllocation {
            monitor: self.clone(),
            size_bytes,
            allocation_type: AllocationType::Queue,
        })
    }

    /// Track memory allocation for checkpoint
    pub fn track_checkpoint_memory(&self, size_bytes: u64) -> Result<MemoryAllocation> {
        let new_total = self
            .current_checkpoint_memory
            .fetch_add(size_bytes, Ordering::SeqCst)
            + size_bytes;

        if new_total > self.max_checkpoint_memory {
            // Rollback the allocation
            self.current_checkpoint_memory
                .fetch_sub(size_bytes, Ordering::SeqCst);
            return Err(crate::error::MeiliBridgeError::ResourceExhausted(format!(
                "Checkpoint memory limit exceeded: {} MB",
                self.max_checkpoint_memory / 1_048_576
            )));
        }

        Ok(MemoryAllocation {
            monitor: self.clone(),
            size_bytes,
            allocation_type: AllocationType::Checkpoint,
        })
    }

    /// Get current memory statistics
    pub fn get_stats(&self) -> MemoryStats {
        MemoryStats {
            queue_memory_bytes: self.current_queue_memory.load(Ordering::Relaxed),
            checkpoint_memory_bytes: self.current_checkpoint_memory.load(Ordering::Relaxed),
            max_queue_memory_bytes: self.max_queue_memory,
            max_checkpoint_memory_bytes: self.max_checkpoint_memory,
            event_count: self.event_count.load(Ordering::Relaxed),
        }
    }

    /// Check if we should apply backpressure
    pub fn should_apply_backpressure(&self) -> bool {
        let queue_usage = self.current_queue_memory.load(Ordering::Relaxed);
        let queue_percent = (queue_usage as f64 / self.max_queue_memory as f64) * 100.0;
        queue_percent > 80.0
    }

    /// Release memory when dropping allocation
    fn release_memory(&self, size_bytes: u64, allocation_type: AllocationType) {
        match allocation_type {
            AllocationType::Queue => {
                self.current_queue_memory
                    .fetch_sub(size_bytes, Ordering::SeqCst);
                self.event_count.fetch_sub(1, Ordering::Relaxed);
            }
            AllocationType::Checkpoint => {
                self.current_checkpoint_memory
                    .fetch_sub(size_bytes, Ordering::SeqCst);
            }
        }
    }
}

/// RAII guard for memory allocation
pub struct MemoryAllocation {
    monitor: MemoryMonitor,
    size_bytes: u64,
    allocation_type: AllocationType,
}

impl Drop for MemoryAllocation {
    fn drop(&mut self) {
        self.monitor
            .release_memory(self.size_bytes, self.allocation_type);
    }
}

#[derive(Debug, Clone, Copy)]
enum AllocationType {
    Queue,
    Checkpoint,
}

/// Memory pressure events
#[derive(Debug, Clone)]
pub enum MemoryPressureEvent {
    HighQueueMemory(f64),
    HighCheckpointMemory(f64),
}

/// Memory usage statistics
#[derive(Debug, Clone)]
pub struct MemoryStats {
    pub queue_memory_bytes: u64,
    pub checkpoint_memory_bytes: u64,
    pub max_queue_memory_bytes: u64,
    pub max_checkpoint_memory_bytes: u64,
    pub event_count: usize,
}

impl MemoryStats {
    pub fn queue_usage_percent(&self) -> f64 {
        (self.queue_memory_bytes as f64 / self.max_queue_memory_bytes as f64) * 100.0
    }

    pub fn checkpoint_usage_percent(&self) -> f64 {
        (self.checkpoint_memory_bytes as f64 / self.max_checkpoint_memory_bytes as f64) * 100.0
    }
}
