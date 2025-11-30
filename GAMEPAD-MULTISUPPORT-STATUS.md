# Multi-Gamepad Support - Development Status

**Branch**: `dev-gamepad`
**Date**: 2025-11-30
**Status**: ⚠️ Implementation Complete, Testing In Progress

## Overview

Implementation of concurrent multi-gamepad support to allow multiple controllers (Xbox Bluetooth + Faceoff) to control different OBS cameras simultaneously.

## ✅ Completed Implementation

### 1. Diagnostic Tool
- **File**: `src/input/gamepad/diagnostics.rs`
- **CLI Flag**: `--gamepad-diagnostics`
- Added 5-second wait for Bluetooth controller enumeration
- Successfully detects both gamepads:
  - `"Contrôleur de jeu HID"` (Xbox Bluetooth, 10% battery)
  - `"Faceoff Pro Nintendo Switch Controller"` (Wired)

### 2. Configuration System
- **File**: `src/config/mod.rs`
- Added `GamepadSlotConfig` structure
- Added `gamepads: Vec<GamepadSlotConfig>` array to `GamepadConfig`
- Maintains backward compatibility with legacy single-gamepad config
- Per-gamepad analog settings (deadzone, gamma, inversion)

**Example Config**:
```yaml
gamepad:
  enabled: true
  provider: hid
  gamepads:
    - product_match: "Faceoff"     # Slot 0 → gamepad1.*
      analog:
        deadzone: 0.02
        gamma: 1.5
    - product_match: "Contrôleur"  # Slot 1 → gamepad2.*
      analog:
        deadzone: 0.02
        gamma: 1.5
```

### 3. Slot Management System
- **File**: `src/input/gamepad/slot.rs` (NEW)
- `GamepadSlot`: Tracks individual gamepad position with connection state
- `SlotManager`: Handles slot assignment, connection, disconnection
- Auto-numbering: gamepad1, gamepad2, etc.
- Slot preservation on disconnect/reconnect

### 4. Provider Refactor
- **File**: `src/input/gamepad/provider.rs`
- Multi-gamepad event loop with SlotManager integration
- 3-second initial scan for Bluetooth controller enumeration
- Control IDs include slot prefix: `gamepad1.btn.a`, `gamepad2.axis.lx`
- Per-slot analog configuration embedded in events
- Legacy mode support (empty slot_configs = "gamepad" prefix)

### 5. Mapper Updates
- **File**: `src/input/gamepad/mapper.rs`
- Updated event structure with embedded `analog_config`
- Axis name extraction from multi-gamepad control IDs
- Supports both `gamepad.axis.lx` (legacy) and `gamepad1.axis.lx` (multi)

### 6. Initialization
- **File**: `src/input/gamepad/mod.rs`
- Builds slot configs from `gamepads` array or legacy `hid.product_match`
- Backward compatible with existing single-gamepad configurations

### 7. Dependencies
- **Upgraded**: gilrs from 0.10 → 0.11 for better Windows Bluetooth support

## ⚠️ Known Issues (Testing Phase)

### Issue 1: Faceoff Controller Not Moving OBS Camera
**Symptom**:
- Events are detected: `gamepad1.axis.lx`, `gamepad1.axis.ly`
- Values always show `0.0` in logs
- All values caught by deadzone logic
- OBS commands execute but with zero values

**Logs**:
```
DEBUG Executing obs.nudgeX for control 'gamepad1.axis.lx' (value: Some(Number(0.0)))
DEBUG ✅ Router handled axis (deadzone): gamepad1.axis.lx = 0.0
```

**Hypothesis**:
- Logs only show "stick released" events (value = 0.0)
- Need to capture "stick moved" events with non-zero values
- May be event flood issue where center events override movement events

