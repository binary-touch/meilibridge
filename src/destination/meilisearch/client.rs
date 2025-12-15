use crate::config::MeilisearchConfig;
use crate::error::{MeiliBridgeError, Result};
use meilisearch_sdk::{client::Client, errors::Error as MeilisearchError};
use std::sync::Arc;
use tracing::{debug, info};

/// Wrapper around Meilisearch SDK client with connection pooling
#[derive(Clone)]
pub struct MeilisearchClient {
    inner: Arc<Client>,
    config: MeilisearchConfig,
}

impl MeilisearchClient {
    pub fn new(config: MeilisearchConfig) -> Result<Self> {
        let client = Client::new(&config.url, config.api_key.as_deref()).map_err(convert_error)?;

        Ok(Self {
            inner: Arc::new(client),
            config,
        })
    }

    /// Get a reference to the inner Meilisearch client
    pub fn client(&self) -> &Client {
        &self.inner
    }

    /// Test the connection to Meilisearch
    pub async fn test_connection(&self) -> Result<()> {
        info!("Testing connection to Meilisearch at {}", self.config.url);

        match self.inner.health().await {
            Ok(health) => {
                debug!("Meilisearch health: {:?}", health);
                info!("Successfully connected to Meilisearch");
                Ok(())
            }
            Err(e) => Err(MeiliBridgeError::Meilisearch(format!(
                "Failed to connect to Meilisearch: {e}",
            ))),
        }
    }

    /// Get server version
    pub async fn get_version(&self) -> Result<String> {
        match self.inner.get_version().await {
            Ok(version) => Ok(version.pkg_version),
            Err(e) => Err(convert_error(e)),
        }
    }
}

/// Convert Meilisearch SDK errors to our error type
pub fn convert_error(error: MeilisearchError) -> MeiliBridgeError {
    MeiliBridgeError::Meilisearch(format!("{error:?}"))
}
