use crate::error::MeiliBridgeError;
use std::error::Error;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

type Result<T> = std::result::Result<T, MeiliBridgeError>;

/// Retry configuration
#[derive(Debug, Clone)]
pub struct RetryConfig {
    /// Maximum number of retries
    pub max_retries: usize,
    /// Initial delay between retries (milliseconds)
    pub initial_delay_ms: u64,
    /// Maximum delay between retries (milliseconds)
    pub max_delay_ms: u64,
    /// Exponential backoff multiplier
    pub multiplier: f64,
    /// Whether to use jitter
    pub jitter: bool,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            max_delay_ms: 60000,
            multiplier: 2.0,
            jitter: true,
        }
    }
}

/// Trait for retryable operations
pub trait Retryable {
    /// Check if the error is retryable
    fn is_retryable(&self) -> bool;
}

/// Execute an operation with retries
pub async fn with_retry<F, Fut, T, E>(
    config: &RetryConfig,
    operation_name: &str,
    mut operation: F,
) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = std::result::Result<T, E>>,
    E: std::error::Error + Retryable + Into<crate::error::MeiliBridgeError>,
{
    let mut attempt = 0;
    let mut delay_ms = config.initial_delay_ms;

    loop {
        match operation().await {
            Ok(result) => {
                if attempt > 0 {
                    debug!("{} succeeded after {} retries", operation_name, attempt);
                }
                return Ok(result);
            }
            Err(err) => {
                if !err.is_retryable() || attempt >= config.max_retries {
                    warn!(
                        "{} failed after {} attempts: {}",
                        operation_name,
                        attempt + 1,
                        err
                    );
                    return Err(err.into());
                }

                attempt += 1;

                // Calculate delay with optional jitter
                let mut actual_delay = delay_ms;
                if config.jitter {
                    use rand::Rng;
                    let jitter = rand::rng().random_range(0..=delay_ms / 4);
                    actual_delay = delay_ms.saturating_add(jitter);
                }

                warn!(
                    "{} failed (attempt {}/{}): {}. Retrying in {}ms...",
                    operation_name, attempt, config.max_retries, err, actual_delay
                );

                sleep(Duration::from_millis(actual_delay)).await;

                // Update delay for next retry
                delay_ms = ((delay_ms as f64) * config.multiplier) as u64;
                if delay_ms > config.max_delay_ms {
                    delay_ms = config.max_delay_ms;
                }
            }
        }
    }
}

/// Retry policy for different types of operations
#[derive(Debug, Clone)]
pub enum RetryPolicy {
    /// No retries
    None,
    /// Use default retry configuration
    Default,
    /// Custom retry configuration
    Custom(RetryConfig),
}

impl RetryPolicy {
    /// Get the retry configuration
    pub fn config(&self) -> Option<&RetryConfig> {
        match self {
            RetryPolicy::None => None,
            RetryPolicy::Default => Some(&DEFAULT_RETRY_CONFIG),
            RetryPolicy::Custom(config) => Some(config),
        }
    }
}

lazy_static::lazy_static! {
    static ref DEFAULT_RETRY_CONFIG: RetryConfig = RetryConfig::default();
}

/// Implement Retryable for common error types
impl Retryable for tokio_postgres::Error {
    fn is_retryable(&self) -> bool {
        use tokio_postgres::error::SqlState;

        // Check for retryable SQL states
        if let Some(code) = self.code() {
            matches!(
                code,
                &SqlState::CONNECTION_EXCEPTION
                    | &SqlState::CONNECTION_DOES_NOT_EXIST
                    | &SqlState::CONNECTION_FAILURE
                    | &SqlState::SQLCLIENT_UNABLE_TO_ESTABLISH_SQLCONNECTION
                    | &SqlState::TRANSACTION_ROLLBACK
                    | &SqlState::LOCK_NOT_AVAILABLE
                    | &SqlState::T_R_DEADLOCK_DETECTED
                    | &SqlState::T_R_SERIALIZATION_FAILURE
            )
        } else if self.is_closed() {
            // Connection closed errors are retryable
            true
        } else {
            // IO errors are generally retryable
            self.source()
                .and_then(|e| e.downcast_ref::<std::io::Error>())
                .map(|io_err| {
                    matches!(
                        io_err.kind(),
                        std::io::ErrorKind::ConnectionRefused
                            | std::io::ErrorKind::ConnectionReset
                            | std::io::ErrorKind::ConnectionAborted
                            | std::io::ErrorKind::NotConnected
                            | std::io::ErrorKind::BrokenPipe
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                            | std::io::ErrorKind::UnexpectedEof
                    )
                })
                .unwrap_or(false)
        }
    }
}

impl Retryable for std::io::Error {
    fn is_retryable(&self) -> bool {
        matches!(
            self.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::WouldBlock
        )
    }
}

impl Retryable for crate::error::MeiliBridgeError {
    fn is_retryable(&self) -> bool {
        match self {
            // Network and connection errors are retryable
            crate::error::MeiliBridgeError::Io(_) => true,
            crate::error::MeiliBridgeError::Database(_) => true,
            crate::error::MeiliBridgeError::Meilisearch(msg) => {
                // Meilisearch errors that indicate temporary issues
                msg.contains("timeout")
                    || msg.contains("connection")
                    || msg.contains("unavailable")
                    || msg.contains("Too Many Requests")
            }
            // Configuration and validation errors are not retryable
            crate::error::MeiliBridgeError::Configuration(_) => false,
            crate::error::MeiliBridgeError::Validation(_) => false,
            // Generic errors might be retryable
            _ => false,
        }
    }
}
