# CLAUDE.md

This file provides guidance to Claude (or any AI assistant) when working with the XTouch GW v3 Rust codebase.

## Project Overview

XTouch GW v3 is a MIDI gateway that bridges a Behringer X-Touch control surface with desktop applications (Voicemeeter, QLC+, OBS Studio, Windows audio/media). It also accepts **gamepad/controller** input for OBS camera control, exposes an **HTTP/WebSocket API** (port 8125) for a Stream Deck plugin and a Svelte web editor, and ships a **MIDI sniffer** (port 8123). This is a real-time system with strict latency requirements (<20ms end-to-end).

### What This Project Does
1. **Receives MIDI** from Behringer X-Touch (faders, buttons, encoders)
2. **Accepts gamepad input** (XInput + WGI controllers) for OBS camera/PTZ control
3. **Routes events** to applications via their APIs (OBS WebSocket, MIDI, Win32)
4. **Sends feedback** back to X-Touch (motorized faders, LEDs, LCD)
5. **Manages pages** of control mappings (hot-swappable configurations)
6. **Prevents feedback loops** using time-windowed anti-echo logic
7. **Serves an HTTP/WS API** (port 8125) for Stream Deck and the Svelte web editor

### Critical Performance Constraints
- **Latency**: <20ms end-to-end (MIDI -> App -> Feedback)
- **Memory**: <50MB RAM usage
- **CPU**: <1% during normal operation
- **Reliability**: Zero panics in production

## Build Commands
```bash
cargo build                   # Debug build
cargo build --release         # Optimized build
cargo test                    # Run tests
cargo clippy                  # Linting
cargo fmt                     # Format code
cargo run -- -c config.yaml   # Run with config
```

