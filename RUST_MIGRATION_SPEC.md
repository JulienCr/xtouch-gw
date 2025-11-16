# XTouch GW Rust Migration Specification

You can find the legacy TypeScript implementation D:\dev\xtouch-gw-v2\

## 1. Current Architecture Summary

The XTouch GW TypeScript implementation is an event-driven MIDI gateway that bridges a Behringer X-Touch control surface with desktop applications (Voicemeeter, QLC+, OBS Studio). The core Router (`src/router.ts`) orchestrates bidirectional MIDI flow between the hardware and application drivers, managing page-based control mappings with hot-reloadable YAML configuration. State management (`src/state/`) maintains a centralized MIDI state per application with anti-echo logic using time-windowed suppression (10-250ms by MIDI type) and shadow states for conflict resolution.

The system implements a sophisticated feedback loop where user actions from X-Touch are routed through drivers to applications, then feedback flows back to update motorized faders, LEDs, and LCD displays. Critical features include optimistic state updates, page-based passthrough bridges, MIDI transformations (PitchBend→CC for QLC+ compatibility), and comprehensive CLI tools for development and debugging.

## 2. Config and Behavior Model

### Configuration Schema (YAML)
```yaml
midi:
  input_port: "UM-One"               # MIDI input port name fragment
  output_port: "UM-One"              # MIDI output port name fragment
  apps:                              # App-specific MIDI ports
    - name: "voicemeeter"
      output_port: "xtouch-gw"
      input_port: "xtouch-gw-feedback"

obs:
  host: "localhost"
  port: 4455
  password: "secret"

xtouch:
  mode: "mcu"                        # "mcu" (PitchBend) or "ctrl" (CC)
  overlay:                           # Fader value overlay on LCD
    enabled: true
    cc_bits: "7bit"                  # "7bit" or "8bit" for CC display

paging:
  channel: 1                         # MIDI channel for navigation
  prev_note: 46                      # Previous page note
  next_note: 47                      # Next page note

pages_global:                        # Default values for all pages
  controls: {}
  lcd: {}
  passthroughs: []

pages:
  - name: "Voicemeeter+QLC"
    lcd:
      labels:                        # 8 LCD strips (strings or {upper,lower})
        - "Mic\nBaba"
        - {upper: "Mic", lower: "Math"}
      colors: ["red", 0xFF0000]
    passthroughs:                    # MIDI bridges per page
      - driver: "midi"
        to_port: "qlc-in"
        from_port: "qlc-out"
        filter:
          channels: [1,2,3,4,5,6,7]
          types: ["pitchBend", "controlChange", "noteOn"]
        transform:
          pb_to_cc:                  # Transform PitchBend to CC
            target_channel: 1
            base_cc: 0x45
    controls:                        # Control mappings
      fader1:
        app: "qlc"
        action: "setChannelValue"   # Driver method
        params: [1, "{value}"]
      button1:
        app: "obs"
        midi:                        # Direct MIDI mode
          type: "cc"
          channel: 1
          cc: 81
```

### Rust Type Model
```rust
// Core config types
#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub midi: MidiConfig,
    pub obs: Option<ObsConfig>,
    pub xtouch: Option<XTouchConfig>,
    pub paging: Option<PagingConfig>,
    pub pages_global: Option<GlobalPageDefaults>,
    pub pages: Vec<PageConfig>,
}

#[derive(Debug, Clone)]
pub struct PageConfig {
    pub name: String,
    pub lcd: Option<LcdConfig>,
    pub passthroughs: Vec<PassthroughConfig>,
    pub controls: HashMap<String, ControlMapping>,
}

#[derive(Debug, Clone)]
pub enum ControlAction {
    DriverAction {
        app: String,
        action: String,
        params: Vec<serde_json::Value>,
    },
    MidiDirect {
        app: String,
        spec: MidiSpec,
    },
}

// State types
#[derive(Debug, Clone)]
pub struct MidiStateEntry {
    pub addr: MidiAddr,
    pub value: MidiValue,
    pub timestamp: Instant,
    pub origin: Origin,
    pub known: bool,
    pub stale: bool,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct MidiAddr {
    pub port_id: String,
    pub status: MidiStatus,
    pub channel: Option<u8>,
    pub data1: Option<u8>,
}
```

