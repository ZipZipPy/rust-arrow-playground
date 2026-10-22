# Shortcuts for the commands in README.md. `make` lists them.

SEED_ROWS ?= 1000000

# Local development database (`make dev-db`). Port 5433 avoids a PostgreSQL
# that may already listen on 5432; point DATABASE_URL elsewhere to use yours.
DEV_DB_CONTAINER ?= arrow-playground-dev-db
DEV_DB_PORT ?= 5433
DATABASE_URL ?= postgres://postgres:postgres@localhost:$(DEV_DB_PORT)/playground
TEST_DATABASE_URL ?= postgres://postgres:postgres@localhost:$(DEV_DB_PORT)/playground_test

export DATABASE_URL TEST_DATABASE_URL

SERVER := cd server &&
WEB := cd web &&

.DEFAULT_GOAL := help
.PHONY: help up down reset logs reseed image dev-db dev-db-down seed dev-server dev-web \
	test test-server test-web fmt lint

help: ## List the targets
	@awk 'BEGIN {FS = ":.*## "} /^[a-z-]+:.*## / {printf "  %-12s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

# --- Docker Compose ---------------------------------------------------------

up: ## Build and start db + seed + app, wait until healthy
	docker compose up --build --detach --wait app
	@port=$$(docker compose port app 8080) && echo "Open http://localhost:$${port##*:}"

down: ## Stop the stack, keep the data
	docker compose down

reset: ## Stop the stack and delete the database volume
	docker compose down --volumes

logs: ## Follow the logs of all services
	docker compose logs --follow

reseed: ## Replace the compose data with SEED_ROWS rows
	docker compose run --rm seed seed --rows $(SEED_ROWS)

image: ## Build the image only
	docker build --tag arrow-playground:local .

# --- Local development ------------------------------------------------------

dev-db: ## Start PostgreSQL 18.6 on localhost:DEV_DB_PORT (playground + playground_test)
	@docker start $(DEV_DB_CONTAINER) >/dev/null 2>&1 || docker run --detach --name $(DEV_DB_CONTAINER) \
		--env POSTGRES_PASSWORD=postgres --env POSTGRES_DB=playground \
		--publish 127.0.0.1:$(DEV_DB_PORT):5432 \
		--volume $(DEV_DB_CONTAINER):/var/lib/postgresql \
		postgres:18.6-alpine >/dev/null
	@# TCP readiness: during first-time initialization the server only listens on its socket.
	@until docker exec $(DEV_DB_CONTAINER) pg_isready --quiet --host=127.0.0.1; do sleep 1; done
	@docker exec $(DEV_DB_CONTAINER) psql --quiet --username=postgres --dbname=playground --tuples-only \
		--command="SELECT 1 FROM pg_database WHERE datname = 'playground_test'" | grep -q 1 \
		|| docker exec $(DEV_DB_CONTAINER) createdb --username=postgres playground_test
	@echo "DATABASE_URL=$(DATABASE_URL)"

dev-db-down: ## Stop and remove the dev database container (its volume stays)
	docker rm --force $(DEV_DB_CONTAINER)

seed: ## Seed DATABASE_URL with SEED_ROWS rows (release build)
	$(SERVER) cargo run --release -- seed --rows $(SEED_ROWS)

dev-server: ## Serve the API on :8080 against DATABASE_URL (release build)
	$(SERVER) cargo run --release -- serve --static-dir ../web/dist

dev-web: web/node_modules ## Vite dev server on :5173, proxying /api to :8080
	$(WEB) npm run dev

web/node_modules: web/package-lock.json
	$(WEB) npm ci
	@touch $@

# --- Checks -----------------------------------------------------------------

test: test-server test-web ## Run all tests

test-server: ## Rust tests (database tests need TEST_DATABASE_URL, e.g. make dev-db)
	$(SERVER) cargo test

test-web: web/node_modules ## Frontend unit tests
	$(WEB) npm test

fmt: ## Format the Rust code
	$(SERVER) cargo fmt

lint: web/node_modules ## rustfmt check, clippy with warnings as errors, TypeScript check
	$(SERVER) cargo fmt --check
	$(SERVER) cargo clippy --all-targets -- -D warnings
	$(WEB) npm run typecheck
