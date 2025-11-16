# XTouch GW v3 (Rust) - Task Tracking

> Source of truth for the Rust port progress. Update after each significant milestone.

## ⚠️ IMPORTANT: TypeScript Reference Implementation

**The working TypeScript version is located at: `D:\dev\xtouch-gw-v2\`**

Before implementing ANY feature in Rust:
1. **Study the TypeScript code** in the corresponding module
2. **Run the TS version** with `pnpm dev` to see actual behavior
3. **Capture MIDI logs** from TS for comparison
4. **Validate your Rust implementation** matches TS exactly

This ensures feature parity and correct behavior during the migration.

## Phase 1: Core Runtime Foundation (Week 1) ✅ COMPLETE
- [x] Initialize Rust project structure
- [x] Set up Cargo.toml with dependencies
- [x] Create module skeleton
- [x] Implement AppConfig types with serde
- [x] YAML config parsing and validation
- [x] Set up tracing/logging infrastructure
- [x] Basic CLI with clap argument parsing
- [x] Implement config file watcher with notify
- [x] Create initial error handling strategy
- [x] **Validation**: Load same config.yaml as TS version, verify parsing

## Phase 2: MIDI Infrastructure (Week 2) ✅ COMPLETE
- [x] Implement XTouchDriver with midir
- [x] MIDI message decoder (parse status bytes, channels, data)
- [x] MIDI message encoder (construct valid MIDI messages)
- [x] Port mapping and device discovery
- [x] Implement MIDI value conversions (14bit ↔ 7bit)
- [x] Basic MIDI sniffer with hex output
- [x] CSV control mapping parser (xtouch-matching.csv)
- [x] **Validation**: Port discovery tested, control mappings verified (129 controls, 11 groups)

## Phase 3: Router and State Management (Week 2-3) ⚡ NEXT
- [ ] Implement Router struct with page management
- [ ] Page navigation via MIDI notes (46/47)
- [ ] Control mapping resolution (control_id → action)
- [ ] StateStore with in-memory MIDI state
- [ ] Anti-echo windows (PB:250ms, CC:100ms, Note:10ms)
- [ ] Shadow state implementation
- [ ] State persistence to JSON snapshots
- [ ] **Validation**: Page switching and control routing match TS behavior

## Phase 4: Driver Framework (Week 3)
- [ ] Define Driver trait with async methods
- [ ] Implement console/log driver for testing
- [ ] Driver registration and lifecycle management
- [ ] ExecutionContext passing
- [ ] Driver hot-reload support
- [ ] **Validation**: Control events trigger correct driver calls

## Phase 5: Application Drivers (Week 4)
- [ ] **5a: Voicemeeter MIDI Bridge**
  - [ ] MIDI passthrough with filters
  - [ ] Port management and reconnection
  - [ ] Transform pipeline (PB→CC for QLC+)
- [ ] **5b: QLC+ Driver**
  - [ ] PB→CC transform (base_cc + channel offset)
  - [ ] Feedback handling
- [ ] **5c: OBS WebSocket Driver**
  - [ ] obws integration
  - [ ] Scene switching
  - [ ] Transform operations (nudge, scale)
  - [ ] Reconnection with backoff
- [ ] **Validation**: Test each driver with real applications

## Phase 6: Feedback Loop (Week 5)
- [ ] Feedback ingestion from applications
- [ ] Anti-echo implementation with time windows
- [ ] XTouch output (motorized faders)
- [ ] LED control and indicators
- [ ] LCD text and color management
- [ ] Fader setpoint scheduling
- [ ] Value overlay on LCD during movement
- [ ] **Validation**: Bidirectional sync without feedback loops

## Phase 7: Advanced Features (Week 6)
- [ ] Hot config reload without dropping events
- [ ] LCD management with labels/colors
- [ ] Fader value overlay (percent/7bit/8bit modes)
- [ ] F1-F8 page navigation with LED feedback
- [ ] Gamepad input support (HID)
- [ ] Web sniffer interface (axum + WebSocket)
- [ ] CLI REPL with command completion
- [ ] **Validation**: Feature parity with TS version

## Phase 8: Polish and Optimization (Week 7)
- [ ] Latency optimization (<20ms target)
- [ ] Lock-free data structures where beneficial
- [ ] Error recovery and reconnection strategies
- [ ] Memory optimization and zero-copy where possible
- [ ] Comprehensive testing suite
- [ ] Documentation and examples
- [ ] Windows installer/packaging
- [ ] **Validation**: Performance benchmarks vs TS baseline

## Backlog (Post-MVP)
- [ ] Linux/macOS platform testing
- [ ] Extended driver support (more apps)
- [ ] Network control protocol
- [ ] Plugin system for custom drivers
- [ ] GUI configuration editor

## Critical Success Metrics
- **Latency**: End-to-end <20ms (measure with oscilloscope if possible)
- **Reliability**: No crashes during 24-hour stress test
- **Compatibility**: 100% config compatibility with TS version
- **Performance**: <1% CPU usage during normal operation
- **Memory**: <50MB RAM usage

## Known Risks to Monitor
- [ ] midir behavior on Windows (WinMM quirks)
- [ ] Port naming inconsistencies
- [ ] Exclusive MIDI port access conflicts
- [ ] OBS WebSocket scene transform complexity
- [ ] Hot reload atomicity challenges

## Testing Checklist (Per Phase)
- [ ] Unit tests for new modules
- [ ] Integration tests with mock MIDI
- [ ] Manual testing with real X-Touch
- [ ] Performance profiling
- [ ] Memory leak detection (valgrind/heaptrack)
- [ ] Cross-reference behavior with TS implementation
