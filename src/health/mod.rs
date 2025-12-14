use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Health status of a component
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

/// Health check result for a component
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckResult {
    pub status: HealthStatus,
    pub message: Option<String>,
    pub details: HashMap<String, serde_json::Value>,
    pub last_check: chrono::DateTime<chrono::Utc>,
}

impl HealthCheckResult {
    pub fn healthy() -> Self {
        Self {
            status: HealthStatus::Healthy,
            message: None,
            details: HashMap::new(),
            last_check: chrono::Utc::now(),
        }
    }

    pub fn unhealthy(message: String) -> Self {
        Self {
            status: HealthStatus::Unhealthy,
            message: Some(message),
            details: HashMap::new(),
            last_check: chrono::Utc::now(),
        }
    }

    pub fn degraded(message: String) -> Self {
        Self {
            status: HealthStatus::Degraded,
            message: Some(message),
            details: HashMap::new(),
            last_check: chrono::Utc::now(),
        }
    }

    pub fn with_detail(mut self, key: &str, value: serde_json::Value) -> Self {
        self.details.insert(key.to_string(), value);
        self
    }
}

/// Overall system health
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub status: HealthStatus,
    pub components: HashMap<String, HealthCheckResult>,
    pub version: String,
    pub uptime_seconds: u64,
}

/// Trait for components that can report health
#[async_trait::async_trait]
pub trait HealthCheck: Send + Sync {
    /// Perform health check
    async fn check_health(&self) -> HealthCheckResult;

    /// Get component name
    fn component_name(&self) -> &'static str;
}

/// Health check registry
pub struct HealthRegistry {
    components: Arc<RwLock<HashMap<String, Box<dyn HealthCheck>>>>,
    start_time: std::time::Instant,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self {
            components: Arc::new(RwLock::new(HashMap::new())),
            start_time: std::time::Instant::now(),
        }
    }
}

impl Default for HealthRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl HealthRegistry {
    /// Register a health check component
    pub async fn register(&self, component: Box<dyn HealthCheck>) {
        let name = component.component_name().to_string();
        let mut components = self.components.write().await;
        components.insert(name, component);
    }

    /// Get overall system health
    pub async fn get_system_health(&self) -> SystemHealth {
        let components = self.components.read().await;
        let mut health_results = HashMap::new();
        let mut overall_status = HealthStatus::Healthy;

        // Check all components
        for (name, component) in components.iter() {
            let result = component.check_health().await;

            // Update overall status based on component status
            match result.status {
                HealthStatus::Unhealthy => overall_status = HealthStatus::Unhealthy,
                HealthStatus::Degraded => {
                    if overall_status == HealthStatus::Healthy {
                        overall_status = HealthStatus::Degraded;
                    }
                }
                HealthStatus::Healthy => {}
            }

            health_results.insert(name.clone(), result);
        }

        SystemHealth {
            status: overall_status,
            components: health_results,
            version: env!("CARGO_PKG_VERSION").to_string(),
            uptime_seconds: self.start_time.elapsed().as_secs(),
        }
    }

    /// Get health of a specific component
    pub async fn get_component_health(&self, name: &str) -> Option<HealthCheckResult> {
        let components = self.components.read().await;
        if let Some(component) = components.get(name) {
            Some(component.check_health().await)
        } else {
            None
        }
    }
}

/// PostgreSQL health check
pub struct PostgresHealthCheck {
    connector: Arc<RwLock<crate::source::postgres::PostgresConnector>>,
}

impl PostgresHealthCheck {
    pub fn new(connector: Arc<RwLock<crate::source::postgres::PostgresConnector>>) -> Self {
        Self { connector }
    }
}

#[async_trait::async_trait]
impl HealthCheck for PostgresHealthCheck {
    async fn check_health(&self) -> HealthCheckResult {
        // Try to ensure connection first
        {
            let mut connector = self.connector.write().await;
            if let Err(e) = connector.ensure_connected().await {
                return HealthCheckResult::unhealthy(format!("Failed to connect: {}", e));
            }
        }

        // Now check health with read lock
        let connector = self.connector.read().await;

        // Try to get a client from the pool
        match connector.get_client().await {
            Ok(client) => {
                // Execute a simple query to verify connection
                match client.simple_query("SELECT 1").await {
                    Ok(_) => {
                        let pool = connector.get_pool();
                        HealthCheckResult::healthy()
                            .with_detail("pool_size", serde_json::json!(pool.status().size))
                            .with_detail(
                                "pool_available",
                                serde_json::json!(pool.status().available),
                            )
                    }
                    Err(e) => HealthCheckResult::unhealthy(format!("Query failed: {}", e)),
                }
            }
            Err(e) => HealthCheckResult::unhealthy(format!("Failed to get client: {}", e)),
        }
    }

