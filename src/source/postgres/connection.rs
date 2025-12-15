use crate::config::{PostgreSQLConfig, PostgreSQLConnection};
use crate::error::{MeiliBridgeError, Result};
use crate::source::postgres::statement_cache::{CacheConfig, CachedConnection, StatementCache};
use deadpool_postgres::{Manager, ManagerConfig, Pool, RecyclingMethod, Runtime};
use std::sync::Arc;
use tokio_postgres::{Client, Config as PgConfig, NoTls};
use tracing::{debug, error, info, warn};

#[derive(Clone)]
pub struct PostgresConnector {
    config: PostgreSQLConfig,
    pool: Option<Pool>,
    statement_cache: Arc<StatementCache>,
}

impl PostgresConnector {
    pub fn new(config: PostgreSQLConfig) -> Self {
        let cache_config = CacheConfig {
            max_size: config.statement_cache.max_size,
            enabled: config.statement_cache.enabled,
        };
        let statement_cache = Arc::new(StatementCache::new(cache_config));

        Self {
            config,
            pool: None,
            statement_cache,
        }
    }

    pub async fn connect(&mut self) -> Result<()> {
        info!("Connecting to PostgreSQL...");
        let pool = self.create_pool().await?;

        // Test the connection
        let client = pool.get().await.map_err(|e| {
            MeiliBridgeError::Source(format!("Failed to get connection from pool: {e}"))
        })?;

        // Verify we can query the database
        client
            .simple_query("SELECT 1")
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Connection test failed: {e}")))?;

        self.pool = Some(pool);
        info!("Successfully connected to PostgreSQL");
        Ok(())
    }

    async fn create_pool(&self) -> Result<Pool> {
        let pg_config = self.build_pg_config()?;

        let manager_config = ManagerConfig {
            recycling_method: RecyclingMethod::Fast,
        };

        let manager = Manager::from_config(pg_config, NoTls, manager_config);
        let pool = Pool::builder(manager)
            .max_size(self.config.pool.max_size as usize)
            .runtime(Runtime::Tokio1)
            .build()
            .map_err(|e| MeiliBridgeError::Source(format!("Failed to create pool: {e}")))?;

        Ok(pool)
    }

    fn build_pg_config(&self) -> Result<PgConfig> {
        let mut config = match &self.config.connection {
            PostgreSQLConnection::ConnectionString(conn_str) => conn_str
                .parse::<PgConfig>()
                .map_err(|e| MeiliBridgeError::Config(format!("Invalid connection string: {e}")))?,
            PostgreSQLConnection::Parameters {
                host,
                port,
                database,
                username,
                password,
            } => {
                let mut config = PgConfig::new();
                config
                    .host(host)
                    .port(*port)
                    .dbname(database)
                    .user(username)
                    .password(password);
                config
            }
        };

        // Apply additional settings
        config.application_name("meilibridge");

        Ok(config)
    }

    pub async fn get_client(&self) -> Result<deadpool_postgres::Object> {
        let pool = self
            .pool
            .as_ref()
            .ok_or_else(|| MeiliBridgeError::Source("Not connected to PostgreSQL".to_string()))?;

        pool.get().await.map_err(|e| {
            MeiliBridgeError::Source(format!("Failed to get connection from pool: {e}"))
        })
    }

    pub async fn get_replication_client(&self) -> Result<(Client, tokio::task::JoinHandle<()>)> {
        debug!("Creating replication connection...");

        // Build config for replication connection
        let mut config = self.build_pg_config()?;

        // Set connection parameters for better stability
        config.keepalives(true);
        config.keepalives_idle(std::time::Duration::from_secs(30));
        config.keepalives_interval(std::time::Duration::from_secs(10));
        config.keepalives_retries(3);

        let (client, connection) = config.connect(NoTls).await.map_err(|e| {
            MeiliBridgeError::Source(format!("Failed to create replication connection: {e}"))
        })?;

        // Spawn connection handler with error logging
        let handle = tokio::spawn(async move {
            match connection.await {
                Ok(()) => debug!("Replication connection closed normally"),
                Err(e) => error!("Replication connection error: {}", e),
            }
        });

        debug!("Replication connection established with keepalive");
        Ok((client, handle))
    }

    pub fn is_connected(&self) -> bool {
        self.pool.is_some()
    }

    /// Connect if not already connected (for health checks)
    pub async fn ensure_connected(&mut self) -> Result<()> {
        if !self.is_connected() {
            self.connect().await?;
        }
        Ok(())
    }

