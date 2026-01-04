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
- **Fichier**: `src/main.rs`, `src/router/refresh.rs`, `src/router/mod.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent router-expert
- **Notes**:
  - Ajouter synchronisation entre page change et feedback
- **Fix appliqué**:
  - Implémentation d'un Page Epoch Counter (`AtomicU64`) lock-free
  - L'epoch est incrémenté au début de chaque `refresh_page()` AVANT de vider le shadow
  - Le feedback capture l'epoch à la réception et le vérifie à chaque étape critique
  - Si l'epoch a changé pendant le traitement, le feedback est ignoré (page change en cours)
  - 3 tests unitaires ajoutés pour valider le comportement

### BUG-007: Context Config Stale
- **Fichier**: `src/router/feedback.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent router-expert
- **Notes**:
  - Capturer config snapshot au début de process_feedback
- **Fix appliqué**:
  - Capture atomique de config + active_page_index dans un seul bloc au début de process_feedback()
  - Les guards sont libérés immédiatement après la capture
  - Toutes les références utilisent maintenant config_snapshot (owned value)
  - Garantit la cohérence même si hot-reload survient pendant le traitement

### BUG-008: Snapshot Prioritaire au Démarrage
- **Fichier**: `src/main.rs`, `src/config/mod.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent rust-engineer
- **Notes**:
  - Attendre connexion drivers avant refresh initial
- **Fix appliqué**:
  - Supprimé le refresh_page() prématuré (avant l'enregistrement des drivers)
  - Ajouté un délai configurable `startup_refresh_delay_ms` (défaut: 500ms)
  - Le refresh est maintenant différé jusqu'APRÈS l'enregistrement de tous les drivers
  - Permet aux drivers de se connecter et d'envoyer du feedback frais avant le refresh
  - Combiné avec BUG-005 (stale priority), les valeurs fraîches prennent le dessus

### BUG-009: Epoch Non Vérifié dans Refresh
- **Fichier**: `src/router/refresh.rs`, `src/xtouch/fader_setpoint.rs`
- **Status**: [x] DONE
- **Assigné à**: Agent router-expert
- **Notes**:
  - Intégrer vérification epoch dans plan_page_refresh
- **Fix appliqué**:
  - Ajouté `page_epoch` dans `ChannelState` pour tracker quelle page a créé chaque setpoint
  - Ajouté `set_page_epoch()` pour synchroniser avec le page epoch du Router
  - `get_desired()` vérifie maintenant que le setpoint appartient à l'epoch courant
  - Les setpoints obsolètes (d'une page précédente) retournent `None`
  - L'epoch est mis à jour dans `refresh_page()` juste après l'incrément
  - 2 tests unitaires ajoutés pour valider le comportement

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
| 2026-01-04 | BUG-006 | Fix: page epoch counter for race condition protection | router-expert | pending |
| 2026-01-04 | BUG-007 | Fix: config snapshot capture in process_feedback | router-expert | pending |
| 2026-01-04 | BUG-008 | Fix: deferred startup refresh with configurable delay | rust-engineer | pending |
| 2026-01-04 | BUG-009 | Fix: page epoch integration in FaderSetpoint | router-expert | pending |

---

## Notes de Session

### Session 2026-01-04
- Audit initial complété
- 12 bugs identifiés (4 critiques, 5 hauts, 3 moyens)
- Agents lancés en parallèle pour corrections P0

---

## Pour Reprendre (utilisé par /continue-fix)

**Dernier bug traité**: BUG-009
**Prochain bug à traiter**: QLC-001 (ou RACE-001)
**Contexte important**:
- All P0 bugs (BUG-001 to BUG-004) are DONE ✅
- All P1 bugs (BUG-005 to BUG-009) are DONE ✅
- Next: P2 bugs (QLC-001 to QLC-003) or P3 (RACE-001)
- Note: QLC-004 was already fixed earlier
