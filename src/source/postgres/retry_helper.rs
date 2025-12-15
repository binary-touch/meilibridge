use crate::error::MeiliBridgeError;
use crate::error::retry::{RetryConfig, Retryable, with_retry};
use deadpool_postgres::Pool;
use tokio_postgres::Client;
use tracing::debug;

/// Helper functions for PostgreSQL operations with retry logic
pub struct PostgresRetryHelper;

impl PostgresRetryHelper {
    /// Execute a query with retry logic
    pub async fn query_with_retry(
        client: &Client,
        query: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
        operation_name: &str,
    ) -> Result<Vec<tokio_postgres::Row>, MeiliBridgeError> {
        let config = RetryConfig::default();

        with_retry(&config, operation_name, || async {
            client
                .query(query, params)
                .await
                .map_err(PostgresRetryError::Postgres)
        })
        .await
    }

    /// Get a client from pool with retry logic
    pub async fn get_client_with_retry(
        pool: &Pool,
        operation_name: &str,
    ) -> Result<deadpool_postgres::Object, MeiliBridgeError> {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 500,
            max_delay_ms: 10000,
            multiplier: 2.0,
            jitter: true,
        };

        with_retry(&config, operation_name, || async {
            pool.get().await.map_err(|e| match e {
                deadpool_postgres::PoolError::Backend(backend_err) => {
                    PostgresRetryError::Postgres(backend_err)
                }
                e => PostgresRetryError::Pool(e.to_string()),
            })
        })
        .await
    }

    /// Execute a transaction with retry logic
    pub async fn transaction_with_retry<F, Fut, T>(
        pool: &Pool,
        operation_name: &str,
        transaction_fn: F,
    ) -> Result<T, MeiliBridgeError>
    where
        F: FnMut(&mut tokio_postgres::Transaction<'_>) -> Fut + Clone,
        Fut: std::future::Future<Output = Result<T, tokio_postgres::Error>>,
    {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 30000,
            multiplier: 2.0,
            jitter: true,
        };

        let transaction_fn = transaction_fn.clone();
        with_retry(&config, operation_name, move || {
            let mut transaction_fn = transaction_fn.clone();
            async move {
                let mut client = pool.get().await.map_err(|e| match e {
                    deadpool_postgres::PoolError::Backend(backend_err) => {
                        PostgresRetryError::Postgres(backend_err)
                    }
                    e => PostgresRetryError::Pool(e.to_string()),
                })?;

                let mut transaction = client
                    .transaction()
                    .await
                    .map_err(PostgresRetryError::Postgres)?;

                match transaction_fn(&mut transaction).await {
                    Ok(result) => {
                        transaction
                            .commit()
                            .await
                            .map_err(PostgresRetryError::Postgres)?;
                        Ok(result)
                    }
                    Err(e) => {
                        // Transaction will be rolled back automatically on drop
                        debug!("Transaction failed, will be rolled back: {}", e);
                        Err(PostgresRetryError::Postgres(e))
                    }
                }
            }
        })
        .await
    }
}

/// Wrapper for PostgreSQL errors to implement Retryable
#[derive(Debug)]
enum PostgresRetryError {
    Postgres(tokio_postgres::Error),
    Pool(String),
}

impl std::fmt::Display for PostgresRetryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PostgresRetryError::Postgres(e) => write!(f, "{e}"),
            PostgresRetryError::Pool(msg) => write!(f, "Pool error: {msg}"),
        }
    }
}

impl std::error::Error for PostgresRetryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            PostgresRetryError::Postgres(e) => e.source(),
            PostgresRetryError::Pool(_) => None,
        }
    }
}

impl Retryable for PostgresRetryError {
    fn is_retryable(&self) -> bool {
        match self {
            PostgresRetryError::Postgres(e) => e.is_retryable(),
            PostgresRetryError::Pool(_) => true, // Pool errors are generally retryable
        }
    }
}

impl From<PostgresRetryError> for MeiliBridgeError {
    fn from(err: PostgresRetryError) -> Self {
        MeiliBridgeError::Database(err.to_string())
    }
}
