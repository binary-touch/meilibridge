use crate::config::MeilisearchConfig;
use crate::destination::circuit_breaker::{
    CircuitBreakerBuilder, CircuitBreakerError, MeilisearchCircuitBreaker,
};
use crate::destination::meilisearch::client::{MeilisearchClient, convert_error};
use crate::error::{MeiliBridgeError, Result};
use meilisearch_sdk::indexes::Index;
use serde_json::Value;
use std::time::Duration;
use tracing::{debug, info};

/// A Meilisearch client with circuit breaker protection
pub struct ProtectedMeilisearchClient {
    client: MeilisearchClient,
    circuit_breaker: Option<MeilisearchCircuitBreaker>,
}

impl ProtectedMeilisearchClient {
    pub fn new(config: MeilisearchConfig) -> Result<Self> {
        let client = MeilisearchClient::new(config.clone())?;

        let circuit_breaker = if config.circuit_breaker.enabled {
            info!("Initializing circuit breaker for Meilisearch");
            Some(
                CircuitBreakerBuilder::new("meilisearch".to_string())
                    .error_rate(config.circuit_breaker.error_rate)
                    .min_request_count(config.circuit_breaker.min_request_count)
                    .consecutive_failures(config.circuit_breaker.consecutive_failures)
                    .timeout(Duration::from_secs(config.circuit_breaker.timeout_secs))
                    .build(),
            )
        } else {
            None
        };

        Ok(Self {
            client,
            circuit_breaker,
        })
    }

    /// Get a reference to the inner client
    pub fn inner(&self) -> &MeilisearchClient {
        &self.client
    }

