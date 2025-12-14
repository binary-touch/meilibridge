use crate::error::Result;
use crate::models::{CdcEvent, EventType, stream_event::Event};
use crate::source::postgres::pgoutput::{PgOutputMessage, PgOutputParser};
use chrono::Utc;
use futures::Stream;
use serde_json::json;
use std::collections::HashMap;
use std::pin::Pin;
use std::str;
use std::sync::Arc;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tokio_postgres::Client;
use tracing::{debug, info, warn};

pub struct WalConsumer {
    client: Arc<Client>,
    slot_name: String,
    publication_name: String,
    parser: PgOutputParser,
    event_rx: Option<mpsc::Receiver<Result<Event>>>,
    event_tx: mpsc::Sender<Result<Event>>,
}

impl WalConsumer {
    pub fn new(client: Client, slot_name: String, publication_name: String) -> Self {
        let (tx, rx) = mpsc::channel(1000);

        Self {
            client: Arc::new(client),
            slot_name,
            publication_name,
            parser: PgOutputParser::new(),
            event_rx: Some(rx),
            event_tx: tx,
        }
    }

    pub async fn start(&mut self, start_lsn: Option<String>) -> Result<()> {
        let lsn = start_lsn.unwrap_or_else(|| "0/0".to_string());

        info!("Starting WAL consumer from LSN: {}", lsn);

        // Clone necessary values for the spawned task
        let client = self.client.clone();
        let tx = self.event_tx.clone();
        let slot_name = self.slot_name.clone();
        let publication_name = self.publication_name.clone();
        let parser = self.parser.clone();

        // Spawn a task to poll for CDC changes
        tokio::spawn(async move {
            info!("WAL consumer started for slot '{}'", slot_name);

            let mut last_lsn = lsn.clone();

            loop {
                // Poll for changes using pg_logical_slot_peek_changes
                let query = format!(
                    "SELECT lsn, xid, data FROM pg_logical_slot_peek_changes('{}', NULL, NULL, 'proto_version', '1', 'publication_names', '{}')",
                    slot_name, publication_name
                );

                match client.query(&query, &[]).await {
                    Ok(rows) => {
                        if !rows.is_empty() {
                            debug!("Found {} CDC changes", rows.len());

                            for row in &rows {
                                let lsn: String = row.get(0);
                                let _xid: i64 = row.get(1);
                                let data: String = row.get(2);

                                // Parse the logical decoding message
                                if let Some(event) = parse_logical_decoding_message(&data, &parser)
                                {
                                    if tx.send(Ok(event)).await.is_err() {
                                        info!("WAL consumer stopping - channel closed");
                                        return;
                                    }
                                }

                                last_lsn = lsn;
                            }

                            // Advance the replication slot
                            let advance_query = format!(
                                "SELECT pg_replication_slot_advance('{}', '{}')",
                                slot_name, last_lsn
                            );

                            if let Err(e) = client.execute(&advance_query, &[]).await {
                                warn!("Failed to advance replication slot: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to poll for changes: {}", e);
                    }
                }

                // Wait before next poll
                tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

                // Check if channel is still open
                if tx.is_closed() {
                    info!("WAL consumer stopping - channel closed");
                    break;
                }
            }
        });

        Ok(())
    }

    pub fn into_stream(mut self) -> impl Stream<Item = Result<Event>> {
        // Return the receiver as a stream
        EventStream {
            rx: self.event_rx.take().unwrap(),
        }
    }
}

struct EventStream {
    rx: mpsc::Receiver<Result<Event>>,
}

impl Stream for EventStream {
    type Item = Result<Event>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

fn _process_pgoutput_message(message: PgOutputMessage, parser: &PgOutputParser) -> Option<Event> {
    match message {
        PgOutputMessage::Insert(msg) => {
            let relation = parser.get_relation(msg.relation_id)?;
            let data = _decode_tuple_data(&msg.tuple, relation)?;

            let cdc_event = CdcEvent {
                event_type: EventType::Create,
                table: relation.relation_name.clone(),
                schema: relation.namespace.clone(),
                data,
                timestamp: Utc::now(),
                position: None,
            };

            Some(Event::Cdc(cdc_event))
        }
        PgOutputMessage::Update(msg) => {
            let relation = parser.get_relation(msg.relation_id)?;
            let data = _decode_tuple_data(&msg.new_tuple, relation)?;

            let cdc_event = CdcEvent {
                event_type: EventType::Update,
                table: relation.relation_name.clone(),
                schema: relation.namespace.clone(),
                data,
                timestamp: Utc::now(),
                position: None,
            };

            Some(Event::Cdc(cdc_event))
        }
        PgOutputMessage::Delete(msg) => {
            let relation = parser.get_relation(msg.relation_id)?;
            let data = _decode_tuple_data(&msg.old_tuple, relation)?;

            let cdc_event = CdcEvent {
                event_type: EventType::Delete,
                table: relation.relation_name.clone(),
                schema: relation.namespace.clone(),
                data,
                timestamp: Utc::now(),
                position: None,
            };

            Some(Event::Cdc(cdc_event))
        }
        _ => None,
    }
}

fn _decode_tuple_data(
    tuple: &crate::source::postgres::pgoutput::TupleData,
    relation: &crate::source::postgres::pgoutput::RelationMessage,
) -> Option<HashMap<String, serde_json::Value>> {
    let mut data = HashMap::new();

    for (i, column_data) in tuple.columns.iter().enumerate() {
        if i >= relation.columns.len() {
            warn!("Column index {} out of bounds", i);
            continue;
        }

        let column = &relation.columns[i];
        let value = if let Some(bytes) = column_data {
            _decode_value(bytes, column.type_id).unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Null
        };

        data.insert(column.name.clone(), value);
    }

    Some(data)
}

fn _decode_value(bytes: &[u8], type_oid: u32) -> Result<serde_json::Value> {
    // Use the new type decoder
    crate::source::postgres::types::decode_value(bytes, type_oid)
}

/// Parse logical decoding text format messages
fn parse_logical_decoding_message(data: &str, _parser: &PgOutputParser) -> Option<Event> {
    // PostgreSQL logical decoding text format
    // Example formats:
    // table public.users: INSERT: id[integer]:1 email[text]:'test@example.com'
    // table public.users: UPDATE: id[integer]:1 email[text]:'new@example.com'
    // table public.users: DELETE: id[integer]:1

    let parts: Vec<&str> = data.split(':').collect();
    if parts.len() < 3 {
        warn!("Invalid logical decoding message format: {}", data);
        return None;
    }

    // Parse table info
    let table_info = parts[0].trim();
    if !table_info.starts_with("table ") {
        return None;
    }

    let table_parts: Vec<&str> = table_info[6..].split('.').collect();
    if table_parts.len() != 2 {
        return None;
    }

    let schema = table_parts[0];
    let table = table_parts[1];

    // Parse operation
    let operation = parts[1].trim();
    let event_type = match operation {
        "INSERT" => EventType::Create,
        "UPDATE" => EventType::Update,
        "DELETE" => EventType::Delete,
        _ => {
            warn!("Unknown operation type: {}", operation);
            return None;
        }
    };

    // Parse data fields
    let mut data_map = HashMap::new();
    let data_part = parts[2..].join(":");

    // Simple parser for field data
    // Format: field_name[type]:value field_name[type]:'string value'
    let mut current_field = String::new();
    let mut current_value = String::new();
    let mut in_quotes = false;
    let mut in_brackets = false;

    for ch in data_part.chars() {
        match ch {
            '[' if !in_quotes => in_brackets = true,
            ']' if !in_quotes => in_brackets = false,
            '\'' if !in_brackets => in_quotes = !in_quotes,
            ':' if !in_quotes && !in_brackets && current_field.is_empty() => {
                // Field name complete
                if let Some(bracket_pos) = current_value.find('[') {
                    current_field = current_value[..bracket_pos].trim().to_string();
                }
                current_value.clear();
            }
            ' ' if !in_quotes && !current_field.is_empty() => {
                // Value complete
                let value = if current_value.starts_with('\'') && current_value.ends_with('\'') {
                    json!(current_value[1..current_value.len() - 1].to_string())
                } else if current_value == "null" {
                    json!(null)
                } else if let Ok(n) = current_value.parse::<i64>() {
                    json!(n)
                } else if let Ok(f) = current_value.parse::<f64>() {
                    json!(f)
                } else if current_value == "true" || current_value == "false" {
                    json!(current_value == "true")
                } else {
                    json!(current_value)
                };

                data_map.insert(current_field.clone(), value);
                current_field.clear();
                current_value.clear();
            }
            _ => {
                current_value.push(ch);
            }
        }
    }

    // Handle last field
    if !current_field.is_empty() && !current_value.is_empty() {
        let value = if current_value.starts_with('\'') && current_value.ends_with('\'') {
            json!(current_value[1..current_value.len() - 1].to_string())
        } else if current_value == "null" {
            json!(null)
        } else if let Ok(n) = current_value.parse::<i64>() {
            json!(n)
        } else if let Ok(f) = current_value.parse::<f64>() {
            json!(f)
        } else if current_value == "true" || current_value == "false" {
            json!(current_value == "true")
        } else {
            json!(current_value)
        };

        data_map.insert(current_field, value);
    }

    let cdc_event = CdcEvent {
        event_type,
        table: table.to_string(),
        schema: schema.to_string(),
        data: data_map,
        timestamp: Utc::now(),
        position: None,
    };

    Some(Event::Cdc(cdc_event))
}
