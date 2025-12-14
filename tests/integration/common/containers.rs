// Test container utilities
#![allow(clippy::collapsible_if)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::time::Duration;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, Image};
use testcontainers_modules::{postgres::Postgres, redis::Redis};

// Re-export Container type for easier use in tests
pub type Container<I> = ContainerAsync<I>;

#[derive(Default)]
pub struct TestContainers {
    pub postgres: Option<Container<Postgres>>,
    pub redis: Option<Container<Redis>>,
    pub meilisearch: Option<Container<MeilisearchImage>>,
}

impl TestContainers {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn postgres_url(&self) -> String {
        if let Some(container) = &self.postgres {
            let port = container
                .get_host_port_ipv4(5432)
                .await
                .expect("Failed to get port");
            format!("postgresql://postgres:postgres@localhost:{}/postgres", port)
        } else {
            panic!("PostgreSQL container not started");
        }
    }

    pub async fn redis_url(&self) -> String {
        if let Some(container) = &self.redis {
            let port = container
                .get_host_port_ipv4(6379)
                .await
                .expect("Failed to get port");
            format!("redis://localhost:{}", port)
        } else {
            panic!("Redis container not started");
        }
    }

    pub async fn meilisearch_url(&self) -> String {
        if let Some(container) = &self.meilisearch {
            let port = container
                .get_host_port_ipv4(7700)
                .await
                .expect("Failed to get port");
            format!("http://localhost:{}", port)
        } else {
            panic!("Meilisearch container not started");
        }
    }
}

// Custom Meilisearch image since it's not in testcontainers-modules
#[derive(Debug, Clone)]
pub struct MeilisearchImage {
    tag: String,
    env_vars: HashMap<String, String>,
}

impl Default for MeilisearchImage {
    fn default() -> Self {
        let mut env_vars = HashMap::new();
        env_vars.insert("MEILI_MASTER_KEY".to_string(), "masterKey".to_string());
        env_vars.insert("MEILI_ENV".to_string(), "development".to_string());

        MeilisearchImage {
            tag: "v1.13".to_string(),
            env_vars,
        }
    }
}

impl Image for MeilisearchImage {
    fn name(&self) -> &str {
        "getmeili/meilisearch"
    }

    fn tag(&self) -> &str {
        &self.tag
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            // Just wait a bit for Meilisearch to start
            // We'll rely on our wait_for_meilisearch function to check health
            WaitFor::millis(3000),
        ]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        &self.env_vars
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(7700)]
    }
}

// Container lifecycle helpers
pub async fn start_postgres() -> Container<Postgres> {
    Postgres::default()
        .start()
        .await
        .expect("Failed to start Postgres")
}

// Custom PostgreSQL image with CDC support (wal_level=logical)
#[derive(Debug, Clone)]
pub struct PostgresCDCImage {
    env_vars: HashMap<String, String>,
}

impl Default for PostgresCDCImage {
    fn default() -> Self {
        let mut env_vars = HashMap::new();
        env_vars.insert("POSTGRES_USER".to_string(), "postgres".to_string());
        env_vars.insert("POSTGRES_PASSWORD".to_string(), "postgres".to_string());
        env_vars.insert("POSTGRES_DB".to_string(), "testdb".to_string());

        PostgresCDCImage { env_vars }
    }
}

impl Image for PostgresCDCImage {
    fn name(&self) -> &str {
        "binarytouch/postgres"
    }

    fn tag(&self) -> &str {
        "17"
    }

    fn ready_conditions(&self) -> Vec<WaitFor> {
        vec![
            WaitFor::message_on_stderr("database system is ready to accept connections"),
            WaitFor::millis(1000),
        ]
    }

    fn env_vars(
        &self,
    ) -> impl IntoIterator<Item = (impl Into<Cow<'_, str>>, impl Into<Cow<'_, str>>)> {
        &self.env_vars
    }

    fn expose_ports(&self) -> &[ContainerPort] {
        &[ContainerPort::Tcp(5432)]
    }
}

// Start PostgreSQL with logical replication enabled
pub async fn start_postgres_with_cdc() -> Container<PostgresCDCImage> {
    PostgresCDCImage::default()
        .start()
        .await
        .expect("Failed to start Postgres CDC")
}

pub async fn start_redis() -> Container<Redis> {
    Redis::default()
        .start()
        .await
        .expect("Failed to start Redis")
}

pub async fn start_meilisearch() -> Container<MeilisearchImage> {
    let image = MeilisearchImage::default();
    image.start().await.expect("Failed to start Meilisearch")
}

// Wait for services to be ready
pub async fn wait_for_postgres(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let max_retries = 30;
    let mut retries = 0;

    loop {
        match tokio_postgres::connect(url, tokio_postgres::NoTls).await {
            Ok((client, connection)) => {
                // Spawn connection handler
                tokio::spawn(async move {
                    if let Err(e) = connection.await {
                        eprintln!("connection error: {}", e);
                    }
                });

                // Test connection
                if client.simple_query("SELECT 1").await.is_ok() {
                    return Ok(());
                }
            }
            Err(_) => {
                retries += 1;
                if retries >= max_retries {
                    return Err("PostgreSQL failed to start".into());
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
}

pub async fn wait_for_redis(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let max_retries = 30;
    let mut retries = 0;

    loop {
        if let Ok(client) = redis::Client::open(url) {
            if let Ok(mut conn) = client.get_connection() {
                if redis::cmd("PING").query::<String>(&mut conn).is_ok() {
                    return Ok(());
                }
            }
        }

        retries += 1;
        if retries >= max_retries {
            return Err("Redis failed to start".into());
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

pub async fn wait_for_meilisearch(url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let max_retries = 30;
    let mut retries = 0;

    let client = reqwest::Client::new();
    println!("Waiting for Meilisearch at {}...", url);

    loop {
        match client.get(format!("{}/health", url)).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    println!("Meilisearch is ready!");
                    return Ok(());
                }
                println!("Meilisearch returned status: {}", response.status());
            }
            Err(e) => {
                if retries % 5 == 0 {
                    println!(
                        "Waiting for Meilisearch... (attempt {}/{}): {}",
                        retries + 1,
                        max_retries,
                        e
                    );
                }
            }
        }

        retries += 1;
        if retries >= max_retries {
            return Err(
                format!("Meilisearch failed to start after {} attempts", max_retries).into(),
            );
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}
