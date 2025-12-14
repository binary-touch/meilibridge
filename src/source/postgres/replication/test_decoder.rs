use crate::error::Result;
use crate::models::stream_event::Event;
use crate::models::{CdcEvent, EventType};
use chrono::Utc;

/// Parse test_decoding message into an Event
pub fn parse_test_decoding_message(lsn: String, message: &str) -> Result<Option<Event>> {
    if !message.starts_with("table ") {
        return Ok(None);
    }

    let remainder = message[6..].trim();
    let (table_token, rest) = match remainder.split_once(':') {
        Some((table, rest)) => (table.trim_end_matches(':'), rest.trim()),
        None => return Ok(None),
    };

    let (operation_token, data_section) = match rest.split_once(':') {
        Some((op, data)) => (op.trim(), data.trim()),
        None => return Ok(None),
    };

    let table_full = table_token;

    // Parse schema and table from format: schema.table
    let (schema, table) = if let Some(dot_pos) = table_full.rfind('.') {
        let schema_part = &table_full[..dot_pos];
        let table_part = &table_full[dot_pos + 1..];
        (schema_part, table_part)
    } else {
        ("public", table_full)
    };

    // Determine event type
    let event_type = match operation_token {
        "INSERT" => EventType::Create,
        "UPDATE" => EventType::Update,
        "DELETE" => EventType::Delete,
        _ => return Ok(None),
    };

    // Tokenize the remaining payload so quoted values (with spaces) stay intact
    let data_parts = tokenize_parts(data_section);

    // Parse the data based on operation type
    let (_primary_key, data) = match event_type {
        EventType::Create => parse_insert_data(&data_parts)?,
        EventType::Update => parse_update_data(&data_parts)?,
        EventType::Delete => parse_delete_data(&data_parts)?,
        _ => return Ok(None),
    };

    let cdc_event = CdcEvent {
        event_type,
        table: table.trim_end_matches(':').to_string(),
        schema: schema.to_string(),
        data: match data {
            serde_json::Value::Object(map) => map.into_iter().collect(),
            _ => std::collections::HashMap::new(),
        },
        timestamp: Utc::now(),
        position: Some(crate::models::Position::postgresql(lsn)),
    };

    Ok(Some(Event::Cdc(cdc_event)))
}

fn parse_insert_data(parts: &[String]) -> Result<(Option<String>, serde_json::Value)> {
    let mut fields = serde_json::Map::new();
    let mut primary_key = None;

    for part in parts {
        if let Some((col_info, value)) = split_first_colon(part) {
            let column = extract_column_name(col_info);
            let sanitized = sanitize_value(value);
            let parsed_value = parse_value(&sanitized);

            if column == "id" && primary_key.is_none() {
                primary_key = Some(sanitized);
            }

            fields.insert(column, parsed_value);
        }
    }

    Ok((primary_key, serde_json::Value::Object(fields)))
}

fn parse_update_data(parts: &[String]) -> Result<(Option<String>, serde_json::Value)> {
    let mut new_fields = serde_json::Map::new();
    let mut old_fields = serde_json::Map::new();
    let mut primary_key = None;
    let mut is_new_section = false;

    for part in parts {
        if part == "new-key:" {
            is_new_section = true;
            continue;
        }
        if part == "old-key:" {
            is_new_section = false;
            continue;
        }

        if let Some((col_info, value)) = split_first_colon(part) {
            let column = extract_column_name(col_info);
            let sanitized = sanitize_value(value);
            let parsed_value = parse_value(&sanitized);

            if is_new_section {
                if column == "id" && primary_key.is_none() {
                    primary_key = Some(sanitized.clone());
                }
                new_fields.insert(column.clone(), parsed_value.clone());
            } else {
                old_fields.insert(column.clone(), parsed_value.clone());
            }
        }
    }

    let data = if !new_fields.is_empty() {
        serde_json::Value::Object(new_fields)
    } else {
        serde_json::Value::Object(old_fields)
    };

    Ok((primary_key, data))
}

