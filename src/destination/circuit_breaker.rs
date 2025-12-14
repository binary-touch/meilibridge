use circuit_breaker::{CircuitBreaker as ExternalCircuitBreaker, CircuitState as ExternalState};
use futures::future::BoxFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, warn};

/// Circuit breaker wrapper for Meilisearch operations
pub struct MeilisearchCircuitBreaker {
    inner: Arc<Mutex<ExternalCircuitBreaker>>,
    name: String,
}

impl MeilisearchCircuitBreaker {
    /// Create a new circuit breaker with default configuration
    pub fn new(name: String) -> Self {
        let circuit_breaker = ExternalCircuitBreaker::new(
            5,                       // failure_threshold: 5 consecutive failures
            Duration::from_secs(60), // reset_timeout: 60 seconds
        );

        Self {
            inner: Arc::new(Mutex::new(circuit_breaker)),
            name,
        }
    }

    /// Create a circuit breaker with custom configuration
    pub fn with_config(name: String, failure_threshold: u32, reset_timeout: Duration) -> Self {
        let circuit_breaker = ExternalCircuitBreaker::new(failure_threshold, reset_timeout);

        Self {
            inner: Arc::new(Mutex::new(circuit_breaker)),
            name,
        }
    }

    /// Execute an async operation with circuit breaker protection
    pub async fn call<F, T, E>(&self, op: F) -> Result<T, CircuitBreakerError<E>>
    where
        F: FnOnce() -> BoxFuture<'static, Result<T, E>>,
        E: std::fmt::Debug,
    {
        let cb = self.inner.lock().await;

        // Check circuit state first
        let state = cb.state();
        debug!("Circuit breaker '{}' state: {:?}", self.name, state);

        match state {
            ExternalState::Open => {
                error!("Circuit breaker '{}' is open, rejecting request", self.name);

                // Update metrics
                crate::metrics::CIRCUIT_BREAKER_CALLS
                    .with_label_values(&[self.name.as_str(), "rejected"])
                    .inc();

                Err(CircuitBreakerError::CircuitOpen)
            }
            _ => {
                // Circuit is closed or half-open, allow the call
                drop(cb); // Release the lock before executing the operation

                debug!("Circuit breaker '{}' allowing request", self.name);

                // Execute the operation
                let result = op().await;

                // Handle result
                let cb = self.inner.lock().await;
                match &result {
                    Ok(_) => {
                        cb.handle_success();
                        debug!("Circuit breaker '{}' recorded success", self.name);

                        // Update metrics
                        crate::metrics::CIRCUIT_BREAKER_CALLS
                            .with_label_values(&[self.name.as_str(), "success"])
                            .inc();
                    }
                    Err(e) => {
                        cb.handle_failure();
                        warn!("Circuit breaker '{}' recorded error: {:?}", self.name, e);

                        // Update metrics
                        crate::metrics::CIRCUIT_BREAKER_CALLS
                            .with_label_values(&[self.name.as_str(), "failure"])
                            .inc();
                    }
                }

                result.map_err(CircuitBreakerError::OperationError)
            }
        }
    }

    /// Get the current state of the circuit breaker
    pub async fn get_state(&self) -> CircuitState {
        let cb = self.inner.lock().await;

        match cb.state() {
            ExternalState::Closed => CircuitState::Closed,
            ExternalState::Open => CircuitState::Open,
            ExternalState::HalfOpen => CircuitState::HalfOpen,
        }
    }

    /// Get circuit breaker statistics
    pub async fn get_stats(&self) -> CircuitBreakerStats {
        let cb = self.inner.lock().await;
        let state = cb.state();

        CircuitBreakerStats {
            state: match state {
                ExternalState::Closed => CircuitState::Closed,
                ExternalState::Open => CircuitState::Open,
                ExternalState::HalfOpen => CircuitState::HalfOpen,
            },
            name: self.name.clone(),
        }
    }
}

/// Circuit breaker states
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Circuit is closed, requests pass through normally
    Closed,
    /// Circuit is open, requests are rejected
    Open,
    /// Circuit is testing if the service has recovered
    HalfOpen,
}

/// Circuit breaker error types
#[derive(Debug, thiserror::Error)]
pub enum CircuitBreakerError<E> {
    #[error("Circuit breaker is open")]
    CircuitOpen,

    #[error("Operation failed: {0:?}")]
    OperationError(E),
}

/// Circuit breaker statistics
#[derive(Debug, Clone)]
pub struct CircuitBreakerStats {
    pub state: CircuitState,
    pub name: String,
}

/// Builder for circuit breaker configuration
pub struct CircuitBreakerBuilder {
    name: String,
    failure_threshold: u32,
    reset_timeout: Duration,
}

impl CircuitBreakerBuilder {
    pub fn new(name: String) -> Self {
        Self {
            name,
            failure_threshold: 5,
            reset_timeout: Duration::from_secs(60),
        }
    }

    pub fn error_rate(self, _rate: f64) -> Self {
        // Note: circuit_breaker 0.1.1 doesn't support error rate, only consecutive failures
        warn!("Error rate configuration not supported in circuit_breaker 0.1.1, using consecutive failures instead");
        self
    }

    pub fn min_request_count(self, _count: u64) -> Self {
        // Note: circuit_breaker 0.1.1 doesn't support min request count
        warn!("Min request count configuration not supported in circuit_breaker 0.1.1");
        self
    }

    pub fn consecutive_failures(mut self, count: u64) -> Self {
        self.failure_threshold = count as u32;
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.reset_timeout = timeout;
        self
    }

    pub fn build(self) -> MeilisearchCircuitBreaker {
        MeilisearchCircuitBreaker::with_config(
            self.name,
            self.failure_threshold,
            self.reset_timeout,
        )
    }
}
