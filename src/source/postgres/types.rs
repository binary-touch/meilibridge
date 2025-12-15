use crate::error::{MeiliBridgeError, Result};
use serde_json::{Value, json};
use std::str;

/// PostgreSQL type OIDs
/// See: <https://github.com/postgres/postgres/blob/master/src/include/catalog/pg_type.dat>
pub mod oids {
    pub const BOOL: u32 = 16;
    pub const BYTEA: u32 = 17;
    pub const CHAR: u32 = 18;
    pub const INT8: u32 = 20;
    pub const INT2: u32 = 21;
    pub const INT4: u32 = 23;
    pub const TEXT: u32 = 25;
    pub const OID: u32 = 26;
    pub const JSON: u32 = 114;
    pub const XML: u32 = 142;
    pub const FLOAT4: u32 = 700;
    pub const FLOAT8: u32 = 701;
    pub const MONEY: u32 = 790;
    pub const MACADDR: u32 = 829;
    pub const INET: u32 = 869;
    pub const DATE: u32 = 1082;
    pub const TIME: u32 = 1083;
    pub const TIMESTAMP: u32 = 1114;
    pub const TIMESTAMPTZ: u32 = 1184;
    pub const INTERVAL: u32 = 1186;
    pub const NUMERIC: u32 = 1700;
    pub const TIMETZ: u32 = 1266;
    pub const UUID: u32 = 2950;
    pub const JSONB: u32 = 3802;

    // Array types (type OID + 1000 typically, but some exceptions)
    pub const BOOL_ARRAY: u32 = 1000;
    pub const BYTEA_ARRAY: u32 = 1001;
    pub const CHAR_ARRAY: u32 = 1002;
    pub const INT2_ARRAY: u32 = 1005;
    pub const INT4_ARRAY: u32 = 1007;
    pub const TEXT_ARRAY: u32 = 1009;
    pub const INT8_ARRAY: u32 = 1016;
    pub const FLOAT4_ARRAY: u32 = 1021;
    pub const FLOAT8_ARRAY: u32 = 1022;
    pub const OID_ARRAY: u32 = 1028;
    pub const JSON_ARRAY: u32 = 199;
    pub const TIMESTAMP_ARRAY: u32 = 1115;
    pub const DATE_ARRAY: u32 = 1182;
    pub const TIME_ARRAY: u32 = 1183;
    pub const TIMESTAMPTZ_ARRAY: u32 = 1185;
    pub const INTERVAL_ARRAY: u32 = 1187;
    pub const NUMERIC_ARRAY: u32 = 1231;
    pub const TIMETZ_ARRAY: u32 = 1270;
    pub const UUID_ARRAY: u32 = 2951;
    pub const JSONB_ARRAY: u32 = 3807;
    pub const INET_ARRAY: u32 = 1041;
    pub const MACADDR_ARRAY: u32 = 1040;
}

/// Decode a PostgreSQL value from its text representation
pub fn decode_value(bytes: &[u8], type_oid: u32) -> Result<Value> {
    let text = str::from_utf8(bytes)
        .map_err(|e| MeiliBridgeError::Source(format!("Invalid UTF-8: {e}")))?;

    // Handle array types
    if is_array_type(type_oid) {
        return decode_array(text, type_oid);
    }

    // Handle scalar types
    let value = match type_oid {
        oids::BOOL => json!(text == "t" || text == "true"),
        oids::INT2 => json!(text.parse::<i16>().unwrap_or(0)),
        oids::INT4 => json!(text.parse::<i32>().unwrap_or(0)),
        oids::INT8 => json!(text.parse::<i64>().unwrap_or(0)),
        oids::FLOAT4 | oids::FLOAT8 => json!(text.parse::<f64>().unwrap_or(0.0)),
        oids::NUMERIC => {
            // Try to parse as number, fallback to string
            if let Ok(f) = text.parse::<f64>() {
                json!(f)
            } else {
                json!(text)
            }
        }
        oids::MONEY => {
            // PostgreSQL money format: $1,234.56
            let cleaned = text.trim_start_matches('$').replace(',', "");
            json!(cleaned.parse::<f64>().unwrap_or(0.0))
        }
        oids::JSON | oids::JSONB => {
            // Parse JSON/JSONB
            serde_json::from_str(text).unwrap_or_else(|_| json!(text))
        }
        oids::DATE | oids::TIME | oids::TIMETZ | oids::TIMESTAMP | oids::TIMESTAMPTZ => {
            // Return timestamps as strings in ISO format
            json!(text)
        }
        oids::INTERVAL => {
            // PostgreSQL interval format
            json!(text)
        }
        oids::UUID => {
            // UUID is always returned as string regardless of format
            json!(text)
        }
        oids::INET | oids::MACADDR => {
            // Network addresses
            json!(text)
        }
        oids::BYTEA => {
            // Bytea is always returned as string (typically \xHEXSTRING format)
            json!(text)
        }
        oids::TEXT | oids::CHAR => {
            // Text types
            json!(text)
        }
        _ => {
            // Unknown types default to string
            json!(text)
        }
    };

    Ok(value)
}

