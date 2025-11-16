# Development Guide

## Quick Commands

### Working with Both Versions

#### TypeScript Reference (v2)
```bash
# Navigate to TypeScript version
cd D:\dev\xtouch-gw-v2

# Run the TypeScript version
pnpm dev

# Run the MIDI sniffer
pnpm sniff:web

# Run tests
pnpm test

# Check the configuration
cat config.yaml
```

#### Rust Development (v3)
```bash
# Navigate to Rust version
cd D:\dev\xtouch-gw-v3

# Build debug version
cargo build

# Run with config
cargo run -- -c config.example.yaml

# Run tests
cargo test

# Check for issues
cargo clippy

# Format code
cargo fmt
```

## Side-by-Side Development

For effective porting, run both versions simultaneously:

### Terminal 1 - TypeScript Reference
```bash
cd D:\dev\xtouch-gw-v2
pnpm dev
```

### Terminal 2 - Rust Development
```bash
cd D:\dev\xtouch-gw-v3
cargo watch -x run
```

### Terminal 3 - MIDI Comparison
```bash
# TS Sniffer
cd D:\dev\xtouch-gw-v2
pnpm sniff:web
# Open http://localhost:8123

# Rust Sniffer (when implemented)
cd D:\dev\xtouch-gw-v3
cargo run -- --web-sniffer
# Open http://localhost:8124
```

## Validation Process

When implementing a feature:

1. **Understand the TypeScript**
   ```bash
   # Read the TS implementation
   code D:\dev\xtouch-gw-v2\src\[module].ts
   ```

2. **Capture TS Behavior**
   - Run TS version
   - Perform the action
   - Save MIDI logs

3. **Implement in Rust**
   ```bash
   # Edit Rust code
   code D:\dev\xtouch-gw-v3\src\[module].rs
   ```

4. **Compare Outputs**
   - Run Rust version
   - Perform same action
   - Compare MIDI logs

5. **Verify Timing**
   - Check latency matches
   - Verify anti-echo windows
   - Ensure state consistency

## Common File Mappings

| TypeScript (v2) | Rust (v3) | Purpose |
|-----------------|-----------|---------|
| `src/router.ts` | `src/router.rs` | Event orchestration |
| `src/state/store.ts` | `src/state.rs` | State management |
| `src/xtouch/driver.ts` | `src/xtouch.rs` | Hardware driver |
| `src/midi/utils.ts` | `src/midi.rs` | MIDI utilities |
| `src/drivers/obs/` | `src/drivers/obs.rs` | OBS integration |
| `src/config.ts` | `src/config.rs` | Configuration |
| `src/cli/` | `src/cli.rs` | REPL interface |

## Testing Strategy

### Unit Tests
```rust
// Match TS test cases
// Example: src/midi.rs
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_14bit_conversion() {
        // Port from TS: src/midi/_tests/convert.test.ts
        assert_eq!(to_7bit(16383), 127);
        assert_eq!(to_7bit(8192), 64);
    }
}
```

### Integration Tests
```rust
// tests/integration.rs
// Compare with TS integration tests
```

### Golden Tests
Save known-good MIDI sequences from TS and validate Rust produces identical output.

## Performance Tracking

Track these metrics during development:

| Metric | TypeScript | Rust Target | Actual |
|--------|------------|-------------|--------|
| MIDI Parse | ~50μs | <10μs | TBD |
| State Lookup | ~10μs | <1μs | TBD |
| Route Decision | ~200μs | <50μs | TBD |
| Total Latency | ~25ms | <20ms | TBD |

## Debugging Tips

### MIDI Issues
1. Compare hex dumps between TS and Rust
2. Check byte order and encoding
3. Verify channel calculations (0-based vs 1-based)

### State Inconsistency
1. Dump state from both versions
2. Compare JSON outputs
3. Check update ordering

### Performance Problems
1. Profile with `cargo flamegraph`
2. Use `tracing` spans
3. Compare with TS profiling

## Getting Help

If stuck:
1. Check `D:\dev\xtouch-gw-v2\MEMORY.md` for known issues
2. Review TS implementation for correct behavior
3. Run TS tests to understand expectations
4. Use TS sniffer to capture expected sequences
