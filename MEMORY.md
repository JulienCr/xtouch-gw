# XTouch GW v3 (Rust) - Development Memory

> Important lessons from the TypeScript implementation and new discoveries during Rust development.
> Goal: Avoid repeating mistakes and document critical implementation details.

## Phase 1 Completion Summary (November 2025)

**✅ Successfully completed Phase 1: Core Runtime Foundation**

Key achievements:
- Comprehensive config types with serde (all TS config fields supported)
- Config validation catches errors before runtime (found real config bugs!)
- Config file watcher with notify for hot-reload
- Error handling strategy documented (anyhow for app code, thiserror for libraries)
- Full config.yaml loading with validation working

Important learnings:
1. **Config Validation is Critical**: Found incomplete control mappings that would cause runtime issues
2. **Make Fields Optional First**: Use `Option<T>` for all optional fields, validate in separate step
3. **MIDI passthrough type**: Special case that doesn't require channel/cc/note fields
4. **Windows Process Locking**: Running process prevents rebuild - always kill before rebuild
5. **Validation Messages**: Rich error messages with context help debug config issues quickly

## Phase 2 Completion Summary (November 2025)

**✅ Successfully completed Phase 2: MIDI Infrastructure**

Key achievements:
- Complete MIDI message parsing/encoding (all MIDI 1.0 types)
- XTouch hardware driver with async I/O and event channels
- Port discovery with Windows-friendly substring matching
- MIDI sniffer with CLI and web UI foundation
- Control mapping database (129 controls, 11 groups)
- Value conversion utilities (14-bit ↔ 7-bit, percentages)

Critical learnings:
1. **midir v0.10 API Change**: Uses port references, not indices - `midi_in.ports().iter()` required
2. **Windows MIDI Port Names**: Vary ("UM-One" vs "2- UM-ONE") - substring matching essential
3. **MIDI Channel Convention**: External 1-16, internal 0-15 - always subtract 1 when parsing "ch1"
4. **Arc<Mutex<>> for MIDI Output**: Required for thread-safe async sending from multiple contexts
5. **Event Channel Size**: 1000 messages buffer prevents loss during rapid MIDI bursts
6. **Embedded CSV**: Use `include_str!` for control mappings - eliminates file dependency
7. **Async Callbacks with midir**: Callback must be `'static` - use channels to escape lifetime
8. **PitchBend vs CC**: MCU mode uses PitchBend for 14-bit faders, CTRL mode uses CC (7-bit)
9. **CSV Parser Validation**: Validate MIDI specs during parsing to catch errors early
10. **Reverse Lookup Pattern**: O(n) scan acceptable for 129 controls - HashMap would be overkill

## 🎯 REMEMBER: TypeScript is the Reference