    /// Get a cached connection that uses prepared statement caching
    pub async fn get_cached_connection(&self) -> Result<CachedConnection> {
        // For cached connections, we need to create a new client
        // rather than using the pooled one directly
        let config = self.build_pg_config()?;
        let (client, connection) = config.connect(NoTls).await.map_err(|e| {
            MeiliBridgeError::Source(format!("Failed to create cached connection: {e}"))
        })?;

        // Spawn connection handler
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                error!("Cached connection error: {}", e);
            }
        });

        Ok(CachedConnection::new(client, self.statement_cache.clone()))
    }

    /// Get the statement cache for direct access
    pub fn statement_cache(&self) -> &Arc<StatementCache> {
        &self.statement_cache
    }

    /// Clear the statement cache
    pub async fn clear_statement_cache(&self) {
        self.statement_cache.clear().await;
    }

    pub fn get_pool(&self) -> &Pool {
        self.pool.as_ref().expect("Connection pool not initialized")
    }

    pub async fn disconnect(&mut self) {
        if let Some(pool) = self.pool.take() {
            pool.close();
            info!("Disconnected from PostgreSQL");
        }
    }
}

pub struct ReplicationSlotManager {
    slot_name: String,
    publication_name: String,
}

impl ReplicationSlotManager {
    pub fn new(slot_name: String, publication_name: String) -> Self {
        Self {
            slot_name,
            publication_name,
        }
    }

    pub fn slot_name(&self) -> &str {
        &self.slot_name
    }

    pub fn publication_name(&self) -> &str {
        &self.publication_name
    }

    pub async fn get_slot_lsn(&self, client: &Client) -> Result<Option<String>> {
        let query =
            "SELECT confirmed_flush_lsn::text FROM pg_replication_slots WHERE slot_name = $1";

        match client.query_opt(query, &[&self.slot_name]).await {
            Ok(Some(row)) => {
                let lsn: Option<String> = row.get(0);
                Ok(lsn)
            }
            Ok(None) => Ok(None),
            Err(e) => Err(MeiliBridgeError::Source(format!(
                "Failed to get slot LSN: {e}",
            ))),
        }
    }

    pub async fn create_slot(&self, client: &Client) -> Result<()> {
        // Check if slot already exists
        let check_query = "SELECT plugin FROM pg_replication_slots WHERE slot_name = $1";
        match client.query_opt(check_query, &[&self.slot_name]).await {
            Ok(Some(row)) => {
                let plugin: String = row.get(0);
                if plugin == "test_decoding" {
                    info!(
                        "Replication slot '{}' already exists with test_decoding",
                        self.slot_name
                    );
                    return Ok(());
                } else {
                    // Drop and recreate with test_decoding
                    warn!(
                        "Slot '{}' exists with plugin '{}', recreating with test_decoding",
                        self.slot_name, plugin
                    );
                    let drop_query =
                        format!("SELECT pg_drop_replication_slot('{}')", self.slot_name);
                    client.execute(&drop_query, &[]).await.map_err(|e| {
                        MeiliBridgeError::Source(format!("Failed to drop slot: {e}"))
                    })?;
                }
            }
            Ok(None) => {
                // Slot doesn't exist, create it
            }
            Err(e) => {
                return Err(MeiliBridgeError::Source(format!(
                    "Failed to check slot: {e}",
                )));
            }
        }

        let query = format!(
            "SELECT pg_create_logical_replication_slot('{}', 'test_decoding')",
            self.slot_name
        );

        client
            .execute(&query, &[])
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Failed to create slot: {e}")))?;

        info!("Created replication slot '{}'", self.slot_name);
        Ok(())
    }

    pub async fn create_publication(&self, client: &Client, tables: &[String]) -> Result<()> {
        // Check if publication already exists
        let check_query = "SELECT 1 FROM pg_publication WHERE pubname = $1";
        match client
            .query_opt(check_query, &[&self.publication_name])
            .await
        {
            Ok(Some(_)) => {
                info!("Publication '{}' already exists", self.publication_name);
                return Ok(());
            }
            Ok(None) => {
                // Publication doesn't exist, create it
            }
            Err(e) => {
                return Err(MeiliBridgeError::Source(format!(
                    "Failed to check publication: {e}",
                )));
            }
        }

        let table_list = if tables.is_empty() {
            "ALL TABLES".to_string()
        } else {
            format!("TABLE {}", tables.join(", "))
        };

        let query = format!(
            "CREATE PUBLICATION {} FOR {}",
            self.publication_name, table_list
        );

        client
            .execute(&query, &[])
            .await
            .map_err(|e| MeiliBridgeError::Source(format!("Failed to create publication: {e}")))?;

        info!("Created publication '{}'", self.publication_name);
        Ok(())
    }
}
