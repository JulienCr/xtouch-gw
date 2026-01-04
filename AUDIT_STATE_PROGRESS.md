# Suivi des Corrections - Audit State Bugs

**Dernière mise à jour**: 2026-01-04
**Status global**: EN COURS

---

## Légende des Status

- [ ] `PENDING` - Non commencé
- [~] `IN_PROGRESS` - En cours de correction
- [x] `DONE` - Corrigé et testé
- [!] `BLOCKED` - Bloqué (dépendance ou question)

---

## Corrections P0 (CRITIQUES)

### BUG-001: Anti-Echo Non Vérifié Avant State Update
- **Fichier**: `src/router/feedback.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Modifier `on_midi_from_app()` pour vérifier anti-echo AVANT state update
  - Ajouter early return si suppression détectée
- **Fix appliqué**: Ajout de `should_suppress_anti_echo()` check AVANT `update_from_feedback()` avec early return si echo détecté

### BUG-002: Squelch Activé Après State Update
- **Fichier**: `src/main.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Réordonner: squelch -> state_update -> send
  - Extraire channel/value AVANT les autres opérations
- **Fix appliqué**:
  - Added `extract_pitchbend_from_feedback()` helper function to detect PitchBend early
  - Reordered feedback handling: (1) extract PB info, (2) activate squelch BEFORE state update, (3) update state, (4) forward to X-Touch
  - Race condition eliminated: squelch is now active before any state change, so user fader movements during state update are properly suppressed

### BUG-003: État Toujours Mis à Jour, Feedback Conditionnel
- **Fichier**: `src/main.rs`, `src/router/page.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent router-expert
- **Notes**:
  - Option A: Filtrer state update par page active (CHOSEN)
  - Option B: Marquer entrées hors-page comme "background"
- **Fix appliqué**:
  - Added `get_apps_for_active_page()` async method to `Router` in `src/router/page.rs`
  - Modified feedback handling in `src/main.rs` to check if app is on active page BEFORE calling `on_midi_from_app()`
  - Off-page app feedback is now fully ignored (not stored in StateStore, not forwarded to X-Touch)
  - This ensures StateStore and X-Touch forwarding are symmetric

### BUG-004: Lock Contention Désactive Anti-Echo
- **Fichier**: `src/router/anti_echo.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Remplacer `try_read()` par `read()`
  - Gérer le cas de lock poisoned
- **Fix appliqué**: Remplacé `try_read()` par `read().unwrap_or_else()` avec logging en cas de lock poisoned

---

## Corrections P1 (HAUTES)

### BUG-005: Flag Stale Ignoré
- **Fichier**: `src/state/store.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent router-expert
- **Notes**:
  - Modifier `get_known_latest_for_app()` pour prioriser non-stale
- **Fix appliqué**:
  - Modified `get_known_latest_for_app()` in `src/state/store.rs` to prioritize non-stale entries over stale
  - Priority order: (1) non-stale with most recent timestamp, (2) stale with most recent timestamp
  - Fresh app feedback now correctly supersedes snapshot-restored values
  - Added 2 unit tests: `test_stale_flag_priority` and `test_stale_flag_same_status_uses_timestamp`

### BUG-006: Race Page Change vs State Update
- **Fichier**: `src/main.rs`, `src/router/refresh.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent router-expert
- **Notes**:
  - Ajouter synchronisation entre page change et feedback

### BUG-007: Context Config Stale
- **Fichier**: `src/router/feedback.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent router-expert
- **Notes**:
  - Capturer config snapshot au début de process_feedback

### BUG-008: Snapshot Prioritaire au Démarrage
- **Fichier**: `src/main.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Attendre connexion drivers avant refresh initial

### BUG-009: Epoch Non Vérifié dans Refresh
- **Fichier**: `src/router/refresh.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent router-expert
- **Notes**:
  - Intégrer vérification epoch dans plan_page_refresh

---

## Corrections P2 (QLC Spécifiques)

### QLC-001: Driver Stub Inutile
- **Fichier**: `src/drivers/qlc.rs`, `src/main.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Décision: Supprimer ou implémenter correctement

### QLC-002: Pas d'Indicateurs LED
- **Fichier**: `src/drivers/qlc.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Implémenter subscribe_indicators()

### QLC-003: Status Connexion Faux
- **Fichier**: `src/drivers/qlc.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Déléguer au MIDI bridge correspondant

### QLC-004: Perte Feedback si App Non Configurée
- **Fichier**: `src/config/mod.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent config-expert
- **Notes**:
  - Added validation warnings for MIDI app port configuration
  - Warns if app has no ports configured (bidirectional will not work)
  - Warns if app has output but no input port (feedback will not be received)
  - Warns if app has input but no output port (commands will not be sent)
  - Uses tracing::warn! so existing valid configs still load

---

## Corrections P3 (Race Conditions)

### RACE-001: Shadow/State Non Atomiques
- **Fichier**: `src/router/feedback.rs`
- **Status**: [ ] PENDING
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Unifier les primitives de synchronisation

---

## Journal des Modifications

| Date | Bug ID | Action | Agent | Commit |
|------|--------|--------|-------|--------|
| 2026-01-04 | - | Création fichier de suivi | Claude | - |
| 2026-01-04 | QLC-004 | Added MIDI app port validation warnings | config-expert | - |
| 2026-01-04 | BUG-001 | Fix: anti-echo check avant state update | rust-engineer | pending |
| 2026-01-04 | BUG-004 | Fix: blocking read() pour anti-echo | rust-engineer | pending |
| 2026-01-04 | BUG-002 | Fix: squelch activated before state update | rust-engineer | pending |
| 2026-01-04 | BUG-003 | Fix: page-aware state filtering | router-expert | pending |
| 2026-01-04 | BUG-005 | Fix: stale flag priority in get_known_latest_for_app | router-expert | pending |

---

## Notes de Session

### Session 2026-01-04
- Audit initial complété
- 12 bugs identifiés (4 critiques, 5 hauts, 3 moyens)
- Agents lancés en parallèle pour corrections P0

---

## Pour Reprendre (utilisé par /continue-fix)

**Dernier bug traité**: BUG-005
**Prochain bug à traiter**: BUG-006
**Contexte important**:
- BUG-003 and BUG-005 fixed together as they both relate to state filtering
- All P0 bugs (BUG-001 to BUG-004) are now DONE
- BUG-005 (P1) is also DONE - stale flag now properly checked in state queries
