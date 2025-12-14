<div align="center">
<h1>MeiliBridge</h1>
</div>

<div align="center">

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Docker](https://img.shields.io/badge/docker-%230db7ed.svg?style=flat&logo=docker&logoColor=white)](https://hub.docker.com/r/binarytouch/meilibridge)
[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![codecov](https://codecov.io/github/binary-touch/meilibridge/graph/badge.svg?token=E8W87QQO3G)](https://codecov.io/github/binary-touch/meilibridge)
[![GitHub Release](https://img.shields.io/github/v/release/binary-touch/meilibridge?style=flat&logo=github)](https://github.com/binary-touch/meilibridge/releases/latest)

**Lightning-fast PostgreSQL to Meilisearch sync engine**

Real-time data synchronization with automatic retries, parallel processing, and zero downtime

[Features](#-features) • [Quick Start](#-quick-start) • [Monitoring](#-monitoring--observability) • [Configuration](#-configuration) • [Contributing](#-contributing) • [Documentation](docs/)

</div>

---

### Core Capabilities

- **🚄 Real-time CDC** - Sub-second data synchronization using PostgreSQL logical replication
- **⚡ High Performance** - Process 10,000+ events/second with parallel work-stealing architecture
- **🔄 Automatic Recovery** - Built-in retry mechanisms with exponential backoff and circuit breakers
- **💾 Persistent State** - Redis-based checkpointing for seamless restarts and recovery
- **📊 Production Ready** - Comprehensive metrics, health checks, and monitoring integrations
- **🎯 Flexible Mapping** - Transform, filter, and enrich data with powerful pipeline configuration
- **🔌 Extensible** - Plugin system for custom transformations and data processing

### Data Integrity & Reliability

- **✅ At-Least-Once Delivery** - Transaction-based checkpointing with event deduplication to minimize duplicates
- **🔐 Atomic Operations** - Two-phase commit protocol ensures data consistency
- **🗄️ Multi-Source Support** - Sync from multiple PostgreSQL databases simultaneously
- **🗑️ Soft Delete Handling** - Configurable detection and transformation of soft deletes
- **📦 Dead Letter Queue** - Automatic handling of failed events with retry policies
- **🔍 Snapshot Isolation** - Consistent reads during full table synchronization
- **♻️ Checkpoint Management** - Automatic cleanup with retention policies and memory pressure triggers
- **🔄 CDC Resume** - Seamless resume from saved checkpoints after restart
- **⚡ Field Validation** - Automatic validation against PostgreSQL schema at startup

### Performance Optimization

- **📈 Adaptive Batching** - Dynamic batch sizing based on workload and latency
- **🧠 Smart Work Stealing** - Automatic load balancing across parallel workers
- **💪 Connection Pooling** - Optimized connection management for high throughput
- **🚦 Memory Efficient** - Streaming processing with bounded memory usage
- **⏱️ Sub-100ms P50 Latency** - Optimized for real-time synchronization
- **🎛️ Backpressure Control** - Automatic flow control with channel-based backpressure
- **📊 Memory Monitoring** - Real-time system memory tracking with adaptive batch sizing

### Operations & Monitoring

- **📡 Prometheus Metrics** - Comprehensive metrics for monitoring and alerting
- **🔧 REST API** - Full management API for runtime control and diagnostics
- **🏥 Health Checks** - Liveness and readiness probes for container orchestration
- **📋 Event Replay** - Replay events from specific checkpoints for recovery
- **🔍 Diagnostic Tools** - Built-in debugging and troubleshooting endpoints
- **📚 Structured Logging** - JSON-formatted logs with correlation IDs
- **🔄 Full Sync API** - Trigger full table synchronization via REST API
- **🔌 Startup Validation** - Comprehensive connectivity checks for all components

---

## 🚀 Quick Start

Get MeiliBridge running in under 2 minutes!

### Prerequisites

- PostgreSQL 10+ with logical replication enabled
- Meilisearch 1.12+ instance
- Docker (recommended) or Rust 1.82+ (for manual build)

### PostgreSQL Setup

Before starting MeiliBridge, prepare your PostgreSQL database:

```sql
-- 1. Enable logical replication in postgresql.conf
-- wal_level = logical
-- max_replication_slots = 4
-- max_wal_senders = 4

-- 2. Create a user with replication privileges
CREATE USER meilibridge WITH REPLICATION LOGIN PASSWORD 'your_password';

-- 3. Grant necessary permissions on your database
GRANT CONNECT ON DATABASE your_database TO meilibridge;
GRANT USAGE ON SCHEMA public TO meilibridge;
GRANT SELECT ON ALL TABLES IN SCHEMA public TO meilibridge;
ALTER DEFAULT PRIVILEGES IN SCHEMA public GRANT SELECT ON TABLES TO meilibridge;

-- 4. Create publication for the tables you want to sync
CREATE PUBLICATION meilibridge_pub FOR TABLE users, products, orders;
-- Or for all tables:
-- CREATE PUBLICATION meilibridge_pub FOR ALL TABLES;
```

**Note**: MeiliBridge will automatically create the replication slot if configured with `create_slot: true`.

### Docker (Recommended)

```bash
# Pull and run with minimal config
docker run -d \
  --name meilibridge \
  -e POSTGRES_URL="postgresql://user:pass@host:5432/db" \
  -e MEILISEARCH_URL="http://localhost:7700" \
  -e MEILISEARCH_API_KEY="your-api-key" \
  -p 7701:7701 \
  binarytouch/meilibridge:latest

# Check health status
curl http://localhost:7701/health

# View logs
docker logs -f meilibridge
```

### Docker Compose (Full Stack)

```bash
# Clone the repository
git clone https://github.com/binary-touch/meilibridge.git
cd meilibridge

# Copy example environment file
cp .env.example .env

# Start PostgreSQL, Meilisearch, Redis, and MeiliBridge
docker-compose up -d

# Verify all services are running
docker-compose ps

# Check synchronization status
curl http://localhost:7701/status
```

More reference at [Docker Documentation](docker/README.md)

### Configuration File Setup

Create a `config.yaml` file:

```yaml
# Minimal configuration
source:
  type: postgresql
  host: localhost
  port: 5432
  database: myapp
  username: postgres
  password: ${POSTGRES_PASSWORD}

meilisearch:
  url: http://localhost:7700
  api_key: ${MEILI_MASTER_KEY}

redis:
  url: redis://localhost:6379

sync_tasks:
  - table: users
    index: users
    primary_key: id
    full_sync_on_start: true
```

Run with configuration:

```bash
# Using Docker
docker run -d \
  --name meilibridge \
  -v $(pwd)/config.yaml:/config.yaml \
  -p 7701:7701 \
  binarytouch/meilibridge:latest --config /config.yaml

# Using binary
./meilibridge --config config.yaml
```

### Manual Installation

<details>
<summary><b>Build from Source</b></summary>

```bash
# Clone repository
git clone https://github.com/binary-touch/meilibridge.git
cd meilibridge

# Build in release mode
cargo build --release

# Run with configuration
./target/release/meilibridge --config config.yaml
```
</details>

<details>
<summary><b>Download Pre-built Binary</b></summary>

```bash
# Linux (x86_64)
curl -L https://github.com/binary-touch/meilibridge/releases/latest/download/meilibridge-linux-amd64.tar.gz -o meilibridge.tar.gz
tar -xzf meilibridge.tar.gz
chmod +x meilibridge

# macOS (Intel)
curl -L https://github.com/binary-touch/meilibridge/releases/latest/download/meilibridge-darwin-amd64.tar.gz -o meilibridge.tar.gz
tar -xzf meilibridge.tar.gz
chmod +x meilibridge

# macOS (Apple Silicon M1/M2/M3)
curl -L https://github.com/binary-touch/meilibridge/releases/latest/download/meilibridge-darwin-arm64.tar.gz -o meilibridge.tar.gz
tar -xzf meilibridge.tar.gz
chmod +x meilibridge

# Windows (PowerShell)
# Download the Windows binary
Invoke-WebRequest -Uri "https://github.com/binary-touch/meilibridge/releases/latest/download/meilibridge-windows-amd64.exe.zip" -OutFile "meilibridge.zip"
# Extract the zip file
Expand-Archive -Path "meilibridge.zip" -DestinationPath "."
# Run the executable
.\meilibridge.exe --config config.yaml

# Run on Unix-like systems
./meilibridge --config config.yaml
```

**Note**: 
- All Unix binaries are packaged as `.tar.gz` files
- Windows binary is packaged as `.zip` file
- Linux ARM64 can be built using `make build-linux-arm64` (see Building for ARM64 below)
</details>

### Verify Installation

```bash
# Check version
meilibridge --version

# Validate configuration
meilibridge validate --config config.yaml

# Generate sample configuration
meilibridge generate-sample > config.yaml

# Start with debug logging
meilibridge --config config.yaml --log-level debug
```

### Building for ARM64

<details>
<summary><b>Build Linux ARM64 Binary</b></summary>

```bash
# Install build dependencies (one-time setup)
make install-deps

# Build using Docker
make build-linux-arm64

# Package the binary
make package-linux-arm64

# The packaged binary will be in dist/meilibridge-linux-arm64.tar.gz
```

**Requirements:**
- Docker must be installed and running
- Docker buildx support (included in recent Docker versions)
</details>

<details>
<summary><b>Build Multi-Architecture Docker Images</b></summary>

```bash
# Build multi-arch Docker image locally (AMD64 + ARM64)
make docker-build-multiarch

# Build and push to Docker Hub
make docker-push

# Verify the multi-arch image
make docker-verify

# Custom registry and image name
DOCKER_USERNAME=myuser DOCKER_REGISTRY=ghcr.io make docker-push
```

The multi-architecture Docker image supports:
- `linux/amd64` - For standard x86_64 servers
- `linux/arm64` - For ARM64 servers (AWS Graviton, Apple Silicon, etc.)

Docker will automatically pull the correct architecture for your platform.
</details>

### Next Steps

- 📚 Read the [Getting Started Guide](docs/getting-started.md) for detailed setup
- 📊 Set up [monitoring](#monitoring--observability) with Prometheus
- 🚀 Check our [deployment examples](docker/) for production deployment

---

## 🎮 Quick Demo Start

```bash
# Clone and start the demo
git clone https://github.com/binary-touch/meilibridge.git
cd meilibridge/demo
./start.sh

# Search products (after ~30 seconds)
curl -X POST 'http://localhost:7700/indexes/products/search' \
  -H 'Authorization: Bearer masterKey123' \
  -H 'Content-Type: application/json' \
  -d '{"q":"laptop","offset":0,"limit":20}'
```

### Demo Features

- **Automatic Data Generation**: Simulates real-world e-commerce activity
- **Error Recovery**: Test fault tolerance by stopping/starting services  
- **Performance Testing**: Handle bulk operations and high-frequency updates
- **Visual Interface**: Browse and search data at http://localhost:24900

👉 **[Full Demo Documentation](demo/README.md)** - Detailed scenarios, troubleshooting, and advanced usage

---

## 📊 Monitoring & Observability

MeiliBridge provides comprehensive monitoring capabilities:

### Prometheus Metrics

```yaml
# Enable metrics in config.yaml
monitoring:
  metrics_enabled: true
  metrics_interval_seconds: 60
```

Available metrics:
- `meilibridge_events_processed_total` - Total events processed
- `meilibridge_events_failed_total` - Failed events count
- `meilibridge_sync_lag_seconds` - Replication lag in seconds
- `meilibridge_batch_size` - Current batch size
- `meilibridge_checkpoint_lag` - Checkpoint delay

### Health Endpoints

- `GET /health` - Overall system health
- `GET /health/liveness` - Kubernetes liveness probe
- `GET /health/readiness` - Kubernetes readiness probe
- `GET /health/{component}` - Component-specific health (postgresql, meilisearch, redis)

### Grafana Dashboard

Import our [Grafana dashboard](monitoring/grafana-dashboard.json) for visualizing:
- Event throughput and latency
- Error rates and recovery metrics
- Resource utilization
- Sync task status

### REST API Endpoints

**Base URL**: `http://localhost:7701`

#### Sync Task Management
- `GET /tasks` - List all sync tasks
- `GET /tasks/{id}` - Get sync task details
- `POST /tasks` - Create new sync task
- `PUT /tasks/{id}` - Update sync task
- `DELETE /tasks/{id}` - Delete sync task
- `POST /tasks/{id}/pause` - Pause sync task
- `POST /tasks/{id}/resume` - Resume sync task
- `POST /tasks/{id}/full-sync` - Trigger full table sync
- `GET /tasks/{id}/stats` - Get task statistics

#### CDC Control
- `POST /cdc/pause` - Pause all CDC processing
- `POST /cdc/resume` - Resume all CDC processing
- `GET /cdc/status` - Get CDC status and replication info

#### Dead Letter Queue
- `GET /dead-letters` - Get DLQ statistics
- `POST /dead-letters/{task_id}/reprocess` - Reprocess failed events

#### Cache Management
- `GET /cache/stats` - Get statement cache statistics
- `POST /cache/clear` - Clear statement cache

#### Parallel Processing
- `GET /parallel/status` - Get parallel processing status
- `GET /parallel/queues` - Get queue sizes for all tables

More info at [API Documentation](docs/api-development.md)

### Logging

Configure structured logging:

```yaml
logging:
  level: info  # trace, debug, info, warn, error
  format: json # json or pretty
```

Use correlation IDs to trace requests:
```bash
grep "correlation_id=abc123" logs.json
```

---

## ⚙️ Configuration

Create a `config.yaml` file with your settings:

```yaml
# Basic connection settings
source:
  type: postgresql
  host: localhost
  port: 5432
  database: myapp
  username: postgres
  password: ${POSTGRES_PASSWORD}  # Environment variable support

meilisearch:
  url: http://localhost:7700
  api_key: ${MEILI_MASTER_KEY}

redis:
  url: redis://localhost:6379

# Define sync tasks
sync_tasks:
  - table: users
    index: users
    primary_key: id
    full_sync_on_start: true
```

### Configuration Reference

<details>
<summary><b>Source Configuration</b></summary>

#### PostgreSQL Source

```yaml
source:
  type: postgresql
  # Connection parameters
  host: localhost               # PostgreSQL host
  port: 5432                    # PostgreSQL port
  database: myapp               # Database name
  username: postgres            # Username (needs REPLICATION privilege)
  password: ${POSTGRES_PASSWORD} # Password (supports ${ENV_VAR})
  
  # Replication settings
  slot_name: meilibridge_slot   # Replication slot name (default: "meilibridge")
  publication: meilibridge_pub  # Publication name (default: "meilibridge_pub")
  
  # Connection pool settings
  pool:
    max_size: 10                # Maximum connections
    min_idle: 1                 # Minimum idle connections
    connection_timeout: 30      # Connection timeout (seconds)
    idle_timeout: 600           # Idle connection timeout (seconds)
  
  # SSL/TLS configuration
  ssl:
    mode: disable               # disable, prefer, require, verify-ca, verify-full
    ca_cert: /path/to/ca.crt    # CA certificate path
    client_cert: /path/to/cert  # Client certificate
    client_key: /path/to/key    # Client key
  
  # Statement cache
  statement_cache:
    enabled: true               # Enable prepared statement caching
    max_size: 100               # Maximum cached statements
```

#### Multiple Sources (Multi-database)

```yaml
sources:
  - name: primary               # Unique source identifier
    type: postgresql
    host: primary.db.com
    port: 5432
    database: main
    username: replicator
    password: ${PRIMARY_PASSWORD}
    slot_name: meilibridge_primary
    publication: meilibridge_pub_primary
    
  - name: secondary
    type: postgresql
    host: secondary.db.com
    port: 5432
    database: analytics
    username: replicator
    password: ${SECONDARY_PASSWORD}
    slot_name: meilibridge_secondary
    publication: meilibridge_pub_secondary
```
</details>

<details>
<summary><b>Destination Configuration</b></summary>

#### Meilisearch Destination

```yaml
meilisearch:
  url: http://localhost:7700    # Meilisearch URL
  api_key: ${MEILI_MASTER_KEY} # API key (supports ${ENV_VAR})
  timeout: 30                   # Request timeout (seconds)
  max_connections: 10           # Connection pool size
  batch_size: 1000              # Batch size for bulk operations
  auto_create_index: true       # Auto-create missing indexes
  primary_key: id               # Default primary key field
  
  # Index settings template (applied to new indexes)
  index_settings:
    searchable_attributes: []   # Fields to search
    displayed_attributes: []    # Fields to return
    filterable_attributes: []   # Fields for filtering
    sortable_attributes: []     # Fields for sorting
    ranking_rules: []           # Custom ranking rules
    stop_words: []              # Stop words list
    synonyms: {}                # Synonyms mapping
  
  # Circuit breaker (fault tolerance)
  circuit_breaker:
    enabled: true               # Enable circuit breaker
    error_rate: 0.5             # Open circuit at 50% error rate
    min_request_count: 10       # Min requests before evaluation
    consecutive_failures: 5     # Or 5 consecutive failures
    timeout_secs: 60            # Time before half-open state
```
</details>

<details>
<summary><b>Sync Task Configuration</b></summary>

```yaml
sync_tasks:
  - id: users_sync              # Unique task ID
    source_name: primary        # Source name (for multi-source setups)
    table: public.users         # Source table (schema.table)
    index: users                # Target Meilisearch index
    primary_key: id             # Primary key field
    
    # Sync behavior
    full_sync_on_start: true    # Perform full sync on startup
    auto_start: true            # Auto-start this task
    
    # Soft delete detection
    soft_delete:
      field: status             # Field to check
      delete_values:            # Values indicating deletion
        - DELETED
        - INACTIVE
      handle_on_full_sync: true # Filter during full sync
      handle_on_cdc: true       # Convert to DELETE during CDC
    
    # Filtering
    filter:
      event_types: [create, update, delete]  # Event types to process
      conditions:
        - field: deleted
          op: not_equals
          value: true           # Skip soft-deleted records
    
    # Field transformations
    transform:
      fields:
        email:
          type: lowercase       # Convert email to lowercase
        full_name:
          type: compute
          expression: "concat(first_name, ' ', last_name)"
    
    # Field mapping
    mapping:
      fields:
        user_id: id            # Rename user_id to id
        created_at: created_timestamp
      unmapped_fields_strategy: include  # include/exclude/prefix
    
    # Processing options
    options:
      batch_size: 1000          # Events per batch
      batch_timeout_ms: 1000    # Batch timeout (milliseconds)
      retry:
        max_retries: 3          # Max retry attempts
        initial_delay: 1000     # Initial retry delay (ms)
        max_delay: 60000        # Maximum retry delay (ms)
        multiplier: 2.0         # Backoff multiplier
```
</details>

<details>
<summary><b>Advanced Configuration</b></summary>

#### Redis Configuration

```yaml
redis:
  url: redis://localhost:6379   # Redis URL
  password: ${REDIS_PASSWORD}   # Redis password
  database: 0                   # Redis database number
  key_prefix: meilibridge       # Key prefix for all keys
  
  pool:
    max_size: 10               # Maximum connections
    min_idle: 1                # Minimum idle connections
    connection_timeout: 5      # Connection timeout (seconds)
  
  # Checkpoint retention (New!)
  checkpoint_retention:
    max_checkpoints_per_task: 10  # Keep last 10 checkpoints per task
    cleanup_on_memory_pressure: true  # Auto-cleanup when memory is high
    memory_pressure_threshold: 80.0   # Trigger cleanup at 80% memory
```

#### Performance Tuning

```yaml
performance:
  parallel_processing:
    enabled: true              # Enable parallel processing
    workers_per_table: 4       # Worker threads per table
    max_concurrent_events: 1000 # Max concurrent events
    work_stealing: true        # Enable work stealing
    work_steal_interval_ms: 100 # Work steal check interval
    work_steal_threshold: 50   # Min queue size difference
  
  batch_processing:
    default_batch_size: 100    # Default batch size
    max_batch_size: 1000       # Maximum batch size
    min_batch_size: 10         # Minimum batch size
    batch_timeout_ms: 5000     # Batch timeout
    adaptive_batching: true    # Dynamic batch sizing
    
    adaptive_config:
      target_latency_ms: 1000  # Target processing time
      adjustment_factor: 0.2   # Adjustment aggressiveness (0-1)
      metric_window_size: 10   # Metrics to average
      adjustment_interval_ms: 5000 # Min time between adjustments
      memory_pressure_threshold: 80.0 # Memory % to reduce batch
      per_table_optimization: true # Per-table batch sizing
  
  connection_pool:
    max_connections: 20        # Max connections
    min_connections: 5         # Min connections
    connection_timeout: 30     # Timeout (seconds)
    idle_timeout: 600          # Idle timeout (seconds)
```

#### API Server Configuration

```yaml
api:
  enabled: true                # Enable REST API
  host: 0.0.0.0               # API host
  port: 7701                  # API port
  
  cors:
    enabled: true              # Enable CORS
    origins: ["*"]             # Allowed origins
  
  auth:
    enabled: false             # Enable authentication
    jwt_secret: ${JWT_SECRET}  # JWT secret key
    token_expiry: 3600         # Token expiry (seconds)
    api_keys:
      - name: admin
        key: ${API_KEY}        # API key
        permissions: ["read", "write", "admin"]
```

#### Monitoring & Logging

```yaml
logging:
  level: info                  # Log level (trace/debug/info/warn/error)
  format: pretty               # Log format (pretty/json)
  
monitoring:
  metrics_enabled: true        # Enable Prometheus metrics
  metrics_interval_seconds: 60 # Metrics collection interval
  health_checks_enabled: true  # Enable health checks
  health_check_interval_seconds: 30 # Health check interval

features:
  auto_recovery: true         # Auto-recover from failures
  health_checks: true         # Enable health endpoints
  metrics_export: true        # Export Prometheus metrics
  distributed_mode: false     # Enable distributed mode
```

#### At-Least-Once Delivery with Deduplication

```yaml
at_least_once_delivery:       # At-least-once delivery with deduplication
  enabled: true               # Enable at-least-once delivery
  deduplication_window: 10000 # Events to track for deduplication
  transaction_timeout_secs: 30 # Transaction timeout
  two_phase_commit: true      # Use two-phase commit protocol
  checkpoint_before_write: true # Atomic checkpoint before write
```

#### Error Handling

```yaml
error_handling:
  retry:
    enabled: true
    max_attempts: 3
    initial_backoff_ms: 100
    max_backoff_ms: 30000
    backoff_multiplier: 2.0
    jitter_factor: 0.1
  
  dead_letter_queue:
    enabled: true
    storage: memory           # memory or redis
    max_entries_per_task: 10000
    retention_hours: 24
    auto_reprocess_interval_minutes: 0 # 0 = disabled
  
  circuit_breaker:
    enabled: false            # Global circuit breaker
    failure_threshold_percent: 50
    min_requests: 10
    reset_timeout_seconds: 60
    half_open_max_requests: 3
```
</details>

### Environment Variables

All configuration values support environment variable substitution:

```yaml
password: ${POSTGRES_PASSWORD}
api_key: ${MEILI_MASTER_KEY:-default_value}  # With default
```

Common environment variables:
- `MEILIBRIDGE_CONFIG` - Config file path
- `MEILIBRIDGE_LOG_LEVEL` - Log level
- `POSTGRES_PASSWORD` - PostgreSQL password
- `MEILI_MASTER_KEY` - Meilisearch API key
- `REDIS_PASSWORD` - Redis password

---

## 🔧 Command Line Options

```bash
meilibridge [OPTIONS] [COMMAND]

OPTIONS:
    -c, --config <FILE>      Configuration file path
    -l, --log-level <LEVEL>  Log level (trace/debug/info/warn/error)
    -h, --help              Print help information
    -V, --version           Print version information

COMMANDS:
    run             Run the synchronization service (default)
    validate        Validate configuration file
    generate-sample Generate sample configuration
    version         Show version information
```

---

## 🤝 Contributing

We welcome contributions! Please see our [Contributing Guide](CONTRIBUTING.md) for details on:

- 📝 How to submit bug reports and feature requests
- 🔧 Setting up your development environment
- 🚀 Our development workflow and coding standards
- ✅ Testing requirements and guidelines

For a quick start:
```bash
git clone https://github.com/YOUR_USERNAME/meilibridge.git
cd meilibridge
cargo build
cargo test
```

**Code of Conduct**: Please treat everyone with respect and kindness.

---

## 📚 Documentation

- **[Getting Started Guide](docs/getting-started.md)** - Detailed installation and setup
- **[Configuration & Architecture](docs/configuration-architecture.md)** - Deep dive into configuration
- **[API Reference](docs/api-development.md)** - REST API documentation
- **[CDC Operations](docs/cdc-operations.md)** - CDC setup and troubleshooting
- **[Contributing Guide](CONTRIBUTING.md)** - How to contribute to the project

---

## 🆘 Getting Help

- 📋 [GitHub Issues](https://github.com/binary-touch/meilibridge/issues) - Report bugs or request features
- 💬 [Discussions](https://github.com/binary-touch/meilibridge/discussions) - Ask questions and share ideas
- 📖 [Documentation](docs/) - Browse all documentation

---

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

---

<div align="center">
Made with ❤️ in India
</div>