## Project Structure
```
src/
├── main.rs                     # Entry point, Tokio runtime, CLI args
├── lib.rs                      # Library crate root (config + editor API reused by tests/bins)
├── app.rs                      # Event loop, driver wiring, API server startup
├── event_bus.rs                # Broadcast bus for live editor events
├── cli.rs                      # REPL interface
├── control_mapping.rs          # CSV control mapping parser
├── display.rs                  # X-Touch LCD/LED update helpers
├── driver_setup.rs             # Driver registration and init
├── helpers.rs                  # Startup/shutdown utilities
├── midi.rs                     # MIDI utilities and conversions
├── obs_indicators.rs           # OBS LED/camera indicator callbacks
├── paths.rs                    # Portable/installed path resolution
├── sniffer.rs                  # MIDI debug/sniffing tools
├── state.rs                    # State module re-export
├── xtouch.rs                   # X-Touch driver entry
│
├── api/                        # HTTP/WebSocket API for Stream Deck (port 8125)
│   └── mod.rs                  # Camera-target REST + WS, CSRF guard; mounts editor when enabled
│
├── api_editor/                 # Svelte web editor backend (mounted under /api + /editor on 8125)
│   ├── mod.rs                  # EditorState + router wiring
│   ├── profiles.rs             # Profile CRUD (load/save/activate)
│   ├── schema.rs               # JSON Schema for config (schemars)
│   ├── validate.rs             # Config validation endpoint
│   ├── actions.rs              # Driver action catalogs (DriverCatalogs)
│   ├── action_catalog.rs       # ActionDescriptor / ParamDescriptor types
│   ├── midi_picker.rs          # MIDI port enumeration
│   ├── obs_picker.rs           # OBS scene / source picker
│   ├── page.rs                 # Active-page get/set
│   ├── live.rs                 # Live editor WS (fader snapshot + events)
│   └── static_spa.rs           # Embedded SPA serving (rust-embed)
│
├── config/                     # YAML configuration types
│   ├── mod.rs
│   ├── profiles.rs             # ProfileStore: multi-profile config management
│   └── watcher.rs              # Config file watcher (hot-reload via notify)
│
├── drivers/                    # Application drivers (selected by `app:` name in YAML)
│   ├── mod.rs                  # Driver trait + registration
│   ├── console.rs              # Console/debug driver
│   ├── midibridge.rs           # Generic MIDI bridge — backs `voicemeeter` & `qlc`
│   ├── obs/                    # OBS Studio driver (obs-websocket v5 via obws)
│   │   ├── mod.rs
│   │   ├── driver.rs           # Main OBS driver impl
│   │   ├── connection.rs       # WebSocket connection management
│   │   ├── event_listener.rs   # OBS event subscriptions
│   │   ├── actions.rs          # Button/fader action dispatch
│   │   ├── catalog.rs          # Action catalog for the editor
│   │   ├── picker.rs           # Scene/source picker source
│   │   ├── analog.rs           # Analog control handling
│   │   ├── encoder.rs          # Encoder rotation handling
│   │   ├── camera.rs           # Camera/PTZ control
│   │   ├── camera_actions.rs   # Camera action dispatch (selectCamera/enter/exitSplit)
│   │   ├── ptz_actions.rs      # PTZ movement actions (nudge/zoom via transforms)
│   │   ├── split_mode.rs       # Split-screen mode (FULL/SPLIT_LEFT/SPLIT_RIGHT)
│   │   └── transform.rs        # OBS source transforms
│   ├── winaudio/               # Windows audio (`winaudio`) — WASAPI sessions via Win32 COM
│   │   ├── mod.rs              # DRIVER_NAME, master + per-session volume/mute
│   │   ├── master.rs           # Master endpoint volume
│   │   ├── session.rs          # Per-app session control (pinned/discovered/auto)
│   │   ├── com_thread.rs       # Dedicated COM apartment thread
│   │   └── ...                 # actions, callback, catalog, mapping, events
│   └── winmedia/               # Windows media transport (`winmedia`) — SMTC + SendInput
│       └── mod.rs              # play_pause/stop/next/prev, SMTC playing-state feedback
│
├── input/                      # External input handling
│   ├── mod.rs
│   └── gamepad/                # Gamepad/controller input
│       ├── mod.rs
│       ├── mapper.rs           # Gamepad-to-action mapping
│       ├── provider.rs         # Gamepad provider trait
│       ├── analog.rs           # Analog stick processing
│       ├── axis.rs             # Axis normalization
│       ├── buttons.rs          # Button state handling
│       ├── diagnostics.rs      # Gamepad diagnostics
│       ├── normalize.rs        # Value normalization
│       ├── slot.rs             # Gamepad slot management
│       ├── stick_buffer.rs     # Stick input buffering
│       ├── xinput_convert.rs   # XInput conversion
│       ├── hybrid_id.rs        # Hybrid provider ID
│       ├── hybrid_provider/    # Hybrid gilrs+XInput provider
│       │   ├── mod.rs
│       │   ├── gilrs_events.rs
│       │   ├── scan.rs
│       │   └── xinput.rs
│       └── visualizer/         # Gamepad input visualizer
│           ├── mod.rs
│           ├── app.rs
│           ├── drawing.rs
│           ├── normalize.rs
│           └── rendering.rs
│
├── router/                     # Event orchestration
│   ├── mod.rs                  # Router core
│   ├── page.rs                 # Page management
│   ├── driver.rs               # Driver dispatch
│   ├── feedback.rs             # Feedback routing
│   ├── refresh.rs              # State refresh logic
│   ├── refresh_plan.rs         # Refresh planning
│   ├── anti_echo.rs            # Anti-echo filter
│   ├── camera_target.rs        # Camera auto-targeting
│   ├── indicators.rs           # LED indicator logic
│   ├── xtouch_input.rs         # X-Touch input processing
│   └── tests.rs                # Router unit tests
│
├── state/                      # Actor Model state management
│   ├── actor.rs                # StateActor (single-threaded owner)
│   ├── actor_handle.rs         # Public async API (StateActorHandle)
│   ├── commands.rs             # Message types for actor
│   ├── persistence.rs          # StateSnapshot type
│   ├── persistence_actor.rs    # sled integration (ACID persistence)
│   ├── types.rs                # MidiStateEntry, AppKey, etc.
│   └── builders.rs             # Entry constructors
│
├── tray/                       # System tray integration
│   ├── mod.rs
│   ├── manager.rs              # Tray lifecycle management
│   ├── handler.rs              # Tray event handling
│   ├── activity.rs             # Activity indicator
│   └── icons.rs                # Icon resources
│
└── xtouch/                     # X-Touch hardware sub-modules
    ├── fader_setpoint.rs       # Fader position tracking
    └── pitch_bend_squelch.rs   # PitchBend noise filtering
```