    fn component_name(&self) -> &'static str {
        "postgresql"
    }
}

/// Meilisearch health check
pub struct MeilisearchHealthCheck {
    config: crate::config::MeilisearchConfig,
}

impl MeilisearchHealthCheck {
    pub fn new(config: crate::config::MeilisearchConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl HealthCheck for MeilisearchHealthCheck {
    async fn check_health(&self) -> HealthCheckResult {
        let client = match meilisearch_sdk::client::Client::new(
            &self.config.url,
            self.config.api_key.as_deref(),
        ) {
            Ok(client) => client,
            Err(e) => {
                return HealthCheckResult::unhealthy(format!("Failed to create client: {}", e))
            }
        };

        // Check Meilisearch health endpoint
        match client.health().await {
            Ok(health) => {
                HealthCheckResult::healthy().with_detail("status", serde_json::json!(health.status))
            }
            Err(e) => HealthCheckResult::unhealthy(format!("Health check failed: {}", e)),
        }
    }

    fn component_name(&self) -> &'static str {
        "meilisearch"
    }
}

/// Redis health check
pub struct RedisHealthCheck {
    url: String,
}

impl RedisHealthCheck {
    pub fn new(url: String) -> Self {
        Self { url }
    }
}

#[async_trait::async_trait]
impl HealthCheck for RedisHealthCheck {
    async fn check_health(&self) -> HealthCheckResult {
        // Try to connect to Redis
        match redis::Client::open(self.url.as_str()) {
            Ok(client) => {
                match client.get_multiplexed_async_connection().await {
                    Ok(mut conn) => {
                        // Execute a PING command
                        let ping_result: Result<String, redis::RedisError> =
                            redis::cmd("PING").query_async(&mut conn).await;

                        match ping_result {
                            Ok(response) if response == "PONG" => {
                                // Try to get server info for more details
                                let info_result: Result<String, redis::RedisError> =
                                    redis::cmd("INFO")
                                        .arg("server")
                                        .query_async(&mut conn)
                                        .await;

                                let mut result = HealthCheckResult::healthy();

                                if let Ok(info) = info_result {
                                    // Parse Redis version from info
                                    if let Some(version_line) =
                                        info.lines().find(|l| l.starts_with("redis_version:"))
                                    {
                                        if let Some(version) = version_line.split(':').nth(1) {
                                            result = result.with_detail(
                                                "version",
                                                serde_json::json!(version.trim()),
                                            );
                                        }
                                    }
                                }

                                result
                            }
                            Ok(_) => {
                                HealthCheckResult::degraded("Unexpected PING response".to_string())
                            }
                            Err(e) => HealthCheckResult::unhealthy(format!("PING failed: {}", e)),
                        }
                    }
                    Err(e) => HealthCheckResult::unhealthy(format!("Connection failed: {}", e)),
                }
            }
            Err(e) => HealthCheckResult::unhealthy(format!("Failed to create client: {}", e)),
        }
    }

    fn component_name(&self) -> &'static str {
        "redis"
    }
}

/// API server health check
pub struct ApiHealthCheck {
    port: u16,
}

impl ApiHealthCheck {
    pub fn new(port: u16) -> Self {
        Self { port }
    }
}

#[async_trait::async_trait]
impl HealthCheck for ApiHealthCheck {
    async fn check_health(&self) -> HealthCheckResult {
        // Check if API server is responding
        let url = format!("http://127.0.0.1:{}/health", self.port);

        match reqwest::get(&url).await {
            Ok(response) => {
                if response.status().is_success() {
                    HealthCheckResult::healthy().with_detail("port", serde_json::json!(self.port))
                } else {
                    HealthCheckResult::degraded(format!(
                        "API returned status: {}",
                        response.status()
                    ))
                }
            }
            Err(e) => {
                // API might not be accessible locally, but that doesn't mean it's unhealthy
                HealthCheckResult::degraded(format!("Could not reach API: {}", e))
                    .with_detail("port", serde_json::json!(self.port))
            }
        }
    }

    fn component_name(&self) -> &'static str {
        "api"
    }
}
