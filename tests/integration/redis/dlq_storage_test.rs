// Redis DLQ (Dead Letter Queue) storage integration tests

// Note: redis_storage module is private, so we can't directly test it
// These tests demonstrate DLQ patterns using Redis directly
use crate::common::containers::Container;
use chrono::Utc;
use meilibridge::error::MeiliBridgeError;
use meilibridge::models::event::{
    Event, EventData, EventId, EventMetadata, EventSource, EventType,
};
use redis::Commands;
use serde_json::json;
use std::collections::HashMap;
use testcontainers_modules::redis::Redis;

#[cfg(test)]
mod redis_dlq_tests {
    use super::*;

    async fn setup_redis(
    ) -> Result<(Container<Redis>, redis::Client, String), Box<dyn std::error::Error>>
    {
        crate::common::setup_redis().await
    }

    fn create_test_event(id: i32) -> Event {
        let mut data = HashMap::new();
        data.insert("id".to_string(), json!(id));
        data.insert("name".to_string(), json!(format!("Test Event {}", id)));

        Event {
            id: EventId::new(),
            event_type: EventType::Create,
            source: EventSource {
                database: "testdb".to_string(),
                schema: "public".to_string(),
                table: "test_table".to_string(),
            },
            data: EventData {
                key: json!({"id": id}),
                old: None,
                new: Some(data),
            },
            metadata: EventMetadata {
                transaction_id: Some("tx123".to_string()),
                position: format!("0/{}", id * 1000000),
                custom: HashMap::new(),
            },
            timestamp: Utc::now(),
        }
    }

    fn create_dlq_entry(
        event: Event,
        error: MeiliBridgeError,
        retry_count: u32,
    ) -> serde_json::Value {
        json!({
            "event": event,
            "error": error.to_string(),
            "retry_count": retry_count,
            "failed_at": Utc::now().to_rfc3339(),
            "task_id": "test_task",
        })
    }

    #[tokio::test]
    async fn test_failed_event_storage() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let event = create_test_event(1);
        let error = MeiliBridgeError::Meilisearch("Connection timeout".to_string());
        let dlq_entry = create_dlq_entry(event.clone(), error, 3);

