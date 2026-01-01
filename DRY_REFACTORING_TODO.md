# DRY Refactoring - Remaining Work

This document tracks DRY (Don't Repeat Yourself) refactoring tasks that were identified but not completed in PR #5.

## OBS Driver Refactoring

### 1. Extract `get_client()` Helper (7 locations)
**File:** `src/drivers/obs/actions.rs`

The following pattern is repeated 7+ times:
```rust
let guard = self.client.read().await;
let client = guard.as_ref()
    .context("OBS not connected")?
    .clone();
```

**Recommendation:** Create helper method:
```rust
async fn get_client(&self) -> Result<&obws::Client> {
    self.client.read().await
        .as_ref()
        .context("OBS not connected")
}
```

**Locations:**
- Lines 80-83
- Lines 101-103
- Lines 125-128
- Lines 372-375
- Lines 426-429
- Lines 490-493
- Lines 554-557
- Lines 599-602

---

### 2. Extract Analog Value Shaping Helper (3 locations)
**File:** `src/drivers/obs/actions.rs`

Pattern repeated in `nudgeX`, `nudgeY`, `scaleUniform`:
```rust
let clamped = v.clamp(-1.0, 1.0);
let gamma = *self.analog_gamma.read();
let shaped = shape_analog(clamped, gamma);
```

**Recommendation:** Create helper:
```rust
fn shape_gamepad_input(&self, value: f64) -> f64 {
    let clamped = value.clamp(-1.0, 1.0);
    let gamma = *self.analog_gamma.read();
    shape_analog(clamped, gamma)
}
```

**Locations:**
- Line 152 (nudgeX)
- Line 221 (nudgeY)
- Line 290 (scaleUniform)

---

### 3. Extract Encoder Acceleration Helper (3 locations)
**File:** `src/drivers/obs/actions.rs`

Pattern repeated:
```rust
let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
let accel = self.encoder_tracker.lock().track_event(control_id, delta);
let final_delta = delta * accel;
debug!("OBS nudgeX encoder: control={} delta={} accel={} final={}",
    control_id, delta, accel, final_delta);
```

**Recommendation:** Create helper:
```rust
fn apply_encoder_acceleration(&self, ctx: &ExecutionContext, delta: f64, action_name: &str) -> f64 {
    let control_id = ctx.control_id.as_deref().unwrap_or("encoder");
    let accel = self.encoder_tracker.lock().track_event(control_id, delta);
    let final_delta = delta * accel;
    debug!("OBS {} encoder: control={} delta={} accel={} final={}",
        action_name, control_id, delta, accel, final_delta);
    final_delta
}
```

**Locations:**
- Lines 188-198 (nudgeX)
- Lines 257-267 (nudgeY)
- Lines 326-336 (scaleUniform)

---

### 4. Extract Scene Change Helper (4 locations)
**File:** `src/drivers/obs/actions.rs`

Pattern repeated:
```rust
let studio_mode = *self.studio_mode.read();
if studio_mode {
    client.scenes().set_current_preview_scene(scene_name).await?;
} else {
    client.scenes().set_current_program_scene(scene_name).await?;
}
```

**Recommendation:** Create helper:
```rust
async fn set_active_scene(&self, client: &obws::Client, scene_name: &str) -> Result<()> {
    let studio_mode = *self.studio_mode.read();
    if studio_mode {
        client.scenes().set_current_preview_scene(scene_name).await?;
    } else {
        client.scenes().set_current_program_scene(scene_name).await?;
    }
    Ok(())
}
```

**Locations:**
- Lines 88-94 (changeScene)
- Lines 497-501 (selectCamera)
- Lines 562-565 (selectSplitView)
- Lines 607-610 (selectCameraByIndex)

---

## Cross-Driver Refactoring

### 5. Extract Connection Status Emission Trait
**Files:** `src/drivers/midibridge.rs`, `src/drivers/obs/connection.rs`

Both files have identical `emit_status()` implementations:
```rust
fn emit_status(&self, status: crate::tray::ConnectionStatus) {
    *self.current_status.write() = status.clone();
    for callback in self.status_callbacks.read().iter() {
        callback(status.clone());
    }
}
```

**Recommendation:** Create a trait or shared utility:
```rust
pub trait StatusEmitter {
    fn current_status(&self) -> &RwLock<ConnectionStatus>;
    fn status_callbacks(&self) -> &RwLock<Vec<StatusCallback>>;

    fn emit_status(&self, status: ConnectionStatus) {
        *self.current_status().write() = status.clone();
        for callback in self.status_callbacks().read().iter() {
            callback(status.clone());
        }
    }
}
```

---

### 6. Extract MIDI Message Construction Helper
**Files:** `src/router/feedback.rs`, `src/router/xtouch_input.rs`

Both files have ~50 lines of identical logic to construct MIDI messages from `MidiSpec`:
- Build CC from spec
- Build Note from spec
- Build PitchBend from spec

**Recommendation:** Create helper in router module:
```rust
pub fn build_midi_from_spec(
    spec: &MidiSpec,
    normalized_value: f64
) -> Option<MidiMessage> {
    // Unified construction logic
}
```

**Locations:**
- `feedback.rs` lines 157-204
- `xtouch_input.rs` lines 206-259

---

## Priority

| Task | Impact | Effort | Priority |
|------|--------|--------|----------|
| get_client() helper | High (7 locations) | Low | P1 |
| Scene change helper | Medium (4 locations) | Low | P2 |
| Encoder acceleration | Medium (3 locations) | Low | P2 |
| Analog shaping | Medium (3 locations) | Low | P2 |
| Status emission trait | Medium (2 files) | Medium | P3 |
| MIDI message construction | Medium (2 files) | Medium | P3 |

---

## Notes

- These refactorings were deferred from PR #5 as they are more invasive and OBS-specific
- The existing code works correctly; these are code quality improvements only
- Consider doing these incrementally in separate PRs to minimize risk
