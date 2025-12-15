// PostgreSQL connection management integration tests

use crate::common::fixtures::*;
use crate::common::setup_postgres_cdc;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::postgres::Postgres;

#[cfg(test)]
mod connection_tests {
    use super::*;
    use crate::common::containers::wait_for_postgres;

    #[tokio::test]
    async fn test_connection_establishment() {
        let (_container, client, connection_string) = setup_postgres_cdc().await.unwrap();

        // Test connection
        let _config = create_test_postgres_config(&connection_string);

        // Test query
        let row = client.query_one("SELECT 1 as num", &[]).await.unwrap();
        let num: i32 = row.get("num");
        assert_eq!(num, 1);
    }

    #[tokio::test]
    async fn test_connection_pool_behavior() {
        let (_container, _client, connection_string) = setup_postgres_cdc().await.unwrap();

        // Test multiple connections
        let mut handles = vec![];

        for i in 0..5 {
            let conn_str = connection_string.clone();
            let handle = tokio::spawn(async move {
                let (client, connection) =
                    tokio_postgres::connect(&conn_str, tokio_postgres::NoTls)
                        .await
                        .unwrap();

                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("connection error: {e}");
                    }
                });

                // Execute a query to ensure connection works
                let row = client
                    .query_one(&format!("SELECT {i} as num"), &[])
                    .await
                    .unwrap();
                let num: i32 = row.get("num");
                assert_eq!(num, i);
            });
            handles.push(handle);
        }

        // Wait for all connections to complete
        for handle in handles {
            handle.await.unwrap();
        }
    }

    #[tokio::test]
    async fn test_connection_with_invalid_credentials() {
        use crate::common::containers::{start_postgres_with_cdc, wait_for_postgres};

        let postgres = start_postgres_with_cdc().await;
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();

        // Wait for PostgreSQL to be ready
        let valid_connection = format!("postgresql://postgres:postgres@localhost:{port}/testdb");
        wait_for_postgres(&valid_connection).await.unwrap();

        // Try with invalid password
        let invalid_connection = format!("postgresql://postgres:wrongpass@localhost:{port}/testdb");
        let result = tokio_postgres::connect(&invalid_connection, tokio_postgres::NoTls).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_connection_timeout() {
        // Test connection to non-existent server
        let invalid_connection = "postgresql://postgres:postgres@localhost:59999/testdb";

        let start = tokio::time::Instant::now();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            tokio_postgres::connect(invalid_connection, tokio_postgres::NoTls),
        )
        .await;

        assert!(result.is_err() || result.unwrap().is_err());
        assert!(start.elapsed() < std::time::Duration::from_secs(6));
    }

    #[tokio::test]
    async fn test_ssl_connection_modes() {
        let postgres = Postgres::default()
            .with_db_name("testdb")
            .start()
            .await
            .unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        // Test NoTls connection (should work with default PostgreSQL container)
        let result = tokio_postgres::connect(&connection_string, tokio_postgres::NoTls).await;
        assert!(result.is_ok());

        // Note: Testing actual SSL would require a PostgreSQL container configured with SSL
    }

    #[tokio::test]
    async fn test_connection_parameter_validation() {
        let postgres = Postgres::default()
            .with_db_name("testdb")
            .start()
            .await
            .unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();

        // Test various connection string formats
        let connection_strings = vec![
            format!("postgresql://postgres:postgres@localhost:{port}/testdb"),
            format!("postgres://postgres:postgres@localhost:{port}/testdb"),
            format!(
                "postgresql://postgres:postgres@localhost:{port}/testdb?application_name=meilibridge",
            ),
        ];

        for conn_str in connection_strings {
            wait_for_postgres(&conn_str).await.unwrap();

            let result = tokio_postgres::connect(&conn_str, tokio_postgres::NoTls).await;
            assert!(result.is_ok(), "Failed to connect with: {conn_str}");
        }
    }

    #[tokio::test]
    async fn test_reconnection_after_disconnect() {
        let postgres = Postgres::default()
            .with_db_name("testdb")
            .start()
            .await
            .unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();
        let connection_string = format!("postgresql://postgres:postgres@localhost:{port}/testdb");

        wait_for_postgres(&connection_string).await.unwrap();

        // Establish initial connection
        let (client, connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        let conn_handle = tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Verify connection works
        assert!(client.query_one("SELECT 1", &[]).await.is_ok());

        // Drop the client to simulate disconnect
        drop(client);
        conn_handle.await.ok();

        // Try to reconnect
        let (new_client, new_connection) =
            tokio_postgres::connect(&connection_string, tokio_postgres::NoTls)
                .await
                .unwrap();

        tokio::spawn(async move {
            if let Err(e) = new_connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Verify new connection works
        let row = new_client.query_one("SELECT 2 as num", &[]).await.unwrap();
        let num: i32 = row.get("num");
        assert_eq!(num, 2);
    }

    #[tokio::test]
    async fn test_connection_with_different_databases() {
        let postgres = Postgres::default()
            .with_db_name("testdb")
            .start()
            .await
            .unwrap();
        let port = postgres.get_host_port_ipv4(5432).await.unwrap();

        // Connect to default database first
        let postgres_db = format!("postgresql://postgres:postgres@localhost:{port}/postgres");
        wait_for_postgres(&postgres_db).await.unwrap();

        let (client, connection) = tokio_postgres::connect(&postgres_db, tokio_postgres::NoTls)
            .await
            .unwrap();

        tokio::spawn(async move {
            if let Err(e) = connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Create a new database
        client
            .execute("CREATE DATABASE meilibridge_test", &[])
            .await
            .ok();

        // Connect to the new database
        let new_db = format!("postgresql://postgres:postgres@localhost:{port}/meilibridge_test",);
        let (new_client, new_connection) = tokio_postgres::connect(&new_db, tokio_postgres::NoTls)
            .await
            .unwrap();

        tokio::spawn(async move {
            if let Err(e) = new_connection.await {
                eprintln!("connection error: {e}");
            }
        });

        // Verify we're connected to the right database
        let row = new_client
            .query_one("SELECT current_database()", &[])
            .await
            .unwrap();
        let db_name: String = row.get(0);
        assert_eq!(db_name, "meilibridge_test");
    }
}
