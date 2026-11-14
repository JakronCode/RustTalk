# RustTalk developer tasks. Run `make` (or `make help`) for the list.
# Commands carry the project flags (--locked, serial integration tests).

SHELL := /usr/bin/env bash
.DEFAULT_GOAL := help

CARGO     ?= cargo
DEV_DIR   := .dev
SERVER_CFG := $(DEV_DIR)/server/server.toml
DEV_PORT  ?= 7331

# Colors (off with NO_COLOR=1 or when not on a terminal).
ifeq ($(NO_COLOR)$(shell test -t 1 || echo no),)
  BOLD  := \033[1m
  DIM   := \033[2m
  CYAN  := \033[36m
  GREEN := \033[32m
  YELLOW:= \033[33m
  RED   := \033[31m
  RESET := \033[0m
endif

say  = @printf "$(CYAN)==>$(RESET) $(BOLD)%s$(RESET)\n" "$(1)"
ok   = @printf "$(GREEN)ok$(RESET)  %s\n" "$(1)"

##@ Help

.PHONY: help
help: ## Show this help
	@printf "$(BOLD)RustTalk$(RESET) $(DIM)- make <target>$(RESET)\n"
	@awk 'BEGIN {FS = ":.*## "} \
		/^##@/ { printf "\n$(YELLOW)%s$(RESET)\n", substr($$0, 5); next } \
		/^[a-zA-Z0-9_-]+:.*## / { printf "  $(CYAN)%-16s$(RESET) %s\n", $$1, $$2 }' $(MAKEFILE_LIST)
	@echo

##@ Setup

.PHONY: setup
setup: ## Check build tools and fetch all crates
	$(call say,Checking tools)
	@missing=0; for tool in cargo rustc cmake pkg-config nasm; do \
		if command -v $$tool >/dev/null 2>&1; then printf "$(GREEN)ok$(RESET)  %s\n" "$$tool"; \
		else printf "$(RED)missing$(RESET)  %s\n" "$$tool"; missing=1; fi; \
	done; \
	if [ $$missing -ne 0 ]; then \
		printf "$(YELLOW)Install the missing tools. Linux system libraries (Debian/Ubuntu) are listed in .github/workflows/build.yml.$(RESET)\n"; \
		exit 1; \
	fi
	$(call say,Fetching crates)
	@$(CARGO) fetch --locked
	$(call ok,Setup done - try 'make setup-server' and 'make server')

.PHONY: setup-server
setup-server: ## Create a local dev server config in .dev/server (no system changes)
	@mkdir -p $(DEV_DIR)/server
	@if [ -f $(SERVER_CFG) ]; then \
		printf "$(DIM)%s already exists, left as is$(RESET)\n" "$(SERVER_CFG)"; \
	else \
		sed -e 's|^bind_address = .*|bind_address = "127.0.0.1:$(DEV_PORT)"   # local only|' \
		    -e 's|^db_path = .*|db_path = "$(DEV_DIR)/server/rusttalk.db"|' \
		    -e 's|^check_updates = true|check_updates = false|' \
		    -e 's|^server_name = .*|server_name = "RustTalk Dev"|' \
		    Server/server.toml > $(SERVER_CFG); \
		chmod 600 $(SERVER_CFG); \
		printf "$(GREEN)ok$(RESET)  created %s (127.0.0.1:$(DEV_PORT))\n" "$(SERVER_CFG)"; \
	fi

##@ Run

.PHONY: server
server: ## Run the dev server (needs 'make setup-server')
	@test -f $(SERVER_CFG) || { printf "$(RED)No dev config.$(RESET) Run 'make setup-server' first.\n"; exit 1; }
	$(call say,Server on 127.0.0.1:$(DEV_PORT) - data in $(DEV_DIR)/server)
	@$(CARGO) run --locked -p rusttalk-server --bin rusttalk-server -- $(SERVER_CFG)

.PHONY: client
client: ## Run the client with its own config in .dev/client
	@mkdir -p $(DEV_DIR)/client
	$(call say,Client - config in $(DEV_DIR)/client)
	@cd $(DEV_DIR)/client && $(CARGO) run --locked --manifest-path ../../Cargo.toml -p rusttalk-client

##@ Build

.PHONY: build
build: ## Debug build of the whole workspace
	@$(CARGO) build --locked --workspace

.PHONY: build-server
build-server: ## Release build of the server
	@$(CARGO) build --locked --release -p rusttalk-server

.PHONY: build-client
build-client: ## Release build of the client
	@$(CARGO) build --locked --release -p rusttalk-client

.PHONY: release
release: ## Release build of client and server
	@$(CARGO) build --locked --release --workspace

.PHONY: appimage
appimage: ## Linux AppImage and server bundle into dist/
	@bash scripts/package-linux.sh

##@ Test

.PHONY: test
test: ## All tests (serial, like CI)
	@$(CARGO) test --locked --workspace -- --test-threads=1

.PHONY: test-packaging
test-packaging: ## Component routing and AppImage packaging checks (offline, no build or hardware)
	@python3 scripts/test-appimage-audio.py
	@python3 scripts/test-component-packaging.py
	@bash -n scripts/build-component.sh
	@bash -n scripts/package-linux.sh
	@bash -n packaging/linux/AppRun

.PHONY: test-server
test-server: ## Server unit and integration tests
	@bash Server/Test_Scripts/run_tests.sh

.PHONY: test-client
test-client: ## Client tests
	@$(CARGO) test --locked -p rusttalk-client

.PHONY: e2e
e2e: ## Local end-to-end smoke test (server + two test clients)
	@bash Server/Test_Scripts/e2e_local.sh

##@ Docker

DOCKER_IMAGE ?= rusttalk-server:dev
COMPOSE_DIR  := Server/deploy

.PHONY: docker-build
docker-build: ## Build the server image for this machine (DOCKER_IMAGE=rusttalk-server:dev)
	@docker build -f Server/Dockerfile -t $(DOCKER_IMAGE) .

.PHONY: docker-test
docker-test: docker-build ## Smoke test the image (fresh volume, read-only, no caps, SIGTERM)
	@bash Server/Test_Scripts/docker_smoke.sh $(DOCKER_IMAGE)

.PHONY: compose-check
compose-check: ## Validate the compose examples
	@docker compose -f $(COMPOSE_DIR)/docker-compose.yml config -q
	@docker compose -f $(COMPOSE_DIR)/docker-compose.yml -f $(COMPOSE_DIR)/docker-compose.watchtower.yml config -q
	$(call ok,compose files valid)

##@ Quality

.PHONY: lint
lint: ## Clippy over the workspace
	@$(CARGO) clippy --locked --workspace --all-targets

.PHONY: fmt-check
fmt-check: ## Show rustfmt differences (the tree is not fully formatted yet)
	@$(CARGO) fmt --all -- --check

.PHONY: audit
audit: ## Dependency audit (needs cargo-audit)
	@command -v cargo-audit >/dev/null 2>&1 || { printf "$(YELLOW)cargo install cargo-audit$(RESET)\n"; exit 1; }
	@$(CARGO) audit

##@ Clean

.PHONY: clean
clean: ## Remove build output
	@$(CARGO) clean

.PHONY: clean-dev
clean-dev: ## Remove the local dev server and client data in .dev
	@rm -rf $(DEV_DIR)
	$(call ok,removed $(DEV_DIR))

.PHONY: test-release
test-release: ## Offline synthetic component release boundary and workflow checks (Python 3.11+)
	@python3 scripts/component-release-tests.py
.PHONY: test-update-discovery lint-update-discovery format-update-discovery
test-update-discovery: ## Strict tags, bounded discovery, updater checks and actual server notices
	@$(CARGO) test --locked -p rusttalk-shared release:: -- --test-threads=1
	@$(CARGO) test --locked -p rusttalk-server --bin rusttalk-server update:: -- --test-threads=1
	@$(CARGO) test --locked -p rusttalk-client update:: -- --test-threads=1
	@$(CARGO) test --locked -p rusttalk-server --test integration update_ -- --test-threads=1

lint-update-discovery: ## Check both updater crates with existing dependencies
	@$(CARGO) clippy --locked -p rusttalk-shared -p rusttalk-server -p rusttalk-client --all-targets

format-update-discovery: ## Format only the release and updater sources
	@rustfmt --edition 2021 shared/src/release.rs shared/update_discovery.rs Server/src/update.rs Client/Windows/src/update.rs
