use meilibridge::config::*;
use meilibridge::error::MeiliBridgeError;
use meilibridge::models::Position;
use meilibridge::pipeline::PipelineOrchestrator;
use serde_json::Value;
use std::collections::HashMap;

// Basic test to verify orchestrator creation with minimal config
#[tokio::test]
async fn test_orchestrator_creation_no_sources() {
    // Create a minimal config without sources - should fail
    let config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: None,
        sources: vec![],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "redis://localhost:6379".to_string(),
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    let orchestrator = PipelineOrchestrator::new(config);

    // Expect error because no sources are configured
    assert!(orchestrator.is_err());
    if let Err(e) = orchestrator {
        match e {
            MeiliBridgeError::Configuration(msg) => {
                assert!(msg.contains("No data sources configured"));
            }
            _ => panic!("Expected Configuration error"),
        }
    }
}

#[tokio::test]
async fn test_orchestrator_creation_with_single_source() {
    let mut config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: None,
        sources: vec![],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "".to_string(), // Empty for in-memory
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![SyncTaskConfig {
            id: "task1".to_string(),
            source_name: "primary".to_string(),
            table: "users".to_string(),
            index: "users".to_string(),
            primary_key: "id".to_string(),
            full_sync_on_start: Some(false),
            auto_start: Some(true),
            filter: None,
            transform: None,
            mapping: None,
            soft_delete: None,
            options: SyncOptions {
                batch_size: 100,
                batch_timeout_ms: 1000,
                deduplicate: false,
                retry: Default::default(),
            },
        }],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    // Add a PostgreSQL source
    config.source = Some(SourceConfig::PostgreSQL(Box::new(PostgreSQLConfig {
        connection: PostgreSQLConnection::ConnectionString(
            "postgresql://test:test@localhost:5432/test".to_string(),
        ),
        slot_name: "test_slot".to_string(),
        publication: "test_pub".to_string(),
        pool: Default::default(),
        ssl: Default::default(),
        statement_cache: Default::default(),
    })));

    let orchestrator = PipelineOrchestrator::new(config);
    assert!(orchestrator.is_ok());
}

#[tokio::test]
async fn test_orchestrator_with_multiple_sources() {
    let config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: None,
        sources: vec![NamedSourceConfig {
            name: "source1".to_string(),
            config: SourceConfig::PostgreSQL(Box::new(PostgreSQLConfig {
                connection: PostgreSQLConnection::ConnectionString(
                    "postgresql://test:test@localhost:5432/test1".to_string(),
                ),
                slot_name: "test_slot1".to_string(),
                publication: "test_pub1".to_string(),
                pool: Default::default(),
                ssl: Default::default(),
                statement_cache: Default::default(),
            })),
        }],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "".to_string(),
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![SyncTaskConfig {
            id: "task1".to_string(),
            source_name: "source1".to_string(),
            table: "users".to_string(),
            index: "users".to_string(),
            primary_key: "id".to_string(),
            full_sync_on_start: Some(false),
            auto_start: Some(true),
            filter: None,
            transform: None,
            mapping: None,
            soft_delete: None,
            options: SyncOptions {
                batch_size: 100,
                batch_timeout_ms: 1000,
                deduplicate: false,
                retry: Default::default(),
            },
        }],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    let orchestrator = PipelineOrchestrator::new(config);
    assert!(orchestrator.is_ok());
}

#[tokio::test]
async fn test_orchestrator_with_filters() {
    let mut config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: Some(SourceConfig::PostgreSQL(Box::new(PostgreSQLConfig {
            connection: PostgreSQLConnection::ConnectionString(
                "postgresql://test:test@localhost:5432/test".to_string(),
            ),
            slot_name: "test_slot".to_string(),
            publication: "test_pub".to_string(),
            pool: Default::default(),
            ssl: Default::default(),
            statement_cache: Default::default(),
        }))),
        sources: vec![],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "".to_string(),
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![SyncTaskConfig {
            id: "task1".to_string(),
            source_name: "primary".to_string(),
            table: "users".to_string(),
            index: "users".to_string(),
            primary_key: "id".to_string(),
            full_sync_on_start: Some(false),
            auto_start: Some(true),
            filter: None,
            transform: None,
            mapping: None,
            soft_delete: None,
            options: SyncOptions {
                batch_size: 100,
                batch_timeout_ms: 1000,
                deduplicate: false,
                retry: Default::default(),
            },
        }],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    // Add filter configuration
    config.sync_tasks[0].filter = Some(FilterConfig {
        tables: TableFilter {
            whitelist: Some(vec!["users".to_string()]),
            blacklist: Some(vec![]),
        },
        event_types: Some(vec!["create".to_string(), "update".to_string()]),
        conditions: Some(vec![]),
    });

    let orchestrator = PipelineOrchestrator::new(config);
    assert!(orchestrator.is_ok());
}

