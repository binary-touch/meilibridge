// Meilisearch batch processor integration tests

// Testing Meilisearch batch processing patterns
use crate::common::setup_meilisearch;
use chrono::Utc;
use meilibridge::models::event::{
    Event, EventData, EventId, EventMetadata, EventSource, EventType,
};
use serde_json::json;
use std::collections::HashMap;

#[cfg(test)]
mod batch_processor_tests {
    use super::*;

    #[allow(dead_code)]
    fn create_test_event(id: i32, event_type: EventType) -> Event {
        let mut data = HashMap::new();
        data.insert("id".to_string(), json!(id));
        data.insert("title".to_string(), json!(format!("Document {}", id)));
        data.insert(
            "content".to_string(),
            json!(format!("Content for document {}", id)),
        );

        Event {
            id: EventId::new(),
            event_type: event_type.clone(),
            source: EventSource {
                database: "testdb".to_string(),
                schema: "public".to_string(),
                table: "documents".to_string(),
            },
            data: EventData {
                key: json!({"id": id}),
                old: if matches!(event_type, EventType::Delete) {
                    Some(data.clone())
                } else {
                    None
                },
                new: if !matches!(event_type, EventType::Delete) {
                    Some(data)
                } else {
                    None
                },
            },
            metadata: EventMetadata {
                transaction_id: Some("tx123".to_string()),
                position: "0/1234567".to_string(),
                custom: HashMap::new(),
            },
            timestamp: Utc::now(),
        }
    }

