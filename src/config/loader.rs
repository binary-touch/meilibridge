use super::Config;
use crate::error::{MeiliBridgeError, Result};
use config::{Config as ConfigBuilder, Environment, File};
use std::env;
use std::path::Path;

pub struct ConfigLoader;

impl ConfigLoader {
    pub fn load(path: Option<&str>) -> Result<Config> {
        let mut builder = ConfigBuilder::builder();

        // Load from config file if specified
        let config_path = path.map(String::from).or_else(|| env::var("CONFIG_PATH").ok());

        if let Some(config_path) = config_path {
            builder = builder.add_source(File::with_name(&config_path));
        } else {
            // Try to load default config files
            let config_files = [
                "config.yaml",
                "config.yml",
                "meilibridge.yaml",
                "meilibridge.yml",
            ];
            for file in &config_files {
                if Path::new(file).exists() {
                    builder = builder.add_source(File::with_name(file));
                    break;
                }
            }
        }

        // Load environment-specific config
        if let Ok(env_name) = env::var("APP_ENV") {
            let env_configs = [
                format!("config.{}.yaml", env_name),
                format!("config.{}.yml", env_name),
                format!("{}.yaml", env_name),
                format!("{}.yml", env_name),
            ];

            for file in &env_configs {
                if Path::new(file).exists() {
                    builder = builder.add_source(File::with_name(file));
                    break;
                }
            }
        }

        // Override with environment variables
        // MEILIBRIDGE__SOURCE__HOST=localhost becomes source.host
        builder = builder.add_source(
            Environment::with_prefix("MEILIBRIDGE")
                .separator("__")
                .try_parsing(true),
        );

        // Build and deserialize
        let config = builder
            .build()
            .map_err(|e| MeiliBridgeError::Config(format!("Failed to build config: {}", e)))?;

        let config: Config = config.try_deserialize().map_err(|e| {
            MeiliBridgeError::Config(format!("Failed to deserialize config: {}", e))
        })?;

        // Validate configuration
        Self::validate(&config)?;

        Ok(config)
    }

    pub fn validate(config: &Config) -> Result<()> {
        let mut errors = Vec::new();

        // Validate source configuration
        if let Some(super::SourceConfig::PostgreSQL(pg)) = &config.source {
            if pg.pool.max_size < pg.pool.min_idle {
                errors.push("PostgreSQL pool max_size must be >= min_idle".to_string());
            }
            if pg.pool.max_size == 0 {
                errors.push("PostgreSQL pool max_size must be > 0".to_string());
            }
        }

        // Validate sources configuration
        for named_source in &config.sources {
            if let super::SourceConfig::PostgreSQL(pg) = &named_source.config {
                if pg.pool.max_size < pg.pool.min_idle {
                    errors.push(format!(
                        "PostgreSQL source '{}' pool max_size must be >= min_idle",
                        named_source.name
                    ));
                }
                if pg.pool.max_size == 0 {
                    errors.push(format!(
                        "PostgreSQL source '{}' pool max_size must be > 0",
                        named_source.name
                    ));
                }
            }
        }

        // Validate sync tasks
        // We allow starting with no sync tasks (can be added via API)

        for (i, task) in config.sync_tasks.iter().enumerate() {
            if task.table.is_empty() {
                errors.push(format!("Sync task {} has empty table name", i));
            }
            if task.index.is_empty() {
                errors.push(format!("Sync task {} has empty index name", i));
            }
            if task.options.batch_size == 0 {
                errors.push(format!("Sync task {} batch_size must be > 0", i));
            }
        }

        // Validate Meilisearch configuration
        if config.meilisearch.url.is_empty() {
            errors.push("Meilisearch URL cannot be empty".to_string());
        }

        // Validate Redis configuration
        if config.redis.url.is_empty() {
            errors.push("Redis URL cannot be empty".to_string());
        }

        // Validate API configuration
        if config.api.port == 0 {
            errors.push("API port must be > 0".to_string());
        }

        if !errors.is_empty() {
            return Err(MeiliBridgeError::Validation(errors.join(", ")));
        }

        Ok(())
    }

    /// Load configuration from a specific file
    pub fn load_from_file(path: &str) -> Result<Config> {
        Self::load(Some(path))
    }

    /// Create a sample configuration file
    pub fn generate_sample() -> &'static str {
        r#"# MeiliBridge Configuration Example
# Copy this file to config.yaml and adjust for your environment

app:
  name: meilibridge-dev
  # instance_id: auto  # Automatically generated if not specified
  tags:
    environment: development
    region: local

# PostgreSQL source configuration
source:
  type: postgresql
  host: localhost
  port: 5432
  database: myapp
  username: postgres
  password: ${POSTGRES_PASSWORD}  # Use environment variable
  slot_name: meilibridge_slot
  publication: meilibridge_pub
  pool:
    max_size: 10
    min_idle: 1
  ssl:
    mode: prefer

# Meilisearch configuration
meilisearch:
  url: http://localhost:7700
  api_key: ${MEILISEARCH_API_KEY}  # Use environment variable
  timeout: 30
  max_connections: 10
  index_settings:
    searchable_attributes:
      - title
      - description
      - content
    filterable_attributes:
      - category
      - status
      - created_at

# Redis configuration for state management
redis:
  url: redis://localhost:6379
  # password: ${REDIS_PASSWORD}  # Uncomment if Redis requires auth
  database: 0
  key_prefix: meilibridge

# Sync task definitions
sync_tasks:
  - table: users
    index: users
    primary_key: id
    field_mappings:
      user_id: id
      full_name: name
      email_address: email
    filters:
      exclude:
        - password_hash
        - internal_notes
    options:
      batch_size: 1000
      batch_timeout: 10

  - table: products
    index: products
    primary_key: sku
    full_sync: true
    filters:
      expressions:
        - field: status
          operator: eq
          value: "active"
    options:
      batch_size: 5000

# API server configuration
api:
  host: 127.0.0.1
  port: 7701
  cors:
    enabled: true
    allowed_origins:
      - http://localhost:3000
  auth:
    enabled: false
    # jwt_secret: ${JWT_SECRET}
    # token_expiry: 3600

# Logging configuration
logging:
  level: info
  format: text  # text, json, or pretty
  outputs:
    - type: stdout
    # - type: file
    #   path: ./logs/meilibridge.log
    #   rotation: daily

# Feature flags
features:
  auto_recovery: true
  health_checks: true
  metrics_export: true
  distributed_mode: false
"#
    }
}
