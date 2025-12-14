use crate::error::Result;
use std::future::Future;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, warn};

/// Retry policy configuration
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_retries: u32,
    pub initial_delay: Duration,
    pub max_delay: Duration,
    pub multiplier: f64,
    pub jitter: bool,
}

impl RetryPolicy {
    pub fn new() -> Self {
        Self {
            max_retries: 3,
            initial_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(60),
            multiplier: 2.0,
            jitter: true,
        }
    }

    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    pub fn with_initial_delay(mut self, delay: Duration) -> Self {
        self.initial_delay = delay;
        self
    }

    pub fn with_max_delay(mut self, delay: Duration) -> Self {
        self.max_delay = delay;
        self
    }

    pub fn with_multiplier(mut self, multiplier: f64) -> Self {
        self.multiplier = multiplier;
        self
    }

    pub fn with_jitter(mut self, jitter: bool) -> Self {
        self.jitter = jitter;
        self
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self::new()
    }
}

/// Exponential backoff calculator
pub struct ExponentialBackoff {
    policy: RetryPolicy,
    current_retry: u32,
    current_delay: Duration,
}

impl ExponentialBackoff {
    pub fn new(policy: RetryPolicy) -> Self {
        Self {
            current_delay: policy.initial_delay,
            policy,
            current_retry: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_retry = 0;
        self.current_delay = self.policy.initial_delay;
    }

    pub fn next_delay(&mut self) -> Option<Duration> {
        if self.current_retry >= self.policy.max_retries {
            return None;
        }

        self.current_retry += 1;

        let mut delay = self.current_delay;

        // Apply jitter if enabled
        if self.policy.jitter {
            use rand::Rng;
            let mut rng = rand::rng();
            let jitter_factor = rng.random_range(0.5..1.5);
            delay = delay.mul_f64(jitter_factor);
        }

        // Calculate next delay for future calls
        self.current_delay = self.current_delay.mul_f64(self.policy.multiplier);
        if self.current_delay > self.policy.max_delay {
            self.current_delay = self.policy.max_delay;
        }

        Some(delay)
    }

    pub fn retries_remaining(&self) -> u32 {
        self.policy.max_retries.saturating_sub(self.current_retry)
    }
}

/// Retry manager for executing operations with retry logic
pub struct RetryManager;

impl RetryManager {
    /// Execute an operation with retry logic
    pub async fn execute_with_retry<F, Fut, T>(
        policy: RetryPolicy,
        operation_name: &str,
        mut operation: F,
    ) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
    {
        let mut backoff = ExponentialBackoff::new(policy);

        loop {
            match operation().await {
                Ok(result) => {
                    if backoff.current_retry > 0 {
                        debug!(
                            "Operation '{}' succeeded after {} retries",
                            operation_name, backoff.current_retry
                        );
                    }
                    return Ok(result);
                }
                Err(error) => {
                    if let Some(delay) = backoff.next_delay() {
                        warn!(
                            "Operation '{}' failed (retry {}/{}), retrying in {:?}: {}",
                            operation_name,
                            backoff.current_retry,
                            backoff.policy.max_retries,
                            delay,
                            error
                        );
                        sleep(delay).await;
                    } else {
                        return Err(error);
                    }
                }
            }
        }
    }

    /// Execute an operation with conditional retry
    pub async fn execute_with_conditional_retry<F, Fut, T, R>(
        policy: RetryPolicy,
        operation_name: &str,
        mut operation: F,
        mut should_retry: R,
    ) -> Result<T>
    where
        F: FnMut() -> Fut,
        Fut: Future<Output = Result<T>>,
        R: FnMut(&crate::error::MeiliBridgeError) -> bool,
    {
        let mut backoff = ExponentialBackoff::new(policy);

        loop {
            match operation().await {
                Ok(result) => {
                    if backoff.current_retry > 0 {
                        debug!(
                            "Operation '{}' succeeded after {} retries",
                            operation_name, backoff.current_retry
                        );
                    }
                    return Ok(result);
                }
                Err(error) => {
                    if !should_retry(&error) {
                        debug!(
                            "Operation '{}' failed with non-retryable error: {}",
                            operation_name, error
                        );
                        return Err(error);
                    }

                    if let Some(delay) = backoff.next_delay() {
                        warn!(
                            "Operation '{}' failed (retry {}/{}), retrying in {:?}: {}",
                            operation_name,
                            backoff.current_retry,
                            backoff.policy.max_retries,
                            delay,
                            error
                        );
                        sleep(delay).await;
                    } else {
                        return Err(error);
                    }
                }
            }
        }
    }
}

/// Retry policies for different scenarios
pub mod policies {
    use super::*;

    /// Fast retry for transient errors
    pub fn fast() -> RetryPolicy {
        RetryPolicy::new()
            .with_max_retries(3)
            .with_initial_delay(Duration::from_millis(50))
            .with_max_delay(Duration::from_secs(1))
            .with_multiplier(2.0)
    }

    /// Standard retry for most operations
    pub fn standard() -> RetryPolicy {
        RetryPolicy::new()
            .with_max_retries(5)
            .with_initial_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(30))
            .with_multiplier(2.0)
    }

    /// Slow retry for rate-limited operations
    pub fn rate_limited() -> RetryPolicy {
        RetryPolicy::new()
            .with_max_retries(10)
            .with_initial_delay(Duration::from_secs(1))
            .with_max_delay(Duration::from_secs(300))
            .with_multiplier(1.5)
    }

    /// Aggressive retry for critical operations
    pub fn aggressive() -> RetryPolicy {
        RetryPolicy::new()
            .with_max_retries(20)
            .with_initial_delay(Duration::from_millis(100))
            .with_max_delay(Duration::from_secs(60))
            .with_multiplier(1.5)
            .with_jitter(true)
    }
}
