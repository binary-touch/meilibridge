use crate::error::Result;
use crate::metrics;
use crate::models::stream_event::Event;
use crate::source::adapter::SourceAdapter;
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio::sync::mpsc;
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

/// Coordinates CDC event consumption and distribution to sync tasks
pub struct CdcCoordinator {
    /// Source adapter for CDC events
    source_adapter: Box<dyn SourceAdapter>,
    /// Channels to send events to each sync task (keyed by table name)
    task_channels: HashMap<String, mpsc::Sender<Result<Event>>>,
    /// Shutdown signal
    shutdown_rx: watch::Receiver<bool>,
    /// Paused state for the coordinator
    is_paused: Arc<RwLock<bool>>,
    /// Paused tables - if empty, all tables are paused when is_paused is true
    paused_tables: Arc<RwLock<std::collections::HashSet<String>>>,
}

impl CdcCoordinator {
    pub fn new(source_adapter: Box<dyn SourceAdapter>, shutdown_rx: watch::Receiver<bool>) -> Self {
        Self {
            source_adapter,
            task_channels: HashMap::new(),
            shutdown_rx,
            is_paused: Arc::new(RwLock::new(false)),
            paused_tables: Arc::new(RwLock::new(std::collections::HashSet::new())),
        }
    }

    /// Register a sync task to receive events for a specific table
    pub fn register_task(&mut self, table_name: String, tx: mpsc::Sender<Result<Event>>) {
        info!(
            "Coordinator: Registering sync task for table '{}'",
            table_name
        );
        self.task_channels.insert(table_name.clone(), tx);
        debug!(
            "Coordinator: Registered tasks: {:?}",
            self.task_channels.keys().collect::<Vec<_>>()
        );
    }

    /// Start the coordinator
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting CDC coordinator");

        // Get the CDC event stream
        let mut event_stream = self.source_adapter.get_changes().await?;

        loop {
            tokio::select! {
                // Process events from stream
                Some(event_result) = event_stream.next() => {
                    match event_result {
                        Ok(event) => {
                            debug!("Coordinator: Received event from stream");

                            // Check if we're paused globally
                            if *self.is_paused.read().await {
                                debug!("Coordinator is paused, skipping event");
                                continue;
                            }

                            // Track metrics for received events
                            if let Event::Cdc(ref cdc_event) = event {
                                let table = format!("{}.{}", cdc_event.schema, cdc_event.table);
                                let event_type = format!("{:?}", cdc_event.event_type).to_lowercase();
                                metrics::record_cdc_event_received(&table, &event_type);
                            }

                            self.distribute_event(event).await;
                        }
                        Err(e) => {
                            error!("Coordinator: Error receiving CDC event: {}", e);
                            // Log the error but continue processing
                            // Individual sync tasks will handle reconnection if needed
                        }
                    }
                }

                // Check for shutdown
                _ = self.shutdown_rx.changed() => {
                    if *self.shutdown_rx.borrow() {
                        info!("CDC coordinator received shutdown signal");
                        break;
                    }
                }
            }
        }

