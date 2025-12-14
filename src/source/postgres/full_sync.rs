use crate::error::Result;
use crate::source::postgres::PostgresConnector;
use serde_json::Value;
use tokio::sync::mpsc;
use tracing::{debug, info};

/// Handles full table synchronization with statement caching
pub struct FullSyncHandler {
    connector: PostgresConnector,
}

impl FullSyncHandler {
    pub fn new(connector: PostgresConnector) -> Self {
        Self { connector }
    }

    /// Perform full table sync with prepared statement caching
    pub async fn sync_table(
        &self,
        table: &str,
        batch_size: usize,
        tx: mpsc::Sender<Result<Value>>,
    ) -> Result<()> {
        info!("Starting full sync for table: {}", table);

        // Get a cached connection
        let cached_conn = self.connector.get_cached_connection().await?;

        // Get total count first (using cached statement)
        let count_query = format!("SELECT COUNT(*) FROM {}", table);
        let count_row = cached_conn.query_one::<()>(&count_query, &[]).await?;
        let total_count: i64 = count_row.get(0);

        info!("Total rows to sync for {}: {}", table, total_count);

        // Prepare the main query once (will be cached)
        let select_query = format!("SELECT row_to_json(t) FROM {} t LIMIT $1 OFFSET $2", table);

        let mut offset = 0i64;
        let mut synced_count = 0i64;

        while offset < total_count {
            // Execute the cached prepared statement
            let rows = cached_conn
                .query::<()>(&select_query, &[&(batch_size as i64), &offset])
                .await?;

            if rows.is_empty() {
                break;
            }

            for row in rows {
                let json: Value = row.get(0);
                if tx.send(Ok(json)).await.is_err() {
                    debug!("Receiver dropped, stopping sync");
                    return Ok(());
                }
                synced_count += 1;
            }

            offset += batch_size as i64;

            // Log progress periodically
            if synced_count % 10000 == 0 {
                info!(
                    "Synced {}/{} rows from {} ({:.1}%)",
                    synced_count,
                    total_count,
                    table,
                    (synced_count as f64 / total_count as f64) * 100.0
                );
            }
        }

        // Export cache statistics
        let stats = cached_conn.cache_stats().await;
        info!(
            "Full sync completed for {}. Synced {} rows. Cache stats: {} hits, {} misses, {:.2}% hit rate",
            table,
            synced_count,
            stats.hits,
            stats.misses,
            stats.hit_rate * 100.0
        );

        // Export metrics
        stats.export_metrics();

        Ok(())
    }

    /// Check if a table exists using cached statement
    pub async fn table_exists(&self, table: &str) -> Result<bool> {
        let cached_conn = self.connector.get_cached_connection().await?;

        let query = r#"
            SELECT EXISTS (
                SELECT 1 
                FROM information_schema.tables 
                WHERE table_schema = 'public' 
                AND table_name = $1
            )
        "#;

        let row = cached_conn.query_one::<()>(query, &[&table]).await?;
        let exists: bool = row.get(0);

        Ok(exists)
    }

    /// Get table columns using cached statement
    pub async fn get_table_columns(&self, table: &str) -> Result<Vec<String>> {
        let cached_conn = self.connector.get_cached_connection().await?;

        let query = r#"
            SELECT column_name 
            FROM information_schema.columns 
            WHERE table_schema = 'public' 
            AND table_name = $1 
            ORDER BY ordinal_position
        "#;

        let rows = cached_conn.query::<()>(query, &[&table]).await?;

        let columns: Vec<String> = rows.into_iter().map(|row| row.get(0)).collect();

        Ok(columns)
    }
}
