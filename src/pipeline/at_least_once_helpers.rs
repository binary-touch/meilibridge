//! Helper functions for at-least-once delivery in the pipeline

use crate::delivery::DeduplicationKey;
use crate::models::{Position, stream_event::Event};

/// Extract position from event
pub fn extract_position_from_event(event: &Event) -> Option<Position> {
    match event {
        Event::Cdc(cdc_event) => cdc_event.position.clone(),
        Event::Insert { position, .. } => position.clone(),
        Event::Update { position, .. } => position.clone(),
        Event::Delete { position, .. } => position.clone(),
        Event::Checkpoint(position) => Some(position.clone()),
        _ => None,
    }
}

/// Create deduplication key from event
pub fn create_dedup_key_from_event(event: &Event) -> DeduplicationKey {
    match event {
        Event::Cdc(cdc_event) => {
            let lsn = match &cdc_event.position {
                Some(Position::PostgreSQL { lsn }) => lsn.clone(),
                _ => String::new(),
            };

            let mut key = DeduplicationKey::new(
                lsn,
                None, // XID not available in current model
                cdc_event.table.clone(),
            );

            // Add primary key if available (assuming 'id' field)
            if let Some(pk_value) = cdc_event.data.get("id") {
                key = key.with_primary_key(pk_value.to_string());
            }

            key
        }
        Event::Insert {
            table,
            new_data,
            position,
            ..
        } => {
            let lsn = match position {
                Some(Position::PostgreSQL { lsn }) => lsn.clone(),
                _ => String::new(),
            };

            let mut key = DeduplicationKey::new(lsn, None, table.clone());

            // Add primary key if available
            if let Some(id) = new_data.get("id") {
                key = key.with_primary_key(id.to_string());
            }

            key
        }
        Event::Update {
            table,
            new_data,
            position,
            ..
        } => {
            let lsn = match position {
                Some(Position::PostgreSQL { lsn }) => lsn.clone(),
                _ => String::new(),
            };

            let mut key = DeduplicationKey::new(lsn, None, table.clone());

            // Add primary key if available
            if let Some(id) = new_data.get("id") {
                key = key.with_primary_key(id.to_string());
            }

            key
        }
        Event::Delete {
            table,
            old_data,
            position,
            ..
        } => {
            let lsn = match position {
                Some(Position::PostgreSQL { lsn }) => lsn.clone(),
                _ => String::new(),
            };

            let mut key = DeduplicationKey::new(lsn, None, table.clone());

            // Add primary key if available
            if let Some(id) = old_data.get("id") {
                key = key.with_primary_key(id.to_string());
            }

            key
        }
        _ => DeduplicationKey::new(String::new(), None, String::new()),
    }
}
