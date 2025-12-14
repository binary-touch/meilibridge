// Redis checkpoint storage integration tests

use crate::common::containers::Container;
use crate::common::test_data::create_checkpoint;
use meilibridge::models::progress::{Checkpoint, Position};
use redis::Commands;
use serde_json::json;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::redis::Redis;

#[cfg(test)]
mod redis_checkpoint_tests {
    use super::*;

    async fn setup_redis()
    -> Result<(Container<Redis>, redis::Client, String), Box<dyn std::error::Error>> {
        crate::common::setup_redis().await
    }

    #[tokio::test]
    async fn test_checkpoint_save_and_load() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let checkpoint = create_checkpoint("task1", "0/1234567");

        // Save checkpoint
        let key = format!("checkpoint:{}", checkpoint.task_id);
        let serialized = serde_json::to_string(&checkpoint).unwrap();
        let result: Result<(), redis::RedisError> = conn.set(&key, serialized);
        assert!(result.is_ok());

        // Load checkpoint
        let loaded: Result<String, redis::RedisError> = conn.get(&key);
        assert!(loaded.is_ok());

        let deserialized: Checkpoint = serde_json::from_str(&loaded.unwrap()).unwrap();
        assert_eq!(deserialized.task_id, checkpoint.task_id);

        match (&deserialized.position, &checkpoint.position) {
            (Position::PostgreSQL { lsn: lsn1, .. }, Position::PostgreSQL { lsn: lsn2, .. }) => {
                assert_eq!(lsn1, lsn2);
            }
            _ => panic!("Position types don't match"),
        }
    }

    #[tokio::test]
    async fn test_distributed_locking() {
        let (_container, client, _url) = setup_redis().await.unwrap();

        let lock_key = "lock:checkpoint:task1";
        let lock_value = "instance1";
        let ttl_seconds = 5;

        // Acquire lock
        let mut conn1 = client.get_connection().unwrap();
        let acquired: Result<bool, redis::RedisError> = redis::cmd("SET")
            .arg(lock_key)
            .arg(lock_value)
            .arg("NX")
            .arg("EX")
            .arg(ttl_seconds)
            .query(&mut conn1);

        assert!(acquired.unwrap());

        // Try to acquire same lock from another connection (should fail)
        let mut conn2 = client.get_connection().unwrap();
        let acquired2: Result<bool, redis::RedisError> = redis::cmd("SET")
            .arg(lock_key)
            .arg("instance2")
            .arg("NX")
            .arg("EX")
            .arg(ttl_seconds)
            .query(&mut conn2);

        assert!(!acquired2.unwrap_or(false));

        // Verify lock owner
        let owner: Result<String, redis::RedisError> = conn1.get(lock_key);
        assert_eq!(owner.unwrap(), lock_value);

        // Release lock
        let _: Result<u32, redis::RedisError> = conn1.del(lock_key);

        // Now another instance can acquire
        let acquired3: Result<bool, redis::RedisError> = redis::cmd("SET")
            .arg(lock_key)
            .arg("instance2")
            .arg("NX")
            .arg("EX")
            .arg(ttl_seconds)
            .query(&mut conn2);

        assert!(acquired3.unwrap());
    }

    #[tokio::test]
    async fn test_concurrent_updates() {
        let (_container, client, _url) = setup_redis().await.unwrap();

        let task_id = "concurrent_task";
        let key = format!("checkpoint:{}", task_id);

        // Simulate concurrent updates
        let mut handles = vec![];

        for i in 0..10 {
            let client_clone = client.clone();
            let key_clone = key.clone();
            let handle = tokio::spawn(async move {
                let mut conn = client_clone.get_connection().unwrap();
                let checkpoint = create_checkpoint(task_id, &format!("0/{}", i * 1000000));
                let serialized = serde_json::to_string(&checkpoint).unwrap();

                // Use atomic operations with optimistic locking
                loop {
                    // Watch the key
                    let _: Result<(), redis::RedisError> =
                        redis::cmd("WATCH").arg(&key_clone).query(&mut conn);

                    // Get current value
                    let _current: Result<Option<String>, redis::RedisError> = conn.get(&key_clone);

                    // Start transaction
                    let _: Result<(), redis::RedisError> = redis::cmd("MULTI").query(&mut conn);
                    let _: Result<(), redis::RedisError> = conn.set(&key_clone, &serialized);

                    // Execute transaction
                    let result: Result<Option<Vec<String>>, redis::RedisError> =
                        redis::cmd("EXEC").query(&mut conn);

                    match result {
                        Ok(Some(_)) => break, // Transaction succeeded
                        Ok(None) => continue, // Transaction aborted, retry
                        Err(e) => panic!("Transaction error: {}", e),
                    }
                }
            });
            handles.push(handle);
        }

        // Wait for all updates
        for handle in handles {
            handle.await.unwrap();
        }

        // Verify final state exists
        let mut conn = client.get_connection().unwrap();
        let final_value: Result<String, redis::RedisError> = conn.get(&key);
        assert!(final_value.is_ok());

        let final_checkpoint: Checkpoint = serde_json::from_str(&final_value.unwrap()).unwrap();
        assert_eq!(final_checkpoint.task_id, task_id);
    }

    #[tokio::test]
    async fn test_ttl_management() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let checkpoint = create_checkpoint("ttl_task", "0/7777777");
        let key = format!("checkpoint:{}", checkpoint.task_id);
        let serialized = serde_json::to_string(&checkpoint).unwrap();

        // Set with TTL
        let _: Result<(), redis::RedisError> = conn.set_ex(&key, serialized, 2);

        // Verify it exists
        let exists: Result<bool, redis::RedisError> = conn.exists(&key);
        assert!(exists.unwrap());

        // Check TTL
        let ttl: Result<i32, redis::RedisError> = conn.ttl(&key);
        let ttl_value = ttl.unwrap();
        assert!(ttl_value > 0 && ttl_value <= 2);

        // Wait for expiration
        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;

        // Verify it's gone
        let exists_after: Result<bool, redis::RedisError> = conn.exists(&key);
        assert!(!exists_after.unwrap());
    }

    #[tokio::test]
    async fn test_connection_failure_handling() {
        // Use Redis::default() and start() instead of DOCKER
        let container = Redis.start().await.unwrap();
        let port = container.get_host_port_ipv4(6379).await.unwrap();
        let url = format!("redis://localhost:{}", port);

        crate::common::containers::wait_for_redis(&url)
            .await
            .unwrap();

        let client = redis::Client::open(url.clone()).unwrap();
        let mut conn = client.get_connection().unwrap();

        // Set initial checkpoint
        let checkpoint = create_checkpoint("conn_test", "0/8888888");
        let key = format!("checkpoint:{}", checkpoint.task_id);
        let serialized = serde_json::to_string(&checkpoint).unwrap();
        let _: Result<(), redis::RedisError> = conn.set(&key, serialized);

        // Stop container to simulate connection failure
        // With ContainerAsync, dropping it stops the container
        drop(container);

        // Try to use connection (should fail)
        let result: Result<String, redis::RedisError> = conn.get(&key);
        assert!(result.is_err());

        // Start new container on same port (simulating recovery)
        // Note: In real tests, we'd need to ensure the port is free
        // For this test, we just verify that connection errors are properly handled
    }

    #[tokio::test]
    async fn test_multiple_checkpoints() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        // Save multiple checkpoints
        let task_ids = &["task_a", "task_b", "task_c"];
        let mut checkpoints = vec![];

        for (i, task_id) in task_ids.iter().enumerate() {
            let checkpoint = create_checkpoint(task_id, &format!("0/{}", (i + 1) * 1000000));
            let key = format!("checkpoint:{}", checkpoint.task_id);
            let serialized = serde_json::to_string(&checkpoint).unwrap();
            let _: Result<(), redis::RedisError> = conn.set(&key, serialized);
            checkpoints.push(checkpoint);
        }

        // List all checkpoints
        let pattern = "checkpoint:*";
        let keys: Result<Vec<String>, redis::RedisError> =
            redis::cmd("KEYS").arg(pattern).query(&mut conn);
        let keys = keys.unwrap();
        assert_eq!(keys.len(), 3);

        // Load all checkpoints
        for key in keys {
            let value: Result<String, redis::RedisError> = conn.get(&key);
            assert!(value.is_ok());

            let checkpoint: Checkpoint = serde_json::from_str(&value.unwrap()).unwrap();
            assert!(task_ids.contains(&checkpoint.task_id.as_str()));
        }

        // Delete specific checkpoint
        let delete_key = "checkpoint:task_b";
        let deleted: Result<u32, redis::RedisError> = conn.del(delete_key);
        assert_eq!(deleted.unwrap(), 1);

        // Verify deletion
        let remaining: Result<Vec<String>, redis::RedisError> =
            redis::cmd("KEYS").arg(pattern).query(&mut conn);
        assert_eq!(remaining.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn test_checkpoint_metadata() {
        let (_container, client, _url) = setup_redis().await.unwrap();
        let mut conn = client.get_connection().unwrap();

        let mut checkpoint = create_checkpoint("meta_task", "0/9999999");

        // Add metadata
        checkpoint.metadata = json!({
            "source": "postgres",
            "table_count": "5",
            "last_error": "none"
        });

        let key = format!("checkpoint:{}", checkpoint.task_id);
        let serialized = serde_json::to_string(&checkpoint).unwrap();
        let _: Result<(), redis::RedisError> = conn.set(&key, serialized);

        // Load and verify metadata
        let loaded: Result<String, redis::RedisError> = conn.get(&key);
        let deserialized: Checkpoint = serde_json::from_str(&loaded.unwrap()).unwrap();

        assert_eq!(deserialized.metadata["source"], "postgres");
        assert_eq!(deserialized.metadata["table_count"], "5");
        assert_eq!(deserialized.metadata["last_error"], "none");
    }

    #[tokio::test]
    async fn test_atomic_checkpoint_update() {
        let (_container, client, _url) = setup_redis().await.unwrap();

        let task_id = "atomic_task";
        let key = format!("checkpoint:{}", task_id);

        // Initial checkpoint
        let mut conn = client.get_connection().unwrap();
        let initial = create_checkpoint(task_id, "0/1000000");
        let _: Result<(), redis::RedisError> =
            conn.set(&key, serde_json::to_string(&initial).unwrap());

        // Atomic update using Lua script
        let lua_script = r#"
            local key = KEYS[1]
            local new_position = ARGV[1]
            local current = redis.call('GET', key)
            if current then
                local checkpoint = cjson.decode(current)
                local old_lsn = checkpoint.position.lsn
                checkpoint.position.lsn = new_position
                redis.call('SET', key, cjson.encode(checkpoint))
                return {old_lsn, new_position}
            end
            return nil
        "#;

        let result: Result<(String, String), redis::RedisError> = redis::cmd("EVAL")
            .arg(lua_script)
            .arg(1)
            .arg(&key)
            .arg("0/2000000")
            .query(&mut conn);

        assert!(result.is_ok());
        let (old_lsn, new_lsn) = result.unwrap();
        assert_eq!(old_lsn, "0/1000000");
        assert_eq!(new_lsn, "0/2000000");

        // Verify update
        let updated: Result<String, redis::RedisError> = conn.get(&key);
        let checkpoint: Checkpoint = serde_json::from_str(&updated.unwrap()).unwrap();

        match checkpoint.position {
            Position::PostgreSQL { lsn, .. } => assert_eq!(lsn, "0/2000000"),
            _ => panic!("Wrong position type"),
        }
    }
}