**Test Needed**:
- Move stick and HOLD (don't release) to see if non-zero values appear
- Check if camera moves during hold
- Verify analog processing chain: raw value → deadzone → gamma → inversion

### Issue 2: Xbox Bluetooth Controller Not Generating Events
**Symptom**:
- Controller detected and assigned to slot 1 (gamepad2)
- No events appear in logs when pressing buttons
- Only see `gamepad1.*` events, never `gamepad2.*`

**Logs**:
```
✅ Gamepad 2 connected: Contrôleur de jeu HID (ID: GamepadId(0))
```
(But no subsequent button/axis events)

**Hypothesis**:
1. Events are generated but filtered out somewhere
2. Bluetooth controller not sending events to gilrs (Windows driver issue)
3. Event loop not processing events from GamepadId(0) correctly

**Test Needed**:
- Press buttons on Xbox controller while monitoring logs
- Run diagnostic tool to verify button detection
- Check if any `gamepad2.*` events appear with DEBUG logging

### Issue 3: Slot Assignment Warnings (RESOLVED)
**Symptom** (FIXED):
```
WARN ⚠️  Gamepad "Faceoff" matches slot 1 but already occupied
```

**Resolution**:
- Fixed in `slot.rs`: `try_connect()` now silently ignores re-connection attempts
- Slots correctly assigned:
  - Slot 0 (gamepad1): Faceoff
  - Slot 1 (gamepad2): Xbox Bluetooth

## 🔍 Debugging Strategy

### Enable Detailed Logging
```bash
RUST_LOG=debug cargo run --release
```

### Test Scenarios

1. **Faceoff Axis Test**:
   - Move left stick and HOLD
   - Look for non-zero values: `gamepad1.axis.lx: value: 0.75`
   - Verify OBS camera movement

2. **Faceoff Button Test**:
   - Press button A
   - Look for: `gamepad1.btn.a` event
   - Verify OBS scene change

3. **Xbox Button Test**:
   - Press button A on Xbox controller
   - Look for: `gamepad2.btn.a` event
   - Verify scene change in OBS

4. **Xbox Axis Test**:
   - Move left stick on Xbox
   - Look for: `gamepad2.axis.lx` events
   - Verify CAM Jardin movement

5. **Concurrent Test**:
   - Move both controllers simultaneously
   - Verify both cameras move independently

### Diagnostic Commands
```bash
# Check gamepad detection
cargo run --release -- --gamepad-diagnostics

# Run with full debug logging
RUST_LOG=trace cargo run --release
```

## 📁 Modified Files

### Core Implementation
- `src/config/mod.rs` - Config structures
- `src/input/gamepad/slot.rs` - NEW: Slot manager
- `src/input/gamepad/provider.rs` - Multi-gamepad event loop
- `src/input/gamepad/mapper.rs` - Event handling
- `src/input/gamepad/mod.rs` - Initialization
- `src/input/gamepad/diagnostics.rs` - Detection tool
- `src/main.rs` - CLI flag

### Configuration
- `config.yaml` - Multi-gamepad setup
- `Cargo.toml` - gilrs 0.11 upgrade

## 🎯 Next Steps

1. **Resolve Faceoff Issue**:
   - Verify analog value processing
   - Check event generation timing
   - Ensure non-zero values reach OBS driver

2. **Resolve Xbox Issue**:
   - Verify event generation for GamepadId(0)
   - Check Windows Bluetooth HID driver compatibility
   - Test with USB connection as baseline

3. **Integration Testing**:
   - Verify both controllers work independently
   - Test concurrent usage
   - Verify hot-plug/reconnect behavior

4. **Performance Validation**:
   - Measure latency with multiple gamepads
   - Verify <20ms target maintained
   - Check CPU usage <5%

5. **Documentation**:
   - Update README with multi-gamepad setup
   - Add migration guide from single to multi-gamepad
   - Document control ID naming scheme

## 🔧 Architecture

### Control ID Format
- **Legacy**: `gamepad.btn.a`, `gamepad.axis.lx`
- **Multi-gamepad**: `gamepad1.btn.a`, `gamepad2.axis.lx`
- **Numbering**: Slot index + 1 (slot 0 = gamepad1)

### Event Flow
```
Gamepad Hardware
  ↓ (gilrs polling)
GilrsProvider::event_loop_blocking
  ├─ SlotManager: Match gamepad ID → slot
  ├─ Generate control_id with slot prefix
  ├─ Embed analog_config in Axis events
  ↓
GamepadMapper::handle_event
  ├─ Button: Forward to router (on press only)
  ├─ Axis: Apply deadzone/gamma/inversion
  ↓
Router::handle_control
  ↓
OBS Driver (or other drivers)
```

### Slot Assignment
1. gilrs detects gamepad
2. SlotManager matches name against patterns
3. Assign to first matching empty slot
4. Generate events with slot prefix (gamepad1, gamepad2)
5. On disconnect: preserve slot for reconnection

## 💾 Backup/Rollback

### Legacy Config (Works)
```yaml
gamepad:
  enabled: true
  provider: hid
  analog:
    deadzone: 0.02
    gamma: 1.5
  hid:
    product_match: "Faceoff Pro Nintendo Switch Controller"

pages_global:
  controls:
    gamepad.axis.lx: { app: "obs", ... }  # Old format
```

To rollback: Use legacy config above with control IDs without numbers.

## 📊 Current Gamepad Status

| Controller | ID | Slot | Prefix | Pattern | Status |
|-----------|-----|------|--------|---------|--------|
| Faceoff Pro | GamepadId(1) | 0 | gamepad1 | "Faceoff" | ⚠️ Detected, events @ 0.0 |
| Xbox BT (Contrôleur) | GamepadId(0) | 1 | gamepad2 | "Contrôleur" | ⚠️ Detected, no events |

## 📝 Notes

- **Windows Bluetooth Timing**: Controllers need 2-3 seconds to enumerate after gilrs init
- **Battery Level**: Xbox controller at 10% - may affect Bluetooth reliability
- **Slot Preservation**: Disconnecting/reconnecting maintains slot assignment
- **Event Filtering**: Only events from assigned slots are processed
