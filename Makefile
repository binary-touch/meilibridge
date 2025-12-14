.PHONY: help build run test clean docker-up docker-down docker-logs docker-reset dev
.PHONY: build-linux-arm64 build-linux-amd64 build-all-platforms docker-build docker-build-multiarch docker-push

# Variables
DOCKER_REGISTRY ?= docker.io
DOCKER_USERNAME ?= binarytouch
DOCKER_IMAGE ?= meilibridge
# Get clean version from git tag (remove 'v' prefix if present)
VERSION ?= $(shell git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//' || echo "latest")
# For builds not on exact tags, use git describe for informational purposes
FULL_VERSION ?= $(shell git describe --tags --always --dirty)
PLATFORMS ?= linux/amd64,linux/arm64

# Default target
help:
	@echo "MeiliBridge Development Commands:"
	@echo "  make build        - Build the project in release mode"
	@echo "  make dev          - Build the project in debug mode"
	@echo "  make run          - Run MeiliBridge with local config"
	@echo "  make test         - Run all tests"
	@echo "  make clean        - Clean build artifacts"
	@echo ""
	@echo "Cross-Platform Build Commands:"
	@echo "  make build-linux-arm64    - Build for Linux ARM64 using Docker"
	@echo "  make build-linux-amd64    - Build for Linux AMD64 using Docker"
	@echo "  make build-all-platforms  - Build for all platforms"
	@echo "  make package-linux-arm64  - Package ARM64 binary as tar.gz"
	@echo ""
	@echo "Docker Commands:"
	@echo "  make docker-up    - Start all Docker services"
	@echo "  make docker-down  - Stop all Docker services"
	@echo "  make docker-logs  - View logs from all services"
	@echo "  make docker-reset - Reset all data (WARNING: destructive)"
	@echo "  make docker-build - Build Docker image for current platform"
	@echo "  make docker-build-multiarch - Build multi-arch Docker image"
	@echo "  make docker-push  - Push multi-arch image to registry"
	@echo ""
	@echo "Quick Start:"
	@echo "  make docker-up    - Start services"
	@echo "  make dev          - Build MeiliBridge"
	@echo "  make run          - Run MeiliBridge"

# Build commands
build:
	cargo build --release

dev:
	cargo build

run:
	cargo run -- --config config.yaml

test:
	cargo test

clean:
	cargo clean
	rm -rf logs/

# Docker commands
docker-up:
	@echo "Starting Docker services..."
	docker compose -f docker/compose.yaml up -d
	@echo "Waiting for services to be healthy..."
	@sleep 5
	@docker compose -f docker/compose.yaml ps
	@echo ""
	@echo "Services are running:"
	@echo "  PostgreSQL:      localhost:5433"
	@echo "  Redis:           localhost:6380"
	@echo "  Meilisearch:     http://localhost:7700"
	@echo "  Meilisearch UI:  http://localhost:24900"
	@echo "  Adminer:         http://localhost:8090"
	@echo "  RedisInsight:    http://localhost:8001"
	@echo ""
	@echo "Run 'make run' to start MeiliBridge"

docker-down:
	docker compose -f docker/compose.yaml down

docker-logs:
	docker compose -f docker/compose.yaml logs -f

docker-reset:
	@echo "WARNING: This will delete all Docker volumes and data!"
	@echo "Press Ctrl+C to cancel, or wait 5 seconds to continue..."
	@sleep 5
	docker compose -f docker/compose.yaml down -v
	@echo "All data has been reset"

# Combined commands
quick-start: docker-up dev run

# Development helpers
watch:
	cargo watch -x run

fmt:
	cargo fmt

lint:
	cargo clippy -- -D warnings

# Check if services are healthy
check-services:
	@echo "Checking service health..."
	@curl -s http://localhost:7700/health > /dev/null && echo "✓ Meilisearch is healthy" || echo "✗ Meilisearch is not responding"
	@docker exec meilibridge-postgres pg_isready > /dev/null 2>&1 && echo "✓ PostgreSQL is healthy" || echo "✗ PostgreSQL is not responding"
	@docker exec meilibridge-redis redis-cli ping > /dev/null 2>&1 && echo "✓ Redis is healthy" || echo "✗ Redis is not responding"

# Generate sample data
sample-data:
	@echo "Inserting sample data into PostgreSQL..."
	@docker exec meilibridge-postgres psql -U postgres -d meilibridge_dev -c "INSERT INTO users (email, first_name, last_name, age, metadata) VALUES ('alice.johnson@example.com', 'Alice', 'Johnson', 28, '{\"interests\": [\"art\", \"design\"]}'), ('charlie.brown@example.com', 'Charlie', 'Brown', 42, '{\"interests\": [\"finance\", \"investing\"]}'), ('diana.prince@example.com', 'Diana', 'Prince', 33, '{\"interests\": [\"fitness\", \"travel\"]}') ON CONFLICT (email) DO NOTHING;"
	@docker exec meilibridge-postgres psql -U postgres -d meilibridge_dev -c "INSERT INTO products (sku, name, description, price, category, tags) VALUES ('DESK-001', 'Standing Desk', 'Adjustable height standing desk', 599.99, 'Furniture', ARRAY['desk', 'office', 'ergonomic']), ('MOUSE-001', 'Wireless Mouse', 'Ergonomic wireless mouse', 49.99, 'Electronics', ARRAY['mouse', 'wireless', 'peripherals']), ('CHAIR-001', 'Ergonomic Chair', 'High-back office chair', 399.99, 'Furniture', ARRAY['chair', 'office', 'ergonomic']) ON CONFLICT (sku) DO NOTHING;"
	@echo "Sample data inserted!"

# Validate configuration
validate:
	cargo run -- validate

# Show current sync status
status:
	@echo "Checking MeiliBridge status..."
	@curl -s http://localhost:7701/status || echo "API server not running"

# Cross-platform builds
build-linux-arm64:
	@echo "Building for Linux ARM64 using Docker..."
	@docker build --platform linux/arm64 \
		--target builder \
		-t meilibridge-builder-arm64 \
		-f docker/Dockerfile .
	@docker create --name temp-meilibridge-arm64 meilibridge-builder-arm64
	@mkdir -p target/aarch64-unknown-linux-gnu/release
	@docker cp temp-meilibridge-arm64:/usr/src/meilibridge/target/release/meilibridge target/aarch64-unknown-linux-gnu/release/meilibridge
	@docker rm temp-meilibridge-arm64
	@echo "Binary extracted to: target/aarch64-unknown-linux-gnu/release/meilibridge"

build-linux-amd64:
	@echo "Building for Linux AMD64 using Docker..."
	@docker build --platform linux/amd64 \
		--target builder \
		-t meilibridge-builder-amd64 \
		-f docker/Dockerfile .
	@docker create --name temp-meilibridge-amd64 meilibridge-builder-amd64
	@mkdir -p target/x86_64-unknown-linux-gnu/release
	@docker cp temp-meilibridge-amd64:/usr/src/meilibridge/target/release/meilibridge target/x86_64-unknown-linux-gnu/release/meilibridge
	@docker rm temp-meilibridge-amd64
	@echo "Binary extracted to: target/x86_64-unknown-linux-gnu/release/meilibridge"

build-all-platforms: build-linux-amd64 build-linux-arm64
	@echo "All platform builds complete!"

# Docker multi-architecture builds
docker-setup-buildx:
	@echo "Setting up Docker buildx..."
	@docker buildx create --name meilibridge-builder --use || docker buildx use meilibridge-builder
	@docker buildx inspect --bootstrap

docker-build:
	@echo "Building Docker image for current platform..."
	@echo "Version: $(VERSION) (Full: $(FULL_VERSION))"
	docker build -t $(DOCKER_USERNAME)/$(DOCKER_IMAGE):$(VERSION) \
		--build-arg VERSION=$(VERSION) \
		-f docker/Dockerfile .
	@if [ "$(VERSION)" != "latest" ]; then \
		docker tag $(DOCKER_USERNAME)/$(DOCKER_IMAGE):$(VERSION) $(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest; \
	fi

docker-build-multiarch: docker-setup-buildx
	@echo "Building multi-architecture Docker image..."
	@echo "Platforms: $(PLATFORMS)"
	@echo "Version: $(VERSION) (Full: $(FULL_VERSION))"
	@if [ "$(VERSION)" = "latest" ]; then \
		docker buildx build \
			--platform $(PLATFORMS) \
			--tag $(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest \
			--file docker/Dockerfile \
			--build-arg VERSION=$(FULL_VERSION) \
			.; \
	else \
		docker buildx build \
			--platform $(PLATFORMS) \
			--tag $(DOCKER_USERNAME)/$(DOCKER_IMAGE):$(VERSION) \
			--tag $(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest \
			--file docker/Dockerfile \
			--build-arg VERSION=$(VERSION) \
			.; \
	fi

docker-push: docker-setup-buildx
	@echo "Building and pushing multi-architecture Docker image..."
	@echo "Registry: $(DOCKER_REGISTRY)"
	@echo "Image: $(DOCKER_USERNAME)/$(DOCKER_IMAGE)"
	@echo "Platforms: $(PLATFORMS)"
	@echo "Version: $(VERSION) (Full: $(FULL_VERSION))"
	@if [ "$(VERSION)" = "latest" ]; then \
		echo "Warning: Not on a tagged release, pushing as 'latest' only"; \
		docker buildx build \
			--platform $(PLATFORMS) \
			--tag $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest \
			--file docker/Dockerfile \
			--build-arg VERSION=$(FULL_VERSION) \
			--push \
			.; \
		echo ""; \
		echo "Pushed to:"; \
		echo "  - $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest"; \
	else \
		docker buildx build \
			--platform $(PLATFORMS) \
			--tag $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):$(VERSION) \
			--tag $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest \
			--file docker/Dockerfile \
			--build-arg VERSION=$(VERSION) \
			--push \
			.; \
		echo ""; \
		echo "Pushed to:"; \
		echo "  - $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):$(VERSION)"; \
		echo "  - $(DOCKER_REGISTRY)/$(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest"; \
	fi

# Package release artifacts
package-linux-arm64: build-linux-arm64
	@echo "Packaging Linux ARM64 binary..."
	@mkdir -p dist
	@cd target/aarch64-unknown-linux-gnu/release && \
		tar czf ../../../dist/meilibridge-linux-arm64.tar.gz meilibridge
	@echo "Package created: dist/meilibridge-linux-arm64.tar.gz"

package-all: package-linux-arm64
	@echo "All packages created in dist/"

# Clean everything including cross compilation artifacts
deep-clean: clean
	rm -rf target/
	rm -rf dist/
	docker buildx rm meilibridge-builder || true

# Install development dependencies
install-deps:
	@echo "Installing development dependencies..."
	@echo "Checking Docker..."
	@if ! command -v docker > /dev/null 2>&1; then \
		echo "ERROR: Docker is required for cross-platform builds. Please install Docker first."; \
		exit 1; \
	fi
	@echo "Installing cargo-watch for development..."
	cargo install cargo-watch
	@echo "Setting up Docker buildx for multi-arch builds..."
	@docker buildx create --name meilibridge-builder --use 2>/dev/null || true
	@echo ""
	@echo "Dependencies installed!"
	@echo "You can now use:"
	@echo "  make build-linux-arm64      - Build ARM64 binary using Docker"
	@echo "  make build-linux-amd64      - Build AMD64 binary using Docker"
	@echo "  make docker-build-multiarch - Build multi-arch Docker image"

# Verify multi-arch image
docker-verify:
	@echo "Verifying multi-architecture image..."
	docker buildx imagetools inspect $(DOCKER_USERNAME)/$(DOCKER_IMAGE):latest