### Runtime Behavior
1. **Page Navigation**: NoteOn events on configured channel trigger page switches
2. **Control Routing**: Router resolves control_id → mapping → driver action or MIDI send
3. **Feedback Flow**: App changes → StateStore update → anti-echo check → XTouch update
4. **Hot Reload**: Config watcher triggers atomic swap of routing tables and initial feedback
5. **Anti-Echo**: Time-windowed suppression prevents feedback loops (PB:250ms, CC:100ms, Note:10ms)

## 3. Rust Architecture Proposal

### Module Structure
```
src/
├── main.rs                 # Tokio runtime, CLI args, app startup
├── config/
│   ├── mod.rs             # Config types and loader
│   ├── watcher.rs         # Hot reload via notify crate
│   └── validator.rs       # Config validation
├── router/
│   ├── mod.rs             # Core Router implementation
│   ├── anti_echo.rs       # Anti-feedback logic
│   ├── page.rs            # Page management
│   ├── planner.rs         # Refresh planning
│   └── forward.rs         # Feedback forwarding
├── state/
│   ├── mod.rs             # StateStore trait
│   ├── store.rs           # In-memory state implementation
│   ├── persistence.rs     # Snapshot save/restore
│   └── types.rs           # MidiStateEntry, MidiAddr
├── xtouch/
│   ├── mod.rs             # XTouchDriver
│   ├── api.rs             # High-level API (LCD, LEDs, faders)
│   ├── input_mapper.rs    # Input event mapping
│   └── indicators.rs      # LED indicator management
├── midi/
│   ├── mod.rs             # MIDI utilities
│   ├── client.rs          # MidiAppClient
│   ├── decoder.rs         # MIDI message parsing
│   ├── convert.rs         # Value conversions (14bit↔7bit)
│   └── transform.rs       # PB→CC transforms
├── drivers/
│   ├── mod.rs             # Driver trait
│   ├── obs.rs             # OBS WebSocket driver
│   ├── qlc.rs             # QLC+ driver
│   ├── voicemeeter.rs     # Voicemeeter MIDI bridge
│   └── midi_bridge.rs     # Generic MIDI passthrough
├── cli/
│   ├── mod.rs             # CLI runner
│   ├── commands.rs        # Command implementations
│   └── repl.rs            # Interactive REPL
└── sniffer/
    ├── mod.rs             # MIDI sniffer
    └── web.rs             # Web interface via warp
```

### Key Traits/Interfaces
```rust
#[async_trait]
pub trait Driver: Send + Sync {
    fn name(&self) -> &str;
    async fn init(&mut self, config: &AppConfig) -> Result<()>;
    async fn execute(&self, action: &str, params: Vec<Value>, context: ExecutionContext) -> Result<()>;
    async fn sync(&self) -> Result<()>;
    fn subscribe_indicators(&self, tx: mpsc::Sender<(String, Value)>) -> Result<()>;
    async fn shutdown(&self) -> Result<()>;
}

pub trait StateStore: Send + Sync {
    fn update_from_feedback(&self, app: &str, entry: MidiStateEntry);
    fn get_state(&self, app: &str, addr: &MidiAddr) -> Option<MidiStateEntry>;
    fn list_states(&self, app: &str) -> Vec<MidiStateEntry>;
    fn subscribe(&self, callback: StateCallback) -> SubscriptionId;
}

pub trait MidiDevice: Send + Sync {
    async fn open(&mut self, input: &str, output: &str) -> Result<()>;
    async fn send(&self, data: &[u8]) -> Result<()>;
    fn set_callback(&mut self, callback: MidiCallback);
    async fn close(&mut self) -> Result<()>;
}
```

