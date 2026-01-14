# State Actor Architecture

## Overview

This document describes the major refactoring of the state management system from a `HashMap + RwLock` approach to an **Actor Model** architecture. This change was made to eliminate race conditions and provide ACID persistence.

**Branch:** `feature/actor-model-state`
**PR:** #9
**Issue for follow-up:** #8

---

## Problem Statement

### Original Issues

The previous state system had several problems despite 12 bug fixes (BUG-001 to BUG-009):

1. **Race Conditions (RACE-001):** Gap between state update and shadow update
2. **Lock Contention:** `RwLock<HashMap>` caused blocking during concurrent access
3. **Data Corruption:** Users reported lost state when switching pages
4. **No ACID Persistence:** JSON snapshots could be corrupted on crash

### User-Reported Symptoms

```
1) Page 1 with Voicemeeter - faders update correctly
2) Switch to page 2 - all faders go to 0 (expected, no QLC data)
3) Return to page 1 - nothing moves, Voicemeeter values lost!
```

---

## Architecture Decision

Three approaches were evaluated:

| Approach | Latency Impact | Effort | Risk |
|----------|---------------|--------|------|
| **DashMap + Fixes** | None | ~1 day | Low |
| **SQLite + DashMap** | None (async writes) | ~5 days | Low |
| **Actor Model + sled** | +10-20μs queries | ~2 weeks | Medium |

**Chosen:** Actor Model with sled persistence

**Rationale:**
- Eliminates ALL race conditions by design (single-owner state)
- ACID persistence via sled embedded database
- Clean separation of concerns
- Easier to reason about and debug

---

## New Architecture

### Component Diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                              Router                                  │
│  - state_actor: StateActorHandle                                    │
│  - persistence_actor: PersistenceActorHandle                        │
└────────────────────────────┬────────────────────────────────────────┘
                             │
              ┌──────────────┴──────────────┐
              │                             │
              ▼                             ▼
┌─────────────────────────┐   ┌─────────────────────────┐
│    StateActorHandle     │   │ PersistenceActorHandle  │
│  (Public API - Clone)   │   │   (Public API - Clone)  │
└────────────┬────────────┘   └────────────┬────────────┘
             │                             │
             │ mpsc::unbounded             │ mpsc::bounded(100)
             │                             │
             ▼                             ▼
┌─────────────────────────┐   ┌─────────────────────────┐
│      StateActor         │   │   PersistenceActor      │
│  - app_states           │   │   - db: sled::Db        │
│  - app_shadows          │   │   - pending_snapshot    │
│  - last_user_action_ts  │   │   - debounce: 500ms     │
│  - subscribers          │   │                         │
└─────────────────────────┘   └─────────────────────────┘
```

### Data Flow

```
App Feedback (MIDI)
    │
    ▼
Router::on_midi_from_app()     [async]
    │
    ├─► state_actor.should_suppress_anti_echo()  [async query]
    │       │
    │       └─► StateActor checks shadow state
    │
    ├─► state_actor.update_state()               [fire-and-forget]
    │       │
    │       └─► StateActor stores in app_states HashMap
    │
    └─► state_actor.update_shadow()              [fire-and-forget]
            │
            └─► StateActor updates app_shadows HashMap

Page Refresh
    │
    ▼
Router::refresh_page()         [async]
    │
    ├─► state_actor.clear_shadows()
    │
    └─► plan_page_refresh()
            │
            └─► state_actor.get_known_latest()   [async query]
                    │
                    └─► StateActor searches app_states
```

---

## New Files

| File | Purpose | Lines |
|------|---------|-------|
| `src/state/commands.rs` | `StateCommand` and `PersistenceCommand` enums | ~400 |
| `src/state/actor.rs` | `StateActor` implementation | ~700 |
| `src/state/actor_handle.rs` | `StateActorHandle` public API | ~300 |
| `src/state/persistence_actor.rs` | `PersistenceActor` with sled | ~500 |

### Key Types

```rust
// Commands sent to StateActor
pub enum StateCommand {
    // Hot path (fire-and-forget)
    UpdateState { app: AppKey, entry: MidiStateEntry },
    UpdateShadow { app: String, entry: MidiStateEntry },
    MarkUserAction { key: String, ts: u64 },

    // Queries (async with oneshot response)
    GetState { app, addr, response },
    GetKnownLatest { app, status, channel, data1, response },
    ListStates { app, response },

    // Anti-echo
    CheckSuppressAntiEcho { app, entry, response },
    CheckSuppressLWW { entry, response },