## Architecture

### Core Design Patterns
1. **Actor Model**: Single-threaded state ownership via `StateActor` (eliminates race conditions)
2. **Event-Driven**: Tokio channels for async message passing
3. **ACID Persistence**: sled embedded database with write debouncing (250ms)
4. **Zero-Copy MIDI**: Avoid allocations in hot path
5. **Shadow State**: Track last-sent values for anti-echo (in `StateActor`)
6. **Atomic Config Swap**: Hot-reload without dropping events

### Data Flow
```
X-Touch Input -> MIDI Parser -> Router -> Driver -> Application
                                  |
                            update_state()
                                  v
Application -> Feedback -> StateActor -> Anti-Echo -> X-Touch Output
                              |
                         PersistenceActor (sled)
```

### State Management (Actor Model)
- `StateActor`: Single-threaded owner of all state (runs in dedicated task)
- `StateActorHandle`: Async API for other components
- `PersistenceActor`: sled database writes with debouncing
- All state access via `StateCommand` messages (no locks needed)
- Anti-echo shadow state tracking internal to `StateActor`
- Hydrated entries marked `stale: true`, superseded by fresh feedback
- State stored in `.state/sled/` with keys `state:{app}:{status}:{channel}:{data1}`

## Integrations & Ports

| Integration | Transport / Protocol | Port | Notes |
|-------------|----------------------|------|-------|
| X-Touch surface | USB MIDI (MCU or CTRL mode) | — | Bidirectional; motorized faders, LEDs, LCD |
| Voicemeeter | MIDI (bidirectional) | — | `voicemeeter` app → `midibridge` driver |
| QLC+ | MIDI (output) | — | `qlc` app → `midibridge` driver |
| OBS Studio | obs-websocket v5 (`obws`) | 4455 | `obs` driver; default host `localhost`, `obs.port` configurable |
| Windows audio | Win32 WASAPI (COM) | — | `winaudio` driver; master + per-app session volume/mute |
| Windows media | SMTC + `SendInput` | — | `winmedia` driver; transport keys + play-state feedback |
| Gamepads | HID/XInput (hybrid `gilrs` + `rusty-xinput`) | — | OBS camera/PTZ control; multi-gamepad with shared state |
| HTTP/WS API | axum (loopback only) | **8125** | Stream Deck plugin + Svelte web editor (`/editor`) |
| MIDI sniffer | axum web UI / CLI | **8123** | `--sniffer` / `--web-sniffer --web-port` |

The API server binds `127.0.0.1` only and applies a CSRF Origin allowlist on mutating verbs (loopback + no-Origin clients allowed). Ports are constants: `api::DEFAULT_API_PORT = 8125`; sniffer default `8123`.

### App / Driver Names
Control mappings select a driver via `app:`. Known names: `voicemeeter`, `qlc` (MIDI bridge), `obs`, `winaudio`, `winmedia`. `obs`/`winaudio`/`winmedia` are non-MIDI apps (no `midi.apps` port entry required); MIDI bridge apps must be declared under `midi.apps`.

