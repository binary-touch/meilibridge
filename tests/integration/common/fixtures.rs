// Test fixtures for integration tests

use meilibridge::api::ApiState;
use meilibridge::config::{
    Config, MeilisearchConfig, NamedSourceConfig, PostgreSQLConfig, PostgreSQLConnection,
    RedisConfig, SourceConfig, SyncTaskConfig,
};
use meilibridge::pipeline::PipelineOrchestrator;
use meilibridge::sync::SyncTaskManager;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::RwLock;

// PostgreSQL test fixtures
pub fn create_test_postgres_config(connection_string: &str) -> PostgreSQLConfig {
    PostgreSQLConfig {
        connection: PostgreSQLConnection::ConnectionString(connection_string.to_string()),
        slot_name: "test_slot".to_string(),
        publication: "test_pub".to_string(),
        pool: Default::default(),
        ssl: Default::default(),
        statement_cache: Default::default(),
    }
}

pub async fn setup_postgres_replication(
    client: &tokio_postgres::Client,
    publication: &str,
    tables: &[&str],
) -> Result<(), Box<dyn std::error::Error>> {
    // Create publication
    let tables_list = tables.join(", ");
    client
        .execute(
            &format!("CREATE PUBLICATION {publication} FOR TABLE {tables_list}"),
            &[],
        )
        .await?;

    Ok(())
}

pub async fn create_test_table(
    client: &tokio_postgres::Client,
    table_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    client
        .execute(
            &format!(
                "CREATE TABLE {table_name} (
                    id SERIAL PRIMARY KEY,
                    name TEXT NOT NULL,
                    email TEXT,
                    created_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
                    deleted_at TIMESTAMP
                )"
            ),
            &[],
        )
        .await?;

    Ok(())
}

// Meilisearch test fixtures
pub fn create_test_meilisearch_config(url: &str) -> MeilisearchConfig {
    MeilisearchConfig {
        url: url.to_string(),
        api_key: Some("masterKey".to_string()),
        timeout: 30,
        max_connections: 10,
        index_settings: Default::default(),
        primary_key: None,
        batch_size: 1000,
        auto_create_index: true,
        circuit_breaker: Default::default(),
    }
}

// Redis test fixtures
pub fn create_test_redis_config(url: &str) -> RedisConfig {
    RedisConfig {
        url: url.to_string(),
        password: None,
        database: 0,
        key_prefix: "test_".to_string(),
        pool: Default::default(),
        checkpoint_retention: Default::default(),
    }
}

// Complete config fixture
pub fn create_test_config(postgres_url: &str, meilisearch_url: &str, redis_url: &str) -> Config {
    Config {
        app: Default::default(),
        source: Some(SourceConfig::PostgreSQL(Box::new(
            create_test_postgres_config(postgres_url),
        ))),
        sources: vec![NamedSourceConfig {
            name: "default".to_string(),
            config: SourceConfig::PostgreSQL(Box::new(create_test_postgres_config(postgres_url))),
        }],
        sync_tasks: vec![],
        meilisearch: create_test_meilisearch_config(meilisearch_url),
        redis: create_test_redis_config(redis_url),
        api: Default::default(),
        logging: Default::default(),
        monitoring: Default::default(),
        performance: Default::default(),
        error_handling: Default::default(),
        plugins: Default::default(),
        features: Default::default(),
        at_least_once_delivery: Default::default(),
    }
}

pub fn create_test_sync_task() -> SyncTaskConfig {
    SyncTaskConfig {
        id: "test_task".to_string(),
        source_name: "default".to_string(),
        table: "test_table".to_string(),
        index: "test_index".to_string(),
        primary_key: "id".to_string(),
        full_sync_on_start: Some(false),
        filter: None,
        transform: None,
        mapping: None,
        soft_delete: None,
        options: Default::default(),
        auto_start: Some(true),
    }
}

// Test data generators
pub fn generate_test_data(count: usize) -> Vec<HashMap<String, serde_json::Value>> {
    (0..count)
        .map(|i| {
            let mut data = HashMap::new();
            data.insert("id".to_string(), serde_json::json!(i + 1));
            data.insert(
                "name".to_string(),
                serde_json::json!(format!("User {}", i + 1)),
            );
            data.insert(
                "email".to_string(),
                serde_json::json!(format!("user{}@example.com", i + 1)),
            );
            data
        })
        .collect()
}

// API test helper functions
pub async fn create_test_api_state() -> ApiState {
    let config = create_test_config(
        "postgres://localhost:5432/test",
        "http://localhost:7700",
        "redis://localhost:6379",
    );

    let orchestrator =
        PipelineOrchestrator::new(config.clone()).expect("Failed to create orchestrator");
    let task_manager = SyncTaskManager::new(config);

    let state = ApiState::new(
        Arc::new(RwLock::new(orchestrator)),
        Arc::new(RwLock::new(task_manager)),
    );

    // Add health registry
    let health_registry = Arc::new(meilibridge::health::HealthRegistry::new());
    state.with_health_registry(health_registry)
}

pub async fn start_test_server(state: ApiState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    use axum::Router;
    use axum::routing::{delete, get, post, put};
    use meilibridge::api::handlers;

    let app = Router::new()
        // Health check
        .route("/health", get(handlers::health))
        .route("/health/{component}", get(handlers::get_component_health))
        // Task management
        .route("/tasks", get(handlers::get_tasks))
        .route("/tasks", post(handlers::create_task))
        .route("/tasks/{id}", get(handlers::get_task))
        .route("/tasks/{id}", delete(handlers::delete_task))
        .route("/tasks/{id}/pause", put(handlers::pause_task))
        .route("/tasks/{id}/resume", put(handlers::resume_task))
        // CDC control
        .route("/cdc/status", get(handlers::get_cdc_status))
        // Metrics
        .route("/metrics", get(handlers::get_metrics))
        // Cache control
        .route(
            "/cache/clear",
            post(meilibridge::api::cache_handlers::clear_cache),
        )
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("Failed to bind test server");
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait for server to start
    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;

    (addr, handle)
}
