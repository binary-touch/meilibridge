use chrono::Utc;
use meilibridge::config::{MappingConfig, TableMapping, pipeline::FieldMapping};
use meilibridge::models::stream_event::Event;
use meilibridge::models::{CdcEvent, EventType, Position};
use meilibridge::pipeline::mapper::FieldMapper;
use serde_json::{Value, json};
use std::collections::HashMap;

fn create_test_cdc_event(table: &str, data: HashMap<String, Value>) -> Event {
    Event::Cdc(CdcEvent {
        event_type: EventType::Create,
        table: table.to_string(),
        schema: "public".to_string(),
        data,
        timestamp: Utc::now(),
        position: Some(Position::PostgreSQL {
            lsn: "0/0".to_string(),
        }),
    })
}

fn create_test_full_sync_event(table: &str, data: Value) -> Event {
    Event::FullSync {
        table: table.to_string(),
        data,
    }
}

#[test]
fn test_mapper_creation() {
    let config = MappingConfig::default();
    let _mapper = FieldMapper::new(config);
}

#[test]
fn test_simple_field_mapping() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Map "user_id" to "id"
    field_mappings.insert(
        "user_id".to_string(),
        FieldMapping::Simple("id".to_string()),
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "include".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("user_id".to_string(), json!(123));
    data.insert("name".to_string(), json!("John"));
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify mapping
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("id"), Some(&json!(123)));
        assert_eq!(cdc_event.data.get("name"), Some(&json!("John")));
        assert_eq!(cdc_event.data.get("user_id"), None); // Original field removed
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_table_name_mapping() {
    let mut tables = HashMap::new();
    tables.insert(
        "old_users".to_string(),
        TableMapping {
            name: Some("new_users".to_string()),
            fields: None,
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "include".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("id".to_string(), json!(1));
    let event = create_test_cdc_event("old_users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify table name changed
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.table, "new_users");
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_complex_mapping_with_default() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Map with default value
    field_mappings.insert(
        "status".to_string(),
        FieldMapping::Complex {
            to: Some("user_status".to_string()),
            default: Some(json!("active")),
            sources: None,
            separator: None,
            path: None,
        },
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "exclude".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event without status field
    let data = HashMap::new();
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify default value used
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("user_status"), Some(&json!("active")));
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_complex_mapping_composite_fields() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Combine multiple fields
    field_mappings.insert(
        "first_name".to_string(),
        FieldMapping::Complex {
            to: Some("full_name".to_string()),
            default: None,
            sources: Some(vec!["first_name".to_string(), "last_name".to_string()]),
            separator: Some(" ".to_string()),
            path: None,
        },
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "exclude".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("first_name".to_string(), json!("John"));
    data.insert("last_name".to_string(), json!("Doe"));
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify composite field
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("full_name"), Some(&json!("John Doe")));
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_nested_field_extraction() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Extract nested field
    field_mappings.insert(
        "address".to_string(),
        FieldMapping::Complex {
            to: Some("city".to_string()),
            default: None,
            sources: None,
            separator: None,
            path: Some("city".to_string()),
        },
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "exclude".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event with nested data
    let mut data = HashMap::new();
    data.insert(
        "address".to_string(),
        json!({
            "street": "123 Main St",
            "city": "New York",
            "zip": "10001"
        }),
    );
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify nested extraction
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("city"), Some(&json!("New York")));
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_unmapped_fields_include_strategy() {
    let config = MappingConfig {
        tables: HashMap::new(),
        unmapped_fields_strategy: "include".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("id".to_string(), json!(1));
    data.insert("name".to_string(), json!("Test"));
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify all fields included
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("id"), Some(&json!(1)));
        assert_eq!(cdc_event.data.get("name"), Some(&json!("Test")));
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_unmapped_fields_exclude_strategy() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Only map id field
    field_mappings.insert(
        "id".to_string(),
        FieldMapping::Simple("user_id".to_string()),
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "exclude".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("id".to_string(), json!(1));
    data.insert("name".to_string(), json!("Test"));
    data.insert("email".to_string(), json!("test@example.com"));
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify only mapped field included
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("user_id"), Some(&json!(1)));
        assert_eq!(cdc_event.data.get("name"), None);
        assert_eq!(cdc_event.data.get("email"), None);
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_unmapped_fields_prefix_strategy() {
    let mut tables = HashMap::new();
    // Add empty table mapping to trigger the prefix strategy
    tables.insert(
        "users".to_string(),
        TableMapping {
            name: None,
            fields: Some(HashMap::new()), // Empty field mappings
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "prefix".to_string(),
        unmapped_fields_prefix: Some("orig_".to_string()),
    };

    let mapper = FieldMapper::new(config);

    // Create test event
    let mut data = HashMap::new();
    data.insert("id".to_string(), json!(1));
    data.insert("name".to_string(), json!("Test"));
    let event = create_test_cdc_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify fields have prefix
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("orig_id"), Some(&json!(1)));
        assert_eq!(cdc_event.data.get("orig_name"), Some(&json!("Test")));
        // Original fields should not exist when prefix strategy is used
        assert_eq!(cdc_event.data.get("id"), None);
        assert_eq!(cdc_event.data.get("name"), None);
    } else {
        panic!("Expected CDC event");
    }
}

#[test]
fn test_full_sync_event_mapping() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    field_mappings.insert(
        "user_id".to_string(),
        FieldMapping::Simple("id".to_string()),
    );

    tables.insert(
        "users".to_string(),
        TableMapping {
            name: Some("app_users".to_string()),
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "include".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test full sync event
    let data = json!({
        "user_id": 123,
        "name": "John"
    });
    let event = create_test_full_sync_event("users", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify mapping
    if let Event::FullSync { table, data } = mapped {
        assert_eq!(table, "app_users");
        assert_eq!(data.get("id"), Some(&json!(123)));
        assert_eq!(data.get("name"), Some(&json!("John")));
    } else {
        panic!("Expected FullSync event");
    }
}

#[test]
fn test_pass_through_other_events() {
    let config = MappingConfig::default();
    let mapper = FieldMapper::new(config);

    // Test checkpoint event passes through unchanged
    let event = Event::Checkpoint(Position::PostgreSQL {
        lsn: "0/1".to_string(),
    });
    let mapped = mapper.map_event(event.clone()).unwrap();

    match mapped {
        Event::Checkpoint(Position::PostgreSQL { lsn }) => {
            assert_eq!(lsn, "0/1");
        }
        _ => panic!("Expected checkpoint event with PostgreSQL position"),
    }
}

#[test]
fn test_value_to_string_conversion() {
    let mut tables = HashMap::new();
    let mut field_mappings = HashMap::new();

    // Combine different value types
    field_mappings.insert(
        "num".to_string(),
        FieldMapping::Complex {
            to: Some("combined".to_string()),
            default: None,
            sources: Some(vec![
                "num".to_string(),
                "bool".to_string(),
                "str".to_string(),
            ]),
            separator: Some("-".to_string()),
            path: None,
        },
    );

    tables.insert(
        "test".to_string(),
        TableMapping {
            name: None,
            fields: Some(field_mappings),
        },
    );

    let config = MappingConfig {
        tables,
        unmapped_fields_strategy: "exclude".to_string(),
        unmapped_fields_prefix: None,
    };

    let mapper = FieldMapper::new(config);

    // Create test event with different value types
    let mut data = HashMap::new();
    data.insert("num".to_string(), json!(42));
    data.insert("bool".to_string(), json!(true));
    data.insert("str".to_string(), json!("text"));
    let event = create_test_cdc_event("test", data);

    // Map the event
    let mapped = mapper.map_event(event).unwrap();

    // Verify conversion
    if let Event::Cdc(cdc_event) = mapped {
        assert_eq!(cdc_event.data.get("combined"), Some(&json!("42-true-text")));
    } else {
        panic!("Expected CDC event");
    }
}