## OBS Camera Control (Split Views & PTZ)
- Configured via `obs.camera_control` (`cameras[]` with `id`/`scene`/`source`/`split_source`/`enable_ptz`, plus `splits.left`/`right` and optional `default_camera`).
- Shared state across all gamepads: `currentViewMode` (FULL / SPLIT_LEFT / SPLIT_RIGHT) + `lastCamera`. OBS is stateless; all logic lives in the driver.
- Actions: `selectCamera`, `enterSplit`, `exitSplit` (hybrid full/split behavior); PTZ nudge/zoom via OBS source transforms (`reset_transform` supports position/zoom/both).
- **Camera `scene`/`source`/`split_source` names must match the OBS scene collection.** This collection is shared with sibling projects (`obs-manager`, `avolo-cam` Flash sources, `obs-tools`); renaming scenes there breaks camera control here. Full design: `docs/spec-split-obs.md`.

## Gamepad Multi-Support
- Hybrid provider combines `gilrs` (WGI backend, e.g. FaceOff/Switch Pro) and `rusty-xinput` (direct XInput polling, no window needed). See `src/input/gamepad/hybrid_provider/`.
- `gamepad.gamepads[]` slots match physical controllers by `product_match` (substring, case-insensitive); each slot has its own `analog` config and optional `camera_target`.
- `analog`: `pan_gain` (15.0), `zoom_gain` (3.0), `deadzone` (0.02), `gamma` (1.5), `invert` map. Buttons map to OBS actions via `gamepadN.btn.*` keys in `pages_global`/`pages`.
- `camera_target`: omit for static params, `"dynamic"` for runtime selection via the Stream Deck API, or a fixed camera id. Diagnostics/visualizer UI: `src/input/gamepad/visualizer/` (egui).

## HTTP/WebSocket API & Web Editor (port 8125)
- **Stream Deck endpoints** (`src/api/mod.rs`): `GET/PUT /api/gamepad/:slot/camera`, `GET /api/gamepads`, `GET /api/cameras`, `POST /api/cameras/:id/reset`, `GET /api/ws/camera-updates` (push: snapshot, target/on-air changes), `GET /api/health`.
- **Stream Deck plugin** (`streamdeck-plugin/`, TypeScript, UUID `com.juliencr.xtouch-gw`): actions `camera-select` and `camera-reset` call the 8125 API. Build output in `com.juliencr.xtouch-gw.sdPlugin/`.
- **Web editor** (`editor/`, SvelteKit + Tailwind): edits profiles/config, picks MIDI ports & OBS scenes, live virtual surface via the `/api/live` WS. Built to `editor/build`, embedded and served under `/editor` by `api_editor::static_spa` (mounted only when `EditorState` is wired).

## Configuration & Profiles
- Config file: `config.yaml`. Path resolution (`src/paths.rs`): dev mode (debug build with `config.yaml` in CWD) → portable mode (`.portable` marker next to exe) → installed mode (`%APPDATA%\XTouch GW\config.yaml`, example copied on first run).
- `config.example.yaml` is the canonical example (round-trip tested); `profiles/` holds named profiles (e.g. `twitch.yaml`) managed by `config::profiles::ProfileStore` and the editor.
- Hot-reload via `notify` (`config/watcher.rs`): atomic config swap without dropping events.
- State in `.state/` (sled), logs in `logs/` — alongside config in dev/portable mode, under `%APPDATA%` when installed.
- Config schema is derived with `schemars` (`AppConfig`); export via the `export-schema` bin (`src/bin/export-schema.rs`) for the editor's typed forms.

## Code Quality Rules

### Size Limits
- **Files**: Never exceed 500 lines (break up at 400)
- **Functions**: Target 20-30 lines, max 40
- **Struct impls**: Max ~200 lines (including `impl` blocks)