fn parse_delete_data(parts: &[String]) -> Result<(Option<String>, serde_json::Value)> {
    let mut fields = serde_json::Map::new();
    let mut primary_key = None;

    for part in parts {
        if let Some((col_info, value)) = split_first_colon(part) {
            let column = extract_column_name(col_info);
            let sanitized = sanitize_value(value);

            if column == "id" && primary_key.is_none() {
                primary_key = Some(sanitized.clone());
            }

            fields.insert(column, parse_value(&sanitized));
        }
    }

    Ok((primary_key, serde_json::Value::Object(fields)))
}

fn extract_column_name(col_info: &str) -> String {
    let trimmed = col_info.trim().trim_end_matches(':');
    if let Some(bracket_pos) = trimmed.find('[') {
        trimmed[..bracket_pos].to_string()
    } else {
        trimmed.to_string()
    }
}

fn parse_value(value: &str) -> serde_json::Value {
    let value = value.trim();

    if value.is_empty() {
        return serde_json::Value::Null;
    }

    if let Ok(num) = value.parse::<i64>() {
        return serde_json::Value::Number(num.into());
    }

    if let Ok(num) = value.parse::<f64>() {
        if let Some(n) = serde_json::Number::from_f64(num) {
            return serde_json::Value::Number(n);
        }
    }

    if value.eq_ignore_ascii_case("true") {
        return serde_json::Value::Bool(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return serde_json::Value::Bool(false);
    }

    if value.eq_ignore_ascii_case("null") {
        return serde_json::Value::Null;
    }

    serde_json::Value::String(value.to_string())
}

fn tokenize_parts(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = input.chars().peekable();
    let mut bracket_depth = 0u32;

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                if in_quotes && chars.peek() == Some(&'\'') {
                    current.push(ch);
                    current.push(chars.next().unwrap());
                } else {
                    in_quotes = !in_quotes;
                    current.push(ch);
                }
            }
            '[' => {
                bracket_depth = bracket_depth.saturating_add(1);
                current.push(ch);
            }
            ']' => {
                if bracket_depth > 0 {
                    bracket_depth -= 1;
                }
                current.push(ch);
            }
            ' ' | '\t' | '\n' if !in_quotes && bracket_depth == 0 => {
                if !current.is_empty() {
                    tokens.push(current.trim().to_string());
                    current.clear();
                }
            }
            _ => current.push(ch),
        }
    }

    if !current.is_empty() {
        tokens.push(current.trim().to_string());
    }

    tokens.into_iter().filter(|t| !t.is_empty()).collect()
}

fn split_first_colon(part: &str) -> Option<(&str, &str)> {
    part.split_once(':')
        .map(|(left, right)| (left.trim(), right.trim()))
}

fn sanitize_value(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.starts_with('\'') && trimmed.ends_with('\'') && trimmed.len() >= 2 {
        trimmed[1..trimmed.len() - 1].replace("''", "'")
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::stream_event::Event;
    use crate::models::EventType;

    #[test]
    fn parses_update_with_whitespace_values() {
        let message = "table public.items: UPDATE: id[uuid]:'690594ad-bb6c-474c-9cd1-71451a133e68' name[text]:'Synced v4 Adidas Ultraboost 22' category[character varying]:'Fashion (รองเท้า)'";
        let event = parse_test_decoding_message("0/0".to_string(), message)
            .expect("parser result")
            .expect("event");

        match event {
            Event::Cdc(cdc) => {
                assert_eq!(cdc.event_type, EventType::Update);
                let name = cdc
                    .data
                    .get("name")
                    .expect("name field should exist")
                    .as_str()
                    .expect("name should be string");
                assert_eq!(name, "Synced v4 Adidas Ultraboost 22");

                let category = cdc
                    .data
                    .get("category")
                    .expect("category field should exist")
                    .as_str()
                    .expect("category should be string");
                assert_eq!(category, "Fashion (รองเท้า)");
            }
            _ => panic!("expected CDC event"),
        }
    }
}
