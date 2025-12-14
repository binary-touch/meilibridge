// PostgreSQL replication slot management integration tests
//
// These tests use binarytouch/postgres:17 image which has wal_level=logical configured.

use crate::common::containers::*;

#[cfg(test)]
mod replication_slot_tests {
    use super::*;

    #[tokio::test]
    async fn test_replication_slot_creation() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        // Create regular connection to create the slot
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        // Create replication slot using pg_create_logical_replication_slot
        let slot_name = "test_slot";
        let result = client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await;

        assert!(result.is_ok());

        // Verify slot exists
        let (verify_client, verify_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = verify_connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let rows = verify_client
            .query(
                "SELECT slot_name, plugin FROM pg_replication_slots WHERE slot_name = $1",
                &[&slot_name],
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>("slot_name"), slot_name);
        assert_eq!(rows[0].get::<_, String>("plugin"), "pgoutput");
    }

    #[tokio::test]
    async fn test_replication_slot_deletion() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        // Create slot first
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let slot_name = "test_slot_delete";
        client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await
            .unwrap();

        // Drop the slot
        let (drop_client, drop_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = drop_connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        drop_client
            .execute("SELECT pg_drop_replication_slot($1)", &[&slot_name])
            .await
            .unwrap();

        // Verify slot is gone
        let rows = drop_client
            .query(
                "SELECT slot_name FROM pg_replication_slots WHERE slot_name = $1",
                &[&slot_name],
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 0);
    }

    #[tokio::test]
    async fn test_slot_recovery_after_disconnect() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        let slot_name = "test_slot_recovery";

        // Create slot
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        let conn_handle = tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await
            .unwrap();

        // Disconnect
        drop(client);
        conn_handle.await.ok();

        // Verify slot still exists after disconnect
        let (verify_client, verify_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = verify_connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let rows = verify_client
            .query(
                "SELECT slot_name, active FROM pg_replication_slots WHERE slot_name = $1",
                &[&slot_name],
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].get::<_, String>("slot_name"), slot_name);
        assert!(!rows[0].get::<_, bool>("active")); // Should be inactive after disconnect
    }

    #[tokio::test]
    async fn test_multiple_slots() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        let slot_names = vec!["slot1", "slot2", "slot3"];

        // Create multiple slots using single connection
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        for slot_name in &slot_names {
            client
                .query(
                    "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                    &[slot_name],
                )
                .await
                .unwrap();
        }

        // Verify all slots exist
        let (verify_client, verify_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = verify_connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        let rows = verify_client
            .query(
                "SELECT slot_name FROM pg_replication_slots WHERE slot_name = ANY($1)",
                &[&slot_names],
            )
            .await
            .unwrap();

        assert_eq!(rows.len(), 3);

        // Clean up
        for slot_name in &slot_names {
            verify_client
                .execute("SELECT pg_drop_replication_slot($1)", &[slot_name])
                .await
                .ok();
        }
    }

    #[tokio::test]
    async fn test_slot_with_publication() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        // Create table and publication
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        // Create test table
        client
            .execute(
                "CREATE TABLE test_table (id SERIAL PRIMARY KEY, name TEXT)",
                &[],
            )
            .await
            .unwrap();

        // Create publication
        client
            .execute("CREATE PUBLICATION test_pub FOR TABLE test_table", &[])
            .await
            .unwrap();

        // Create replication slot
        let slot_name = "test_slot_pub";
        client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await
            .unwrap();

        // Verify slot was created successfully
        let slot_exists = client
            .query(
                "SELECT 1 FROM pg_replication_slots WHERE slot_name = $1",
                &[&slot_name],
            )
            .await
            .unwrap();

        assert_eq!(slot_exists.len(), 1);
    }

    #[tokio::test]
    async fn test_slot_cleanup_on_failure() {
        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{}/testdb", port);

        wait_for_postgres(&connection_string).await.unwrap();

        let slot_name = "test_slot_cleanup";

        // Try to create slot twice (should fail second time)
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        // First creation should succeed
        client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await
            .unwrap();

        // Second creation should fail
        let result = client
            .query(
                "SELECT pg_create_logical_replication_slot($1, 'pgoutput')",
                &[&slot_name],
            )
            .await;

        assert!(result.is_err());

        // Clean up
        let (cleanup_client, cleanup_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = cleanup_connection.await {
                eprintln!("connection error: {}", e);
            }
        });

        cleanup_client
            .execute("SELECT pg_drop_replication_slot($1)", &[&slot_name])
            .await
            .ok();
    }
}
