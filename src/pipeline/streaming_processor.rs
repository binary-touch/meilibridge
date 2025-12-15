use crate::error::{MeiliBridgeError, Result};
use crate::pipeline::memory_monitor::MemoryMonitor;
use serde::Deserialize;
use serde_json::{Deserializer, Value};
use std::io::Read;
use tracing::{debug, warn};

/// Process JSON documents in a streaming fashion to minimize memory usage
pub struct StreamingJsonProcessor {
    /// Maximum document size in bytes
    max_document_size: usize,
    /// Memory monitor for tracking allocations
    memory_monitor: Option<MemoryMonitor>,
}

impl StreamingJsonProcessor {
    pub fn new(max_document_size: usize) -> Self {
        Self {
            max_document_size,
            memory_monitor: None,
        }
    }

    pub fn with_memory_monitor(mut self, monitor: MemoryMonitor) -> Self {
        self.memory_monitor = Some(monitor);
        self
    }

    /// Parse a JSON document from a reader with size limits
    pub fn parse_document<R: Read>(&self, mut reader: R) -> Result<Value> {
        // Use a limited reader to prevent OOM
        let limited_reader = reader.by_ref().take(self.max_document_size as u64);

        // Parse JSON in streaming mode
        let mut de = Deserializer::from_reader(limited_reader);
        let value = Value::deserialize(&mut de)
            .map_err(|e| MeiliBridgeError::Validation(format!("Failed to parse JSON: {e}")))?;

        // Estimate memory usage
        let estimated_size = Self::estimate_value_size(&value);
        debug!(
            "Parsed document with estimated size: {} bytes",
            estimated_size
        );

        // Track memory if monitor is available
        if let Some(monitor) = &self.memory_monitor {
            let _allocation = monitor.track_event_memory(estimated_size as u64)?;
            // The allocation guard will be dropped when the value is processed
        }

        Ok(value)
    }

    /// Parse multiple documents from a reader
    pub fn parse_document_stream<R: Read>(&self, reader: R) -> StreamingDocumentIterator<'_, R> {
        StreamingDocumentIterator {
            reader,
            processor: self,
            done: false,
        }
    }

    /// Process large arrays by yielding items one at a time
    pub async fn process_array_streaming<F>(
        &self,
        json_str: &str,
        mut processor: F,
    ) -> Result<usize>
    where
        F: FnMut(Value) -> Result<()>,
    {
        let mut count = 0;
        let mut depth = 0;
        let mut in_array = false;
        let mut current_item = String::new();
        let chars = json_str.chars().peekable();

        for ch in chars {
            if !in_array {
                if ch == '[' {
                    in_array = true;
                    continue;
                }
            } else {
                match ch {
                    '{' => {
                        depth += 1;
                        current_item.push(ch);
                    }
                    '}' => {
                        depth -= 1;
                        current_item.push(ch);

                        if depth == 0 && !current_item.trim().is_empty() {
                            // Parse and process the item
                            match serde_json::from_str::<Value>(&current_item) {
                                Ok(value) => {
                                    processor(value)?;
                                    count += 1;
                                }
                                Err(e) => {
                                    warn!("Failed to parse array item: {}", e);
                                }
                            }
                            current_item.clear();
                        }
                    }
                    ',' if depth == 0 => {
                        // Skip commas between items
                        continue;
                    }
                    ']' if depth == 0 => {
                        // End of array
                        break;
                    }
                    _ => {
                        if depth > 0 || ch != ' ' {
                            current_item.push(ch);
                        }
                    }
                }
            }
        }

        Ok(count)
    }

    /// Estimate the memory size of a JSON value
    fn estimate_value_size(value: &Value) -> usize {
        match value {
            Value::Null => 8,
            Value::Bool(_) => 8,
            Value::Number(_) => 16,
            Value::String(s) => 24 + s.len(),
            Value::Array(arr) => 24 + arr.iter().map(Self::estimate_value_size).sum::<usize>(),
            Value::Object(obj) => {
                24 + obj
                    .iter()
                    .map(|(k, v)| 24 + k.len() + Self::estimate_value_size(v))
                    .sum::<usize>()
            }
        }
    }
}

/// Iterator for streaming document parsing
pub struct StreamingDocumentIterator<'a, R: Read> {
    reader: R,
    processor: &'a StreamingJsonProcessor,
    done: bool,
}

impl<'a, R: Read> Iterator for StreamingDocumentIterator<'a, R> {
    type Item = Result<Value>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }

        // Try to parse the next document
        match self.processor.parse_document(&mut self.reader) {
            Ok(doc) => Some(Ok(doc)),
            Err(e) => {
                self.done = true;
                if e.to_string().contains("EOF") {
                    None
                } else {
                    Some(Err(e))
                }
            }
        }
    }
}

/// Zero-copy event router using references where possible
pub struct ZeroCopyEventRouter {}

impl ZeroCopyEventRouter {
    pub fn new() -> Self {
        Self {}
    }
}

impl Default for ZeroCopyEventRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl ZeroCopyEventRouter {
    /// Route an event without unnecessary cloning
    pub fn route_event<'a>(&'a mut self, event_data: &'a [u8]) -> Result<EventView<'a>> {
        // Parse just enough to determine routing
        let mut offset = 0;

        // Skip whitespace
        while offset < event_data.len() && event_data[offset].is_ascii_whitespace() {
            offset += 1;
        }

        if offset >= event_data.len() || event_data[offset] != b'{' {
            return Err(MeiliBridgeError::Validation(
                "Invalid JSON object".to_string(),
            ));
        }

        // Extract table name without parsing entire document
        if let Some(table_start) = find_json_string_value(event_data, b"\"table\"") {
            let table = std::str::from_utf8(&event_data[table_start.0..table_start.1])
                .map_err(|e| MeiliBridgeError::Validation(format!("Invalid UTF-8: {e}")))?;

            Ok(EventView {
                table,
                raw_data: event_data,
            })
        } else {
            Err(MeiliBridgeError::Validation(
                "Missing table field".to_string(),
            ))
        }
    }
}

/// A zero-copy view of an event
pub struct EventView<'a> {
    pub table: &'a str,
    pub raw_data: &'a [u8],
}

impl<'a> EventView<'a> {
    /// Parse the full event only when needed
    pub fn parse_full(&self) -> Result<Value> {
        serde_json::from_slice(self.raw_data)
            .map_err(|e| MeiliBridgeError::Validation(format!("Failed to parse event: {e}")))
    }
}

/// Find a JSON string value without full parsing
fn find_json_string_value(data: &[u8], key: &[u8]) -> Option<(usize, usize)> {
    let mut i = 0;
    while i < data.len() {
        if data[i..].starts_with(key) {
            i += key.len();

            // Skip whitespace and colon
            while i < data.len() && (data[i].is_ascii_whitespace() || data[i] == b':') {
                i += 1;
            }

            // Expect opening quote
            if i < data.len() && data[i] == b'"' {
                i += 1;
                let start = i;

                // Find closing quote (handling escapes)
                while i < data.len() {
                    if data[i] == b'\\' {
                        i += 2; // Skip escaped character
                    } else if data[i] == b'"' {
                        return Some((start, i));
                    } else {
                        i += 1;
                    }
                }
            }
        }
        i += 1;
    }
    None
}
