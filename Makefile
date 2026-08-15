SHELL := /bin/sh

PREFIX ?= /usr/local
DESTDIR ?=
BUILD_DIR ?= target
PROFILE ?= release
PACKAGE_DIR ?= dist
VERSION := $(shell sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -1)
TARGET := $(shell rustc -vV | sed -n 's/^host: //p')
ARCHIVE := avm-v$(VERSION)-$(TARGET).tar.gz

.DEFAULT_GOAL := help

.PHONY: help setup build release fmt fmt-check lint test test-rust test-node \
	test-contracts check ci doc install uninstall package clean

help: ## Show available targets
	@awk 'BEGIN {FS = ":.*## "; printf "AVM developer commands\n\n"} /^[a-zA-Z_-]+:.*## / {printf "  %-16s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

setup: ## Fetch Rust and Node dependencies
	cargo fetch --locked
	npm --prefix supervisor/browser ci

build: ## Build a debug binary
	cargo build --locked

release: ## Build an optimized binary
	cargo build --locked --release

fmt: ## Format Rust sources
	cargo fmt --all

fmt-check: ## Check Rust formatting
	cargo fmt --all -- --check

lint: ## Run Clippy with warnings denied
	cargo clippy --locked --all-targets -- -D warnings

test-rust: ## Run all Rust tests
	cargo test --locked --all-targets

test-node: ## Run browser observer tests
	npm --prefix supervisor/browser test

test-contracts: ## Run MCP and experiment contract tests
	node supervisor/mcp/check.mjs
	node experiments/check.mjs
	node experiments/agent-metrics-check.mjs
	node experiments/real-agent-check.mjs
	node experiments/real-evaluator-check.mjs
	node experiments/final-demo-audit-check.mjs

test: test-rust test-node test-contracts ## Run the complete test suite

check: fmt-check lint test ## Run every local quality gate

ci: setup check ## Reproduce the GitHub Actions quality gate

doc: ## Build local Rust API documentation
	cargo doc --locked --no-deps

install: release ## Install the AVM binary under PREFIX
	install -d "$(DESTDIR)$(PREFIX)/bin"
	install -m 0755 "$(BUILD_DIR)/release/avm" "$(DESTDIR)$(PREFIX)/bin/avm"

uninstall: ## Remove the installed AVM binary
	rm -f "$(DESTDIR)$(PREFIX)/bin/avm"

package: release ## Create a checksummed release archive for the host target
	rm -rf "$(PACKAGE_DIR)/avm-v$(VERSION)-$(TARGET)"
	mkdir -p "$(PACKAGE_DIR)/avm-v$(VERSION)-$(TARGET)"
	cp "$(BUILD_DIR)/release/avm" LICENSE README.md "$(PACKAGE_DIR)/avm-v$(VERSION)-$(TARGET)/"
	tar -C "$(PACKAGE_DIR)" -czf "$(PACKAGE_DIR)/$(ARCHIVE)" "avm-v$(VERSION)-$(TARGET)"
	cd "$(PACKAGE_DIR)" && shasum -a 256 "$(ARCHIVE)" > "$(ARCHIVE).sha256"

clean: ## Remove generated build and package output
	cargo clean
	rm -rf "$(PACKAGE_DIR)"
