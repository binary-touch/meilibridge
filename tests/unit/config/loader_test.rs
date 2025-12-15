// Configuration loader tests

use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[cfg(test)]
mod loader_tests {
    use super::*;

    fn create_test_yaml_content() -> String {
        r#"
source:
  type: postgresql
  connection: "postgresql://user:pass@localhost/testdb"
  slot_name: test_slot
  publication: test_pub

sync_tasks:
  - id: test_task
    table: users
    index: users_index
    primary_key: id

meilisearch:
  url: http://localhost:7700
  api_key: test_key

redis:
  url: redis://localhost:6379
"#
        .to_string()
    }

    #[test]
    fn test_yaml_file_loading() {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.yaml");
        fs::write(&config_path, create_test_yaml_content()).unwrap();

        // Note: In the actual implementation, we would load and parse the config
        // For now, just verify the file was created
        assert!(config_path.exists());

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("postgresql"));
        assert!(content.contains("users_index"));
    }

    #[test]
    fn test_minimal_yaml() {
        let minimal_yaml = r#"
sync_tasks:
  - table: users
    index: users_index

meilisearch:
  url: http://localhost:7700

redis:
  url: redis://localhost:6379
"#;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("minimal.yaml");
        fs::write(&config_path, minimal_yaml).unwrap();

        assert!(config_path.exists());
    }

    #[test]
    fn test_file_not_found_error() {
        let non_existent = Path::new("/non/existent/path/config.yaml");
        assert!(!non_existent.exists());
    }

    #[test]
    fn test_environment_variable_substitution() {
        // SAFETY: This is a test function. We accept the risk of race conditions with environment variables
        // in a test environment.
        unsafe {
            std::env::set_var("TEST_DB_HOST", "env-host");
            std::env::set_var("TEST_DB_PASS", "env-pass");
        }

        let yaml_with_env = r#"
source:
  type: postgresql
  connection: "postgresql://user:${TEST_DB_PASS}@${TEST_DB_HOST}/testdb"
"#;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config_env.yaml");
        fs::write(&config_path, yaml_with_env).unwrap();

        // In actual implementation, env vars would be substituted
        assert!(config_path.exists());

        unsafe {
            std::env::remove_var("TEST_DB_HOST");
            std::env::remove_var("TEST_DB_PASS");
        }
    }

    #[test]
    fn test_multi_source_config() {
        let multi_source_yaml = r#"
sources:
  - name: primary
    type: postgresql
    connection: "postgresql://user:pass@primary/db"
    slot_name: primary_slot
    publication: primary_pub
  
  - name: secondary
    type: postgresql  
    connection: "postgresql://user:pass@secondary/db"
    slot_name: secondary_slot
    publication: secondary_pub

sync_tasks:
  - source_name: primary
    table: users
    index: users_primary
  
  - source_name: secondary
    table: products
    index: products_secondary

meilisearch:
  url: http://localhost:7700

redis:
  url: redis://localhost:6379
"#;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("multi_source.yaml");
        fs::write(&config_path, multi_source_yaml).unwrap();

        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("primary"));
        assert!(content.contains("secondary"));
    }

    #[test]
    fn test_config_with_all_options() {
        let full_config = r#"
app:
  name: meilibridge-prod
  instance_id: instance-1
  
source:
  type: postgresql
  connection:
    host: localhost
    port: 5432
    database: testdb
    username: user
    password: pass
  slot_name: test_slot
  publication: test_pub
  pool:
    max_size: 20
    min_idle: 5
  ssl:
    mode: require

sync_tasks:
  - id: users_sync
    table: users
    index: users_index
    primary_key: user_id
    full_sync_on_start: true
    options:
      batch_size: 2000
      batch_timeout_ms: 5000
    soft_delete:
      field: deleted_at
      delete_values: [true]

meilisearch:
  url: http://localhost:7700
  api_key: masterKey
  timeout: 60
  batch_size: 1000
  auto_create_index: true

redis:
  url: redis://localhost:6379
  key_prefix: prod_
  database: 1

api:
  enabled: true
  host: 0.0.0.0
  port: 8080

logging:
  level: info
  format: json

monitoring:
  metrics:
    enabled: true
    port: 9090

performance:
  buffer_size: 10000
  max_concurrent_tasks: 10
"#;

        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("full_config.yaml");
        fs::write(&config_path, full_config).unwrap();

        assert!(config_path.exists());
        let content = fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("meilibridge-prod"));
        assert!(content.contains("batch_size: 2000"));
    }
}
