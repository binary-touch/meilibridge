// Pipeline filter tests

use chrono::Utc;
use meilibridge::models::event::{
    Event, EventData, EventId, EventMetadata, EventSource, EventType,
};
use serde_json::json;
use std::collections::HashMap;

#[cfg(test)]
mod filter_tests {
    use super::*;

    fn create_test_event(table: &str, event_type: EventType) -> Event {
        let mut data = HashMap::new();
        data.insert("id".to_string(), json!(1));
        data.insert("name".to_string(), json!("Test"));

        let is_delete = event_type == EventType::Delete;

        Event {
            id: EventId::new(),
            event_type,
            source: EventSource {
                database: "testdb".to_string(),
                schema: "public".to_string(),
                table: table.to_string(),
            },
            data: EventData {
                key: json!({"id": 1}),
                old: if is_delete { Some(data.clone()) } else { None },
                new: if !is_delete { Some(data) } else { None },
            },
            metadata: EventMetadata {
                transaction_id: Some("123".to_string()),
                position: "0/1234567".to_string(),
                custom: HashMap::new(),
            },
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn test_event_type_filtering() {
        let create_event = create_test_event("users", EventType::Create);
        let update_event = create_test_event("users", EventType::Update);
        let delete_event = create_test_event("users", EventType::Delete);

        assert_eq!(create_event.event_type, EventType::Create);
        assert_eq!(update_event.event_type, EventType::Update);
        assert_eq!(delete_event.event_type, EventType::Delete);
    }

    #[test]
    fn test_table_name_filtering() {
        let users_event = create_test_event("users", EventType::Create);
        let posts_event = create_test_event("posts", EventType::Create);
        let comments_event = create_test_event("comments", EventType::Create);

        assert_eq!(users_event.source.table, "users");
        assert_eq!(posts_event.source.table, "posts");
        assert_eq!(comments_event.source.table, "comments");
    }

    #[test]
    fn test_schema_filtering() {
        let mut event1 = create_test_event("users", EventType::Create);
        event1.source.schema = "public".to_string();

        let mut event2 = create_test_event("users", EventType::Create);
        event2.source.schema = "private".to_string();

        assert_eq!(event1.source.schema, "public");
        assert_eq!(event2.source.schema, "private");
    }

    #[test]
    fn test_event_data_filtering() {
        let mut data_with_status = HashMap::new();
        data_with_status.insert("id".to_string(), json!(1));
        data_with_status.insert("status".to_string(), json!("active"));

        let event = Event {
            id: EventId::new(),
            event_type: EventType::Create,
            source: EventSource {
                database: "testdb".to_string(),
                schema: "public".to_string(),
                table: "users".to_string(),
            },
            data: EventData {
                key: json!({"id": 1}),
                old: None,
                new: Some(data_with_status),
            },
            metadata: EventMetadata {
                transaction_id: None,
                position: "0/1234567".to_string(),
                custom: HashMap::new(),
            },
            timestamp: Utc::now(),
        };

        let status = event
            .data
            .new
            .as_ref()
            .and_then(|d| d.get("status"))
            .and_then(|v| v.as_str());
        assert_eq!(status, Some("active"));
    }

    #[test]
    fn test_multiple_filter_criteria() {
        // Create events with different combinations
        let events = [
            create_test_event("users", EventType::Create),
            create_test_event("users", EventType::Update),
            create_test_event("posts", EventType::Create),
            create_test_event("posts", EventType::Delete),
        ];

        // Count events by type
        let create_count = events
            .iter()
            .filter(|e| e.event_type == EventType::Create)
            .count();
        assert_eq!(create_count, 2);

        // Count events by table
        let users_count = events.iter().filter(|e| e.source.table == "users").count();
        assert_eq!(users_count, 2);

        // Combined filter
        let users_create_count = events
            .iter()
            .filter(|e| e.source.table == "users" && e.event_type == EventType::Create)
            .count();
        assert_eq!(users_create_count, 1);
    }

    #[test]
    fn test_transaction_filtering() {
        let mut event_with_tx = create_test_event("users", EventType::Create);
        event_with_tx.metadata.transaction_id = Some("tx-123".to_string());

        let mut event_without_tx = create_test_event("users", EventType::Create);
        event_without_tx.metadata.transaction_id = None;

        assert!(event_with_tx.metadata.transaction_id.is_some());
        assert!(event_without_tx.metadata.transaction_id.is_none());
    }

    #[test]
    fn test_event_position_filtering() {
        let positions = vec!["0/1000000", "0/2000000", "0/3000000"];
        let events: Vec<Event> = positions
            .into_iter()
            .map(|pos| {
                let mut event = create_test_event("users", EventType::Create);
                event.metadata.position = pos.to_string();
                event
            })
            .collect();

        assert_eq!(events[0].metadata.position, "0/1000000");
        assert_eq!(events[1].metadata.position, "0/2000000");
        assert_eq!(events[2].metadata.position, "0/3000000");
    }
}
