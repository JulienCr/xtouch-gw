# TODO: Gamepad Code Refactoring

## Priority: HIGH
**Goal:** Ensure diagnostic visualizer represents ground truth of production code behavior

---

## Problem Statement

Currently, gamepad normalization logic is **duplicated** between:
- **Production code:** `src/input/gamepad/xinput_convert.rs`
- **Diagnostic tool:** `src/input/gamepad/visualizer.rs`

**Risk:**
- If implementations diverge, diagnostics will show **incorrect** behavior
- Changes to normalization must be manually synced to both files
- No guarantee that visualizer shows true production behavior

---

## Required Refactoring

### 1. **Mutualize Normalization Logic**

#### Current State (Duplicated)
```rust
// In xinput_convert.rs (lines 184-215)
fn normalize_stick_radial(raw_x: i16, raw_y: i16, deadzone: f32) -> (f32, f32) {
    // ... implementation ...
}

// In visualizer.rs (lines 452-483)
fn normalize_stick_radial(raw_x: i16, raw_y: i16, deadzone: f32) -> (f32, f32) {
    // ... DUPLICATE implementation ...
}
```

#### Target State (Shared)
```rust
// New file: src/input/gamepad/normalize.rs
pub fn normalize_stick_radial(raw_x: i16, raw_y: i16, deadzone: f32) -> (f32, f32) {
    // SINGLE source of truth
}

pub fn normalize_trigger(value: u8, threshold: u8) -> f32 {
    // SINGLE source of truth
}
```

**Benefits:**
- ✅ Single source of truth
- ✅ Visualizer **guaranteed** to show production behavior
- ✅ Changes applied once, reflected everywhere
- ✅ Easier to test and maintain

---

### 2. **Additional Mutualizable Code**

#### Constants
Move to shared module:
```rust
// src/input/gamepad/constants.rs or normalize.rs
pub const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;
pub const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;
pub const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;
pub const MAX_STICK_MAGNITUDE: f32 = 32768.0;
```

**Currently duplicated in:**
- `visualizer.rs` lines 410-412
- Hardcoded in `xinput_convert.rs` line 122

#### Button Bit Flags
```rust
// src/input/gamepad/button_flags.rs or constants.rs
pub mod button_flags {
    pub const DPAD_UP: u16 = 0x0001;
    pub const DPAD_DOWN: u16 = 0x0002;
    // ... etc
}
```

**Currently duplicated in:**
- `xinput_convert.rs` lines 14-29
- `visualizer_state.rs` lines 133-146

---

## Implementation Plan

### Phase 1: Extract Normalization Logic (30 min)
1. Create `src/input/gamepad/normalize.rs`
2. Move `normalize_stick_radial()` to new module
3. Move `normalize_trigger()` to new module
4. Add comprehensive documentation and examples
5. Update `xinput_convert.rs` to use shared function
6. Update `visualizer.rs` to use shared function
7. **Verify:** Run visualizer and main app, ensure identical behavior

### Phase 2: Extract Constants (15 min)
1. Move deadzone constants to `normalize.rs`
2. Update all references
3. Add const assertions to verify values at compile time

### Phase 3: Extract Button Flags (15 min)
1. Create `src/input/gamepad/button_flags.rs` OR add to existing module
2. Move button bit flags to shared module
3. Update `xinput_convert.rs` and `visualizer_state.rs`

### Phase 4: Validation (20 min)
1. **Build:** `cargo build --release`
2. **Test:** Run visualizer with gamepad, verify values match expectations
3. **Test:** Run main app, verify gamepad input works correctly
4. **Compare:** Ensure visualizer shows exact same normalized values as main app uses
5. **Document:** Update CLAUDE.md with new module structure

---

## Testing Strategy

### Before Refactoring
```bash
# Capture baseline behavior
cargo run --release -- --gamepad-diagnostics
# Record: normalized values at full left, right, up, down, diagonal
```

