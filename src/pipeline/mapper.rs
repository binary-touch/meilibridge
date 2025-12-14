use crate::config::MappingConfig;
use crate::error::Result;
use crate::models::stream_event::Event;
use serde_json::Value;
use std::collections::HashMap;
use tracing::debug;

/// Maps fields from source to destination schema
#[derive(Clone)]
pub struct FieldMapper {
    config: MappingConfig,
}

impl FieldMapper {
    pub fn new(config: MappingConfig) -> Self {
        Self { config }
    }

    /// Map an event's fields according to configuration
    pub fn map_event(&self, event: Event) -> Result<Event> {
        match event {
            Event::Cdc(mut cdc_event) => {
                if let Some(table_mapping) = self.config.tables.get(&cdc_event.table) {
                    // Apply table name mapping
                    if let Some(new_table_name) = &table_mapping.name {
                        debug!(
                            "Mapping table '{}' to '{}'",
                            cdc_event.table, new_table_name
                        );
                        cdc_event.table = new_table_name.clone();
                    }

                    // Apply field mappings
                    if let Some(field_mappings) = &table_mapping.fields {
                        cdc_event.data = self.map_fields(cdc_event.data, field_mappings)?;
                    }
                }

                Ok(Event::Cdc(cdc_event))
            }
            Event::FullSync {
                mut table,
                mut data,
            } => {
                if let Some(table_mapping) = self.config.tables.get(&table) {
                    // Apply table name mapping
                    if let Some(new_table_name) = &table_mapping.name {
                        debug!("Mapping table '{}' to '{}'", table, new_table_name);
                        table = new_table_name.clone();
                    }

                    // Apply field mappings
                    if let Some(field_mappings) = &table_mapping.fields {
                        if let Some(obj) = data.as_object() {
                            let data_map: HashMap<String, Value> =
                                obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                            let mapped = self.map_fields(data_map, field_mappings)?;
                            data = Value::Object(mapped.into_iter().collect());
                        }
                    }
                }

                Ok(Event::FullSync { table, data })
            }
            // Pass through other events unchanged
            other => Ok(other),
        }
    }

    /// Map fields according to field mappings
    fn map_fields(
        &self,
        data: HashMap<String, Value>,
        field_mappings: &HashMap<String, crate::config::pipeline::FieldMapping>,
    ) -> Result<HashMap<String, Value>> {
        let mut result = HashMap::new();
        let mut mapped_sources = Vec::new();

        // Apply field mappings
        for (source_field, mapping) in field_mappings {
            match mapping {
                crate::config::pipeline::FieldMapping::Simple(target_field) => {
                    if let Some(value) = data.get(source_field) {
                        result.insert(target_field.clone(), value.clone());
                        mapped_sources.push(source_field.clone());
                    }
                }
                crate::config::pipeline::FieldMapping::Complex {
                    to,
                    default,
                    sources,
                    separator,
                    path,
                } => {
                    // Handle different complex mapping types
                    if let Some(target_field) = to {
                        if let Some(field_sources) = sources {
                            // Composite mapping
                            let mut values = Vec::new();
                            for source in field_sources {
                                if let Some(value) = data.get(source) {
                                    values.push(value_to_string(value));
                                    mapped_sources.push(source.clone());
                                }
                            }
                            if !values.is_empty() {
                                let sep = separator.as_deref().unwrap_or(",");
                                result
                                    .insert(target_field.clone(), Value::String(values.join(sep)));
                            }
                        } else if let Some(path_str) = path {
                            // Nested extraction
                            if let Some(value) = self.extract_nested(&data, source_field, path_str)
                            {
                                result.insert(target_field.clone(), value);
                                mapped_sources.push(source_field.clone());
                            }
                        } else {
                            // Simple mapping with default
                            let value = data
                                .get(source_field)
                                .cloned()
                                .unwrap_or_else(|| default.clone().unwrap_or(Value::Null));
                            result.insert(target_field.clone(), value);
                            mapped_sources.push(source_field.clone());
                        }
                    }
                }
            }
        }

        // Handle unmapped fields based on configuration
        match self.config.unmapped_fields_strategy.as_str() {
            "include" => {
                // Include all unmapped fields
                for (k, v) in data {
                    if !mapped_sources.contains(&k) && !result.contains_key(&k) {
                        result.insert(k, v);
                    }
                }
            }
            "exclude" => {
                // Only include explicitly mapped fields
            }
            "prefix" => {
                // Include unmapped fields with a prefix
                let prefix = self.config.unmapped_fields_prefix.as_deref().unwrap_or("_");
                for (k, v) in data {
                    if !mapped_sources.contains(&k) {
                        result.insert(format!("{}{}", prefix, k), v);
                    }
                }
            }
            _ => {
                // Default: include unmapped fields
                for (k, v) in data {
                    if !mapped_sources.contains(&k) && !result.contains_key(&k) {
                        result.insert(k, v);
                    }
                }
            }
        }

        Ok(result)
    }

    /// Extract a value from a nested structure
    fn extract_nested(
        &self,
        data: &HashMap<String, Value>,
        base_field: &str,
        path: &str,
    ) -> Option<Value> {
        let base_value = data.get(base_field)?;
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = base_value;

        for part in parts {
            match current {
                Value::Object(obj) => {
                    current = obj.get(part)?;
                }
                Value::Array(arr) => {
                    if let Ok(index) = part.parse::<usize>() {
                        current = arr.get(index)?;
                    } else {
                        return None;
                    }
                }
                _ => return None,
            }
        }

        Some(current.clone())
    }
}

/// Convert a JSON value to string
fn value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => String::new(),
        _ => value.to_string(),
    }
}

// Re-export FieldMapping from config module
pub use crate::config::pipeline::FieldMapping;
