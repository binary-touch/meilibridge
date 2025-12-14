use super::parse_test_decoding_message;
use crate::error::{MeiliBridgeError, Result};
use crate::models::Event;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_postgres::Client;
use tracing::{debug, error, info, trace, warn};

pub struct ReplicationConsumer {
    client: Arc<Client>,
    slot_name: String,
    event_tx: mpsc::Sender<Result<Event>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
}

impl ReplicationConsumer {
    pub fn new(
        client: Client,
        slot_name: String,
        _publication_name: String,
        event_tx: mpsc::Sender<Result<Event>>,
    ) -> Self {
        Self {
            client: Arc::new(client),
            slot_name,
            event_tx,
            shutdown_tx: None,
        }
    }

    pub async fn start_replication(&mut self, start_lsn: Option<String>) -> Result<()> {
        // Check if slot exists and get its plugin type
        let check_slot_query = format!(
            "SELECT plugin FROM pg_replication_slots WHERE slot_name = '{}'",
            self.slot_name
        );

        match self.client.query_one(&check_slot_query, &[]).await {
            Ok(row) => {
                let plugin: String = row.get(0);
                info!(
                    "Found replication slot '{}' with plugin '{}'",
                    self.slot_name, plugin
                );

                if plugin == "pgoutput" {
                    // Drop and recreate with test_decoding
                    warn!(
                        "Slot uses pgoutput plugin which requires binary protocol. Recreating with test_decoding..."
                    );
                    self.recreate_slot_with_test_decoding().await?;
                }

                self.start_test_decoding_polling(start_lsn).await
            }
            Err(_) => {
                // Slot doesn't exist, create it with test_decoding
                warn!(
                    "Replication slot '{}' not found, creating with test_decoding plugin",
                    self.slot_name
                );
                self.create_slot_with_test_decoding().await?;
                self.start_test_decoding_polling(start_lsn).await
            }
        }
    }

    async fn recreate_slot_with_test_decoding(&self) -> Result<()> {
        // Drop the existing slot
        let drop_query = format!("SELECT pg_drop_replication_slot('{}')", self.slot_name);

        match self.client.execute(&drop_query, &[]).await {
            Ok(_) => info!("Dropped existing replication slot '{}'", self.slot_name),
            Err(e) => warn!("Failed to drop slot: {}", e),
        }

        // Create new slot with test_decoding
        self.create_slot_with_test_decoding().await
    }

    async fn create_slot_with_test_decoding(&self) -> Result<()> {
        let query = format!(
            "SELECT pg_create_logical_replication_slot('{}', 'test_decoding')",
            self.slot_name
        );

        self.client
            .execute(&query, &[])
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Failed to create slot: {}", e)))?;

        info!(
            "Created replication slot '{}' with test_decoding plugin",
            self.slot_name
        );
        Ok(())
    }

    async fn start_test_decoding_polling(&mut self, start_lsn: Option<String>) -> Result<()> {
        let lsn = start_lsn.unwrap_or_else(|| "0/0".to_string());
        info!("Starting test_decoding polling from LSN: {}", lsn);

        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let slot_name = self.slot_name.clone();

        // Create shutdown channel
        let (shutdown_tx, mut shutdown_rx) = tokio::sync::oneshot::channel::<()>();
        self.shutdown_tx = Some(shutdown_tx);

        tokio::spawn(async move {
            let mut consecutive_errors = 0;
            const MAX_CONSECUTIVE_ERRORS: u32 = 5;

            loop {
                // Check for shutdown signal
                if shutdown_rx.try_recv().is_ok() {
                    info!("Replication consumer received shutdown signal");
                    break;
                }

                // Use pg_logical_slot_get_changes for test_decoding
                let query = format!(
                    "SELECT lsn::text, xid::text, data FROM pg_logical_slot_get_changes('{}', NULL, NULL)",
                    slot_name
                );

                match client.query(&query, &[]).await {
                    Ok(rows) => {
                        consecutive_errors = 0; // Reset error counter on success

                        if !rows.is_empty() {
                            info!("CDC: Found {} changes from replication slot", rows.len());

                            for row in &rows {
                                let lsn: String = row.get(0);
                                let xid: String = row.get(1);
                                let data: String = row.get(2);

                                debug!(
                                    "CDC: Raw message - LSN: {}, XID: {}, Data: {}",
                                    lsn, xid, data
                                );

                                match parse_test_decoding_message(lsn.clone(), &data) {
                                    Ok(Some(event)) => {
                                        debug!("CDC: Successfully parsed event: {:?}", event);
                                        match tx.send(Ok(event)).await {
                                            Ok(_) => debug!("CDC: Event sent to coordinator"),
                                            Err(_) => {
                                                info!(
                                                    "CDC: Replication consumer stopping - channel closed"
                                                );
                                                return;
                                            }
                                        }
                                    }
                                    Ok(None) => {
                                        trace!("CDC: Message did not produce an event");
                                    }
                                    Err(err) => {
                                        warn!(
                                            "CDC: Failed to parse test_decoding message: {} ({})",
                                            data, err
                                        );
                                    }
                                }
                            }
                        } else {
                            trace!("CDC: No changes found in this poll cycle");
                        }
                    }
                    Err(e) => {
                        consecutive_errors += 1;
                        error!(
                            "Failed to get changes (attempt {}/{}): {}",
                            consecutive_errors, MAX_CONSECUTIVE_ERRORS, e
                        );

                        // Check if it's a connection error or we've exceeded max errors
                        if e.to_string().contains("connection")
                            || consecutive_errors >= MAX_CONSECUTIVE_ERRORS
                        {
                            error!(
                                "Connection lost or max errors reached, stopping replication consumer"
                            );
                            let _ = tx
                                .send(Err(MeiliBridgeError::Source(format!(
                                    "Database connection lost after {} consecutive errors",
                                    consecutive_errors
                                ))))
                                .await;
                            return;
                        }

                        // Exponential backoff on errors
                        let backoff_secs = std::cmp::min(2u64.pow(consecutive_errors), 30);
                        warn!("Backing off for {} seconds before retry", backoff_secs);
                        tokio::time::sleep(tokio::time::Duration::from_secs(backoff_secs)).await;
                        continue;
                    }
                }

                // Check if channel is still open
                if tx.is_closed() {
                    info!("Replication consumer stopping - channel closed");
                    break;
                }

                // Wait before next poll
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
            }
        });

        Ok(())
    }

    pub fn stop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}