### After Refactoring
```bash
# Verify identical behavior
cargo run --release -- --gamepad-diagnostics
# Compare: values must match baseline exactly
```

### Unit Tests to Add
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_full_left() {
        let (nx, ny) = normalize_stick_radial(-32768, 0, 7849.0);
        assert!((nx + 1.0).abs() < 0.001);  // -1.0
        assert!(ny.abs() < 0.001);           // 0.0

        let mag = (nx * nx + ny * ny).sqrt();
        assert!((mag - 1.0).abs() < 0.001);  // magnitude = 1.0
    }

    #[test]
    fn test_normalize_diagonal() {
        let (nx, ny) = normalize_stick_radial(23170, 23170, 7849.0);
        let mag = (nx * nx + ny * ny).sqrt();
        assert!((mag - 1.0).abs() < 0.001);  // magnitude = 1.0
    }

    #[test]
    fn test_circular_deadzone() {
        // Inside deadzone
        let (nx, ny) = normalize_stick_radial(5000, 5000, 7849.0);
        assert_eq!(nx, 0.0);
        assert_eq!(ny, 0.0);
    }

    #[test]
    fn test_i16_min_handling() {
        // i16::MIN should not panic
        let (nx, ny) = normalize_stick_radial(-32768, -32768, 7849.0);
        let mag = (nx * nx + ny * ny).sqrt();
        assert!((mag - 1.0).abs() < 0.001);  // clamped to 1.0
    }
}
```

---

## File Structure (After Refactoring)

```
src/input/gamepad/
├── mod.rs                    # Module declarations
├── normalize.rs              # ⭐ NEW: Shared normalization logic
├── xinput_convert.rs         # Uses normalize::normalize_stick_radial()
├── visualizer.rs             # Uses normalize::normalize_stick_radial()
├── visualizer_state.rs       # State tracking
├── hybrid_provider.rs        # Event provider
├── mapper.rs                 # Router integration
├── analog.rs                 # Gamma/inversion processing
├── diagnostics.rs            # Legacy text diagnostics
├── slot.rs                   # Multi-gamepad slots
└── hybrid_id.rs              # Controller identification
```

---

## Success Criteria

- ✅ `normalize_stick_radial()` exists in **exactly one place**
- ✅ Both `xinput_convert.rs` and `visualizer.rs` import from shared module
- ✅ Constants (deadzones, thresholds) defined once
- ✅ Visualizer shows **identical** normalized values to production code
- ✅ All tests pass
- ✅ No behavioral changes (refactor only, zero functional impact)

---

## Risks & Mitigation

### Risk 1: Breaking Production Code
**Mitigation:**
- Extract to new module first, don't modify existing
- Update one file at a time
- Test after each change
- Use git to track and verify changes

### Risk 2: Visualizer Diverges During Development
**Mitigation:**
- This refactoring **prevents** this risk by ensuring single source of truth
- Add CI check to verify no duplication

### Risk 3: Performance Impact
**Mitigation:**
- Normalization is already called per-frame, inlining should be preserved
- Mark functions with `#[inline]` if needed
- Benchmark before/after if concerned

---

## Related Issues

- Original bug: i16::MIN normalization (fixed in commit 92d9cdc)
- Diagonal magnitude bug: MAX_MAGNITUDE using diagonal instead of axis max
- Need for ground truth: Visualizer must show exact production behavior

---

## Notes

**Why This Matters:**
The visualizer is a **debugging tool**. If it doesn't accurately represent what the production code does, it's worse than useless—it's **misleading**. By mutualizing the logic, we guarantee the diagnostic tool shows ground truth.

**Future Proofing:**
Any future changes to normalization (e.g., adjustable deadzones, different scaling curves, anti-snap-back filters) only need to be implemented once and will automatically apply to both production and diagnostics.

---

**Created:** 2025-01-XX
**Priority:** HIGH
**Estimated Effort:** 1-2 hours
**Status:** 🔴 Not Started