    // Lifecycle
    HydrateFromSnapshot { app, entries },
    ClearShadows,
    Shutdown,
}
```

---

## Critical Bug Fix: BUG-003 Rollback

### The Problem

The original BUG-003 fix was **too aggressive**. It prevented state storage for apps not on the active page:

```rust
// BEFORE (broken)
if apps_on_page.contains(&app_name) {
    router.on_midi_from_app(...);  // State ONLY stored if app on page
} else {
    trace!("App not on active page, skipping state update");
}
```

This caused state loss when switching pages because:
1. Voicemeeter on Page 1 → state stored
2. Switch to Page 2 → Voicemeeter keeps sending, but **state ignored**
3. Return to Page 1 → No state to restore!

### The Fix

```rust
// AFTER (correct)
// ALWAYS store state from all apps (needed for page refresh)
router.on_midi_from_app(&app_name, &feedback_data, &app_name).await;

// X-Touch forwarding still filters by active page (via process_feedback)
```

**Commit:** `bd43e3e`

---

## Modified Files

| File | Changes |
|------|---------|
| `src/router/mod.rs` | Replaced `StateStore` with `StateActorHandle`, removed shadow fields |
| `src/router/feedback.rs` | `on_midi_from_app()` now async, uses actor |
| `src/router/refresh.rs` | `plan_page_refresh()` now async, queries actor |
| `src/router/anti_echo.rs` | Simplified, logic moved to StateActor |
| `src/main.rs` | Uses persistence actor, fixed BUG-003 rollback |
| `src/state.rs` | Added module exports |
| `Cargo.toml` | Added `sled = "0.34"` |

---

## Dependencies Added

```toml
[dependencies]
sled = "0.34"  # Embedded key-value store for ACID persistence
```

---

## How to Debug

### Check if State is Being Stored

Add trace logging in `src/state/actor.rs`:

```rust
fn handle_update_state(&mut self, app: AppKey, entry: MidiStateEntry) {
    let key = addr_key(&entry.addr);
    tracing::info!(
        ?app, ?key,
        value = ?entry.value,
        "StateActor: storing state"
    );
    // ...
}
```

### Check if State is Being Retrieved

Add trace logging in `handle_get_known_latest`:

```rust
fn handle_get_known_latest(&self, app: AppKey, status: MidiStatus, ...) {
    let result = /* ... */;
    tracing::info!(
        ?app, ?status, ?channel, ?data1,
        found = result.is_some(),
        "StateActor: get_known_latest query"
    );
    result
}
```

### View sled Database

The sled database is stored in `.state/sled/`. To inspect:

```rust
// In a test or debug binary
let db = sled::open(".state/sled").unwrap();
for kv in db.iter() {
    let (key, value) = kv.unwrap();
    println!("{:?}: {:?}", key, value);
}
```

---

## Known Issues / Future Work

### Issue #8 Tasks

1. **Hardware Testing:** Validate with real X-Touch
2. **Performance Profiling:** Measure actual latency impact
3. **Code Cleanup:** Run `cargo fix` to remove warnings
4. **Remove Legacy Code:** Delete old `StateStore` if stable
5. **Metrics:** Add counters for debugging

### Potential Issues to Watch

1. **Actor Death:** If StateActor panics, state operations fail silently
   - Current behavior: logs error, returns None/false
   - Future: implement actor restart

2. **sled Performance:** First write after startup may be slow
   - Mitigated by 500ms debouncing

3. **Async Queries During Page Refresh:** Multiple await points
   - Could cause timing issues under heavy load
   - Monitor for page refresh taking >100ms

---

## Rollback Instructions

If the Actor Model causes issues, you can rollback:

1. **Revert the commits:**
   ```bash
   git revert bd43e3e  # BUG-003 fix rollback
   git revert aebb1dc  # Actor Model implementation
   ```

2. **Or checkout main:**
   ```bash
   git checkout main
   ```

3. **The old StateStore is still in the codebase** at `src/state/store.rs`

---

## Testing Checklist

Before merging:

- [ ] Page 1 → Page 2 → Page 1: Faders restore correctly
- [ ] QLC+ sync on Page 2: All mapped faders update
- [ ] Voicemeeter continuous feedback: No lost values
- [ ] Rapid page switching: No crashes or hangs
- [ ] App restart: State restored from sled
- [ ] 103 unit tests pass

---

## Contact

For questions about this implementation:
- Check `CLAUDE.md` for AI assistant instructions
- Check `MEMORY.md` for lessons learned
- Check GitHub Issues for related discussions
