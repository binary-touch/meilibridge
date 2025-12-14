// Retry logic tests

use meilibridge::error::MeiliBridgeError;
use meilibridge::error::retry::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

#[cfg(test)]
mod retry_tests {
    use super::*;

    // Test error type that implements Retryable
    #[derive(Debug)]
    struct TestError {
        retryable: bool,
        message: String,
    }

    impl std::fmt::Display for TestError {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.message)
        }
    }

    impl std::error::Error for TestError {}

    impl Retryable for TestError {
        fn is_retryable(&self) -> bool {
            self.retryable
        }
    }

    impl From<TestError> for MeiliBridgeError {
        fn from(err: TestError) -> Self {
            MeiliBridgeError::Source(err.message)
        }
    }

    #[test]
    fn test_retry_config_default() {
        let config = RetryConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.initial_delay_ms, 1000);
        assert_eq!(config.max_delay_ms, 60000);
        assert_eq!(config.multiplier, 2.0);
        assert!(config.jitter);
    }

    #[test]
    fn test_retry_policy() {
        // None policy
        let none_policy = RetryPolicy::None;
        assert!(none_policy.config().is_none());

        // Default policy
        let default_policy = RetryPolicy::Default;
        let config = default_policy.config().unwrap();
        assert_eq!(config.max_retries, 3);

        // Custom policy
        let custom_config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 500,
            max_delay_ms: 30000,
            multiplier: 1.5,
            jitter: false,
        };
        let custom_policy = RetryPolicy::Custom(custom_config.clone());
        let config = custom_policy.config().unwrap();
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.initial_delay_ms, 500);
    }

    #[tokio::test]
    async fn test_successful_operation() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            multiplier: 2.0,
            jitter: false,
        };

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let counter = attempt_count.clone();

        let result = with_retry(&config, "test_operation", || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Ok::<String, TestError>("Success".to_string())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success");
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_retry_with_eventual_success() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            multiplier: 2.0,
            jitter: false,
        };

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let counter = attempt_count.clone();
        let start = Instant::now();

        let result = with_retry(&config, "test_operation", || {
            let counter = counter.clone();
            async move {
                let count = counter.fetch_add(1, Ordering::SeqCst) + 1;
                if count < 3 {
                    Err(TestError {
                        retryable: true,
                        message: "Temporary failure".to_string(),
                    })
                } else {
                    Ok("Success".to_string())
                }
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Success");
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3);

        // Should have delays: 10ms + 20ms = 30ms minimum
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(30));
    }

    #[tokio::test]
    async fn test_non_retryable_error() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            multiplier: 2.0,
            jitter: false,
        };

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let counter = attempt_count.clone();

        let result = with_retry(&config, "test_operation", || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<String, TestError>(TestError {
                    retryable: false,
                    message: "Non-retryable error".to_string(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 1); // Should not retry
    }

    #[tokio::test]
    async fn test_max_retries_exhaustion() {
        let config = RetryConfig {
            max_retries: 2,
            initial_delay_ms: 10,
            max_delay_ms: 100,
            multiplier: 2.0,
            jitter: false,
        };

        let attempt_count = Arc::new(AtomicUsize::new(0));
        let counter = attempt_count.clone();

        let result = with_retry(&config, "test_operation", || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                Err::<String, TestError>(TestError {
                    retryable: true,
                    message: "Always fails".to_string(),
                })
            }
        })
        .await;

        assert!(result.is_err());
        assert_eq!(attempt_count.load(Ordering::SeqCst), 3); // Initial + 2 retries
    }

    #[tokio::test]
    async fn test_exponential_backoff() {
        let config = RetryConfig {
            max_retries: 3,
            initial_delay_ms: 10,
            max_delay_ms: 1000,
            multiplier: 2.0,
            jitter: false,
        };

        let delays = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();
        let last_attempt = Arc::new(std::sync::Mutex::new(Instant::now()));

        let _ = with_retry(&config, "test_operation", || {
            let delays = delays_clone.clone();
            let last = last_attempt.clone();
            async move {
                let now = Instant::now();
                let mut last_time = last.lock().unwrap();
                let delay = now.duration_since(*last_time);
                if delay > Duration::from_millis(1) {
                    delays.lock().unwrap().push(delay);
                }
                *last_time = now;
                drop(last_time);

                Err::<String, TestError>(TestError {
                    retryable: true,
                    message: "Always fails".to_string(),
                })
            }
        })
        .await;

        let recorded_delays = delays.lock().unwrap();
        assert!(recorded_delays.len() >= 2);

        // First delay should be ~10ms (allow more tolerance in CI)
        assert!(recorded_delays[0] >= Duration::from_millis(8));
        assert!(recorded_delays[0] <= Duration::from_millis(20));

        // Second delay should be ~20ms (allow more tolerance in CI)
        if recorded_delays.len() > 1 {
            assert!(recorded_delays[1] >= Duration::from_millis(15));
            assert!(recorded_delays[1] <= Duration::from_millis(30));
        }
    }

    #[tokio::test]
    async fn test_max_delay_cap() {
        let config = RetryConfig {
            max_retries: 10,
            initial_delay_ms: 1000,
            max_delay_ms: 2000,
            multiplier: 10.0,
            jitter: false,
        };

        let delays = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();
        let last_attempt = Arc::new(std::sync::Mutex::new(Instant::now()));

        let _ = with_retry(&config, "test_operation", || {
            let delays = delays_clone.clone();
            let last = last_attempt.clone();
            async move {
                let now = Instant::now();
                let mut last_time = last.lock().unwrap();
                let delay = now.duration_since(*last_time);
                if delay > Duration::from_millis(1) {
                    delays.lock().unwrap().push(delay);
                }
                *last_time = now;
                drop(last_time);

                Err::<String, TestError>(TestError {
                    retryable: true,
                    message: "Always fails".to_string(),
                })
            }
        })
        .await;

        let recorded_delays = delays.lock().unwrap();

        // All delays after the first should be capped at max_delay_ms
        for delay in recorded_delays.iter().skip(1) {
            assert!(*delay <= Duration::from_millis(2100)); // Allow small overhead
        }
    }

    #[test]
    fn test_io_error_retryable() {
        use std::io::{Error, ErrorKind};

        // Retryable IO errors
        assert!(Error::new(ErrorKind::ConnectionRefused, "").is_retryable());
        assert!(Error::new(ErrorKind::ConnectionReset, "").is_retryable());
        assert!(Error::new(ErrorKind::ConnectionAborted, "").is_retryable());
        assert!(Error::new(ErrorKind::NotConnected, "").is_retryable());
        assert!(Error::new(ErrorKind::BrokenPipe, "").is_retryable());
        assert!(Error::new(ErrorKind::TimedOut, "").is_retryable());
        assert!(Error::new(ErrorKind::Interrupted, "").is_retryable());
        assert!(Error::new(ErrorKind::UnexpectedEof, "").is_retryable());
        assert!(Error::new(ErrorKind::WouldBlock, "").is_retryable());

        // Non-retryable IO errors
        assert!(!Error::new(ErrorKind::PermissionDenied, "").is_retryable());
        assert!(!Error::new(ErrorKind::InvalidInput, "").is_retryable());
        assert!(!Error::new(ErrorKind::InvalidData, "").is_retryable());
        assert!(!Error::new(ErrorKind::NotFound, "").is_retryable());
    }

    #[test]
    fn test_meilibridge_error_retryable() {
        // Retryable MeiliBridge errors
        assert!(
            MeiliBridgeError::Io(std::io::Error::new(
                std::io::ErrorKind::ConnectionRefused,
                "Connection refused"
            ))
            .is_retryable()
        );

        assert!(MeiliBridgeError::Database("connection error".to_string()).is_retryable());

        assert!(MeiliBridgeError::Meilisearch("timeout occurred".to_string()).is_retryable());
        assert!(MeiliBridgeError::Meilisearch("connection failed".to_string()).is_retryable());
        assert!(MeiliBridgeError::Meilisearch("service unavailable".to_string()).is_retryable());
        assert!(MeiliBridgeError::Meilisearch("429 Too Many Requests".to_string()).is_retryable());

        // Non-retryable MeiliBridge errors
        assert!(!MeiliBridgeError::Configuration("invalid config".to_string()).is_retryable());
        assert!(!MeiliBridgeError::Validation("invalid data".to_string()).is_retryable());
        assert!(!MeiliBridgeError::Meilisearch("invalid API key".to_string()).is_retryable());
    }

    #[tokio::test]
    async fn test_jitter() {
        let config = RetryConfig {
            max_retries: 5,
            initial_delay_ms: 100,
            max_delay_ms: 1000,
            multiplier: 1.0, // Keep delay constant to test jitter
            jitter: true,
        };

        let delays = Arc::new(std::sync::Mutex::new(Vec::new()));
        let delays_clone = delays.clone();
        let last_attempt = Arc::new(std::sync::Mutex::new(Instant::now()));

        let _ = with_retry(&config, "test_operation", || {
            let delays = delays_clone.clone();
            let last = last_attempt.clone();
            async move {
                let now = Instant::now();
                let mut last_time = last.lock().unwrap();
                let delay = now.duration_since(*last_time);
                if delay > Duration::from_millis(1) {
                    delays.lock().unwrap().push(delay);
                }
                *last_time = now;
                drop(last_time);

                Err::<String, TestError>(TestError {
                    retryable: true,
                    message: "Always fails".to_string(),
                })
            }
        })
        .await;

        let recorded_delays = delays.lock().unwrap();

        // With jitter, delays should vary around the base delay (100ms)
        // Jitter typically adds some variation
        for delay in recorded_delays.iter() {
            // Allow for more variation in CI environments
            assert!(*delay >= Duration::from_millis(50)); // Allow up to 50% less
            assert!(*delay <= Duration::from_millis(200)); // Allow up to 100% more
        }

        // Check that not all delays are exactly the same (jitter is working)
        if recorded_delays.len() > 1 {
            let all_same = recorded_delays.windows(2).all(|w| w[0] == w[1]);
            assert!(
                !all_same,
                "All delays are the same, jitter might not be working"
            );
        }
    }
}
