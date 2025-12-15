use crate::api::state::ApiState;
use crate::error::MeiliBridgeError;
use axum::{Json, extract::State};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct CacheStatsResponse {
    pub size: usize,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub hit_rate: f64,
    pub enabled: bool,
    pub max_size: usize,
}

#[derive(Debug, Serialize)]
pub struct CacheClearResponse {
    pub message: String,
    pub cleared_count: usize,
}

/// Get statement cache statistics
pub async fn get_cache_stats(
    State(state): State<ApiState>,
) -> Result<Json<CacheStatsResponse>, MeiliBridgeError> {
    // Check if we have a PostgreSQL cache
    if let Some(cache) = &state.postgres_cache {
        let stats = cache.get_stats().await;

        // Export metrics while we're at it
        stats.export_metrics();

        let response = CacheStatsResponse {
            size: stats.size,
            hits: stats.hits,
            misses: stats.misses,
            evictions: stats.evictions,
            hit_rate: stats.hit_rate,
            enabled: true,
            max_size: cache.max_size(),
        };

        // Note: This shows stats for the API's cache instance only.
        // The actual PostgreSQL adapter uses its own cache instance.

        Ok(Json(response))
    } else {
        // No PostgreSQL cache available (might be using a different source)
        Err(MeiliBridgeError::NotFound(
            "Statement cache not available for current data source".to_string(),
        ))
    }
}

/// Clear the statement cache
pub async fn clear_cache(
    State(state): State<ApiState>,
) -> Result<Json<CacheClearResponse>, MeiliBridgeError> {
    if let Some(cache) = &state.postgres_cache {
        // Get current size before clearing
        let stats = cache.get_stats().await;
        let cleared_count = stats.size;

        // Clear the cache
        cache.clear().await;

        let response = CacheClearResponse {
            message: "Statement cache cleared successfully".to_string(),
            cleared_count,
        };

        Ok(Json(response))
    } else {
        // No PostgreSQL cache available
        Err(MeiliBridgeError::NotFound(
            "Statement cache not available for current data source".to_string(),
        ))
    }
}
