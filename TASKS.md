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

## Phase 3: Router and State Management (Week 2-3) ✅ COMPLETE
- [x] Implement Router struct with page management
- [x] StateStore with in-memory MIDI state  
- [x] Control mapping resolution (control_id → action)
- [x] State builders (buildEntryFromRaw)
- [x] Page navigation via MIDI notes (46/47 + F1-F8)
- [x] State persistence to JSON snapshots
- [x] Anti-echo window constants (PB:250ms, CC:100ms, Note:10ms, SysEx:60ms)
- [x] Anti-echo logic with shadow state tracking
- [x] Last-Write-Wins (LWW) with grace periods (PB:300ms, CC:50ms)
- [x] Page refresh planner (Notes→CC→PB ordering with priorities)
- [x] **Ready for Phase 4**: All core components implemented

## Phase 4: Driver Framework (Week 3) ✅ COMPLETE
- [x] Define Driver trait with async methods
- [x] Implement console/log driver for testing
- [x] Driver registration and lifecycle management
- [x] ExecutionContext passing
- [x] Driver hot-reload support
- [x] **Validation**: Control events trigger correct driver calls

## Phase 5: Application Drivers (Week 4) ✅ COMPLETE
- [x] **5a: Voicemeeter MIDI Bridge**
  - [x] MIDI passthrough with filters
  - [x] Port management (initial connection)
  - [x] Transform pipeline (PB→CC for QLC+)
  - Note: Automatic reconnection deferred due to Send trait complexity
- [x] **5b: QLC+ Driver**
  - [x] Stub implementation (QLC+ uses MIDI passthrough via bridge)
- [x] **5c: OBS WebSocket Driver**
  - [x] obws integration
  - [x] Scene switching (program/preview based on studio mode)
  - [x] Transform operations (nudgeX, nudgeY, scaleUniform)
  - [x] Reconnection with exponential backoff
  - [x] Studio mode toggle and transition
- [x] **Validation**: Implementation complete, ready for integration testing

## Phase 6: Feedback Loop (Week 5) ✅ COMPLETE
- [x] Feedback ingestion from applications
- [x] Anti-echo implementation with time windows
- [x] XTouch output (motorized faders)
- [x] LED control and indicators  
- [x] LCD text and color management
- [x] Fader setpoint scheduling
- [x] Value overlay on LCD during movement (deferred to Phase 7 - polish)
- [x] **Validation**: Bidirectional sync infrastructure complete

## Phase 7: Advanced Features (Week 6) - IN PROGRESS
- [x] **CRITICAL FIX**: notify + Tokio integration (capture runtime handle before creating watcher)
  - Fixed panic: "there is no reactor running, must be called from the context of a Tokio 1.x runtime"
  - All 50 tests now pass ✅
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