    #[tokio::test]
    async fn test_batch_size_optimization() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        // Create index
        let index = client
            .create_index("batch_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Test small batch
        let small_batch: Vec<_> = (1..=10)
            .map(|i| {
                json!({
                    "id": i,
                    "title": format!("Doc {}", i)
                })
            })
            .collect();

        let task = index.add_documents(&small_batch, Some("id")).await.unwrap();

        task.wait_for_completion(&client, None, None).await.unwrap();

        // Test large batch
        let large_batch: Vec<_> = (11..=1010)
            .map(|i| {
                json!({
                    "id": i,
                    "title": format!("Doc {}", i),
                    "content": "x".repeat(1000) // Make documents larger
                })
            })
            .collect();

        let task = index.add_documents(&large_batch, Some("id")).await.unwrap();

        task.wait_for_completion(&client, None, None).await.unwrap();

        // Verify all documents were added
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 1010);
    }

    #[tokio::test]
    async fn test_partial_failure_handling() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        // Create index with specific schema
        let index = client
            .create_index("partial_failure_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Create batch with some invalid documents
        let batch = vec![
            json!({"id": 1, "title": "Valid doc 1"}),
            json!({"id": 2, "title": "Valid doc 2"}),
            json!({"id": 3, "title": "Valid doc 3"}),
        ];

        let task = index.add_documents(&batch, Some("id")).await.unwrap();

        let result = task.wait_for_completion(&client, None, None).await;
        assert!(result.is_ok());

        // Verify valid documents were added
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 3);
    }

    #[tokio::test]
    async fn test_document_validation() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        let index = client
            .create_index("validation_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Test various document types
        let valid_docs = vec![
            json!({"id": 1, "text": "Simple text"}),
            json!({"id": 2, "number": 42, "float": std::f64::consts::PI}),
            json!({"id": 3, "bool": true, "array": [1, 2, 3]}),
            json!({"id": 4, "nested": {"key": "value"}}),
            json!({"id": 5, "unicode": "你好世界 🌍"}),
        ];

        let task = index.add_documents(&valid_docs, Some("id")).await.unwrap();

        task.wait_for_completion(&client, None, None).await.unwrap();

        // Verify all documents were added
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 5);
    }

    #[tokio::test]
    async fn test_performance_under_load() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        let index = client
            .create_index("performance_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        let start = tokio::time::Instant::now();

        // Send multiple batches concurrently
        let mut handles = vec![];

        for batch_num in 0..10 {
            let index_clone = index.clone();
            let handle = tokio::spawn(async move {
                let batch: Vec<_> = (batch_num * 100..(batch_num + 1) * 100)
                    .map(|i| {
                        json!({
                            "id": i,
                            "title": format!("Document {}", i),
                            "content": format!("Content for document {}", i),
                            "timestamp": Utc::now().timestamp(),
                        })
                    })
                    .collect();

                index_clone
                    .add_documents(&batch, Some("id"))
                    .await
                    .unwrap()
                    .wait_for_completion(&index_clone.client, None, None)
                    .await
                    .unwrap();
            });
            handles.push(handle);
        }

        // Wait for all batches to complete
        for handle in handles {
            handle.await.unwrap();
        }

        let elapsed = start.elapsed();
        println!("Processed 1000 documents in {elapsed:?}");

        // Verify all documents were added
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 1000);

        // Performance assertion - should complete in reasonable time
        assert!(elapsed.as_secs() < 30, "Batch processing took too long");
    }

    #[tokio::test]
    async fn test_update_operations() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        let index = client
            .create_index("update_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Initial documents
        let initial_docs = vec![
            json!({"id": 1, "title": "Original 1", "version": 1}),
            json!({"id": 2, "title": "Original 2", "version": 1}),
            json!({"id": 3, "title": "Original 3", "version": 1}),
        ];

        index
            .add_documents(&initial_docs, Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Update documents
        let updated_docs = vec![
            json!({"id": 1, "title": "Updated 1", "version": 2}),
            json!({"id": 2, "title": "Updated 2", "version": 2, "new_field": "new"}),
        ];

        index
            .add_documents(&updated_docs, Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Verify updates
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let doc1 = index.get_document::<serde_json::Value>("1").await.unwrap();
        assert_eq!(doc1["title"], "Updated 1");
        assert_eq!(doc1["version"], 2);

        let doc2 = index.get_document::<serde_json::Value>("2").await.unwrap();
        assert_eq!(doc2["new_field"], "new");

        let doc3 = index.get_document::<serde_json::Value>("3").await.unwrap();
        assert_eq!(doc3["title"], "Original 3");
    }

    #[tokio::test]
    async fn test_delete_operations() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        let index = client
            .create_index("delete_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Add documents
        let docs = vec![
            json!({"id": 1, "title": "Doc 1"}),
            json!({"id": 2, "title": "Doc 2"}),
            json!({"id": 3, "title": "Doc 3"}),
            json!({"id": 4, "title": "Doc 4"}),
            json!({"id": 5, "title": "Doc 5"}),
        ];

        index
            .add_documents(&docs, Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Delete single document
        index
            .delete_document("1")
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Delete multiple documents
        index
            .delete_documents(&["2", "3"])
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Verify deletions
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let result1 = index.get_document::<serde_json::Value>("1").await;
        assert!(result1.is_err());

        let result2 = index.get_document::<serde_json::Value>("2").await;
        assert!(result2.is_err());

        let result3 = index.get_document::<serde_json::Value>("3").await;
        assert!(result3.is_err());

        let doc4 = index.get_document::<serde_json::Value>("4").await.unwrap();
        assert_eq!(doc4["title"], "Doc 4");

        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 2);
    }

    #[tokio::test]
    async fn test_mixed_operations_batch() {
        let (_container, client, _url) = setup_meilisearch().await.unwrap();

        let index = client
            .create_index("mixed_ops_test", Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap()
            .try_make_index(&client)
            .unwrap();

        // Initial data
        let initial = vec![
            json!({"id": 1, "title": "Keep 1"}),
            json!({"id": 2, "title": "Update 2"}),
            json!({"id": 3, "title": "Delete 3"}),
        ];

        index
            .add_documents(&initial, Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Mixed batch: new doc, update, and prepare for delete
        let mixed = vec![
            json!({"id": 2, "title": "Updated 2", "modified": true}),
            json!({"id": 4, "title": "New 4"}),
        ];

        index
            .add_documents(&mixed, Some("id"))
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Delete
        index
            .delete_document(3)
            .await
            .unwrap()
            .wait_for_completion(&client, None, None)
            .await
            .unwrap();

        // Verify final state
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        let stats = index.get_stats().await.unwrap();
        assert_eq!(stats.number_of_documents, 3);

        let doc2 = index.get_document::<serde_json::Value>("2").await.unwrap();
        assert_eq!(doc2["modified"], true);

        let result3 = index.get_document::<serde_json::Value>("3").await;
        assert!(result3.is_err());
    }
}