        // Store in DLQ
        let dlq_key = "dlq:test_task";
        let serialized = serde_json::to_string(&dlq_entry).unwrap();
        let result: Result<i32, redis::RedisError> = conn.rpush(dlq_key, serialized);

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 1);

        // Verify storage
        let length: Result<usize, redis::RedisError> = conn.llen(dlq_key);
        assert_eq!(length.unwrap(), 1);

        // Retrieve entry
        let stored: Result<Vec<String>, redis::RedisError> = conn.lrange(dlq_key, 0, -1);
        let entries = stored.unwrap();
        assert_eq!(entries.len(), 1);

        let retrieved: serde_json::Value = serde_json::from_str(&entries[0]).unwrap();
        assert_eq!(retrieved["task_id"], "test_task");
        assert_eq!(retrieved["retry_count"], 3);
    }

    #[tokio::test]
    async fn test_bulk_operations() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let dlq_key = "dlq:bulk_test";
        let mut entries = vec![];

        // Create multiple failed events
        for i in 1..=100 {
            let event = create_test_event(i);
            let error = MeiliBridgeError::Database(format!("Error {}", i));
            let entry = create_dlq_entry(event, error, i as u32 % 5);
            entries.push(serde_json::to_string(&entry).unwrap());
        }

        // Bulk insert using pipeline
        let mut pipe = redis::pipe();
        for entry in &entries {
            pipe.rpush(dlq_key, entry);
        }

        let result: Result<Vec<i32>, redis::RedisError> = pipe.query(&mut conn);
        assert!(result.is_ok());

        // Verify count
        let count: Result<usize, redis::RedisError> = conn.llen(dlq_key);
        assert_eq!(count.unwrap(), 100);

        // Bulk retrieve in pages
        let page_size = 20;
        let mut total_retrieved = 0;

        for page in 0..5 {
            let start = page * page_size;
            let end = start + page_size - 1;

            let page_entries: Result<Vec<String>, redis::RedisError> =
                conn.lrange(dlq_key, start as isize, end as isize);

            let entries = page_entries.unwrap();
            assert_eq!(entries.len(), page_size);
            total_retrieved += entries.len();
        }

        assert_eq!(total_retrieved, 100);
    }

    #[tokio::test]
    async fn test_event_retrieval() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let dlq_key = "dlq:retrieval_test";

        // Add events with different retry counts
        for i in 1..=10 {
            let event = create_test_event(i);
            let error = MeiliBridgeError::Validation(format!("Validation error {}", i));
            let entry = create_dlq_entry(event, error, i as u32);
            let _: Result<i32, redis::RedisError> =
                conn.rpush(dlq_key, serde_json::to_string(&entry).unwrap());
        }

        // Retrieve and filter events with retry_count < 5
        let all_entries: Result<Vec<String>, redis::RedisError> = conn.lrange(dlq_key, 0, -1);
        let entries = all_entries.unwrap();

        let retryable_events: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| serde_json::from_str(e).unwrap())
            .filter(|e: &serde_json::Value| e["retry_count"].as_u64().unwrap() < 5)
            .collect();

        assert_eq!(retryable_events.len(), 4);

        // Pop events for reprocessing (FIFO)
        let popped: Result<Option<String>, redis::RedisError> = conn.lpop(dlq_key, None);
        assert!(popped.unwrap().is_some());

        // Verify one less event
        let remaining: Result<usize, redis::RedisError> = conn.llen(dlq_key);
        assert_eq!(remaining.unwrap(), 9);
    }

    #[tokio::test]
    async fn test_expiration_handling() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        // Use sorted set for expiration tracking
        let dlq_zset = "dlq:expiring:sorted";
        let current_time = Utc::now().timestamp();

        // Add events with different expiration times
        for i in 1..=5 {
            let event = create_test_event(i);
            let error =
                MeiliBridgeError::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, "Timeout"));
            let entry = create_dlq_entry(event, error, 1);

            let score = current_time + (i as i64 * 60); // Different expiration times
            let _: Result<i32, redis::RedisError> = redis::cmd("ZADD")
                .arg(dlq_zset)
                .arg(score)
                .arg(serde_json::to_string(&entry).unwrap())
                .query(&mut conn);
        }

        // Get expired events (older than current_time + 180 seconds)
        let expiration_threshold = current_time + 180;
        let expired: Result<Vec<String>, redis::RedisError> = redis::cmd("ZRANGEBYSCORE")
            .arg(dlq_zset)
            .arg("-inf")
            .arg(expiration_threshold)
            .query(&mut conn);

        let expired_events = expired.unwrap();
        assert_eq!(expired_events.len(), 3); // Events 1, 2, 3 are expired

        // Remove expired events
        let removed: Result<i32, redis::RedisError> = redis::cmd("ZREMRANGEBYSCORE")
            .arg(dlq_zset)
            .arg("-inf")
            .arg(expiration_threshold)
            .query(&mut conn);

        assert_eq!(removed.unwrap(), 3);

        // Verify remaining events
        let remaining: Result<usize, redis::RedisError> =
            redis::cmd("ZCARD").arg(dlq_zset).query(&mut conn);

        assert_eq!(remaining.unwrap(), 2);
    }

    #[tokio::test]
    async fn test_dlq_statistics() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        // Create DLQ entries for multiple tasks
        let tasks = vec!["task_a", "task_b", "task_c"];
        let mut total_events: usize = 0;

        for task in &tasks {
            let dlq_key = format!("dlq:{}", task);
            let event_count = match *task {
                "task_a" => 15usize,
                "task_b" => 8usize,
                "task_c" => 23usize,
                _ => 0,
            };

            for i in 1..=event_count {
                let event = create_test_event(i as i32);
                let error = MeiliBridgeError::Configuration("Config error".to_string());
                let entry = create_dlq_entry(event, error, 1);
                let _: Result<i32, redis::RedisError> =
                    conn.rpush(&dlq_key, serde_json::to_string(&entry).unwrap());
            }

            total_events += event_count;
        }

        // Gather statistics
        let mut stats = HashMap::new();

        for task in &tasks {
            let dlq_key = format!("dlq:{}", task);
            let count: Result<usize, redis::RedisError> = conn.llen(&dlq_key);
            stats.insert(task.to_string(), count.unwrap());
        }

        assert_eq!(stats["task_a"], 15);
        assert_eq!(stats["task_b"], 8);
        assert_eq!(stats["task_c"], 23);

        // Global statistics using pattern matching
        let pattern = "dlq:*";
        let all_keys: Result<Vec<String>, redis::RedisError> =
            redis::cmd("KEYS").arg(pattern).query(&mut conn);

        let keys = all_keys.unwrap();
        assert_eq!(keys.len(), 3);

        // Calculate total across all DLQs
        let mut grand_total = 0;
        for key in keys {
            let count: Result<usize, redis::RedisError> = conn.llen(&key);
            grand_total += count.unwrap();
        }

        assert_eq!(grand_total, total_events);
    }

    #[tokio::test]
    async fn test_reprocessing_workflow() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let dlq_key = "dlq:reprocess_test";
        let processing_key = "dlq:reprocess_test:processing";

        // Add failed events
        for i in 1..=5 {
            let event = create_test_event(i);
            let error = MeiliBridgeError::Source("Source error".to_string());
            let entry = create_dlq_entry(event, error, 2);
            let _: Result<i32, redis::RedisError> =
                conn.rpush(dlq_key, serde_json::to_string(&entry).unwrap());
        }

        // Move events to processing queue atomically
        let lua_script = r#"
            local dlq_key = KEYS[1]
            local processing_key = KEYS[2]
            local batch_size = tonumber(ARGV[1])
            
            local events = redis.call('LRANGE', dlq_key, 0, batch_size - 1)
            if #events > 0 then
                for _, event in ipairs(events) do
                    redis.call('RPUSH', processing_key, event)
                end
                redis.call('LTRIM', dlq_key, batch_size, -1)
            end
            
            return #events
        "#;

        let moved: Result<i32, redis::RedisError> = redis::cmd("EVAL")
            .arg(lua_script)
            .arg(2)
            .arg(dlq_key)
            .arg(processing_key)
            .arg(3) // batch size
            .query(&mut conn);

        assert_eq!(moved.unwrap(), 3);

        // Verify state
        let dlq_remaining: Result<usize, redis::RedisError> = conn.llen(dlq_key);
        assert_eq!(dlq_remaining.unwrap(), 2);

        let processing_count: Result<usize, redis::RedisError> = conn.llen(processing_key);
        assert_eq!(processing_count.unwrap(), 3);

        // Simulate successful reprocessing - remove from processing queue
        let _: Result<(), redis::RedisError> = conn.del(processing_key);

        // Verify processing queue is empty
        let final_processing: Result<usize, redis::RedisError> = conn.llen(processing_key);
        assert_eq!(final_processing.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_dlq_monitoring() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        // Set up monitoring keys
        let metrics_hash = "dlq:metrics";

        // Initialize metrics
        let _: Result<(), redis::RedisError> = conn.hset(metrics_hash, "total_failures", 0);
        let _: Result<(), redis::RedisError> = conn.hset(metrics_hash, "reprocessed", 0);
        let _: Result<(), redis::RedisError> = conn.hset(metrics_hash, "expired", 0);

        // Simulate event failures
        for i in 1..=20 {
            // Increment failure counter
            let _: Result<i32, redis::RedisError> = conn.hincr(metrics_hash, "total_failures", 1);

            // Add to DLQ
            let event = create_test_event(i);
            let error = MeiliBridgeError::Meilisearch("Destination unavailable".to_string());
            let entry = create_dlq_entry(event, error, 1);
            let _: Result<i32, redis::RedisError> =
                conn.rpush("dlq:monitored", serde_json::to_string(&entry).unwrap());
        }

        // Simulate reprocessing
        let _: Result<i32, redis::RedisError> = conn.hincr(metrics_hash, "reprocessed", 5);

        // Simulate expiration
        let _: Result<i32, redis::RedisError> = conn.hincr(metrics_hash, "expired", 3);

        // Get all metrics
        let metrics: Result<HashMap<String, i32>, redis::RedisError> = conn.hgetall(metrics_hash);
        let metrics = metrics.unwrap();

        assert_eq!(metrics["total_failures"], 20);
        assert_eq!(metrics["reprocessed"], 5);
        assert_eq!(metrics["expired"], 3);

        // Calculate active DLQ size
        let dlq_size: Result<usize, redis::RedisError> = conn.llen("dlq:monitored");
        let active_failures = dlq_size.unwrap();

        assert_eq!(active_failures, 20);

        // Success rate calculation
        let success_rate =
            (metrics["reprocessed"] as f64 / metrics["total_failures"] as f64) * 100.0;
        assert_eq!(success_rate, 25.0);
    }
}
