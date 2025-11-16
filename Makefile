# XTouch GW v3 - Rust Implementation Makefile
# For Windows, use with: make (if GNU Make installed) or nmake

.PHONY: help build release run test check clean fmt clippy watch docs \
        ts-run ts-test ts-sniff compare setup install-deps bench profile \
        dev sniffer web-sniffer all ci

# Default target
.DEFAULT_GOAL := help

# Configuration
CARGO := cargo
PNPM := pnpm
TS_DIR := D:\dev\xtouch-gw-v2
CONFIG := config.example.yaml
LOG_LEVEL := info

# Colors for output (Windows compatible)
NO_COLOR := \033[0m
GREEN := \033[0;32m
YELLOW := \033[0;33m
BLUE := \033[0;34m
RED := \033[0;31m

## help: Display this help message
help:
	@echo $(GREEN)XTouch GW v3 - Rust Implementation$(NO_COLOR)
	@echo.
	@echo $(YELLOW)Available targets:$(NO_COLOR)
	@echo.
	@echo $(BLUE)Development:$(NO_COLOR)
	@echo   make build      - Build debug version
	@echo   make release    - Build optimized release version
	@echo   make run        - Run with example config
	@echo   make dev        - Run in watch mode (auto-rebuild)
	@echo   make watch      - Watch for changes and check
	@echo.
	@echo $(BLUE)Testing:$(NO_COLOR)
	@echo   make test       - Run all tests
	@echo   make bench      - Run benchmarks
	@echo   make check      - Type check without building
	@echo   make clippy     - Run clippy linter
	@echo   make fmt        - Format code
	@echo.
	@echo $(BLUE)TypeScript Reference:$(NO_COLOR)
	@echo   make ts-run     - Run TypeScript version
	@echo   make ts-test    - Run TypeScript tests
	@echo   make ts-sniff   - Run TypeScript MIDI sniffer
	@echo   make compare    - Run both versions side-by-side
	@echo.
	@echo $(BLUE)Tools:$(NO_COLOR)
	@echo   make sniffer    - Run MIDI sniffer (CLI)
	@echo   make web-sniffer - Run web MIDI sniffer
	@echo   make docs       - Generate documentation
	@echo   make clean      - Clean build artifacts
	@echo.
	@echo $(BLUE)Setup:$(NO_COLOR)
	@echo   make setup      - Initial project setup
	@echo   make install-deps - Install Rust dependencies
	@echo.
	@echo $(BLUE)CI/Quality:$(NO_COLOR)
	@echo   make ci         - Run CI checks (fmt, clippy, test)
	@echo   make all        - Build everything and run tests
	@echo.

## setup: Initial project setup
setup:
	@echo $(GREEN)Setting up XTouch GW v3...$(NO_COLOR)
	@if not exist "target" mkdir target
	@if not exist ".state" mkdir .state
	@if not exist "logs" mkdir logs
	@if not exist "config.yaml" copy config.example.yaml config.yaml
	@echo $(GREEN)Installing Rust dependencies...$(NO_COLOR)
	@$(CARGO) fetch
	@echo $(GREEN)Setup complete!$(NO_COLOR)

## install-deps: Install/update Rust dependencies
install-deps:
	@echo $(YELLOW)Fetching dependencies...$(NO_COLOR)
	@$(CARGO) fetch
	@$(CARGO) update
	@echo $(GREEN)Dependencies updated!$(NO_COLOR)

## build: Build debug version
build:
	@echo $(YELLOW)Building debug version...$(NO_COLOR)
	@$(CARGO) build
	@echo $(GREEN)Build complete: target/debug/xtouch-gw.exe$(NO_COLOR)

## release: Build optimized release version
release:
	@echo $(YELLOW)Building release version...$(NO_COLOR)
	@$(CARGO) build --release
	@echo $(GREEN)Build complete: target/release/xtouch-gw.exe$(NO_COLOR)

## run: Run with example config
run: build
	@echo $(YELLOW)Running XTouch GW v3...$(NO_COLOR)
	@$(CARGO) run -- -c $(CONFIG) --log-level $(LOG_LEVEL)

## dev: Run in development mode with auto-rebuild
dev:
	@echo $(YELLOW)Starting development mode (Ctrl+C to stop)...$(NO_COLOR)
	@$(CARGO) watch -x "run -- -c $(CONFIG) --log-level debug"

## watch: Watch for changes and check
watch:
	@echo $(YELLOW)Watching for changes...$(NO_COLOR)
	@$(CARGO) watch -x check -x test

## test: Run all tests
test:
	@echo $(YELLOW)Running tests...$(NO_COLOR)
	@$(CARGO) test
	@echo $(GREEN)Tests complete!$(NO_COLOR)

## test-verbose: Run tests with output
test-verbose:
	@$(CARGO) test -- --nocapture --test-threads=1

## bench: Run benchmarks
bench:
	@echo $(YELLOW)Running benchmarks...$(NO_COLOR)
	@$(CARGO) bench

## check: Type check without building
check:
	@echo $(YELLOW)Type checking...$(NO_COLOR)
	@$(CARGO) check
	@echo $(GREEN)Type check complete!$(NO_COLOR)

## clippy: Run clippy linter
clippy:
	@echo $(YELLOW)Running clippy...$(NO_COLOR)
	@$(CARGO) clippy -- -D warnings
	@echo $(GREEN)Clippy complete!$(NO_COLOR)

## fmt: Format code
fmt:
	@echo $(YELLOW)Formatting code...$(NO_COLOR)
	@$(CARGO) fmt
	@echo $(GREEN)Formatting complete!$(NO_COLOR)

## fmt-check: Check formatting without changing files
fmt-check:
	@$(CARGO) fmt -- --check

## clean: Clean build artifacts
clean:
	@echo $(YELLOW)Cleaning build artifacts...$(NO_COLOR)
	@$(CARGO) clean
	@if exist "logs\*.log" del /Q logs\*.log
	@if exist ".state\*.json" del /Q .state\*.json
	@echo $(GREEN)Clean complete!$(NO_COLOR)

## docs: Generate documentation
docs:
	@echo $(YELLOW)Generating documentation...$(NO_COLOR)
	@$(CARGO) doc --no-deps --open
	@echo $(GREEN)Documentation generated!$(NO_COLOR)

## sniffer: Run MIDI sniffer (CLI)
sniffer: build
	@echo $(YELLOW)Starting MIDI sniffer...$(NO_COLOR)
	@$(CARGO) run -- --sniffer

## web-sniffer: Run web MIDI sniffer
web-sniffer: build
	@echo $(YELLOW)Starting web MIDI sniffer on http://localhost:8123...$(NO_COLOR)
	@$(CARGO) run -- --web-sniffer --web-port 8123

# TypeScript reference commands
## ts-run: Run TypeScript version
ts-run:
	@echo $(YELLOW)Running TypeScript reference implementation...$(NO_COLOR)
	@cd $(TS_DIR) && $(PNPM) dev

## ts-test: Run TypeScript tests
ts-test:
	@echo $(YELLOW)Running TypeScript tests...$(NO_COLOR)
	@cd $(TS_DIR) && $(PNPM) test

## ts-sniff: Run TypeScript MIDI sniffer
ts-sniff:
	@echo $(YELLOW)Starting TypeScript MIDI sniffer...$(NO_COLOR)
	@cd $(TS_DIR) && $(PNPM) sniff:web

## ts-build: Build TypeScript version
ts-build:
	@echo $(YELLOW)Building TypeScript version...$(NO_COLOR)
	@cd $(TS_DIR) && $(PNPM) build

## compare: Run both versions side-by-side (requires multiple terminals)
compare:
	@echo $(GREEN)========================================$(NO_COLOR)
	@echo $(GREEN) Running Both Versions Side-by-Side$(NO_COLOR)
	@echo $(GREEN)========================================$(NO_COLOR)
	@echo.
	@echo $(YELLOW)Terminal 1:$(NO_COLOR) make ts-run
	@echo $(YELLOW)Terminal 2:$(NO_COLOR) make run
	@echo $(YELLOW)Terminal 3:$(NO_COLOR) make ts-sniff
	@echo $(YELLOW)Terminal 4:$(NO_COLOR) make web-sniffer
	@echo.
	@echo $(BLUE)Starting Rust version in this terminal...$(NO_COLOR)
	@$(CARGO) run -- -c $(CONFIG) --log-level $(LOG_LEVEL)

## profile: Build and run with profiling
profile:
	@echo $(YELLOW)Building with profiling support...$(NO_COLOR)
	@$(CARGO) build --profile profiling
	@echo $(GREEN)Run with your profiler of choice on:$(NO_COLOR)
	@echo   target/profiling/xtouch-gw.exe

## ci: Run all CI checks
ci: fmt-check clippy test
	@echo $(GREEN)All CI checks passed!$(NO_COLOR)

## all: Build everything and run tests
all: clean build release test docs
	@echo $(GREEN)Full build complete!$(NO_COLOR)

# Development shortcuts
b: build
r: run
t: test
c: check
f: fmt
cl: clean

# Quick test specific module
test-module:
	@$(CARGO) test $(MODULE) -- --nocapture

# Quick run with custom config
run-config:
	@$(CARGO) run -- -c $(CFG)

# Environment info
info:
	@echo $(BLUE)Environment Information:$(NO_COLOR)
	@echo.
	@echo Rust version:
	@rustc --version
	@echo.
	@echo Cargo version:
	@$(CARGO) --version
	@echo.
	@echo Project root: %CD%
	@echo TypeScript dir: $(TS_DIR)
	@echo.
	@echo Config file: $(CONFIG)
	@echo Log level: $(LOG_LEVEL)

# Installation helpers for Windows
install-tools:
	@echo $(YELLOW)Installing development tools...$(NO_COLOR)
	@$(CARGO) install cargo-watch
	@$(CARGO) install cargo-edit
	@$(CARGO) install cargo-audit
	@$(CARGO) install cargo-outdated
	@$(CARGO) install flamegraph
	@echo $(GREEN)Tools installed!$(NO_COLOR)

# Validate against TypeScript
validate:
	@echo $(YELLOW)Validation Steps:$(NO_COLOR)
	@echo 1. Start TypeScript version: make ts-run
	@echo 2. Capture MIDI sequence
	@echo 3. Start Rust version: make run
	@echo 4. Compare MIDI outputs
	@echo 5. Check state consistency
	@echo.
	@echo See DEVELOPMENT.md for detailed validation process.
