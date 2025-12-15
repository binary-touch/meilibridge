use crate::error::{MeiliBridgeError, Result};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_postgres::{Client, Statement};
use tracing::{debug, warn};

/// Configuration for the statement cache
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum number of statements to cache
    pub max_size: usize,
    /// Whether to enable the cache
    pub enabled: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size: 100,
            enabled: true,
        }
    }
}

/// A cache for prepared statements to improve query performance
pub struct StatementCache {
    cache: Arc<RwLock<HashMap<String, Statement>>>,
    config: CacheConfig,
    metrics: CacheMetrics,
}

#[derive(Debug, Default)]
struct CacheMetrics {
    hits: Arc<RwLock<u64>>,
    misses: Arc<RwLock<u64>>,
    evictions: Arc<RwLock<u64>>,
}

impl StatementCache {
    /// Create a new statement cache with the given configuration
    pub fn new(config: CacheConfig) -> Self {
        Self {
            cache: Arc::new(RwLock::new(HashMap::new())),
            config,
            metrics: CacheMetrics::default(),
        }
    }

    /// Get or prepare a statement
    pub async fn get_or_prepare(&self, client: &Client, query: &str) -> Result<Statement> {
        if !self.config.enabled {
            // Cache disabled, prepare directly
            return client.prepare(query).await.map_err(|e| {
                MeiliBridgeError::Source(format!("Failed to prepare statement: {e}"))
            });
        }

        // Check cache first
        {
            let cache = self.cache.read().await;
            if let Some(stmt) = cache.get(query) {
                *self.metrics.hits.write().await += 1;
                debug!("Statement cache hit for query: {}", truncate_query(query));
                return Ok(stmt.clone());
            }
        }

        // Cache miss, prepare the statement
        *self.metrics.misses.write().await += 1;
        debug!("Statement cache miss for query: {}", truncate_query(query));

        let stmt = client
            .prepare(query)
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Failed to prepare statement: {e}")))?;

        // Add to cache
        let mut cache = self.cache.write().await;

        // Check if we need to evict
        if cache.len() >= self.config.max_size {
            // Simple eviction: remove the first entry (oldest)
            if let Some(key) = cache.keys().next().cloned() {
                cache.remove(&key);
                *self.metrics.evictions.write().await += 1;
                warn!("Statement cache full, evicted oldest entry");
            }
        }

        cache.insert(query.to_string(), stmt.clone());
        Ok(stmt)
    }

    /// Clear all cached statements
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let count = cache.len();
        cache.clear();
        debug!("Cleared {} statements from cache", count);
    }

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheStats {
        CacheStats {
            size: self.cache.read().await.len(),
            hits: *self.metrics.hits.read().await,
            misses: *self.metrics.misses.read().await,
            evictions: *self.metrics.evictions.read().await,
            hit_rate: calculate_hit_rate(
                *self.metrics.hits.read().await,
                *self.metrics.misses.read().await,
            ),
        }
    }

    /// Remove a specific statement from the cache
    pub async fn invalidate(&self, query: &str) {
        let mut cache = self.cache.write().await;
        if cache.remove(query).is_some() {
            debug!(
                "Invalidated cached statement for query: {}",
                truncate_query(query)
            );
        }
    }

    /// Check if a statement is cached
    pub async fn contains(&self, query: &str) -> bool {
        self.cache.read().await.contains_key(query)
    }

    /// Get the maximum cache size configuration
    pub fn max_size(&self) -> usize {
        self.config.max_size
    }
}

/// Statistics for the statement cache
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub size: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub hit_rate: f64,
}

impl CacheStats {
    /// Export metrics to Prometheus
    pub fn export_metrics(&self) {
        crate::metrics::STATEMENT_CACHE_SIZE.set(self.size as i64);
        crate::metrics::STATEMENT_CACHE_HITS.inc_by(self.hits as f64);
        crate::metrics::STATEMENT_CACHE_MISSES.inc_by(self.misses as f64);
        crate::metrics::STATEMENT_CACHE_EVICTIONS.inc_by(self.evictions as f64);
        crate::metrics::STATEMENT_CACHE_HIT_RATE.set(self.hit_rate);
    }
}

/// Helper function to calculate hit rate
fn calculate_hit_rate(hits: u64, misses: u64) -> f64 {
    let total = hits + misses;
    if total == 0 {
        0.0
    } else {
        (hits as f64) / (total as f64)
    }
}

/// Truncate long queries for logging
fn truncate_query(query: &str) -> &str {
    const MAX_LENGTH: usize = 100;
    if query.len() <= MAX_LENGTH {
        query
    } else {
        &query[..MAX_LENGTH]
    }
}

/// A connection wrapper that includes a statement cache
pub struct CachedConnection {
    client: Arc<Client>,
    cache: Arc<StatementCache>,
}

impl CachedConnection {
    pub fn new(client: Client, cache: Arc<StatementCache>) -> Self {
        Self {
            client: Arc::new(client),
            cache,
        }
    }

    /// Execute a query with prepared statement caching
    pub async fn query<T>(
        &self,
        query: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Vec<tokio_postgres::Row>>
    where
        T: ?Sized,
    {
        let stmt = self.cache.get_or_prepare(&self.client, query).await?;
        self.client
            .query(&stmt, params)
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Query execution failed: {e}")))
    }

    /// Execute a query that returns at most one row
    pub async fn query_opt<T>(
        &self,
        query: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<Option<tokio_postgres::Row>>
    where
        T: ?Sized,
    {
        let stmt = self.cache.get_or_prepare(&self.client, query).await?;
        self.client
            .query_opt(&stmt, params)
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Query execution failed: {e}")))
    }

    /// Execute a query that returns exactly one row
    pub async fn query_one<T>(
        &self,
        query: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<tokio_postgres::Row>
    where
        T: ?Sized,
    {
        let stmt = self.cache.get_or_prepare(&self.client, query).await?;
        self.client
            .query_one(&stmt, params)
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Query execution failed: {e}")))
    }

    /// Execute a statement
    pub async fn execute(
        &self,
        query: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Result<u64> {
        let stmt = self.cache.get_or_prepare(&self.client, query).await?;
        self.client
            .execute(&stmt, params)
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Statement execution failed: {e}")))
    }

    /// Get the underlying client for operations that don't support caching
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> CacheStats {
        self.cache.get_stats().await
    }
}