### Concurrency and Event Flow
```rust
// Main event loop using Tokio channels
pub struct Router {
    state: Arc<RwLock<StateStore>>,
    drivers: Arc<RwLock<HashMap<String, Box<dyn Driver>>>>,
    config: Arc<RwLock<AppConfig>>,
    xtouch: Arc<Mutex<XTouchDriver>>,
    
    // Event channels
    control_tx: mpsc::Sender<ControlEvent>,
    feedback_tx: mpsc::Sender<FeedbackEvent>,
    config_tx: watch::Sender<AppConfig>,
}

// Event flow:
// 1. XTouch input → control_rx → Router::handle_control
// 2. Router → Driver::execute → Application
// 3. Application feedback → feedback_rx → StateStore update
// 4. StateStore → anti-echo check → XTouch output

// Anti-echo using time-based windows
pub struct AntiEchoFilter {
    shadows: Arc<RwLock<HashMap<String, ShadowEntry>>>,
    windows: HashMap<MidiStatus, Duration>,
}

// Fader event coalescing
pub struct FaderCoalescer {
    pending: Arc<Mutex<HashMap<u8, FaderUpdate>>>,
    interval: Duration, // e.g., 16ms for 60Hz
}
```

### Hot Reload Implementation
```rust
// Atomic config swap with graceful driver reconfiguration
async fn handle_config_reload(router: &Router, new_config: AppConfig) {
    // 1. Parse and validate new config
    let validated = validate_config(&new_config)?;
    
    // 2. Atomic swap routing tables
    {
        let mut cfg = router.config.write().await;
        *cfg = validated;
    }
    
    // 3. Reconfigure drivers without dropping connections
    for (name, driver) in router.drivers.write().await.iter_mut() {
        driver.on_config_changed(&validated).await?;
    }
    
    // 4. Refresh current page state
    router.refresh_page().await?;
}
```

## 4. Dependency Mapping (TS → Rust)

| TypeScript | Rust Equivalent | Notes |
|------------|----------------|-------|
| `@julusian/midi` | `midir` | Cross-platform MIDI I/O, stable on Windows |
| `obs-websocket-js` | `obws` | Native obs-websocket v5 client |
| `yaml` | `serde_yaml` | YAML parsing with serde |
| `chokidar` | `notify` | Cross-platform file watching |
| `node-hid` | `hidapi` | HID device access for gamepad |
| `chalk` | `colored`/`termcolor` | Terminal colors |
| `dotenv` | `dotenvy` | Environment variable loading |
| `readline` | `rustyline` | REPL with history and completion |
| WebSocket server | `tokio-tungstenite` + `warp` | For sniffer web UI |
| Express-like | `axum` or `warp` | HTTP server for web tools |
| PM2 | `systemd` service or custom supervisor | Process management |

## 5. Migration Plan (Phases)

### Phase 1: Core Runtime Foundation (Week 1)
**Scope**: Basic Tokio runtime, config loader, logging, CLI skeleton
- Implement `AppConfig` types with serde
- YAML config parsing and validation
- Basic CLI with rustyline REPL
- Structured logging with `tracing`
**Validation**: Load same `config.yaml` as TS version, verify parsing
**TS Reference**: `src/config.ts`, `src/logger.ts`, `src/cli/index.ts`

### Phase 2: MIDI Infrastructure (Week 2)
**Scope**: XTouch driver, MIDI I/O, sniffer
- Implement `XTouchDriver` with midir
- MIDI message decoder/encoder
- Port mapping and device discovery
- Basic sniffer with hex output
**Validation**: Connect to X-Touch, log raw MIDI, compare with TS sniffer output
**TS Reference**: `src/xtouch/driver.ts`, `src/midi/`, `src/sniffer-server.ts`

### Phase 3: Router and State Management (Week 2-3)
**Scope**: Router core, page system, state store
- Implement `Router` with page navigation
- `StateStore` with in-memory storage
- Control mapping resolution
- Basic anti-echo (no drivers yet)
**Validation**: Page switching via MIDI notes, control routing logs match TS
**TS Reference**: `src/router.ts`, `src/state/`, `src/router/page.ts`

### Phase 4: Driver Framework (Week 3)
**Scope**: Driver trait, one simple driver
- Define `Driver` trait
- Implement console driver (logs only)
- Driver registration and lifecycle
- Execution context passing
**Validation**: Control events trigger driver logs identical to TS
**TS Reference**: `src/types.ts`, `src/drivers/consoleDriver.ts`

