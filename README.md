# XTouch GW v3 - Rust Implementation

A high-performance Rust port of the XTouch Gateway - Control Voicemeeter, QLC+, and OBS from a Behringer X-Touch MIDI controller.

## 🚀 Migration from TypeScript

This is a complete rewrite of the original TypeScript implementation in Rust, targeting:
- **Sub-20ms end-to-end latency** (MIDI → App → Feedback)
- **Zero-copy MIDI processing** where possible
- **Lock-free concurrent state management**
- **Native Windows performance** with cross-platform compatibility

### 📚 Reference Implementation

**IMPORTANT**: The TypeScript version at `D:\dev\xtouch-gw-v2\` is the authoritative reference for all features and behavior. During development:

- **Always consult the TS code** for correct implementation details
- **Run the TS version** to verify expected behavior (`pnpm dev` in v2 folder)
- **Compare outputs** between TS and Rust versions
- **Match exact behavior** including MIDI formats, timing, and state management

Key TS files to reference:
- `xtouch-gw-v2/src/router.ts` - Core orchestration logic
- `xtouch-gw-v2/src/state/` - State management patterns
- `xtouch-gw-v2/src/drivers/` - Application integrations
- `xtouch-gw-v2/config.yaml` - Real configuration example

## 📋 Status

🏗️ **Under Development** - Following the migration plan in [RUST_MIGRATION_SPEC.md](RUST_MIGRATION_SPEC.md)

### Current Phase: **Phase 1 - Core Runtime Foundation**

- [x] Project initialization
- [x] Dependency setup (Cargo.toml)
- [x] Module structure
- [ ] Configuration loading
- [ ] Logging infrastructure
- [ ] CLI skeleton

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

## 🔄 Migration Progress

Following the 8-phase plan from [RUST_MIGRATION_SPEC.md](RUST_MIGRATION_SPEC.md):

- [ ] **Phase 1**: Core Runtime Foundation (Week 1)
- [ ] **Phase 2**: MIDI Infrastructure (Week 2)
- [ ] **Phase 3**: Router and State Management (Week 2-3)
- [ ] **Phase 4**: Driver Framework (Week 3)
- [ ] **Phase 5**: Application Drivers (Week 4)
- [ ] **Phase 6**: Feedback Loop (Week 5)
- [ ] **Phase 7**: Advanced Features (Week 6)
- [ ] **Phase 8**: Polish and Optimization (Week 7)

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
- [Original TypeScript README](README_TS.md) - Reference implementation docs
- [API Documentation](https://docs.rs/xtouch-gw) - Rust API docs (coming soon)

## 🤝 Contributing

This project is currently in active development. Contributions welcome after Phase 3 completion.

## 📞 Support

For issues and questions:
- Check the [RUST_MIGRATION_SPEC.md](RUST_MIGRATION_SPEC.md) for known TODOs
- Review the original TypeScript implementation for behavior reference
- Open an issue with the `rust-port` label