    /// Test connection with circuit breaker protection
    pub async fn test_connection(&self) -> Result<()> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            match cb
                .call(|| Box::pin(async move { client.test_connection().await }))
                .await
            {
                Ok(result) => Ok(result),
                Err(CircuitBreakerError::CircuitOpen) => Err(MeiliBridgeError::Meilisearch(
                    "Circuit breaker is open, cannot test connection".to_string(),
                )),
                Err(CircuitBreakerError::OperationError(e)) => Err(e),
            }
        } else {
            self.client.test_connection().await
        }
    }

    /// Get version with circuit breaker protection
    pub async fn get_version(&self) -> Result<String> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            match cb
                .call(|| Box::pin(async move { client.get_version().await }))
                .await
            {
                Ok(result) => Ok(result),
                Err(CircuitBreakerError::CircuitOpen) => Err(MeiliBridgeError::Meilisearch(
                    "Circuit breaker is open, cannot get version".to_string(),
                )),
                Err(CircuitBreakerError::OperationError(e)) => Err(e),
            }
        } else {
            self.client.get_version().await
        }
    }

    /// Get or create index with circuit breaker protection
    pub async fn get_or_create_index(
        &self,
        index_name: &str,
        primary_key: Option<&str>,
    ) -> Result<Index> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            let index_name = index_name.to_string();
            let primary_key = primary_key.map(|s| s.to_string());

            match cb
                .call(|| {
                    Box::pin(async move {
                        let meilisearch = client.client();
                        match meilisearch.get_index(&index_name).await {
                            Ok(index) => Ok(index),
                            Err(_) => {
                                // Index doesn't exist, create it
                                match meilisearch
                                    .create_index(&index_name, primary_key.as_deref())
                                    .await
                                {
                                    Ok(task) => {
                                        let task_info = meilisearch
                                            .wait_for_task(task, None, None)
                                            .await
                                            .map_err(convert_error)?;

                                        if task_info.is_success() {
                                            meilisearch
                                                .get_index(&index_name)
                                                .await
                                                .map_err(convert_error)
                                        } else {
                                            Err(MeiliBridgeError::Meilisearch(format!(
                                                "Failed to create index: {:?}",
                                                task_info
                                            )))
                                        }
                                    }
                                    Err(e) => Err(convert_error(e)),
                                }
                            }
                        }
                    })
                })
                .await
            {
                Ok(result) => Ok(result),
                Err(CircuitBreakerError::CircuitOpen) => Err(MeiliBridgeError::Meilisearch(
                    "Circuit breaker is open, cannot access index".to_string(),
                )),
                Err(CircuitBreakerError::OperationError(e)) => Err(e),
            }
        } else {
            // No circuit breaker, use client directly
            let meilisearch = self.client.client();
            match meilisearch.get_index(index_name).await {
                Ok(index) => Ok(index),
                Err(_) => match meilisearch.create_index(index_name, primary_key).await {
                    Ok(task) => {
                        let task_info = meilisearch
                            .wait_for_task(task, None, None)
                            .await
                            .map_err(convert_error)?;

                        if task_info.is_success() {
                            meilisearch
                                .get_index(index_name)
                                .await
                                .map_err(convert_error)
                        } else {
                            Err(MeiliBridgeError::Meilisearch(format!(
                                "Failed to create index: {:?}",
                                task_info
                            )))
                        }
                    }
                    Err(e) => Err(convert_error(e)),
                },
            }
        }
    }

    /// Add documents with circuit breaker protection
    pub async fn add_documents(
        &self,
        index: &Index,
        documents: &[Value],
        primary_key: Option<&str>,
    ) -> Result<()> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            let index = index.clone();
            let documents = documents.to_vec();
            let primary_key = primary_key.map(|s| s.to_string());

            match cb
                .call(|| {
                    Box::pin(async move {
                        match index
                            .add_documents(&documents, primary_key.as_deref())
                            .await
                        {
                            Ok(task) => {
                                let task_info = client
                                    .client()
                                    .wait_for_task(task, None, None)
                                    .await
                                    .map_err(convert_error)?;

                                if task_info.is_success() {
                                    Ok(())
                                } else {
                                    Err(MeiliBridgeError::Meilisearch(format!(
                                        "Add documents failed: {:?}",
                                        task_info
                                    )))
                                }
                            }
                            Err(e) => Err(convert_error(e)),
                        }
                    })
                })
                .await
            {
                Ok(result) => Ok(result),
                Err(CircuitBreakerError::CircuitOpen) => Err(MeiliBridgeError::Meilisearch(
                    "Circuit breaker is open, cannot add documents".to_string(),
                )),
                Err(CircuitBreakerError::OperationError(e)) => Err(e),
            }
        } else {
            match index.add_documents(documents, primary_key).await {
                Ok(task) => {
                    let task_info = self
                        .client
                        .client()
                        .wait_for_task(task, None, None)
                        .await
                        .map_err(convert_error)?;

                    if task_info.is_success() {
                        Ok(())
                    } else {
                        Err(MeiliBridgeError::Meilisearch(format!(
                            "Add documents failed: {:?}",
                            task_info
                        )))
                    }
                }
                Err(e) => Err(convert_error(e)),
            }
        }
    }

    /// Delete documents with circuit breaker protection
    pub async fn delete_documents(&self, index: &Index, document_ids: &[String]) -> Result<()> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            let index = index.clone();
            let document_ids = document_ids.to_vec();

            match cb
                .call(|| {
                    Box::pin(async move {
                        match index.delete_documents(&document_ids).await {
                            Ok(task) => {
                                let task_info = client
                                    .client()
                                    .wait_for_task(task, None, None)
                                    .await
                                    .map_err(convert_error)?;

                                if task_info.is_success() {
                                    Ok(())
                                } else {
                                    Err(MeiliBridgeError::Meilisearch(format!(
                                        "Delete documents failed: {:?}",
                                        task_info
                                    )))
                                }
                            }
                            Err(e) => Err(convert_error(e)),
                        }
                    })
                })
                .await
            {
                Ok(result) => Ok(result),
                Err(CircuitBreakerError::CircuitOpen) => Err(MeiliBridgeError::Meilisearch(
                    "Circuit breaker is open, cannot delete documents".to_string(),
                )),
                Err(CircuitBreakerError::OperationError(e)) => Err(e),
            }
        } else {
            match index.delete_documents(document_ids).await {
                Ok(task) => {
                    let task_info = self
                        .client
                        .client()
                        .wait_for_task(task, None, None)
                        .await
                        .map_err(convert_error)?;

                    if task_info.is_success() {
                        Ok(())
                    } else {
                        Err(MeiliBridgeError::Meilisearch(format!(
                            "Delete documents failed: {:?}",
                            task_info
                        )))
                    }
                }
                Err(e) => Err(convert_error(e)),
            }
        }
    }

    /// Health check with circuit breaker protection
    pub async fn health_check(&self) -> Result<bool> {
        if let Some(ref cb) = self.circuit_breaker {
            let client = self.client.clone();
            match cb
                .call(|| {
                    Box::pin(async move {
                        match client.client().health().await {
                            Ok(health) => {
                                Ok::<bool, MeiliBridgeError>(health.status == "available")
                            }
                            Err(_) => Ok(false),
                        }
                    })
                })
                .await
            {
                Ok(is_healthy) => Ok(is_healthy),
                Err(CircuitBreakerError::CircuitOpen) => {
                    debug!("Circuit breaker is open during health check");
                    Ok(false)
                }
                Err(CircuitBreakerError::OperationError(_)) => Ok(false),
            }
        } else {
            match self.client.client().health().await {
                Ok(health) => Ok(health.status == "available"),
                Err(_) => Ok(false),
            }
        }
    }
}