### Phase 5: Application Drivers (Week 4)
**Scope**: Real drivers one by one
- **5a**: Voicemeeter MIDI bridge (simplest)
- **5b**: QLC+ with PB→CC transform
- **5c**: OBS WebSocket integration
**Validation**: Each driver tested against real app, compare MIDI/WebSocket traffic
**TS Reference**: `src/drivers/midibridge/`, `src/drivers/qlc.ts`, `src/drivers/obs/`

### Phase 6: Feedback Loop (Week 5)
**Scope**: Complete bidirectional sync
- Feedback ingestion from apps
- Anti-echo with time windows
- XTouch output (faders, LEDs, LCD)
- State persistence/restoration
**Validation**: Fader movements sync both ways, no feedback loops
**TS Reference**: `src/router/forward.ts`, `src/router/antiEcho.ts`, `src/state/persistence.ts`

### Phase 7: Advanced Features (Week 6)
**Scope**: Feature parity
- Hot config reload with notify
- LCD management with labels/colors
- Fader value overlay
- Gamepad input support
- Web sniffer interface
**Validation**: Full feature comparison with TS version
**TS Reference**: `src/ui/lcd.ts`, `src/input/gamepad/`, `src/app/bootstrap.ts`

### Phase 8: Polish and Optimization (Week 7)
**Scope**: Performance and stability
- Latency optimization (<20ms target)
- Error recovery and reconnection
- Memory optimization
- Documentation and tests
**Validation**: Stress testing, latency measurements vs TS baseline

## 6. Risks and Open Questions

### Technical Risks

1. **MIDI on Windows**: `midir` uses WinMM on Windows which has quirks:
   - Port naming inconsistencies (may need fuzzy matching)
   - Exclusive port access (coordinate between passthrough and control MIDI)
   - **Mitigation**: Early prototype to validate midir behavior, fallback to `windows-sys` FFI if needed

2. **Real-time Constraints**: Achieving <20ms latency requires careful design:
   - Avoid blocking in event loops
   - Use lock-free structures where possible (dashmap, crossbeam)
   - **Mitigation**: Profile early, use `tokio::time::Instant` for measurements

3. **OBS WebSocket Complexity**: The `obws` crate may not cover all edge cases:
   - Scene item transforms (complex nested JSON)
   - Event subscription flags
   - **Mitigation**: Review `src/drivers/obs/transforms.ts` carefully, may need custom JSON handling

4. **Hot Reload Atomicity**: Config reload must not disrupt active MIDI flow:
   - Use RwLock for config with brief write locks
   - Driver reconfiguration must be graceful
   - **Mitigation**: Implement versioned config with smooth transitions

### Open Questions

**TODO: Voicemeeter API Details**
- Current TS uses MIDI bridge only - is there a native API to explore?
- What's the exact MIDI message format for Voicemeeter control?
- **Action**: Sniff and document all Voicemeeter MIDI messages

**TODO: QLC+ WebSocket API**
- TS version has stub driver - is WebSocket API available?
- Document QLC+ MIDI input expectations (CC ranges, channel mapping)
- **Action**: Research QLC+ documentation and test with real instance

**TODO: Exact Anti-Echo Timings**
- Current windows: PB=250ms, CC=100ms, Note=10ms
- Are these optimal or need tuning for Rust's different timing characteristics?
- **Action**: Benchmark Rust event loop latency, adjust windows accordingly

**TODO: Fader Motor Behavior**
- Setpoint scheduling in `src/xtouch/faderSetpoint.ts` needs careful porting
- Motor behavior might differ with Rust's timing
- **Action**: Test fader response curves, implement PID if needed

**TODO: State Persistence Format**
- TS uses JSON snapshots - keep same format for compatibility?
- Consider more efficient format (bincode, MessagePack)?
- **Action**: Benchmark serialization performance, prioritize compatibility

### Platform-Specific Concerns

- **Windows**: Primary target, needs careful testing of MIDI and HID
- **Linux/macOS**: Should work with midir but not primary focus for v1
- **Packaging**: Consider single binary with embedded assets vs installer

### Performance Targets

- MIDI input → Driver execution: <5ms
- Driver → Application: <10ms  
- Application feedback → XTouch: <5ms
- Total round-trip: <20ms
- Config reload: <100ms without dropping events
