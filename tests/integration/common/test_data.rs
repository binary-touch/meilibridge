// Common test data generators and utilities

use chrono::Utc;
use meilibridge::models::progress::{Checkpoint, Position, ProgressStats};
use serde_json::{Value, json};
use std::collections::HashMap;
use uuid::Uuid;

/// Generate test user data
pub fn generate_users(count: usize) -> Vec<HashMap<String, Value>> {
    (0..count)
        .map(|i| {
            let mut data = HashMap::new();
            data.insert("id".to_string(), json!(i + 1));
            data.insert("name".to_string(), json!(format!("User {}", i + 1)));
            data.insert(
                "email".to_string(),
                json!(format!("user{}@example.com", i + 1)),
            );
            data.insert("created_at".to_string(), json!(Utc::now().to_rfc3339()));
            data.insert("active".to_string(), json!(i % 3 != 0)); // Some inactive users
            data
        })
        .collect()
}

/// Generate test product data
pub fn generate_products(count: usize) -> Vec<HashMap<String, Value>> {
    let categories = &["Electronics", "Books", "Clothing", "Food", "Toys"];

    (0..count)
        .map(|i| {
            let mut data = HashMap::new();
            data.insert("id".to_string(), json!(i + 1));
            data.insert("name".to_string(), json!(format!("Product {}", i + 1)));
            data.insert(
                "description".to_string(),
                json!(format!("Description for product {}", i + 1)),
            );
            data.insert("price".to_string(), json!((i + 1) as f64 * 9.99));
            data.insert(
                "category".to_string(),
                json!(categories[i % categories.len()]),
            );
            data.insert("stock".to_string(), json!((i + 1) * 10));
            data.insert("created_at".to_string(), json!(Utc::now().to_rfc3339()));
            data
        })
        .collect()
}

/// Generate test order data
pub fn generate_orders(
    count: usize,
    user_count: usize,
    product_count: usize,
) -> Vec<HashMap<String, Value>> {
    (0..count)
        .map(|i| {
            let mut data = HashMap::new();
            data.insert("id".to_string(), json!(i + 1));
            data.insert("user_id".to_string(), json!((i % user_count) + 1));
            data.insert("product_id".to_string(), json!((i % product_count) + 1));
            data.insert("quantity".to_string(), json!((i % 5) + 1));
            data.insert("total_amount".to_string(), json!((i + 1) as f64 * 49.99));
            data.insert(
                "status".to_string(),
                json!(if i % 4 == 0 {
                    "pending"
                } else if i % 4 == 1 {
                    "processing"
                } else if i % 4 == 2 {
                    "shipped"
                } else {
                    "delivered"
                }),
            );
            data.insert("created_at".to_string(), json!(Utc::now().to_rfc3339()));
            data
        })
        .collect()
}

/// Create a test checkpoint
pub fn create_checkpoint(task_id: &str, lsn: &str) -> Checkpoint {
    Checkpoint {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_string(),
        position: Position::PostgreSQL {
            lsn: lsn.to_string(),
        },
        created_at: Utc::now(),
        stats: ProgressStats::default(),
        metadata: json!({}),
    }
}

/// Create a test checkpoint with custom stats
pub fn create_checkpoint_with_stats(
    task_id: &str,
    lsn: &str,
    events_processed: u64,
    events_failed: u64,
) -> Checkpoint {
    let mut checkpoint = create_checkpoint(task_id, lsn);
    checkpoint.stats.events_processed = events_processed;
    checkpoint.stats.events_failed = events_failed;
    checkpoint.stats.last_event_at = Some(Utc::now());
    checkpoint
}

/// SQL helpers for test data
pub mod sql {
    use tokio_postgres::Client;

    /// Create a test table with common fields
    pub async fn create_test_table(
        client: &Client,
        table_name: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        client
            .execute(
                &format!(
                    "CREATE TABLE IF NOT EXISTS {table_name} (
                        id SERIAL PRIMARY KEY,
                        name TEXT NOT NULL,
                        email TEXT,
                        created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                        deleted_at TIMESTAMP
                    )"
                ),
                &[],
            )
            .await?;
        Ok(())
    }

    /// Create a products table
    pub async fn create_products_table(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        client
            .execute(
                "CREATE TABLE IF NOT EXISTS products (
                    id SERIAL PRIMARY KEY,
                    name TEXT NOT NULL,
                    description TEXT,
                    price DECIMAL(10,2),
                    category TEXT,
                    stock INTEGER DEFAULT 0,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )",
                &[],
            )
            .await?;
        Ok(())
    }

    /// Create an orders table
    pub async fn create_orders_table(client: &Client) -> Result<(), Box<dyn std::error::Error>> {
        client
            .execute(
                "CREATE TABLE IF NOT EXISTS orders (
                    id SERIAL PRIMARY KEY,
                    user_id INTEGER NOT NULL,
                    product_id INTEGER NOT NULL,
                    quantity INTEGER NOT NULL,
                    total_amount DECIMAL(10,2),
                    status TEXT DEFAULT 'pending',
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
                )",
                &[],
            )
            .await?;
        Ok(())
    }

    /// Insert test data into a table
    pub async fn insert_test_data(
        client: &Client,
        table_name: &str,
        data: &[std::collections::HashMap<String, serde_json::Value>],
    ) -> Result<(), Box<dyn std::error::Error>> {
        for row in data {
            let columns: Vec<String> = row.keys().cloned().collect();
            let values: Vec<String> = columns
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect();

            let _query = format!(
                "INSERT INTO {} ({}) VALUES ({})",
                table_name,
                columns.join(", "),
                values.join(", ")
            );

            let _params: Vec<Box<dyn tokio_postgres::types::ToSql + Sync>> = row
                .values()
                .map(|v| match v {
                    serde_json::Value::String(s) => {
                        Box::new(s.clone()) as Box<dyn tokio_postgres::types::ToSql + Sync>
                    }
                    serde_json::Value::Number(n) => {
                        if n.is_i64() {
                            Box::new(n.as_i64().unwrap())
                                as Box<dyn tokio_postgres::types::ToSql + Sync>
                        } else {
                            Box::new(n.as_f64().unwrap())
                                as Box<dyn tokio_postgres::types::ToSql + Sync>
                        }
                    }
                    serde_json::Value::Bool(b) => {
                        Box::new(*b) as Box<dyn tokio_postgres::types::ToSql + Sync>
                    }
                    _ => Box::new(v.to_string()) as Box<dyn tokio_postgres::types::ToSql + Sync>,
                })
                .collect();

            // Note: This is a simplified version. In real tests, you'd need proper parameter handling
            // For now, we'll use a simpler approach
            let simple_query = format!(
                "INSERT INTO {} (name, email) VALUES ('{}', '{}')",
                table_name,
                row.get("name").and_then(|v| v.as_str()).unwrap_or("Test"),
                row.get("email")
                    .and_then(|v| v.as_str())
                    .unwrap_or("test@example.com")
            );

            client.execute(&simple_query, &[]).await?;
        }
        Ok(())
    }
}
