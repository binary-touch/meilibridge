# API & Development Guide

This guide covers the MeiliBridge REST API, development practices, testing strategies, and future recommendations.

## Table of Contents
- [API Documentation](#api-documentation)
- [Authentication & Security](#authentication--security)
- [API Endpoints](#api-endpoints)
- [WebSocket Support](#websocket-support)
- [Development Guide](#development-guide)
- [Testing Strategies](#testing-strategies)
- [Client SDKs](#client-sdks)
- [Future Recommendations](#future-recommendations)

## API Documentation

### Overview

MeiliBridge provides a comprehensive REST API for managing sync tasks, monitoring health, and accessing metrics. The API follows RESTful principles and returns JSON responses.

**Base URL**: `http://localhost:7708`

### API Design Principles

1. **RESTful**: Standard HTTP methods (GET, POST, PUT, DELETE)
2. **Consistent**: Predictable URL patterns and response formats
3. **Versioned**: API version in URL path
4. **Documented**: OpenAPI/Swagger specification available
5. **Secure**: Authentication required for write operations

### Response Format

All API responses follow this structure:

**Success Response**:
```json
{
  "success": true,
  "data": { ... },
  "meta": {
    "timestamp": "2024-01-01T00:00:00Z",
    "version": "1.0.0"
  }
}
```

**Error Response**:
```json
{
  "success": false,
  "error": {
    "code": "TASK_NOT_FOUND",
    "message": "Task with ID 'users_sync' not found",
    "details": { ... }
  },
  "meta": {
    "timestamp": "2024-01-01T00:00:00Z",
    "request_id": "req_123abc"
  }
}
```

### HTTP Status Codes

| Status Code | Description |
|-------------|-------------|
| 200 OK | Successful request |
| 201 Created | Resource created successfully |
| 204 No Content | Successful request with no response body |
| 400 Bad Request | Invalid request parameters |
| 401 Unauthorized | Missing or invalid authentication |
| 403 Forbidden | Insufficient permissions |
| 404 Not Found | Resource not found |
| 409 Conflict | Resource conflict (e.g., duplicate) |
| 422 Unprocessable Entity | Validation error |
| 429 Too Many Requests | Rate limit exceeded |
| 500 Internal Server Error | Server error |

## Authentication & Security

### Bearer Token Authentication

Include the API token in the Authorization header:
```http
Authorization: Bearer your-api-token
```

**Example**:
```bash
curl -H "Authorization: Bearer ${API_TOKEN}" \
  http://localhost:7708/tasks
```

### API Token Configuration

```yaml
api:
  auth:
    enabled: true
    type: "bearer"
    tokens:
      - name: "admin"
        token: "${API_ADMIN_TOKEN}"
        role: "admin"
        permissions: ["read", "write", "admin"]
      
      - name: "readonly"
        token: "${API_READONLY_TOKEN}"
        role: "read"
        permissions: ["read"]
      
      - name: "operator"
        token: "${API_OPERATOR_TOKEN}"
        role: "operator"
        permissions: ["read", "write"]
```

### Role-Based Permissions

| Role | Permissions |
|------|-------------|
| admin | Full access to all endpoints |
| operator | Read/write access, no admin endpoints |
| read | Read-only access to all endpoints |

## API Endpoints

### Health & Monitoring

#### GET /health
Health check endpoint.

**Response**:
```json
{
  "status": "healthy",
  "components": {
    "postgresql": {
      "status": "healthy",
      "message": null,
      "details": {
        "pool_size": 10,
        "pool_available": 8
      }
    },
    "meilisearch": {
      "status": "healthy",
      "message": null,
      "details": {
        "version": "1.5.0"
      }
    },
    "api": {
      "status": "healthy",
      "message": null,
      "details": {
        "uptime_seconds": 3600
      }
    }
  },
  "version": "1.0.0",
  "uptime_seconds": 3600
}
```

#### GET /health/{component}
Get health status for a specific component.

**Parameters**:
- `component`: Component name (postgresql, meilisearch, redis, api)

#### GET /metrics
Prometheus-compatible metrics endpoint.

**Response**: Prometheus text format
```
# HELP meilibridge_cdc_events_total Total number of CDC events received
# TYPE meilibridge_cdc_events_total counter
meilibridge_cdc_events_total{table="users",event_type="insert"} 1234

# HELP meilibridge_cdc_lag_bytes CDC replication lag in bytes
# TYPE meilibridge_cdc_lag_bytes gauge
meilibridge_cdc_lag_bytes{slot="meilibridge_slot"} 1024
```

### Task Management

#### GET /tasks
List all sync tasks.

**Query Parameters**:
- `status`: Filter by status (active, paused, failed)
- `table`: Filter by table name
- `page`: Page number (default: 1)
- `limit`: Items per page (default: 20)

**Response**:
```json
{
  "success": true,
  "data": {
    "tasks": [
      {
        "id": "users_sync",
        "status": "active",
        "table": "public.users",
        "index": "users",
        "events_processed": 12345,
        "last_error": null,
        "last_sync_at": "2024-01-01T00:00:00Z",
        "created_at": "2024-01-01T00:00:00Z",
        "config": { ... }
      }
    ],
    "pagination": {
      "page": 1,
      "limit": 20,
      "total": 5,
      "pages": 1
    }
  }
}
```

#### GET /tasks/{id}
Get specific task details.

**Response**:
```json
{
  "success": true,
  "data": {
    "id": "users_sync",
    "status": "active",
    "table": "public.users",
    "index": "users",
    "primary_key": "id",
    "events_processed": 12345,
    "events_failed": 10,
    "last_error": null,
    "last_sync_at": "2024-01-01T00:00:00Z",
    "created_at": "2024-01-01T00:00:00Z",
    "position": {
      "lsn": "0/1234567",
      "xid": 789
    },
    "statistics": {
      "insert_count": 5000,
      "update_count": 6000,
      "delete_count": 1345,
      "avg_latency_ms": 125
    },
    "config": {
      "full_sync_on_start": true,
      "batch_size": 1000,
      "batch_timeout_ms": 1000
    }
  }
}
```

#### POST /tasks
Create a new sync task.

**Request Body**:
```json
{
  "id": "products_sync",
  "table": "public.products",
  "index": "products",
  "primary_key": "sku",
  "full_sync_on_start": true,
  "auto_start": true,
  "filter": {
    "event_types": ["create", "update"],
    "conditions": [
      {
        "field": "active",
        "op": "equals",
        "value": true
      }
    ]
  },
  "transform": {
    "fields": {
      "public.products": {
        "price": {
          "type": "multiply",
          "factor": 100,
          "to": "price_cents"
        }
      }
    }
  },
  "options": {
    "batch_size": 500,
    "batch_timeout_ms": 2000
  }
}
```

#### PUT /tasks/{id}
Update task configuration.

**Request Body**: Same as POST /tasks

#### DELETE /tasks/{id}
Delete a sync task.

**Query Parameters**:
- `force`: Force delete even if task is active (default: false)

#### POST /tasks/{id}/pause
Pause a sync task.

**Response**:
```json
{
  "success": true,
  "data": {
    "id": "users_sync",
    "status": "paused",
    "paused_at": "2024-01-01T00:00:00Z"
  }
}
```

#### POST /tasks/{id}/resume
Resume a paused sync task.

#### POST /tasks/{id}/full-sync
Trigger a full synchronization for a task.

**Request Body** (optional):
```json
{
  "start_from": "2024-01-01T00:00:00Z",
  "batch_size": 5000,
  "where_clause": "created_at > '2024-01-01'"
}
```

#### GET /tasks/{id}/stats
Get detailed statistics for a task.

**Response**:
```json
{
  "success": true,
  "data": {
    "events_per_second": 125.5,
    "avg_latency_ms": 45,
    "p95_latency_ms": 120,
    "p99_latency_ms": 250,
    "error_rate": 0.001,
    "lag_bytes": 1024,
    "lag_seconds": 2.5,
    "time_series": {
      "events_per_minute": [120, 135, 110, ...],
      "errors_per_minute": [0, 1, 0, ...]
    }
  }
}
```

### Dead Letter Queue

#### GET /dead-letters
Get dead letter queue statistics.

**Response**:
```json
{
  "success": true,
  "data": {
    "total_entries": 25,
    "entries_by_task": {
      "users_sync": 10,
      "orders_sync": 15
    },
    "entries_by_error": {
      "Serialization error": 12,
      "Network timeout": 8,
      "Validation error": 5
    },
    "oldest_entry": "2024-01-01T00:00:00Z",
    "newest_entry": "2024-01-02T00:00:00Z"
  }
}
```

#### POST /dead-letters/{task_id}/reprocess
Reprocess dead letter entries for a task.

**Request Body**:
```json
{
  "limit": 100,
  "error_type": "Network timeout"
}
```

### CDC Control

#### POST /cdc/pause
Pause all CDC consumption.

#### POST /cdc/resume
Resume CDC consumption.

#### GET /cdc/status
Get CDC status and statistics.

**Response**:
```json
{
  "success": true,
  "data": {
    "status": "active",
    "slots": [
      {
        "name": "meilibridge_slot",
        "active": true,
        "lag_bytes": 1024,
        "lag_seconds": 2.5,
        "restart_lsn": "0/1234567",
        "confirmed_flush_lsn": "0/1234560"
      }
    ],
    "publications": [
      {
        "name": "meilibridge_pub",
        "tables": ["public.users", "public.orders"]
      }
    ]
  }
}
```

### Source Management

#### GET /sources
List configured sources.

#### GET /sources/{id}
Get source details and status.

#### POST /sources/test
Test source connection.

**Request Body**:
```json
{
  "type": "postgresql",
  "config": {
    "host": "localhost",
    "port": 5432,
    "database": "test",
    "user": "postgres",
    "password": "secret"
  }
}
```

### Parallel Processing

#### GET /parallel/status

Get parallel processing status and configuration.

**Response**:
```json
{
  "success": true,
  "data": {
    "enabled": true,
    "workers_per_table": 4,
    "max_concurrent_events": 1000,
    "work_stealing_enabled": true,
    "tables": [
      {
        "table_name": "public.users",
        "queue_size": 125,
        "workers": 4
      },
      {
        "table_name": "public.orders",
        "queue_size": 50,
        "workers": 4
      }
    ]
  }
}
```

#### GET /parallel/queues

Get current queue sizes for all tables.

**Response**:
```json
{
  "success": true,
  "data": {
    "queues": {
      "public.users": 125,
      "public.orders": 50,
      "public.products": 0
    },
    "total_events": 175
  }
}
```

### Metrics

#### GET /metrics

Get Prometheus metrics in text format.

**Response**:
```text
# HELP meilibridge_cdc_events_total Total number of CDC events received
# TYPE meilibridge_cdc_events_total counter
meilibridge_cdc_events_total{table="public.users",event_type="insert"} 1234
meilibridge_cdc_events_total{table="public.users",event_type="update"} 567

# HELP meilibridge_parallel_queue_size Current number of events in parallel processing queue
# TYPE meilibridge_parallel_queue_size gauge
meilibridge_parallel_queue_size{table="public.users"} 125
meilibridge_parallel_queue_size{table="public.orders"} 50

# HELP meilibridge_parallel_worker_events_total Total number of events processed by parallel workers
# TYPE meilibridge_parallel_worker_events_total counter
meilibridge_parallel_worker_events_total{table="public.users",worker_id="0"} 5000
meilibridge_parallel_worker_events_total{table="public.users",worker_id="1"} 4800

# HELP meilibridge_statement_cache_size Current number of cached prepared statements
# TYPE meilibridge_statement_cache_size gauge
meilibridge_statement_cache_size 42

# HELP meilibridge_statement_cache_hits_total Total number of statement cache hits
# TYPE meilibridge_statement_cache_hits_total counter
meilibridge_statement_cache_hits_total 8234

# HELP meilibridge_statement_cache_hit_rate Statement cache hit rate (0.0 to 1.0)
# TYPE meilibridge_statement_cache_hit_rate gauge
meilibridge_statement_cache_hit_rate 0.85
```

### Statement Cache Management

MeiliBridge includes a prepared statement cache for PostgreSQL queries to improve performance:

#### GET /cache/stats

Get statement cache statistics.

**Response**:
```json
{
  "size": 42,
  "hits": 8234,
  "misses": 1412,
  "evictions": 12,
  "hit_rate": 0.85,
  "enabled": true,
  "max_size": 100
}
```

#### POST /cache/clear

Clear the statement cache.

**Response**:
```json
{
  "message": "Statement cache cleared",
  "cleared_count": 42
}
```

## WebSocket Support

### Real-time Event Streaming

Connect to WebSocket endpoint for real-time CDC events:
```javascript
const ws = new WebSocket('ws://localhost:7708/ws/events');

ws.on('message', (data) => {
  const event = JSON.parse(data);
  console.log('CDC Event:', event);
});
```

**Event Format**:
```json
{
  "type": "cdc_event",
  "data": {
    "id": "evt_123",
    "table": "users",
    "action": "insert",
    "data": { ... },
    "timestamp": "2024-01-01T00:00:00Z"
  }
}
```

### Subscribing to Specific Tables

```javascript
ws.send(JSON.stringify({
  "action": "subscribe",
  "tables": ["users", "orders"]
}));
```

## Development Guide

### Project Structure

```
meilibridge/
├── src/
│   ├── api/           # API server implementation
│   ├── config/        # Configuration structures
│   ├── source/        # Source adapters (PostgreSQL, etc.)
│   ├── destination/   # Destination adapters (Meilisearch)
│   ├── pipeline/      # Event processing pipeline
│   ├── dlq/           # Dead letter queue
│   ├── metrics/       # Prometheus metrics
│   ├── health/        # Health checks
│   └── main.rs        # Application entry point
├── tests/
│   ├── unit/          # Unit tests
│   ├── integration/   # Integration tests
│   └── e2e/           # End-to-end tests
├── docs/              # Documentation
├── scripts/           # Utility scripts
└── Cargo.toml         # Rust dependencies
```

### Development Setup

1. **Install Dependencies**:
```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install development tools
cargo install cargo-watch cargo-tarpaulin cargo-audit
```

2. **Run Development Server**:
```bash
# Watch mode with auto-reload
cargo watch -x run

# With specific log level
RUST_LOG=debug cargo run
```

3. **Code Formatting**:
```bash
# Format code
cargo fmt

# Check formatting
cargo fmt -- --check
```

4. **Linting**:
```bash
# Run clippy
cargo clippy -- -D warnings

# Fix clippy warnings
cargo clippy --fix
```

### Best Practices

#### Error Handling

Use the custom error types:
```rust
use crate::error::{MeiliBridgeError, Result};

pub async fn process_event(event: Event) -> Result<()> {
    validate_event(&event)
        .map_err(|e| MeiliBridgeError::Validation(e.to_string()))?;
    
    // Process event
    Ok(())
}
```

#### Logging

Use structured logging with tracing:
```rust
use tracing::{info, debug, error, warn, instrument};

#[instrument(
    name = "process_event",
    skip(event),
    fields(
        table = %event.table,
        event_type = ?event.event_type
    )
)]
pub async fn handle_event(event: Event) -> Result<()> {
    debug!("Starting event processing");
    
    let start = std::time::Instant::now();
    
    match process_event(event).await {
        Ok(_) => {
            info!(
                duration_ms = start.elapsed().as_millis(),
                "Event processed successfully"
            );
        }
        Err(e) => {
            error!(
                error = %e,
                duration_ms = start.elapsed().as_millis(),
                "Failed to process event"
            );
        }
    }
    
    Ok(())
}
```

#### Async Best Practices

1. **Use tokio for async runtime**:
```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Application code
}
```

2. **Avoid blocking operations**:
```rust
// Bad
std::thread::sleep(Duration::from_secs(1));

// Good
tokio::time::sleep(Duration::from_secs(1)).await;
```

3. **Use channels for communication**:
```rust
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::channel(100);

// Producer
tokio::spawn(async move {
    tx.send(event).await.unwrap();
});

// Consumer
while let Some(event) = rx.recv().await {
    process_event(event).await?;
}
```

## Testing Strategies

### Unit Testing

Test individual components in isolation:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_filter() {
        let filter = EventFilter::new(FilterConfig {
            event_types: vec!["insert".to_string()],
            ..Default::default()
        });
        
        let event = Event {
            event_type: EventType::Insert,
            ..Default::default()
        };
        
        assert!(filter.should_process(&event));
    }
    
    #[tokio::test]
    async fn test_async_processor() {
        let processor = EventProcessor::new();
        let result = processor.process(test_event()).await;
        assert!(result.is_ok());
    }
}
```

### Integration Testing

Test component interactions:

```rust
#[tokio::test]
async fn test_cdc_to_meilisearch_flow() {
    // Setup test environment
    let env = TestEnvironment::new().await;
    
    // Insert data in PostgreSQL
    env.pg_client
        .execute("INSERT INTO users (name) VALUES ($1)", &[&"Test User"])
        .await
        .unwrap();
    
    // Wait for sync
    tokio::time::sleep(Duration::from_secs(2)).await;
    
    // Verify in Meilisearch
    let results = env.meili_client
        .index("users")
        .search()
        .with_query("Test User")
        .execute::<User>()
        .await
        .unwrap();
    
    assert_eq!(results.hits.len(), 1);
}
```

### End-to-End Testing

Full system tests using Docker:

```bash
# Run E2E tests
cargo test --test e2e

# Run specific test suite
cargo test --test e2e_sync_test -- --test-threads=1
```

### Performance Testing

Benchmark critical paths:

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_event_processing(c: &mut Criterion) {
    c.bench_function("process_event", |b| {
        b.iter(|| {
            process_event(black_box(create_test_event()))
        });
    });
}

criterion_group!(benches, benchmark_event_processing);
criterion_main!(benches);
```

### Test Coverage

Generate coverage reports:
```bash
# Run tests with coverage
cargo tarpaulin --out Html

# View report
open tarpaulin-report.html
```

## Future Recommendations

### 1. Advanced Transformations

**JavaScript Transformations**:
```yaml
transform:
  - type: javascript
    script: |
      function transform(event) {
        // Custom logic
        event.data.fullName = `${event.data.firstName} ${event.data.lastName}`;
        delete event.data.firstName;
        delete event.data.lastName;
        return event;
      }
```

**WASM Support**:
- Load custom WASM modules for transformations
- Better performance than JavaScript
- Language-agnostic transformation logic

### 2. Multi-Source Support

**MySQL Support**:
```yaml
source:
  type: mysql
  mysql:
    host: localhost
    port: 3306
    server_id: 1000
    binlog:
      format: ROW
      start_position: "mysql-bin.000001:154"
```

**MongoDB Support**:
```yaml
source:
  type: mongodb
  mongodb:
    connection_string: "mongodb://localhost:27017"
    database: myapp
    change_stream:
      full_document: "updateLookup"
      start_after: null
```

### 3. Advanced Routing

**Content-Based Routing**:
```yaml
routing:
  rules:
    - condition: |
        event.table == "products" && 
        event.data.category == "electronics"
      destination:
        index: "electronics_products"
        
    - condition: |
        event.data.price > 1000
      destination:
        index: "premium_products"
```

### 4. Data Quality Features

**Schema Validation**:
```yaml
validation:
  schemas:
    public.users:
      required: ["id", "email", "created_at"]
      types:
        id: integer
        email: string
        created_at: timestamp
```

**Data Profiling**:
- Automatic detection of data patterns
- Anomaly detection
- Quality metrics dashboard

### 5. Performance Enhancements

**GPU Acceleration**:
- Use GPU for parallel event processing
- Batch transformations on GPU
- ML-based transformations

**Edge Computing**:
- Deploy lightweight agents near data sources
- Reduce network latency
- Distributed processing

### 6. Enterprise Features

**Multi-Tenancy**:
```yaml
tenancy:
  enabled: true
  isolation: "logical"  # logical, physical
  identifier: "tenant_id"
```

**Audit Logging**:
```yaml
audit:
  enabled: true
  events: ["task_created", "task_deleted", "config_changed"]
  storage: "elasticsearch"
```

**Compliance**:
- GDPR compliance tools
- Data masking/anonymization
- Retention policies

### 7. Observability Improvements

**Distributed Tracing**:
- Full request tracing
- Performance bottleneck identification
- Cross-service correlation

**AI-Powered Monitoring**:
- Anomaly detection
- Predictive failure analysis
- Auto-tuning recommendations

These recommendations represent the future direction of MeiliBridge, focusing on scalability, performance, and enterprise readiness.