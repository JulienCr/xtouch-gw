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

## help: Display this help message
help:
	@echo ""
	@echo   XTouch GW v3 - Rust Implementation
	@echo   ===================================
	@echo ""
	@powershell -Command "Get-Content $(MAKEFILE_LIST) | Select-String -Pattern '^## ' | ForEach-Object { $$_.Line -replace '^## ', '  make ' -replace ': ', ' - ' }"
	@echo ""

## setup: Initial project setup
setup:
	@echo Setting up XTouch GW v3...
	@if not exist "target" mkdir target
	@if not exist ".state" mkdir .state
	@if not exist "logs" mkdir logs
	@if not exist "config.yaml" copy config.example.yaml config.yaml
	@echo Installing Rust dependencies...
	@$(CARGO) fetch
	@echo Setup complete!

## install-deps: Install/update Rust dependencies
install-deps:
	@echo Fetching dependencies...
	@$(CARGO) fetch
	@$(CARGO) update
	@echo Dependencies updated!

## build: Build debug version
build:
	@echo Building debug version...
	@$(CARGO) build
	@echo Build complete: target/debug/xtouch-gw.exe

## release: Build optimized release version
release:
	@echo Building release version...
	@$(CARGO) build --release
	@echo Build complete: target/release/xtouch-gw.exe

## run: Run with example config
run: build
	@echo Running XTouch GW v3...
	@$(CARGO) run -- -c $(CONFIG) --log-level $(LOG_LEVEL)

## dev: Run in development mode with auto-rebuild
dev:
	@echo Starting development mode (Ctrl+C to stop)...
	@$(CARGO) watch -x "run -- -c $(CONFIG) --log-level debug"

## watch: Watch for changes and check
watch:
	@echo Watching for changes...
	@$(CARGO) watch -x check -x test

## test: Run all tests
test:
	@echo Running tests...
	@$(CARGO) test
	@echo Tests complete!

## test-verbose: Run tests with output
test-verbose:
	@$(CARGO) test -- --nocapture --test-threads=1

## bench: Run benchmarks
bench:
	@echo Running benchmarks...
	@$(CARGO) bench

## check: Type check without building
check:
	@echo Type checking...
	@$(CARGO) check
	@echo Type check complete!

## clippy: Run clippy linter
clippy:
	@echo Running clippy...
	@$(CARGO) clippy -- -D warnings
	@echo Clippy complete!

## fmt: Format code
fmt:
	@echo Formatting code...
	@$(CARGO) fmt
	@echo Formatting complete!

## fmt-check: Check formatting without changing files
fmt-check:
	@$(CARGO) fmt -- --check

## clean: Clean build artifacts
clean:
	@echo Cleaning build artifacts...
	@$(CARGO) clean
	@if exist "logs\*.log" del /Q logs\*.log
	@if exist ".state\*.json" del /Q .state\*.json
	@echo Clean complete!

## docs: Generate documentation
docs:
	@echo Generating documentation...
	@$(CARGO) doc --no-deps --open
	@echo Documentation generated!

## sniffer: Run MIDI sniffer (CLI)
sniffer: build
	@echo Starting MIDI sniffer...
	@$(CARGO) run -- --sniffer

## web-sniffer: Run web MIDI sniffer
web-sniffer: build
	@echo Starting web MIDI sniffer on http://localhost:8123...
	@$(CARGO) run -- --web-sniffer --web-port 8123

# TypeScript reference commands
## ts-run: Run TypeScript version
ts-run:
	@echo Running TypeScript reference implementation...
	@cd $(TS_DIR) && $(PNPM) dev

## ts-test: Run TypeScript tests
ts-test:
	@echo Running TypeScript tests...
	@cd $(TS_DIR) && $(PNPM) test

## ts-sniff: Run TypeScript MIDI sniffer
ts-sniff:
	@echo Starting TypeScript MIDI sniffer...
	@cd $(TS_DIR) && $(PNPM) sniff:web

## ts-build: Build TypeScript version
ts-build:
	@echo Building TypeScript version...
	@cd $(TS_DIR) && $(PNPM) build

## compare: Run both versions side-by-side
compare:
	@echo ========================================
	@echo  Running Both Versions Side-by-Side
	@echo ========================================
	@echo ""
	@echo Terminal 1: make ts-run
	@echo Terminal 2: make run
	@echo Terminal 3: make ts-sniff
	@echo Terminal 4: make web-sniffer
	@echo ""
	@echo Starting Rust version in this terminal...
	@$(CARGO) run -- -c $(CONFIG) --log-level $(LOG_LEVEL)

## profile: Build and run with profiling
profile:
	@echo Building with profiling support...
	@$(CARGO) build --profile profiling
	@echo Run with your profiler of choice on:
	@echo   target/profiling/xtouch-gw.exe

## ci: Run all CI checks
ci: fmt-check clippy test
	@echo All CI checks passed!

## all: Build everything and run tests
all: clean build release test docs
	@echo Full build complete!

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
	@echo Environment Information:
	@echo ""
	@echo Rust version:
	@rustc --version
	@echo ""
	@echo Cargo version:
	@$(CARGO) --version
	@echo ""
	@echo Project root: %CD%
	@echo TypeScript dir: $(TS_DIR)
	@echo ""
	@echo Config file: $(CONFIG)
	@echo Log level: $(LOG_LEVEL)

# Installation helpers for Windows
install-tools:
	@echo Installing development tools...
	@$(CARGO) install cargo-watch
	@$(CARGO) install cargo-edit
	@$(CARGO) install cargo-audit
	@$(CARGO) install cargo-outdated
	@$(CARGO) install flamegraph
	@echo Tools installed!

# Validate against TypeScript
validate:
	@echo Validation Steps:
	@echo 1. Start TypeScript version: make ts-run
	@echo 2. Capture MIDI sequence
	@echo 3. Start Rust version: make run
	@echo 4. Compare MIDI outputs
	@echo 5. Check state consistency
	@echo ""
	@echo See DEVELOPMENT.md for detailed validation process.
