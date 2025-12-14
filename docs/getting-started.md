# Getting Started with MeiliBridge

This guide covers installation, setup, and deployment of MeiliBridge for both development and production environments.

## Table of Contents
- [Prerequisites](#prerequisites)
- [Development Setup](#development-setup)
- [Production Installation](#production-installation)
- [Docker Deployment](#docker-deployment)
- [Kubernetes Deployment](#kubernetes-deployment)
- [Configuration Basics](#configuration-basics)
- [Verification](#verification)
- [Port Allocation](#port-allocation)

## Prerequisites

### System Requirements
- **Operating System**: Linux, macOS, or Windows with WSL2
- **Memory**: Minimum 4GB RAM (8GB recommended)
- **Storage**: 10GB free space for data and logs
- **Network**: Stable internet connection for package downloads

### Required Software
- **Rust**: 1.85.0 or higher
- **PostgreSQL**: 10+ with logical replication support
- **Meilisearch**: 1.12+ (destination database)
- **Redis**: 6.0+ (for checkpoint storage and dead letter queue)
- **Docker**: 20.10+ (for containerized deployment)
- **Git**: For source code management

### PostgreSQL Configuration
Enable logical replication in `postgresql.conf`:
```ini
wal_level = logical
max_replication_slots = 4
max_wal_senders = 4
```

## Development Setup

### Using Docker Compose (Recommended)

1. **Clone the repository**:
```bash
git clone https://github.com/binary-touch/MeiliBridge.git
cd meilibridge
```

2. **Create environment file**:
```bash
cp .env.example .env
# Edit .env with your configuration
```

3. **Start services**:
```bash
docker-compose -f docker/compose.yaml up -d
```

This starts:
- PostgreSQL with logical replication enabled
- Meilisearch 
- Redis
- MeiliBridge

4. **View logs**:
```bash
docker-compose -f docker/compose.yaml logs -f meilibridge
```

### Building from Source

1. **Install Rust**:
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

2. **Clone and build**:
```bash
git clone https://github.com/binary-touch/MeiliBridge.git
cd meilibridge
cargo build --release
```

3. **Create configuration**:
```bash
cp config.example.yaml config.yaml
# Edit config.yaml with your settings
```

4. **Run MeiliBridge**:
```bash
# Run with default config search
./target/release/meilibridge run

# Run with specific config
./target/release/meilibridge --config config.yaml run

# Validate configuration
./target/release/meilibridge validate

# Generate sample config
./target/release/meilibridge generate-sample > config.yaml
```

## Production Installation

### Option 1: Pre-built Binary

1. **Download latest release**:
```bash
wget https://github.com/binary-touch/MeiliBridge/releases/latest/download/meilibridge-linux-amd64
chmod +x meilibridge-linux-amd64
sudo mv meilibridge-linux-amd64 /usr/local/bin/meilibridge
```

2. **Create system user**:
```bash
sudo useradd -r -s /bin/false meilibridge
```

3. **Setup directories**:
```bash
sudo mkdir -p /etc/meilibridge /var/log/meilibridge
sudo chown meilibridge:meilibridge /var/log/meilibridge
```

### Option 2: Systemd Service

1. **Create service file** `/etc/systemd/system/meilibridge.service`:
```ini
[Unit]
Description=MeiliBridge - PostgreSQL to Meilisearch Sync
After=network.target postgresql.service

[Service]
Type=simple
User=meilibridge
Group=meilibridge
ExecStart=/usr/local/bin/meilibridge --config /etc/meilibridge/config.yaml run
Restart=always
RestartSec=5
StandardOutput=append:/var/log/meilibridge/output.log
StandardError=append:/var/log/meilibridge/error.log

[Install]
WantedBy=multi-user.target
```

2. **Enable and start**:
```bash
sudo systemctl daemon-reload
sudo systemctl enable meilibridge
sudo systemctl start meilibridge
```

## Docker Deployment

### Production Docker Image

1. **Build image**:
```bash
docker build -f docker/Dockerfile -t meilibridge:latest .
```

2. **Run container**:
```bash
docker run -d \
  --name meilibridge \
  -v $(pwd)/config.yaml:/config.yaml \
  -v meilibridge-data:/data \
  -e MEILIBRIDGE_CONFIG=/config.yaml \
  -p 7708:7708 \
  --restart unless-stopped \
  meilibridge:latest run
```

### Docker Compose Production

Create `docker-compose.prod.yaml`:
```yaml
version: '3.8'

services:
  meilibridge:
    image: meilibridge:latest
    restart: unless-stopped
    volumes:
      - ./config.yaml:/config.yaml
      - meilibridge-data:/data
    environment:
      - MEILIBRIDGE_CONFIG=/config.yaml
      - RUST_LOG=info
    ports:
      - "7708:7708"
    networks:
      - meilibridge-net
    healthcheck:
      test: ["CMD", "curl", "-f", "http://localhost:7708/health"]
      interval: 30s
      timeout: 10s
      retries: 3

volumes:
  meilibridge-data:

networks:
  meilibridge-net:
    external: true
```

## Kubernetes Deployment

### Using Helm (Coming Soon)

Helm charts are planned for future releases. For now, use the manual Kubernetes manifests below.

### Manual Kubernetes Manifests

Create namespace and deployment:
```yaml
apiVersion: v1
kind: Namespace
metadata:
  name: meilibridge
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: meilibridge
  namespace: meilibridge
spec:
  replicas: 1
  selector:
    matchLabels:
      app: meilibridge
  template:
    metadata:
      labels:
        app: meilibridge
    spec:
      containers:
      - name: meilibridge
        image: meilibridge:latest
        ports:
        - containerPort: 7708
        env:
        - name: MEILIBRIDGE_CONFIG
          value: /config/config.yaml
        volumeMounts:
        - name: config
          mountPath: /config
        livenessProbe:
          httpGet:
            path: /health
            port: 7708
          initialDelaySeconds: 30
          periodSeconds: 10
        readinessProbe:
          httpGet:
            path: /ready
            port: 7708
          initialDelaySeconds: 5
          periodSeconds: 5
      volumes:
      - name: config
        configMap:
          name: meilibridge-config
```

## Configuration Basics

### Minimal Configuration

Create `config.yaml`:
```yaml
# PostgreSQL source
source:
  type: postgresql
  postgresql:
    host: localhost
    port: 5432
    database: myapp
    user: postgres
    password: ${POSTGRES_PASSWORD}
    replication:
      slot_name: meilibridge_slot
      publication_name: meilibridge_pub

# Meilisearch destination
meilisearch:
  url: http://localhost:7700
  api_key: ${MEILI_MASTER_KEY}

# Sync tasks
sync_tasks:
  - id: users_sync
    enabled: true
    source:
      table: public.users
    destination:
      index: users
    primary_key: id
    full_sync:
      enabled: true
      on_start: true
      page_size: 1000
    cdc:
      enabled: true
      auto_start: true
    field_mappings:
      - source: created_at
        destination: createdAt
        type: timestamp

# API configuration
api:
  enabled: true
  host: 0.0.0.0
  port: 7708
  cors:
    enabled: true
    allowed_origins: ["*"]

# Performance settings
performance:
  batch_size: 100
  batch_timeout_ms: 1000
  max_concurrent_batches: 10
  connection_pool_size: 10
```

### Environment Variables

MeiliBridge supports environment variable substitution:
```bash
export POSTGRES_PASSWORD=secret
export MEILI_MASTER_KEY=masterkey
export REDIS_URL=redis://localhost:6379
```

## Verification

### Check Service Status

1. **Health check**:
```bash
curl http://localhost:7708/health
```

2. **View metrics**:
```bash
curl http://localhost:7708/metrics
```

3. **Check sync status**:
```bash
curl http://localhost:7708/tasks
```

4. **View specific sync task**:
```bash
curl http://localhost:7708/tasks/users_sync
```

### Test Data Sync

1. **Insert test data in PostgreSQL**:
```sql
INSERT INTO users (name, email) VALUES ('Test User', 'test@example.com');
```

2. **Verify in Meilisearch**:
```bash
curl -H "Authorization: Bearer $MEILI_MASTER_KEY" \
  http://localhost:7700/indexes/users/search?q=Test
```

## Port Allocation

MeiliBridge follows Meilisearch's port numbering convention (770x range):

| Service | Port | Description |
|---------|------|-------------|
| API Server | 7708 | REST API and dashboard |
| Metrics | 7709 | Prometheus metrics endpoint |
| Debug | 7710 | pprof debugging (dev only) |

### Configuring Custom Ports

In `config.yaml`:
```yaml
api:
  port: 7708
  metrics_port: 7709
  debug_port: 7710  # Only in debug builds
```

## Performance Optimization

### Enable Parallel Processing

For high-volume tables, enable parallel processing to maximize throughput:

```yaml
# config.yaml
performance:
  parallel_processing:
    enabled: true
    workers_per_table: 4
    work_stealing: true
```

This will:
- Process events from each table using 4 parallel workers
- Automatically balance load between tables
- Significantly increase throughput for large-scale deployments

Monitor parallel processing:
```bash
# Check memory usage
curl http://localhost:7708/diagnostics/memory

# View connection pool status
curl http://localhost:7708/diagnostics/connections

# Get component health
curl http://localhost:7708/health/pipeline
curl http://localhost:7708/health/source
curl http://localhost:7708/health/destination
```

## PostgreSQL Data Type Support

MeiliBridge supports all common PostgreSQL data types and automatically converts them to appropriate JSON representations for Meilisearch. The type conversion is handled by the PostgreSQL adapter using the `postgres-types` crate:

### Scalar Types
- **Numeric**: `integer`, `bigint`, `smallint`, `numeric`, `real`, `double precision`
- **Text**: `text`, `varchar`, `char`
- **Boolean**: `boolean`
- **Date/Time**: `date`, `time`, `timestamp`, `timestamptz`, `interval`
- **UUID**: `uuid`
- **JSON**: `json`, `jsonb` (preserved as nested JSON)
- **Network**: `inet`, `macaddr`
- **Binary**: `bytea` (encoded as hex string)
- **Money**: `money` (converted to numeric)

### Array Types
MeiliBridge fully supports PostgreSQL arrays:
- `integer[]`, `text[]`, `boolean[]`, etc.
- Multi-dimensional arrays are flattened
- `jsonb[]` arrays preserve JSON structure

Example array handling:
```sql
-- PostgreSQL
CREATE TABLE products (
    id SERIAL PRIMARY KEY,
    name TEXT,
    tags TEXT[],
    prices NUMERIC[],
    metadata JSONB
);

INSERT INTO products VALUES 
    (1, 'Laptop', '{"electronics","computers"}', '{999.99,1199.99}', '{"brand":"Dell"}');
```

```json
// Meilisearch document
{
    "id": 1,
    "name": "Laptop",
    "tags": ["electronics", "computers"],
    "prices": [999.99, 1199.99],
    "metadata": {"brand": "Dell"}
}
```

### JSONB Support
JSONB columns are automatically parsed and included as nested objects:

```sql
-- PostgreSQL
CREATE TABLE users (
    id SERIAL PRIMARY KEY,
    profile JSONB
);

INSERT INTO users VALUES (1, '{"name": "John", "preferences": {"theme": "dark"}}');
```

```json
// Meilisearch document
{
    "id": 1,
    "profile": {
        "name": "John",
        "preferences": {
            "theme": "dark"
        }
    }
}
```

## Next Steps

- Read the [Configuration & Architecture Reference](./configuration-architecture.md) for detailed configuration options
- Check [CDC Implementation & Operations](./cdc-operations.md) for CDC-specific setup
- Review [API & Development Guide](./api-development.md) for API usage

## Troubleshooting Quick Reference

### Common Issues

1. **PostgreSQL Connection Failed**:
   - Check PostgreSQL is running and accessible
   - Verify credentials and database exists
   - Ensure logical replication is enabled in postgresql.conf
   - Check user has REPLICATION privilege

2. **Meilisearch Connection Failed**:
   - Verify Meilisearch is running
   - Check API key is correct
   - Ensure network connectivity
   - Test with: `curl -H "Authorization: Bearer $API_KEY" http://meilisearch:7700/health`

3. **No Data Syncing**:
   - Check replication slot exists: `SELECT * FROM pg_replication_slots;`
   - Verify table has replica identity: `SELECT relreplident FROM pg_class WHERE relname = 'your_table';`
   - Check publication includes table: `SELECT * FROM pg_publication_tables;`
   - Review logs with: `docker logs meilibridge` or `journalctl -u meilibridge`

4. **High Memory Usage**:
   - Reduce batch_size in config
   - Enable adaptive batching
   - Check memory limits: `curl http://localhost:7708/diagnostics/memory`

5. **Replication Lag**:
   - Monitor lag with metrics endpoint
   - Increase parallel workers
   - Enable work-stealing for better load distribution

For detailed troubleshooting, see [CDC Implementation & Operations](./cdc-operations.md).