### Design Principles
- **SRP**: Every file, struct, function does ONE thing
- **DRY**: Extract common patterns into utility functions or traits
- **Composition over inheritance**: Traits for behavior contracts, newtype pattern for semantics
- **No god structs/modules**: Split large components into focused sub-files
- **Reusability**: Decoupled, self-contained, injectable dependencies
- **Testability**: Trait objects for external deps, builder patterns for test setup

### Naming
- **Descriptive and domain-specific**: `PitchBendMessage`, `FaderPosition`, `StateSnapshot`
- **Avoid vague names**: `data`, `info`, `helper`, `temp`, `handle_thing`
- Rust conventions: `snake_case` functions/modules, `PascalCase` types/traits, `SCREAMING_SNAKE_CASE` constants

### Error Handling
- `anyhow::Result` for application code, `thiserror` for library errors
- Never `.unwrap()` or `.expect()` on external input
- Use `.context()` for error chains
- Always retry with backoff for connections

### Concurrency
- `Arc<RwLock<T>>` for shared config
- `Arc<Mutex<T>>` for MIDI ports
- Prefer channels over shared memory
- Always use bounded channels

## MIDI Specifics
- **14-bit values**: Faders use PitchBend (0-16383)
- **7-bit values**: Buttons/encoders use CC/Note (0-127)
- **Channel semantics**: In MCU mode, channel = physical strip
- **Anti-echo windows**: PB=250ms, CC=100ms, Note=10ms

### Windows MIDI Quirks
- Port names include suffixes like "MIDIIN2"
- Exclusive access - only one open per port
- Recovery needs 250ms+ after disconnect
- Use substring matching for port discovery

## Common Tasks

### Adding a New Driver
1. Implement the `Driver` trait in `drivers/`
2. Add configuration types in `config/`
3. Register in `driver_setup.rs`
4. Add tests with mock MIDI

### Modifying MIDI Routing
1. Update state types in `state/`
2. Adjust anti-echo windows in `router/anti_echo.rs` if needed
3. Test with real hardware
4. Verify no feedback loops

### Debugging Latency Issues
1. Use `tracing` spans to measure stages
2. Check for blocking operations
3. Profile with `cargo flamegraph`
4. Verify channel buffer sizes

## Common Pitfalls

### MIDI and Hardware
1. **Channel confusion**: Fader channel != target CC channel
2. **Double port opening**: Check passthrough before control MIDI
3. **Missing feedback**: Drivers must emit or faders won't sync
4. **LCD restoration**: Only bottom line, top unchanged

### Rust-Specific
1. **Blocking runtime**: Use `spawn_blocking` for CPU work
2. **Large futures**: Box recursive async functions
3. **Channel deadlock**: Always use bounded channels
4. **Panic in tasks**: Wrap spawns with error handling

## Performance Hot Paths
Avoid allocations in these paths:
- MIDI message parsing (`midi.rs`)
- State lookups (`state/`)
- Event routing (`router/`)

Tips: Use `with_capacity` for collections, `Arc` over cloning, coalesce events within 16ms windows, profile before optimizing.

## Testing
- Mock MIDI ports with `mockall`
- Use `tokio::test` for async tests
- Test anti-echo windows with `tokio::time::pause()`
- Integration tests require real X-Touch hardware

## Reference Documents
- **[TASKS.md](TASKS.md)**: Current development status and priorities
- **[docs/spec-split-obs.md](docs/spec-split-obs.md)**: OBS camera/split control design (gamepad-driven)
- **[docs/STATE_ACTOR_ARCHITECTURE.md](docs/STATE_ACTOR_ARCHITECTURE.md)**: Actor-model state management rationale
- **[editor/README.md](editor/README.md)**: Svelte web editor

## grepai - Semantic Code Search

**Use grepai as primary tool for intent-based code exploration.** Fall back to Grep/Glob for exact text matching or file path patterns.

```bash
grepai search "anti-echo filtering logic"
grepai search "OBS camera control" --json
grepai search "gamepad input processing"
```

Describe intent, not implementation. Use English queries for best semantic matching.
