use crate::error::{MeiliBridgeError, Result};
use crate::models::{CdcEvent, EventType, stream_event::Event};
use serde_json::Value;
use tracing::{debug, warn};

/// Processes events into batches for efficient Meilisearch updates
pub struct BatchProcessor {
    /// Documents to be added or updated
    pub documents_to_upsert: Vec<Value>,
    /// Document IDs to be deleted
    pub documents_to_delete: Vec<String>,
    /// Primary key field name
    primary_key: Option<String>,
}

impl BatchProcessor {
    pub fn new(primary_key: Option<String>) -> Self {
        Self {
            documents_to_upsert: Vec::new(),
            documents_to_delete: Vec::new(),
            primary_key,
        }
    }

    /// Process a single event
    pub fn process_event(&mut self, event: Event) -> Result<()> {
        match event {
            Event::Cdc(cdc_event) => self.process_cdc_event(cdc_event),
            Event::FullSync { data, .. } => {
                self.documents_to_upsert.push(data);
                Ok(())
            }
            _ => {
                debug!("Skipping non-data event");
                Ok(())
            }
        }
    }

    /// Process a CDC event
    fn process_cdc_event(&mut self, event: CdcEvent) -> Result<()> {
        match event.event_type {
            EventType::Create | EventType::Update => {
                let document = serde_json::to_value(event.data).unwrap_or(Value::Null);
                self.documents_to_upsert.push(document);
            }
            EventType::Delete => {
                if let Some(pk_field) = &self.primary_key {
                    if let Some(pk_value) = event.data.get(pk_field) {
                        if let Some(id) = pk_value.as_str() {
                            self.documents_to_delete.push(id.to_string());
                        } else if let Some(id) = pk_value.as_i64() {
                            self.documents_to_delete.push(id.to_string());
                        } else {
                            warn!(
                                "Primary key value is not a string or number: {:?}",
                                pk_value
                            );
                        }
                    } else {
                        return Err(MeiliBridgeError::Meilisearch(format!(
                            "Missing primary key field '{pk_field}' in delete event",
                        )));
                    }
                } else {
                    return Err(MeiliBridgeError::Meilisearch(
                        "No primary key configured for delete operations".to_string(),
                    ));
                }
            }
            _ => {
                debug!("Skipping event type: {:?}", event.event_type);
            }
        }
        Ok(())
    }

    /// Clear the batch
    pub fn clear(&mut self) {
        self.documents_to_upsert.clear();
        self.documents_to_delete.clear();
    }

    /// Check if the batch is empty
    pub fn is_empty(&self) -> bool {
        self.documents_to_upsert.is_empty() && self.documents_to_delete.is_empty()
    }

    /// Get the total number of operations in the batch
    pub fn len(&self) -> usize {
        self.documents_to_upsert.len() + self.documents_to_delete.len()
    }
}
