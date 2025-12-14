//! Soft delete handling for the pipeline

use crate::config::SoftDeleteConfig;
use crate::models::{EventType, stream_event::Event};
use serde_json::Value;
use tracing::{debug, trace};

/// Handles soft delete transformations
#[derive(Clone)]
pub struct SoftDeleteHandler {
    config: SoftDeleteConfig,
}

impl SoftDeleteHandler {
    pub fn new(config: SoftDeleteConfig) -> Self {
        Self { config }
    }

    /// Check if an event should be transformed to a delete based on soft delete rules
    pub fn transform_event(&self, event: Event) -> Option<Event> {
        match event {
            Event::Cdc(ref cdc_event) => {
                // Only process UPDATE events for soft delete
                if cdc_event.event_type != EventType::Update {
                    return Some(event);
                }

                // Check if CDC handling is enabled
                if !self.config.handle_on_cdc {
                    return Some(event);
                }

                // Check if the field value matches any delete value
                if let Some(field_value) = cdc_event.data.get(&self.config.field)
                    && self.is_soft_deleted(field_value)
                {
                    debug!(
                        "Transforming UPDATE to DELETE for table '{}' due to soft delete field '{}' = {:?}",
                        cdc_event.table, self.config.field, field_value
                    );

                    // Transform to DELETE event
                    let mut delete_event = cdc_event.clone();
                    delete_event.event_type = crate::models::event::EventType::Delete;
                    // For DELETE, we only need the primary key, but keeping data is fine
                    return Some(Event::Cdc(delete_event));
                }

                Some(event)
            }
            Event::Update {
                ref table,
                ref new_data,
                ref old_data,
                ref position,
            } => {
                // Check if CDC handling is enabled
                if !self.config.handle_on_cdc {
                    return Some(event);
                }

                // Check if the field value matches any delete value
                if let Some(field_value) = new_data.get(&self.config.field)
                    && self.is_soft_deleted(field_value)
                {
                    debug!(
                        "Transforming UPDATE to DELETE for table '{}' due to soft delete field '{}' = {:?}",
                        table, self.config.field, field_value
                    );

                    // Transform to DELETE event
                    return Some(Event::Delete {
                        table: table.clone(),
                        old_data: old_data.clone(),
                        position: position.clone(),
                    });
                }

                Some(event)
            }
            Event::FullSync {
                ref table,
                ref data,
            } => {
                // Check if full sync handling is enabled
                if !self.config.handle_on_full_sync {
                    return Some(event);
                }

                // Check if the field value matches any delete value
                if let Some(field_value) = data.get(&self.config.field)
                    && self.is_soft_deleted(field_value)
                {
                    trace!(
                        "Filtering out soft-deleted record from full sync for table '{}': {} = {:?}",
                        table, self.config.field, field_value
                    );
                    return None;
                }

                Some(event)
            }
            // Pass through other events unchanged
            _ => Some(event),
        }
    }

    /// Check if a value matches any of the configured delete values
    fn is_soft_deleted(&self, value: &Value) -> bool {
        self.config.delete_values.iter().any(|delete_value| {
            // Use serde_json's equality comparison which handles different types
            value == delete_value
        })
    }
}