        info!("CDC coordinator stopped");
        Ok(())
    }

    /// Distribute an event to the appropriate sync task
    async fn distribute_event(&self, event: Event) {
        let table_name = match &event {
            Event::Cdc(cdc_event) => {
                // Support both "table" and "schema.table" formats
                if cdc_event.schema.is_empty() {
                    cdc_event.table.clone()
                } else {
                    format!("{}.{}", cdc_event.schema, cdc_event.table)
                }
            }
            _ => {
                debug!("Skipping non-CDC event distribution");
                return;
            }
        };

        // Check if this specific table is paused
        {
            let paused_tables = self.paused_tables.read().await;
            if paused_tables.contains(&table_name)
                || (table_name.contains('.')
                    && paused_tables.contains(table_name.split('.').next_back().unwrap_or("")))
            {
                debug!("Table '{}' is paused, skipping event", table_name);
                return;
            }
        }

        debug!("Distributing event for table: {}", table_name);

        // Send to the specific task
        if let Some(tx) = self.task_channels.get(&table_name) {
            debug!("Sending event to task for table '{}'", table_name);

            // Try to send without blocking first
            match tx.try_send(Ok(event.clone())) {
                Ok(()) => {
                    // Sent successfully without blocking
                }
                Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                    // Channel is full - activate backpressure
                    warn!(
                        "Channel full for table '{}', activating backpressure",
                        table_name
                    );
                    self.pause_table(&table_name).await;

                    // Now do a blocking send
                    if tx.send(Ok(event.clone())).await.is_err() {
                        warn!("Failed to send event to task for table '{}'", table_name);
                    }
                }
                Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                    warn!("Channel closed for table '{}'", table_name);
                }
            }
        } else {
            debug!("No task registered for table '{}'", table_name);
        }

        // Also try without schema prefix
        if table_name.contains('.') {
            let table_only = table_name.split('.').next_back().unwrap_or("");
            if let Some(tx) = self.task_channels.get(table_only) {
                debug!(
                    "Sending event to task for table '{}' (without schema)",
                    table_only
                );
                // Try to send without blocking first
                match tx.try_send(Ok(event)) {
                    Ok(()) => {
                        // Sent successfully without blocking
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Full(msg)) => {
                        // Channel is full - activate backpressure
                        warn!(
                            "Channel full for table '{}' (without schema), activating backpressure",
                            table_only
                        );
                        self.pause_table(table_only).await;

                        // Now do a blocking send
                        if tx.send(msg).await.is_err() {
                            warn!("Failed to send event to task for table '{}'", table_only);
                        }
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                        warn!("Channel closed for table '{}'", table_only);
                    }
                }
            }
        }
    }

    /// Pause all CDC event processing
    pub async fn pause_all(&self) {
        *self.is_paused.write().await = true;
        info!("CDC coordinator paused all event processing");
        metrics::CDC_CONTROL_OPERATIONS
            .with_label_values(&["pause", "global", "all"])
            .inc();
    }

    /// Resume all CDC event processing
    pub async fn resume_all(&self) {
        *self.is_paused.write().await = false;
        info!("CDC coordinator resumed all event processing");
        metrics::CDC_CONTROL_OPERATIONS
            .with_label_values(&["resume", "global", "all"])
            .inc();
    }

    /// Check if coordinator is paused
    pub async fn is_paused(&self) -> bool {
        *self.is_paused.read().await
    }

    /// Pause CDC processing for a specific table
    pub async fn pause_table(&self, table_name: &str) {
        self.paused_tables
            .write()
            .await
            .insert(table_name.to_string());
        info!(
            "CDC coordinator paused processing for table '{}'",
            table_name
        );
        metrics::CDC_CONTROL_OPERATIONS
            .with_label_values(&["pause", "table", table_name])
            .inc();
    }

    /// Resume CDC processing for a specific table
    pub async fn resume_table(&self, table_name: &str) {
        self.paused_tables.write().await.remove(table_name);
        info!(
            "CDC coordinator resumed processing for table '{}'",
            table_name
        );
        metrics::CDC_CONTROL_OPERATIONS
            .with_label_values(&["resume", "table", table_name])
            .inc();
    }

    /// Get list of paused tables
    pub async fn get_paused_tables(&self) -> Vec<String> {
        self.paused_tables.read().await.iter().cloned().collect()
    }

    /// Unregister a sync task
    pub fn unregister_task(&mut self, table_name: &str) {
        info!(
            "Coordinator: Unregistering sync task for table '{}'",
            table_name
        );
        self.task_channels.remove(table_name);
        debug!(
            "Coordinator: Remaining tasks: {:?}",
            self.task_channels.keys().collect::<Vec<_>>()
        );
    }
}
