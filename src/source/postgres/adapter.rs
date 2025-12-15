use crate::config::PostgreSQLConfig;
use crate::error::{MeiliBridgeError, Result};
use crate::models::Position;
use crate::source::adapter::{DataStream, EventStream, SourceAdapter};
use crate::source::postgres::replication::ReplicationConsumer;
use crate::source::postgres::{PostgresConnector, ReplicationSlotManager};
use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_postgres::types::PgLsn;
use tracing::debug;

pub struct PostgresAdapter {
    _config: PostgreSQLConfig,
    connector: PostgresConnector,
    slot_manager: ReplicationSlotManager,
    replication_consumer: Option<ReplicationConsumer>,
    replication_handle: Option<tokio::task::JoinHandle<()>>,
    last_lsn: Option<PgLsn>,
    is_paused: bool,
}

impl PostgresAdapter {
    pub fn new(config: PostgreSQLConfig) -> Self {
        let connector = PostgresConnector::new(config.clone());
        let slot_manager =
            ReplicationSlotManager::new(config.slot_name.clone(), config.publication.clone());

        Self {
            _config: config,
            connector,
            slot_manager,
            replication_consumer: None,
            replication_handle: None,
            last_lsn: None,
            is_paused: false,
        }
    }

    /// Get access to the PostgreSQL connector
    pub fn connector(&self) -> &PostgresConnector {
        &self.connector
    }
}

#[async_trait]
impl SourceAdapter for PostgresAdapter {
    async fn connect(&mut self) -> Result<()> {
        // Connect to PostgreSQL
        self.connector.connect().await?;

        // Get a client for setup
        let client = self.connector.get_client().await?;

        // Create replication slot if needed
        self.slot_manager.create_slot(&client).await?;

        // Create publication if needed (empty tables means all tables)
        self.slot_manager.create_publication(&client, &[]).await?;

        Ok(())
    }

    async fn get_changes(&mut self) -> Result<EventStream> {
        // Get the starting LSN using regular client
        let client = self.connector.get_client().await?;
        let start_lsn = if let Some(lsn) = self.last_lsn {
            Some(lsn.to_string())
        } else {
            // Get confirmed flush LSN from slot
            self.slot_manager.get_slot_lsn(&client).await?
        };

        debug!("Starting replication from LSN: {:?}", start_lsn);

        // Create a new replication connection for the consumer
        let (repl_client, repl_handle) = self.connector.get_replication_client().await?;

        // Create a channel for events
        let (tx, rx) = mpsc::channel(1000);

        // Stop any existing consumer
        if let Some(mut consumer) = self.replication_consumer.take() {
            consumer.stop();
        }
        if let Some(handle) = self.replication_handle.take() {
            handle.abort();
        }

        // Create replication consumer
        let mut consumer = ReplicationConsumer::new(
            repl_client,
            self.slot_manager.slot_name().to_string(),
            self.slot_manager.publication_name().to_string(),
            tx,
        );

        // Start the replication
        consumer.start_replication(start_lsn).await?;

        // Store the consumer and handle
        self.replication_consumer = Some(consumer);
        self.replication_handle = Some(repl_handle);

        // Convert to stream
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);

        Ok(Box::pin(stream))
    }

    async fn get_full_data(&self, table: &str, batch_size: usize) -> Result<DataStream> {
        let (tx, rx) = mpsc::channel(100);

        // Use FullSyncHandler for better performance with statement caching
        let handler = crate::source::postgres::FullSyncHandler::new(self.connector.clone());
        let table_name = table.to_string();

        tokio::spawn(async move {
            if let Err(e) = handler.sync_table(&table_name, batch_size, tx).await {
                tracing::error!("Full sync failed for table {}: {}", table_name, e);
            }
        });

        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }

    async fn get_current_position(&self) -> Result<Position> {
        let client = self.connector.get_client().await?;
        let lsn = self
            .slot_manager
            .get_slot_lsn(&client)
            .await?
            .unwrap_or_else(|| "0/0".to_string());

        Ok(Position::postgresql(lsn))
    }

    async fn set_start_position(&mut self, position: Position) -> Result<()> {
        match position {
            Position::PostgreSQL { lsn } => {
                // Parse LSN
                let pg_lsn: PgLsn = lsn
                    .parse()
                    .map_err(|_| MeiliBridgeError::Source(format!("Invalid LSN: {lsn}")))?;

                self.last_lsn = Some(pg_lsn);
                debug!("Set start position to: {}", lsn);
                Ok(())
            }
            _ => Err(MeiliBridgeError::Source(
                "Invalid position type for PostgreSQL".to_string(),
            )),
        }
    }

    async fn acknowledge(&mut self, position: Position) -> Result<()> {
        match position {
            Position::PostgreSQL { lsn } => {
                // Parse LSN
                let pg_lsn: PgLsn = lsn
                    .parse()
                    .map_err(|_| MeiliBridgeError::Source(format!("Invalid LSN: {lsn}")))?;

                self.last_lsn = Some(pg_lsn);
                debug!("Acknowledged position: {}", lsn);
                Ok(())
            }
            _ => Err(MeiliBridgeError::Source(
                "Invalid position type for PostgreSQL".to_string(),
            )),
        }
    }

    fn is_connected(&self) -> bool {
        self.connector.is_connected()
    }

    async fn pause(&mut self) -> Result<()> {
        self.is_paused = true;
        debug!("PostgreSQL adapter paused");
        Ok(())
    }

    async fn resume(&mut self) -> Result<()> {
        self.is_paused = false;
        debug!("PostgreSQL adapter resumed");
        Ok(())
    }

    fn is_paused(&self) -> bool {
        self.is_paused
    }

    async fn disconnect(&mut self) -> Result<()> {
        // Stop replication consumer
        if let Some(mut consumer) = self.replication_consumer.take() {
            consumer.stop();
        }

        // Drop replication handle
        if let Some(handle) = self.replication_handle.take() {
            handle.abort();
        }

        // Disconnect from PostgreSQL
        self.connector.disconnect().await;

        Ok(())
    }

    fn clone_box(&self) -> Box<dyn SourceAdapter> {
        // Create a new adapter with the same config
        // This will create a new connection from the pool, not a duplicate connection
        // The connection pool itself handles connection reuse efficiently
        Box::new(PostgresAdapter::new(self._config.clone()))
    }
}