**The TypeScript implementation at `D:\dev\xtouch-gw-v2\` is the source of truth.**

When in doubt:
1. Check the TS code for the correct behavior
2. Run both versions side-by-side to compare
3. Use the TS sniffer to capture expected MIDI sequences
4. Replicate the exact timing and message ordering from TS

This is a PORT, not a redesign - match the TS behavior exactly first, optimize later.

## Critical Constants and Timings

### Anti-Echo Windows (Validated in TS)
- **PitchBend**: 250ms - Motors need time to settle
- **Control Change**: 100ms - Encoders can generate rapid changes  
- **Note On/Off**: 10ms - Buttons are discrete events
- **SysEx**: 60ms - Fallback for other messages

### MIDI Value Conversions
- **14-bit to 7-bit**: Divide by 128 (shift right 7)
- **7-bit to 14-bit**: Multiply by 128 (shift left 7)
- **Percent from 14-bit**: `(value * 100) / 16383`
- **Important**: Always preserve LSB in state for fader precision

## Lessons from TypeScript Implementation

### MIDI on Windows (Critical for Rust)
- **Port Names**: Windows adds "MIDIIN2"/"MIDIOUT2" suffixes - use substring matching
- **WinMM Issues**: Ports can become unavailable at runtime - implement aggressive reconnection with backoff
- **Exclusive Access**: Only one process can open a MIDI port - coordinate between passthrough and control modes
- **Port Loss**: After disconnection, ports may need 250ms+ before reopening succeeds

### Fader Motor Behavior
- **Setpoint Scheduling**: After user movement, motor needs explicit position target
- **MCU Mode**: Channel in PitchBend message = physical fader (1-9)
- **Feedback Suppression**: Ignore incoming PB for 250ms after sending to prevent oscillation
- **Resolution**: X-Touch uses full 14-bit range (0-16383), not 7-bit

### State Management
- **Source of Truth**: MIDI state per application, keyed by (port, status, channel, data1)
- **Optimistic Updates**: Update state immediately on send, before app confirms
- **Page Changes**: Must replay entire state to sync hardware
- **Stale Entries**: Mark restored states as potentially outdated

### Router Architecture
- **Single Orchestrator**: All MIDI output should flow through one component to prevent conflicts
- **Shadow States**: Keep last-sent values to implement anti-echo
- **Last-Write-Wins**: User actions take precedence over app feedback within time window
- **Event Ordering**: Refresh sequence must be Notes → CC → SysEx → PitchBend

### Performance Insights
- **Coalesce Fader Events**: Batch updates within 16ms window (60Hz)
- **Avoid Blocking**: Never block the event loop - use channels for communication
- **Preallocate Buffers**: MIDI messages are small, pool them
- **Lock-Free Where Possible**: Use dashmap/crossbeam for concurrent access

## Rust-Specific Considerations

### Tokio Runtime
- **Don't Block**: Use `tokio::spawn_blocking` for CPU-intensive work
- **Channel Selection**: Use `mpsc` for one-to-many, `watch` for config updates
- **Buffer Sizes**: Start with 1000 for MIDI event channels

### Error Handling Strategy
- **Connection Errors**: Retry with exponential backoff
- **MIDI Errors**: Log and continue - never panic
- **Config Errors**: Reject hot-reload, keep previous config
- **Panic Safety**: Only panic on programmer errors, not runtime issues

### Memory Management
- **Arc<RwLock<T>>**: For shared state (config, drivers)
- **Arc<Mutex<T>>**: For MIDI ports (exclusive access)
- **Cow<str>**: For string data that's mostly read
- **SmallVec**: For MIDI messages (usually 3 bytes)

## Common Pitfalls to Avoid

### From TypeScript Experience

1. **Double Port Opening**: Check if passthrough exists before opening control MIDI ports
2. **Channel Confusion**: MCU fader channel ≠ target CC channel for apps
3. **Hardcoded Names**: Never hardcode control names or app names
4. **Missing Feedback**: Drivers must emit feedback or faders won't sync
5. **LCD Restoration**: Only restore bottom line after overlay, top stays unchanged

### Anticipated Rust Challenges

1. **Lifetime Complexity**: Keep callbacks simple, use channels instead
2. **Async Trait Methods**: Use `async-trait` crate, be aware of boxing overhead
3. **Cross-Thread State**: Prefer message passing to shared memory
4. **Error Propagation**: Use `anyhow` for applications, `thiserror` for libraries

## Testing Insights

### MIDI Testing
- **Mock Ports**: Create virtual MIDI ports for CI testing
- **Golden Logs**: Record real MIDI sequences for regression testing  
- **Timing Tests**: Use `tokio::time::pause()` for deterministic tests

### Integration Testing
- **Real Hardware**: Can't fully simulate X-Touch behavior
- **OBS Mock**: Consider mockito for WebSocket testing
- **Config Variations**: Test all transform combinations

## Performance Benchmarks (Target)

Based on TS measurements, Rust should achieve:
- **MIDI Parse**: <10μs per message
- **State Lookup**: <1μs per query
- **Route Decision**: <50μs per control
- **Config Reload**: <10ms for full reload

## Debugging Tools

### Essential for Development
1. **MIDI Sniffer**: Hex dump with timestamps
2. **State Dumper**: JSON export of current state
3. **Latency Tracer**: Measure each pipeline stage
4. **Event Logger**: Structured logs with correlation IDs

## Architecture Decisions

### What to Keep from TS
- ✅ Page-based routing model
- ✅ YAML configuration format
- ✅ Anti-echo time windows
- ✅ Shadow state pattern
- ✅ Driver trait abstraction

### What to Improve in Rust
- ⚡ Lock-free state where possible
- ⚡ Zero-copy MIDI routing
- ⚡ Compile-time config validation
- ⚡ Better error recovery
- ⚡ Native performance

## TODO: Document During Development

- [ ] Exact midir connection sequence for Windows
- [ ] Optimal channel buffer sizes
- [ ] Best `notify` debounce duration for config reload
- [ ] OBS transform calculation precision requirements
- [ ] Gamepad polling rate vs. latency tradeoff
