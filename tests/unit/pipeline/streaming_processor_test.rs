use meilibridge::pipeline::streaming_processor::{StreamingJsonProcessor, ZeroCopyEventRouter};
use serde_json::{Value, json};
use std::io::Cursor;

#[test]
fn test_parse_single_document() {
    let processor = StreamingJsonProcessor::new(1024);
    let json_data = r#"{"id": 1, "name": "test"}"#;
    let reader = Cursor::new(json_data);

    let result = processor.parse_document(reader).unwrap();
    assert_eq!(result["id"], 1);
    assert_eq!(result["name"], "test");
}

#[test]
fn test_parse_document_exceeds_limit() {
    let processor = StreamingJsonProcessor::new(10); // Very small limit
    let json_data = r#"{"id": 1, "name": "very long name that exceeds limit"}"#;
    let reader = Cursor::new(json_data);

    let result = processor.parse_document(reader);
    assert!(result.is_err());
}

#[test]
fn test_parse_document_stream() {
    let processor = StreamingJsonProcessor::new(1024);
    // The current implementation only parses single documents, not streams
    // This is a limitation of the simple JSON parsing approach used
    let json_data = r#"{"id": 1}"#;
    let reader = Cursor::new(json_data);

    let docs: Vec<Value> = processor
        .parse_document_stream(reader)
        .filter_map(Result::ok)
        .collect();

    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["id"], 1);
}

#[test]
fn test_estimate_value_size() {
    let processor = StreamingJsonProcessor::new(1024);

    // Test various value types
    let _null_val = json!(null);
    let _bool_val = json!(true);
    let _num_val = json!(42);
    let _str_val = json!("hello");
    let _arr_val = json!([1, 2, 3]);
    let _obj_val = json!({"key": "value"});

    // Use reflection to test private method
    // Since we can't access private methods directly, we'll test indirectly
    let reader = Cursor::new("null");
    let _ = processor.parse_document(reader).unwrap();

    // Verify processor can handle different JSON types
    assert!(processor.parse_document(Cursor::new("true")).is_ok());
    assert!(processor.parse_document(Cursor::new("42")).is_ok());
    assert!(processor.parse_document(Cursor::new(r#""hello""#)).is_ok());
    assert!(processor.parse_document(Cursor::new("[1,2,3]")).is_ok());
    assert!(
        processor
            .parse_document(Cursor::new(r#"{"key":"value"}"#))
            .is_ok()
    );
}

#[tokio::test]
async fn test_process_array_streaming() {
    let processor = StreamingJsonProcessor::new(1024);
    let json_str = r#"[{"id": 1}, {"id": 2}, {"id": 3}]"#;

    let mut collected = Vec::new();
    let count = processor
        .process_array_streaming(json_str, |value| {
            collected.push(value);
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(count, 3);
    assert_eq!(collected.len(), 3);
    assert_eq!(collected[0]["id"], 1);
    assert_eq!(collected[1]["id"], 2);
    assert_eq!(collected[2]["id"], 3);
}

#[tokio::test]
async fn test_process_array_streaming_with_nested_objects() {
    let processor = StreamingJsonProcessor::new(1024);
    let json_str = r#"[
        {"id": 1, "data": {"nested": true}}, 
        {"id": 2, "array": [1, 2, 3]}
    ]"#;

    let mut collected = Vec::new();
    let count = processor
        .process_array_streaming(json_str, |value| {
            collected.push(value);
            Ok(())
        })
        .await
        .unwrap();

    assert_eq!(count, 2);
    assert_eq!(collected[0]["data"]["nested"], true);
    assert_eq!(collected[1]["array"][1], 2);
}

#[tokio::test]
async fn test_process_array_streaming_empty() {
    let processor = StreamingJsonProcessor::new(1024);
    let json_str = "[]";

    let count = processor
        .process_array_streaming(json_str, |_| Ok(()))
        .await
        .unwrap();

    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_process_array_streaming_invalid_json() {
    let processor = StreamingJsonProcessor::new(1024);
    let json_str = r#"[{"id": 1}, {"invalid": ]"#;

    let mut collected = Vec::new();
    let count = processor
        .process_array_streaming(json_str, |value| {
            collected.push(value);
            Ok(())
        })
        .await
        .unwrap();

    // Should successfully parse the first valid item
    assert_eq!(count, 1);
    assert_eq!(collected.len(), 1);
}

#[test]
fn test_zero_copy_event_router_valid_event() {
    let mut router = ZeroCopyEventRouter::new();
    let event_data = br#"{"table": "users", "id": 1, "name": "test"}"#;

    let event_view = router.route_event(event_data).unwrap();
    assert_eq!(event_view.table, "users");

    // Test full parse
    let full_event = event_view.parse_full().unwrap();
    assert_eq!(full_event["id"], 1);
    assert_eq!(full_event["name"], "test");
}

#[test]
fn test_zero_copy_event_router_missing_table() {
    let mut router = ZeroCopyEventRouter::new();
    let event_data = br#"{"id": 1, "name": "test"}"#;

    let result = router.route_event(event_data);
    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("Missing table field"));
    }
}

#[test]
fn test_zero_copy_event_router_invalid_json() {
    let mut router = ZeroCopyEventRouter::new();
    let event_data = b"not json";

    let result = router.route_event(event_data);
    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("Invalid JSON object"));
    }
}

#[test]
fn test_zero_copy_event_router_escaped_quotes() {
    let mut router = ZeroCopyEventRouter::new();
    let event_data = br#"{"table": "users\"with\"quotes", "id": 1}"#;

    let event_view = router.route_event(event_data).unwrap();
    assert_eq!(event_view.table, r#"users\"with\"quotes"#);
}

#[test]
fn test_zero_copy_event_router_whitespace() {
    let mut router = ZeroCopyEventRouter::new();
    let event_data = br#"  {  "table"  :  "users"  ,  "id"  :  1  }  "#;

    let event_view = router.route_event(event_data).unwrap();
    assert_eq!(event_view.table, "users");
}

#[test]
fn test_streaming_with_memory_monitor() {
    use meilibridge::pipeline::memory_monitor::MemoryMonitor;

    let monitor = MemoryMonitor::new(1, 1); // 1MB for queue, 1MB for checkpoint
    let processor = StreamingJsonProcessor::new(1024).with_memory_monitor(monitor);

    let json_data = r#"{"id": 1, "data": "test"}"#;
    let reader = Cursor::new(json_data);

    let result = processor.parse_document(reader);
    assert!(result.is_ok());
}

#[test]
fn test_streaming_processor_invalid_utf8() {
    let mut router = ZeroCopyEventRouter::new();
    // Create invalid UTF-8 sequence
    let mut event_data = Vec::from(&br#"{"table": ""#[..]);
    event_data.push(0xFF); // Invalid UTF-8
    event_data.extend_from_slice(br#"", "id": 1}"#);

    let result = router.route_event(&event_data);
    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("Invalid UTF-8"));
    }
}

#[test]
fn test_parse_document_empty_reader() {
    let processor = StreamingJsonProcessor::new(1024);
    let reader = Cursor::new("");

    let result = processor.parse_document(reader);
    assert!(result.is_err());
}

#[test]
fn test_event_view_parse_full_invalid() {
    use meilibridge::pipeline::streaming_processor::EventView;

    let event_view = EventView {
        table: "users",
        raw_data: b"invalid json",
    };

    let result = event_view.parse_full();
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("Failed to parse event")
    );
}
