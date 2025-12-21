# XTouch GW v3 - Rust Implementation

A high-performance Rust port of the XTouch Gateway - Control Voicemeeter, QLC+, and OBS from a Behringer X-Touch MIDI controller.

## 🛠️ Architecture

```
src/
├── main.rs          # Entry point, Tokio runtime
├── config/          # YAML configuration management
├── router/          # Event routing and page management
├── state/           # MIDI state store
├── xtouch/          # X-Touch hardware driver
├── midi/            # MIDI utilities and parsing
├── drivers/         # App drivers (OBS, QLC+, Voicemeeter)
├── cli/             # Command-line interface
└── sniffer/         # MIDI debugging tools
```

## 📦 Dependencies

- **Async Runtime**: `tokio` - Event-driven async I/O
- **MIDI**: `midir` - Cross-platform MIDI I/O
- **WebSocket**: `tokio-tungstenite`, `obws` - OBS integration
- **Config**: `serde`, `serde_yaml` - Configuration management
- **Hot Reload**: `notify` - File system watching
- **Logging**: `tracing` - Structured logging

## 🚦 Quick Start

### Prerequisites

- Rust 1.75+ (stable)
- Behringer X-Touch in MCU or CTRL mode
- MIDI interface (e.g., Roland UM-One)
- Target applications: Voicemeeter, QLC+, OBS Studio

### Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run with example config
cargo run -- -c config.example.yaml
```

### Development

```bash
# Watch for changes and rebuild
cargo watch -x build

# Run clippy lints
cargo clippy -- -D warnings

# Format code
cargo fmt

# Run benchmarks
cargo bench
```

## 🎮 Usage

```bash
# Run with default config
xtouch-gw

# Specify config file
xtouch-gw -c my-config.yaml

# Set log level
xtouch-gw --log-level debug

# Run MIDI sniffer
xtouch-gw --sniffer

# Run web sniffer interface
xtouch-gw --web-sniffer --web-port 8123
```

## 📝 Configuration

The configuration format is fully compatible with the TypeScript version. See [config.example.yaml](config.example.yaml) for a complete example.

```yaml
midi:
  input_port: "X-Touch"
  output_port: "X-Touch"

pages:
  - name: "Voicemeeter"
    controls:
      fader1:
        app: "voicemeeter"
        midi:
          type: "cc"
          channel: 1
          cc: 0
```

## 🎯 Performance Targets

- **MIDI input → Driver execution**: <5ms
- **Driver → Application**: <10ms
- **Application feedback → XTouch**: <5ms
- **Total round-trip**: <20ms
- **Config reload**: <100ms without dropping events

## 📄 License

MPL-2.0 (Mozilla Public License 2.0)

## 📚 Documentation

- [Migration Specification](RUST_MIGRATION_SPEC.md) - Detailed migration plan
- [API Documentation](https://docs.rs/xtouch-gw) - Rust API docs (coming soon)
