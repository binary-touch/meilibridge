use crate::config::TransformConfig;
use crate::error::{MeiliBridgeError, Result};
use crate::models::stream_event::Event;
use serde_json::Value;
use std::collections::HashMap;
use tracing::warn;

/// Transforms events based on configuration rules
#[derive(Clone)]
pub struct EventTransformer {
    config: TransformConfig,
}

impl EventTransformer {
    pub fn new(config: TransformConfig) -> Self {
        Self { config }
    }

    /// Transform an event based on configuration
    pub fn transform(&self, event: Event) -> Result<Option<Event>> {
        match event {
            Event::Cdc(mut cdc_event) => {
                // Apply field transformations
                if let Some(field_config) = self.config.fields.get(&cdc_event.table) {
                    cdc_event.data = self.transform_fields(cdc_event.data, field_config)?;
                }

                // Apply global transformations
                if let Some(global_transforms) = &self.config.global_transforms {
                    for transform in global_transforms {
                        cdc_event.data = self.apply_transform(cdc_event.data, transform)?;
                    }
                }

                Ok(Some(Event::Cdc(cdc_event)))
            }
            Event::FullSync { table, mut data } => {
                // Transform full sync data
                if let Some(field_config) = self.config.fields.get(&table) {
                    if let Some(obj) = data.as_object_mut() {
                        let transformed = self.transform_fields(
                            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
                            field_config,
                        )?;
                        data = Value::Object(transformed.into_iter().collect());
                    }
                }

                Ok(Some(Event::FullSync { table, data }))
            }
            // Pass through other events unchanged
            other => Ok(Some(other)),
        }
    }

    /// Transform fields based on field configuration
    fn transform_fields(
        &self,
        mut data: HashMap<String, Value>,
        field_config: &HashMap<String, crate::config::FieldTransform>,
    ) -> Result<HashMap<String, Value>> {
        let mut result = HashMap::new();

        for transform in field_config.values() {
            match transform {
                crate::config::FieldTransform::Rename { from, to } => {
                    if let Some(value) = data.remove(from) {
                        result.insert(to.clone(), value);
                    }
                }
                crate::config::FieldTransform::Convert { field, to_type } => {
                    if let Some(value) = data.get(field) {
                        let converted = self.convert_type(value, to_type)?;
                        result.insert(field.clone(), converted);
                    }
                }
                crate::config::FieldTransform::Extract { from, path, to } => {
                    if let Some(value) = data.get(from) {
                        if let Some(extracted) = self.extract_path(value, path) {
                            result.insert(to.clone(), extracted);
                        }
                    }
                }
                crate::config::FieldTransform::Compute { expression, to } => {
                    // Simple expression evaluation (can be extended)
                    if let Some(computed) = self.evaluate_expression(expression, &data) {
                        result.insert(to.clone(), computed);
                    }
                }
            }
        }

        // Include remaining fields that weren't transformed
        for (k, v) in data {
            result.entry(k).or_insert(v);
        }

        Ok(result)
    }

    /// Apply a single transformation
    fn apply_transform(
        &self,
        mut data: HashMap<String, Value>,
        transform: &crate::config::pipeline::Transform,
    ) -> Result<HashMap<String, Value>> {
        match transform {
            Transform::AddField { name, value } => {
                data.insert(name.clone(), value.clone());
                Ok(data)
            }
            Transform::RemoveField { name } => {
                data.remove(name);
                Ok(data)
            }
            Transform::AddTimestamp { field } => {
                data.insert(
                    field.clone(),
                    Value::String(chrono::Utc::now().to_rfc3339()),
                );
                Ok(data)
            }
            Transform::Lowercase { fields } => {
                for field in fields {
                    if let Some(Value::String(s)) = data.get_mut(field) {
                        *s = s.to_lowercase();
                    }
                }
                Ok(data)
            }
            Transform::Uppercase { fields } => {
                for field in fields {
                    if let Some(Value::String(s)) = data.get_mut(field) {
                        *s = s.to_uppercase();
                    }
                }
                Ok(data)
            }
        }
    }

    /// Convert a value to a different type
    fn convert_type(&self, value: &Value, to_type: &str) -> Result<Value> {
        match to_type {
            "string" => Ok(match value {
                Value::String(s) => Value::String(s.clone()),
                Value::Number(n) => Value::String(n.to_string()),
                Value::Bool(b) => Value::String(b.to_string()),
                _ => Value::String(value.to_string()),
            }),
            "number" => match value {
                Value::Number(n) => Ok(Value::Number(n.clone())),
                Value::String(s) => s
                    .parse::<f64>()
                    .map(|n| Value::Number(serde_json::Number::from_f64(n).unwrap()))
                    .map_err(|_| {
                        MeiliBridgeError::Pipeline(format!("Cannot convert '{s}' to number"))
                    }),
                _ => Err(MeiliBridgeError::Pipeline(
                    "Cannot convert value to number".to_string(),
                )),
            },
            "boolean" => Ok(match value {
                Value::Bool(b) => Value::Bool(*b),
                Value::String(s) => Value::Bool(s.to_lowercase() == "true" || s == "1"),
                Value::Number(n) => Value::Bool(n.as_i64().unwrap_or(0) != 0),
                _ => Value::Bool(false),
            }),
            "array" => Ok(match value {
                Value::Array(a) => Value::Array(a.clone()),
                other => Value::Array(vec![other.clone()]),
            }),
            _ => Err(MeiliBridgeError::Pipeline(format!(
                "Unknown type conversion: {to_type}",
            ))),
        }
    }

    /// Extract a value from a nested path
    fn extract_path(&self, value: &Value, path: &str) -> Option<Value> {
        let parts: Vec<&str> = path.split('.').collect();
        let mut current = value;

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

    /// Evaluate a simple expression
    fn evaluate_expression(
        &self,
        expression: &str,
        data: &HashMap<String, Value>,
    ) -> Option<Value> {
        // Simple implementation - can be extended with a proper expression parser
        if expression.starts_with("concat(") && expression.ends_with(')') {
            let fields = expression[7..expression.len() - 1]
                .split(',')
                .map(|s| s.trim())
                .collect::<Vec<_>>();

            let mut result = String::new();
            for field in fields {
                if let Some(Value::String(s)) = data.get(field) {
                    result.push_str(s);
                } else if let Some(value) = data.get(field) {
                    result.push_str(&value.to_string());
                }
            }
            return Some(Value::String(result));
        }

        // Add more expression types as needed
        warn!("Unknown expression type: {}", expression);
        None
    }
}

// Re-export FieldTransform from config module
pub use crate::config::pipeline::FieldTransform;

// Re-export Transform from config module
pub use crate::config::pipeline::Transform;
