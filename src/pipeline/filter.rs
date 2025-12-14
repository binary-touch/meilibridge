use crate::config::FilterConfig;
use crate::error::Result;
use crate::models::{EventType, stream_event::Event};
use serde_json::Value;
use std::collections::HashMap;
use tracing::debug;

/// Filters events based on configuration rules
#[derive(Clone)]
pub struct EventFilter {
    config: FilterConfig,
}

impl EventFilter {
    pub fn new(config: FilterConfig) -> Self {
        Self { config }
    }

    /// Check if an event should be processed
    pub fn should_process(&self, event: &Event) -> Result<bool> {
        match event {
            Event::Cdc(cdc_event) => {
                // Check table filter
                if !self.is_table_allowed(&cdc_event.table) {
                    debug!("Table '{}' filtered out", cdc_event.table);
                    return Ok(false);
                }

                // Check event type filter
                if !self.is_event_type_allowed(&cdc_event.event_type) {
                    debug!("Event type '{:?}' filtered out", cdc_event.event_type);
                    return Ok(false);
                }

                // Check field conditions
                if !self.check_conditions(&cdc_event.data) {
                    debug!("Event filtered out by field conditions");
                    return Ok(false);
                }

                Ok(true)
            }
            Event::FullSync { table, data } => {
                // Check table filter
                if !self.is_table_allowed(table) {
                    debug!("Table '{}' filtered out", table);
                    return Ok(false);
                }

                // Check conditions for full sync
                if let Some(obj) = data.as_object() {
                    let data_map: HashMap<String, Value> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

                    if !self.check_conditions(&data_map) {
                        debug!("Full sync record filtered out by conditions");
                        return Ok(false);
                    }
                }

                Ok(true)
            }
            // Process individual event types
            Event::Insert {
                table, new_data, ..
            } => {
                if !self.is_table_allowed(table) {
                    return Ok(false);
                }
                if !self.is_event_type_allowed(&EventType::Create) {
                    return Ok(false);
                }
                if let Some(obj) = new_data.as_object() {
                    let data_map: HashMap<String, Value> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    if !self.check_conditions(&data_map) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Event::Update {
                table, new_data, ..
            } => {
                if !self.is_table_allowed(table) {
                    return Ok(false);
                }
                if !self.is_event_type_allowed(&EventType::Update) {
                    return Ok(false);
                }
                if let Some(obj) = new_data.as_object() {
                    let data_map: HashMap<String, Value> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    if !self.check_conditions(&data_map) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Event::Delete {
                table, old_data, ..
            } => {
                if !self.is_table_allowed(table) {
                    return Ok(false);
                }
                if !self.is_event_type_allowed(&EventType::Delete) {
                    return Ok(false);
                }
                if let Some(obj) = old_data.as_object() {
                    let data_map: HashMap<String, Value> =
                        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                    if !self.check_conditions(&data_map) {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            // Always process control events
            Event::Checkpoint(_) | Event::Heartbeat => Ok(true),
        }
    }

    /// Check if a table is allowed
    fn is_table_allowed(&self, table: &str) -> bool {
        // If whitelist is defined, table must be in it
        if let Some(whitelist) = &self.config.tables.whitelist {
            if !whitelist.contains(&table.to_string()) {
                return false;
            }
        }

        // If blacklist is defined, table must not be in it
        if let Some(blacklist) = &self.config.tables.blacklist {
            if blacklist.contains(&table.to_string()) {
                return false;
            }
        }

        true
    }

    /// Check if an event type is allowed
    fn is_event_type_allowed(&self, event_type: &EventType) -> bool {
        if let Some(allowed_types) = &self.config.event_types {
            allowed_types.iter().any(|t| {
                matches!(
                    (t.as_str(), event_type),
                    ("create", EventType::Create)
                        | ("update", EventType::Update)
                        | ("delete", EventType::Delete)
                )
            })
        } else {
            // If no filter specified, allow all event types
            true
        }
    }

    /// Check field conditions
    fn check_conditions(&self, data: &HashMap<String, Value>) -> bool {
        if let Some(conditions) = &self.config.conditions {
            for condition in conditions {
                if !self.evaluate_condition(condition, data) {
                    return false;
                }
            }
        }
        true
    }

    /// Evaluate a single condition
    fn evaluate_condition(
        &self,
        condition: &crate::config::pipeline::Condition,
        data: &HashMap<String, Value>,
    ) -> bool {
        match condition {
            Condition::Equals { field, value } => data.get(field) == Some(value),
            Condition::NotEquals { field, value } => data.get(field) != Some(value),
            Condition::Contains { field, value } => {
                if let Some(Value::String(s)) = data.get(field) {
                    if let Value::String(pattern) = value {
                        s.contains(pattern)
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
            Condition::GreaterThan { field, value } => {
                self.compare_values(data.get(field), value, |a, b| a > b)
            }
            Condition::LessThan { field, value } => {
                self.compare_values(data.get(field), value, |a, b| a < b)
            }
            Condition::In { field, values } => {
                if let Some(field_value) = data.get(field) {
                    values.contains(field_value)
                } else {
                    false
                }
            }
            Condition::IsNull { field } => data.get(field).is_none_or(|v| v.is_null()),
            Condition::IsNotNull { field } => data.get(field).is_some_and(|v| !v.is_null()),
            crate::config::pipeline::Condition::And { conditions } => {
                conditions.iter().all(|c| self.evaluate_condition(c, data))
            }
            crate::config::pipeline::Condition::Or { conditions } => {
                conditions.iter().any(|c| self.evaluate_condition(c, data))
            }
        }
    }

    /// Compare numeric values
    fn compare_values<F>(&self, field_value: Option<&Value>, compare_value: &Value, op: F) -> bool
    where
        F: Fn(f64, f64) -> bool,
    {
        if let (Some(Value::Number(a)), Value::Number(b)) = (field_value, compare_value) {
            if let (Some(a_f64), Some(b_f64)) = (a.as_f64(), b.as_f64()) {
                return op(a_f64, b_f64);
            }
        }
        false
    }
}

// Re-export Condition from config module
pub use crate::config::pipeline::Condition;
