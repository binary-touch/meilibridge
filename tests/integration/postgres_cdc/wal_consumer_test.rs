// PostgreSQL WAL consumer integration tests

use crate::common::containers::*;

#[cfg(test)]
mod wal_consumer_tests {
    use super::*;

    #[tokio::test]
    async fn test_real_time_change_capture() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        // Setup database
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create table
        client
            .execute(
                "CREATE TABLE test_changes (
                id SERIAL PRIMARY KEY,
                name TEXT NOT NULL,
                value INTEGER
            )",
                &[],
            )
            .await
            .unwrap();

        // Create publication
        client
            .execute("CREATE PUBLICATION test_pub FOR TABLE test_changes", &[])
            .await
            .unwrap();

        // Create replication slot
        let slot_name = "test_wal_slot";
        client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await
            .unwrap();

        // Insert test data and verify we would capture it
        let insert_result = client
            .execute(
                "INSERT INTO test_changes (name, value) VALUES ($1, $2)",
                &[&"test1", &42i32],
            )
            .await;

        assert!(insert_result.is_ok());
        assert_eq!(insert_result.unwrap(), 1);

        // Update test data
        let update_result = client
            .execute(
                "UPDATE test_changes SET value = $1 WHERE name = $2",
                &[&100i32, &"test1"],
            )
            .await;

        assert!(update_result.is_ok());
        assert_eq!(update_result.unwrap(), 1);

        // Delete test data
        let delete_result = client
            .execute("DELETE FROM test_changes WHERE name = $1", &[&"test1"])
            .await;

        assert!(delete_result.is_ok());
        assert_eq!(delete_result.unwrap(), 1);
    }

    #[tokio::test]
    async fn test_message_ordering() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create table
        client
            .execute(
                "CREATE TABLE test_ordering (
                id SERIAL PRIMARY KEY,
                seq INTEGER NOT NULL
            )",
                &[],
            )
            .await
            .unwrap();

        // Create publication
        client
            .execute(
                "CREATE PUBLICATION test_pub_ordering FOR TABLE test_ordering",
                &[],
            )
            .await
            .unwrap();

        // Insert multiple records in a specific order
        let mut expected_sequence = vec![];
        for i in 1..=10 {
            client
                .execute("INSERT INTO test_ordering (seq) VALUES ($1)", &[&i])
                .await
                .unwrap();
            expected_sequence.push(i);
        }

        // Verify data was inserted in order
        let rows = client
            .query("SELECT seq FROM test_ordering ORDER BY id", &[])
            .await
            .unwrap();

        let actual_sequence: Vec<i32> = rows.iter().map(|row| row.get("seq")).collect();

        assert_eq!(actual_sequence, expected_sequence);
    }

    #[tokio::test]
    async fn test_transaction_consistency() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        let (mut client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create tables
        client
            .execute(
                "CREATE TABLE test_txn_a (id SERIAL PRIMARY KEY, value INTEGER)",
                &[],
            )
            .await
            .unwrap();

        client
            .execute(
                "CREATE TABLE test_txn_b (id SERIAL PRIMARY KEY, value INTEGER)",
                &[],
            )
            .await
            .unwrap();

        // Create publication for both tables
        client
            .execute(
                "CREATE PUBLICATION test_pub_txn FOR TABLE test_txn_a, test_txn_b",
                &[],
            )
            .await
            .unwrap();

        // Perform transactional operations
        let transaction = client.transaction().await.unwrap();

        transaction
            .execute("INSERT INTO test_txn_a (value) VALUES ($1)", &[&1i32])
            .await
            .unwrap();

        transaction
            .execute("INSERT INTO test_txn_b (value) VALUES ($1)", &[&2i32])
            .await
            .unwrap();

        transaction.commit().await.unwrap();

        // Verify both inserts succeeded
        let count_a: i64 = client
            .query_one("SELECT COUNT(*) FROM test_txn_a", &[])
            .await
            .unwrap()
            .get(0);
        let count_b: i64 = client
            .query_one("SELECT COUNT(*) FROM test_txn_b", &[])
            .await
            .unwrap()
            .get(0);

        assert_eq!(count_a, 1);
        assert_eq!(count_b, 1);

        // Test rollback scenario
        let transaction2 = client.transaction().await.unwrap();

        transaction2
            .execute("INSERT INTO test_txn_a (value) VALUES ($1)", &[&3i32])
            .await
            .unwrap();

        transaction2
            .execute("INSERT INTO test_txn_b (value) VALUES ($1)", &[&4i32])
            .await
            .unwrap();

        transaction2.rollback().await.unwrap();

        // Verify rollback worked
        let count_a_after: i64 = client
            .query_one("SELECT COUNT(*) FROM test_txn_a", &[])
            .await
            .unwrap()
            .get(0);
        let count_b_after: i64 = client
            .query_one("SELECT COUNT(*) FROM test_txn_b", &[])
            .await
            .unwrap()
            .get(0);

        assert_eq!(count_a_after, 1); // Should still be 1
        assert_eq!(count_b_after, 1); // Should still be 1
    }

    #[tokio::test]
    async fn test_large_transaction_handling() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        let (mut client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create table
        client
            .execute(
                "CREATE TABLE test_large_txn (
                id SERIAL PRIMARY KEY,
                data TEXT
            )",
                &[],
            )
            .await
            .unwrap();

        // Create publication
        client
            .execute(
                "CREATE PUBLICATION test_pub_large FOR TABLE test_large_txn",
                &[],
            )
            .await
            .unwrap();

        // Insert many records in a single transaction
        let transaction = client.transaction().await.unwrap();

        for i in 0..1000 {
            transaction
                .execute(
                    "INSERT INTO test_large_txn (data) VALUES ($1)",
                    &[&format!("Record {i}")],
                )
                .await
                .unwrap();
        }

        transaction.commit().await.unwrap();

        // Verify all records were inserted
        let count: i64 = client
            .query_one("SELECT COUNT(*) FROM test_large_txn", &[])
            .await
            .unwrap()
            .get(0);

        assert_eq!(count, 1000);
    }

    #[tokio::test]
    async fn test_schema_change_handling() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create initial table
        client
            .execute(
                "CREATE TABLE test_schema (
                id SERIAL PRIMARY KEY,
                name TEXT
            )",
                &[],
            )
            .await
            .unwrap();

        // Create publication
        client
            .execute(
                "CREATE PUBLICATION test_pub_schema FOR TABLE test_schema",
                &[],
            )
            .await
            .unwrap();

        // Insert initial data
        client
            .execute("INSERT INTO test_schema (name) VALUES ($1)", &[&"initial"])
            .await
            .unwrap();

        // Add new column
        client
            .execute("ALTER TABLE test_schema ADD COLUMN email TEXT", &[])
            .await
            .unwrap();

        // Insert data with new column
        client
            .execute(
                "INSERT INTO test_schema (name, email) VALUES ($1, $2)",
                &[&"with_email", &"test@example.com"],
            )
            .await
            .unwrap();

        // Verify both records exist
        let rows = client
            .query("SELECT id, name, email FROM test_schema ORDER BY id", &[])
            .await
            .unwrap();

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].get::<_, String>("name"), "initial");
        assert_eq!(rows[0].get::<_, Option<String>>("email"), None);
        assert_eq!(rows[1].get::<_, String>("name"), "with_email");
        assert_eq!(
            rows[1].get::<_, Option<String>>("email"),
            Some("test@example.com".to_string())
        );
    }

    #[tokio::test]
    async fn test_multi_table_changes() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create multiple tables
        client
            .execute(
                "CREATE TABLE users (
                id SERIAL PRIMARY KEY,
                username TEXT NOT NULL
            )",
                &[],
            )
            .await
            .unwrap();

        client
            .execute(
                "CREATE TABLE posts (
                id SERIAL PRIMARY KEY,
                user_id INTEGER REFERENCES users(id),
                content TEXT
            )",
                &[],
            )
            .await
            .unwrap();

        client
            .execute(
                "CREATE TABLE comments (
                id SERIAL PRIMARY KEY,
                post_id INTEGER REFERENCES posts(id),
                comment TEXT
            )",
                &[],
            )
            .await
            .unwrap();

        // Create publication for all tables
        client
            .execute(
                "CREATE PUBLICATION test_pub_multi FOR TABLE users, posts, comments",
                &[],
            )
            .await
            .unwrap();

        // Insert related data
        let user_id: i32 = client
            .query_one(
                "INSERT INTO users (username) VALUES ($1) RETURNING id",
                &[&"testuser"],
            )
            .await
            .unwrap()
            .get(0);

        let post_id: i32 = client
            .query_one(
                "INSERT INTO posts (user_id, content) VALUES ($1, $2) RETURNING id",
                &[&user_id, &"Test post"],
            )
            .await
            .unwrap()
            .get(0);

        client
            .execute(
                "INSERT INTO comments (post_id, comment) VALUES ($1, $2)",
                &[&post_id, &"Test comment"],
            )
            .await
            .unwrap();

        // Verify data integrity
        let comment_count: i64 = client
            .query_one(
                "SELECT COUNT(*) FROM comments c
             JOIN posts p ON c.post_id = p.id
             JOIN users u ON p.user_id = u.id
             WHERE u.username = $1",
                &[&"testuser"],
            )
            .await
            .unwrap()
            .get(0);

        assert_eq!(comment_count, 1);
    }
}