#[tokio::test]
async fn test_orchestrator_with_soft_delete() {
    let mut config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: Some(SourceConfig::PostgreSQL(Box::new(PostgreSQLConfig {
            connection: PostgreSQLConnection::ConnectionString(
                "postgresql://test:test@localhost:5432/test".to_string(),
            ),
            slot_name: "test_slot".to_string(),
            publication: "test_pub".to_string(),
            pool: Default::default(),
            ssl: Default::default(),
            statement_cache: Default::default(),
        }))),
        sources: vec![],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "".to_string(),
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![SyncTaskConfig {
            id: "task1".to_string(),
            source_name: "primary".to_string(),
            table: "users".to_string(),
            index: "users".to_string(),
            primary_key: "id".to_string(),
            full_sync_on_start: Some(false),
            auto_start: Some(true),
            filter: None,
            transform: None,
            mapping: None,
            soft_delete: None,
            options: SyncOptions {
                batch_size: 100,
                batch_timeout_ms: 1000,
                deduplicate: false,
                retry: Default::default(),
            },
        }],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    // Add soft delete configuration
    config.sync_tasks[0].soft_delete = Some(SoftDeleteConfig {
        field: "deleted_at".to_string(),
        delete_values: vec![Value::Bool(true)],
        handle_on_full_sync: true,
        handle_on_cdc: true,
    });

    let orchestrator = PipelineOrchestrator::new(config);
    assert!(orchestrator.is_ok());
}

#[tokio::test]
async fn test_dlq_operations() {
    let config = Config {
        app: AppConfig {
            name: "test".to_string(),
            instance_id: "test-1".to_string(),
            tags: HashMap::new(),
        },
        source: Some(SourceConfig::PostgreSQL(Box::new(PostgreSQLConfig {
            connection: PostgreSQLConnection::ConnectionString(
                "postgresql://test:test@localhost:5432/test".to_string(),
            ),
            slot_name: "test_slot".to_string(),
            publication: "test_pub".to_string(),
            pool: Default::default(),
            ssl: Default::default(),
            statement_cache: Default::default(),
        }))),
        sources: vec![],
        meilisearch: MeilisearchConfig {
            url: "http://localhost:7700".to_string(),
            api_key: Some("test".to_string()),
            timeout: 30,
            max_connections: 10,
            index_settings: Default::default(),
            primary_key: Some("id".to_string()),
            batch_size: 1000,
            auto_create_index: true,
            circuit_breaker: Default::default(),
        },
        redis: RedisConfig {
            url: "".to_string(),
            password: None,
            database: 0,
            key_prefix: "meilibridge".to_string(),
            pool: Default::default(),
            checkpoint_retention: Default::default(),
        },
        sync_tasks: vec![SyncTaskConfig {
            id: "task1".to_string(),
            source_name: "primary".to_string(),
            table: "users".to_string(),
            index: "users".to_string(),
            primary_key: "id".to_string(),
            full_sync_on_start: Some(false),
            auto_start: Some(true),
            filter: None,
            transform: None,
            mapping: None,
            soft_delete: None,
            options: SyncOptions {
                batch_size: 100,
                batch_timeout_ms: 1000,
                deduplicate: false,
                retry: Default::default(),
            },
        }],
        api: Default::default(),
        logging: Default::default(),
        features: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        at_least_once_delivery: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
    };

    let orchestrator = PipelineOrchestrator::new(config).unwrap();

    // DLQ operations should fail before starting
    assert!(orchestrator.get_dlq_statistics().await.is_err());
    assert!(
        orchestrator
            .reprocess_dlq_entries("task1", Some(10))
            .await
            .is_err()
    );
    assert!(orchestrator.clear_dlq_task("task1").await.is_err());
}

#[tokio::test]
async fn test_position_variants() {
    // Test different position types
    let pg_position = Position::PostgreSQL {
        lsn: "0/16B3748".to_string(),
    };

    let mysql_position = Position::MySQL {
        file: "mysql-bin.000001".to_string(),
        position: 154,
    };

    let mongodb_position = Position::MongoDB {
        resume_token: "82507632F7000000012B022C0100296E5A10040".to_string(),
    };

    // Just verify they can be created
    assert!(matches!(pg_position, Position::PostgreSQL { .. }));
    assert!(matches!(mysql_position, Position::MySQL { .. }));
    assert!(matches!(mongodb_position, Position::MongoDB { .. }));
}