/// Check if a type OID represents an array type
fn is_array_type(type_oid: u32) -> bool {
    matches!(
        type_oid,
        oids::BOOL_ARRAY
            | oids::BYTEA_ARRAY
            | oids::CHAR_ARRAY
            | oids::INT2_ARRAY
            | oids::INT4_ARRAY
            | oids::TEXT_ARRAY
            | oids::INT8_ARRAY
            | oids::FLOAT4_ARRAY
            | oids::FLOAT8_ARRAY
            | oids::OID_ARRAY
            | oids::JSON_ARRAY
            | oids::TIMESTAMP_ARRAY
            | oids::DATE_ARRAY
            | oids::TIME_ARRAY
            | oids::TIMESTAMPTZ_ARRAY
            | oids::INTERVAL_ARRAY
            | oids::NUMERIC_ARRAY
            | oids::TIMETZ_ARRAY
            | oids::UUID_ARRAY
            | oids::JSONB_ARRAY
            | oids::INET_ARRAY
            | oids::MACADDR_ARRAY
    ) || type_oid >= 1000 // Most array types have OIDs >= 1000
}

/// Decode a PostgreSQL array from its text representation
fn decode_array(text: &str, type_oid: u32) -> Result<Value> {
    // PostgreSQL array format: {value1,value2,"quoted value",NULL}
    if !text.starts_with('{') || !text.ends_with('}') {
        return Ok(json!(text)); // Not a valid array format
    }

    let content = &text[1..text.len() - 1]; // Remove { and }
    if content.is_empty() {
        return Ok(json!([])); // Empty array
    }

    let mut elements = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut escape_next = false;

    for ch in content.chars() {
        if escape_next {
            current.push(ch);
            escape_next = false;
            continue;
        }

        match ch {
            '\\' if in_quotes => escape_next = true,
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                elements.push(parse_array_element(&current, type_oid)?);
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    // Don't forget the last element
    if !current.is_empty() || content.ends_with(',') {
        elements.push(parse_array_element(&current, type_oid)?);
    }

    Ok(json!(elements))
}

/// Parse a single array element based on the array type
fn parse_array_element(element: &str, array_type_oid: u32) -> Result<Value> {
    let trimmed = element.trim();

    // Handle NULL
    if trimmed == "NULL" {
        return Ok(json!(null));
    }

    // Remove quotes if present
    let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') {
        &trimmed[1..trimmed.len() - 1]
    } else {
        trimmed
    };

    // Determine element type from array type
    let element_type_oid = match array_type_oid {
        oids::BOOL_ARRAY => oids::BOOL,
        oids::INT2_ARRAY => oids::INT2,
        oids::INT4_ARRAY => oids::INT4,
        oids::INT8_ARRAY => oids::INT8,
        oids::FLOAT4_ARRAY => oids::FLOAT4,
        oids::FLOAT8_ARRAY => oids::FLOAT8,
        oids::TEXT_ARRAY => oids::TEXT,
        oids::JSON_ARRAY => oids::JSON,
        oids::JSONB_ARRAY => oids::JSONB,
        oids::TIMESTAMP_ARRAY => oids::TIMESTAMP,
        oids::TIMESTAMPTZ_ARRAY => oids::TIMESTAMPTZ,
        oids::DATE_ARRAY => oids::DATE,
        oids::UUID_ARRAY => oids::UUID,
        oids::NUMERIC_ARRAY => oids::NUMERIC,
        _ => oids::TEXT, // Default to text for unknown array types
    };

    // Decode the element value
    decode_value(unquoted.as_bytes(), element_type_oid)
}
