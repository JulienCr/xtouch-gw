# Audit de Bug - Gestion des States XTouch-GW v3

**Date**: 2026-01-04
**Focus**: Logique de gestion des states, notamment QLC
**Sévérité globale**: CRITIQUE - Plusieurs bugs de logique affectent la synchronisation des états

---

## Sommaire Exécutif

L'audit révèle **12 problèmes majeurs** dans la gestion des états, répartis en 3 catégories:
- **4 bugs CRITIQUES** causant des pertes de synchronisation
- **5 bugs HAUTS** créant des race conditions
- **3 bugs MOYENS** affectant la fiabilité

Les problèmes les plus graves concernent:
1. L'ordre des opérations entre mise à jour d'état et anti-echo
2. La synchronisation entre changement de page et mise à jour d'état
3. Le traitement des apps hors-page qui pollue le StateStore

---

## Table des Matières

1. [Bugs Critiques](#1-bugs-critiques)
2. [Bugs de Sévérité Haute](#2-bugs-de-sévérité-haute)
3. [Problèmes Spécifiques à QLC](#3-problèmes-spécifiques-à-qlc)
4. [Bugs de Race Condition](#4-bugs-de-race-condition)
5. [Récapitulatif et Priorités](#5-récapitulatif-et-priorités)
6. [Fixes Proposés](#6-fixes-proposés)

---

## 1. Bugs Critiques

### BUG-001: Anti-Echo Non Vérifié Avant Mise à Jour d'État

**Fichier**: `src/router/feedback.rs` (lignes 231-268)
**Sévérité**: CRITIQUE
**Impact**: Oscillation des faders motorisés

**Problème**:
```rust
// Ligne 251: Met à jour StateStore INCONDITIONNELLEMENT
self.state.update_from_feedback(app, entry.clone());

// Ligne 264: PUIS marque l'anti-echo
self.update_app_shadow(app_key, &entry);
```

**Flux Actuel** (incorrect):
1. État mis à jour avec valeurs potentiellement "echo"
2. Shadow anti-echo mis à jour APRÈS
3. Aucune vérification de suppression d'echo

**Flux Attendu**:
1. Vérifier si la valeur doit être supprimée via anti-echo
2. Si supprimée, ignorer la mise à jour d'état
3. Sinon mettre à jour l'état
4. Puis mettre à jour le shadow

**Conséquence**: Les valeurs "echo" sont stockées dans StateStore et seront rejouées lors des changements de page.

---

### BUG-002: Squelch Activé Après Mise à Jour d'État

**Fichier**: `src/main.rs` (lignes 570-610)
**Sévérité**: CRITIQUE
**Impact**: Perte de synchronisation état/hardware

**Ordre des opérations problématique**:
```rust
// T0: Activité enregistrée
activity_tracker.record(...)

// T1: État + shadow mis à jour
router.on_midi_from_app(...)

// T2: Filtre de page (async!)
if let Some(transformed) = router.process_feedback(...).await {

// T3: Squelch activé
xtouch.activate_squelch(120);

// T4: Envoi au X-Touch
xtouch.set_fader(channel, value14).await;
```

**Race Condition**:
- Entre T1 et T3, l'utilisateur peut bouger le fader
- Le callback X-Touch vérifie le squelch (pas encore activé)
- L'input utilisateur passe → Echo non supprimé

**Fix**: Activer le squelch AVANT `on_midi_from_app()`.

---

### BUG-003: État Toujours Mis à Jour, Feedback Conditionnel

**Fichier**: `src/main.rs` (lignes 573-578)
**Sévérité**: CRITIQUE
**Impact**: Corruption d'état sur configurations multi-pages

```rust
// Ligne 573: TOUJOURS mise à jour de l'état
router.on_midi_from_app(&app_name, &feedback_data, &app_name);

// Ligne 578: Transfert CONDITIONNEL (filtré par page)
if let Some(transformed) = router.process_feedback(...).await {
```

**Scénario de corruption**:
1. Page 1: OBS mappé, Page 2: QLC mappé (pas OBS)
2. Sur Page 1: OBS envoie fader=8000 → État mis à jour + transféré
3. Bascule vers Page 2
4. OBS continue d'envoyer fader=4000 → État mis à jour, PAS transféré
5. Bascule vers Page 1 → plan_page_refresh() lit état "stale" d'OBS
6. Résultat: État incohérent avec ce que l'utilisateur voit

---

### BUG-004: Lock Contention Désactive Silencieusement l'Anti-Echo

**Fichier**: `src/router/anti_echo.rs` (lignes 71-75)
**Sévérité**: CRITIQUE
**Impact**: Echoes passent à travers le système

```rust
pub(crate) fn should_suppress_anti_echo(...) {
    let app_shadows = match self.app_shadows.try_read() {
        Ok(shadows) => shadows,
        Err(_) => return false,  // SILENTLY RETURNS FALSE!
    };
}
```

**Problème**: Si le lock est contesté (écriture en cours ailleurs), la vérification retourne `false` sans logging, permettant les echoes.

**Fix**: Utiliser `read()` bloquant au lieu de `try_read()`.

---

## 2. Bugs de Sévérité Haute

### BUG-005: Flag "Stale" Non Utilisé pour le Page Refresh

**Fichier**: `src/state/store.rs` (lignes 155-169)
**Fichier**: `src/router/refresh.rs` (lignes 373-390)
**Sévérité**: HAUTE

Les entrées restaurées depuis snapshot sont marquées `stale: true`, mais ce flag est ignoré:

```rust
// store.rs ligne 162: Entrées marquées stale
stale: true,

// refresh.rs ligne 69: Filtre seulement sur 'known'
.filter(|entry| entry.known)
// Ne vérifie JAMAIS 'stale'!
```

**Impact**: Les valeurs de snapshot (possiblement obsolètes) ont la même priorité que les valeurs fraîches.

---

### BUG-006: Race Condition Page Change vs State Update

**Fichier**: `src/main.rs` (lignes 566-610)
**Fichier**: `src/router/refresh.rs` (ligne 19)
**Sévérité**: HAUTE

**Scénario**:
```
Thread A: feedback_rx.recv() → on_midi_from_app() → update_from_feedback()
Thread B: Page change → clear_xtouch_shadow() → plan_page_refresh()
```

Timeline problématique:
1. Page refresh démarre, efface shadow
2. Feedback arrive pour ancienne page
3. Shadow mis à jour avec valeur de l'ancienne page
4. Refresh de nouvelle page contaminé

---

### BUG-007: Context Config/Page Stale dans process_feedback()

**Fichier**: `src/router/feedback.rs` (lignes 31-52)
**Sévérité**: HAUTE

```rust
// Ligne 33-34: Lecture de active_page_idx et active_page
// Ligne 36: Récupération de page depuis config
// Ligne 44: get_apps_for_page(active_page, &config)  // Config du PASSÉ!
```

Si un hot-reload de config survient entre ligne 36 et 44, `get_apps_for_page()` utilise une référence obsolète.

---

### BUG-008: Priorité Snapshot vs Feedback Frais au Démarrage

**Fichier**: `src/main.rs` (lignes 161-169, 300)
**Sévérité**: HAUTE

```rust
// Ligne 164: Chargement snapshot (entries marquées stale)
router.get_state_store().load_snapshot(&snapshot_path).await

// Ligne 300: Puis refresh_page()
router.refresh_page().await;
```

**Problème**: Le snapshot charge des valeurs `stale: true` AVANT que les drivers ne soient connectés. Ces valeurs sont envoyées au X-Touch, puis les apps se connectent et envoient des valeurs fraîches qui peuvent avoir des timestamps plus anciens.

---

### BUG-009: Epoch Non Vérifié dans Refresh Page

**Fichier**: `src/router/refresh.rs` (lignes 447-465)
**Sévérité**: HAUTE

```rust
// Ligne 448: Lecture sans vérification d'epoch
if let Some(desired14) = self.fader_setpoint.get_desired(ch) {
```

Le système d'epoch existe (`fader_setpoint.rs`) mais `plan_page_refresh()` l'ignore complètement, permettant des valeurs obsolètes lors de changements de page rapides.

---

## 3. Problèmes Spécifiques à QLC

### QLC-001: Driver Stub Jamais Invoqué

**Fichier**: `src/drivers/qlc.rs`
**Fichier**: `src/main.rs` (lignes 450-464)
**Sévérité**: MOYENNE (fonctionnel via MIDI bridge)

Le `QlcDriver` est un stub no-op enregistré dans le router mais jamais réellement utilisé. Le routage MIDI direct passe par `MidiBridgeDriver`, pas par `QlcDriver`.

```rust
// qlc.rs - execute() fait juste du logging
async fn execute(&self, action: &str, params: Vec<Value>, _ctx: ExecutionContext) -> Result<()> {
    // Essentially no-op
    Ok(())
}
```

**Impact**: Si du code futur attend que `QlcDriver.execute()` fasse quelque chose, il échouera silencieusement.

---

### QLC-002: Pas d'Émission d'Indicateurs LED

**Fichier**: `src/drivers/qlc.rs` (lignes 66-70)
**Sévérité**: MOYENNE

Le driver QLC n'implémente pas `subscribe_indicators()`. Contrairement à OBS (`src/drivers/obs/actions.rs:643`), QLC ne peut pas contrôler les LEDs du X-Touch.

**Impact**: Le feedback QLC ne peut pas allumer/éteindre les LEDs des boutons.

---

### QLC-003: Status de Connexion Toujours "Connected"

**Fichier**: `src/drivers/qlc.rs`
**Sévérité**: MOYENNE

```rust
fn connection_status(&self) -> crate::tray::ConnectionStatus {
    crate::tray::ConnectionStatus::Connected  // Toujours Connected!
}
```

Le stub retourne toujours "Connected" même si:
- Le MIDI bridge QLC est déconnecté
- L'application QLC n'est pas lancée
- Les ports MIDI sont indisponibles

**Impact**: Le system tray affiche QLC+ comme connecté même quand il est offline.

---

### QLC-004: Perte de Feedback si App MIDI Non Configurée

**Fichier**: `src/main.rs` (lignes 332-360)
**Sévérité**: HAUTE

Si `config.midi.apps` ne contient pas "qlc", aucun `MidiBridgeDriver` n'est créé. Mais le stub `QlcDriver` EST enregistré. Les commandes MIDI sont silencieusement ignorées.

**Impact**: Config incorrecte = QLC silencieusement cassé sans erreur.

---

## 4. Bugs de Race Condition

### RACE-001: Shadow State et StateStore Non Atomiques

**Fichier**: `src/router/feedback.rs` (lignes 250-264)
**Sévérité**: MOYENNE

```rust
// Ligne 251: Lock tokio async RwLock
self.state.update_from_feedback(app, entry.clone());

// Ligne 264: Lock std sync RwLock (DIFFÉRENT!)
self.update_app_shadow(app_key, &entry);
```

Ces deux opérations utilisent des primitives de synchronisation différentes. Entre les deux:
- Un subscriber peut observer l'état mis à jour
- Mais l'anti-echo n'est pas encore marqué
- Le subscriber peut traiter un duplicate

---

### RACE-002: Gap Timing Squelch ↔ Envoi Async

**Fichier**: `src/main.rs` (lignes 593-595)
**Sévérité**: MOYENNE

```rust
xtouch.activate_squelch(120);  // T0: Active le squelch
xtouch.set_fader(channel, value14).await;  // T1-T?: Async, pas immédiat!
```

Le `set_fader()` est async. Si l'utilisateur bouge le fader entre T0 et l'exécution réelle, la fenêtre de squelch peut expirer.

---

### RACE-003: Grace Period LWW = 0 pour Notes

**Fichier**: `src/router/anti_echo.rs` (lignes 138-142)
**Sévérité**: BASSE-MOYENNE

```rust
// LWW grace periods
(MidiStatus::PB, 300),
(MidiStatus::CC, 50),
_ => 0,  // Notes ont 0ms de grâce!
```

**Impact**: Les boutons LED peuvent "flicker" car le feedback app arrive immédiatement après l'action utilisateur sans protection.

---

## 5. Récapitulatif et Priorités

| ID | Bug | Sévérité | Composant | Priorité Fix |
|---|---|---|---|---|
| BUG-001 | Anti-echo non vérifié avant state update | CRITIQUE | feedback.rs | P0 |
| BUG-002 | Squelch après state update | CRITIQUE | main.rs | P0 |
| BUG-003 | État toujours mis à jour, feedback conditionnel | CRITIQUE | main.rs | P0 |
| BUG-004 | try_read() silencieux sur anti-echo | CRITIQUE | anti_echo.rs | P1 |
| BUG-005 | Flag stale ignoré | HAUTE | store.rs/refresh.rs | P1 |
| BUG-006 | Race page change vs state update | HAUTE | main.rs | P1 |
| BUG-007 | Context config stale | HAUTE | feedback.rs | P2 |
| BUG-008 | Snapshot prioritaire au démarrage | HAUTE | main.rs | P2 |
| BUG-009 | Epoch non vérifié dans refresh | HAUTE | refresh.rs | P2 |
| QLC-004 | Perte feedback si app non configurée | HAUTE | main.rs | P2 |
| RACE-001 | Shadow/State non atomiques | MOYENNE | feedback.rs | P3 |
| QLC-001 | Stub driver inutile | MOYENNE | qlc.rs | P3 |
| QLC-002 | Pas d'indicateurs LED | MOYENNE | qlc.rs | P4 |
| QLC-003 | Status connexion faux | MOYENNE | qlc.rs | P4 |

---

## 6. Fixes Proposés

### Fix P0-A: Réorganiser l'Ordre d'Opérations Feedback

**Fichier à modifier**: `src/main.rs` (lignes 570-610)

```rust
// AVANT (problématique)
router.on_midi_from_app(&app_name, &feedback_data, &app_name);
if let Some(transformed) = router.process_feedback(&app_name, &feedback_data).await {
    xtouch.activate_squelch(120);
    xtouch.set_fader(channel, value14).await;
}

// APRÈS (corrigé)
// 1. Activer squelch EN PREMIER
if let Some((channel, value14)) = router.extract_pb_channel(&feedback_data) {
    xtouch.activate_squelch(120);
}

// 2. Puis mettre à jour état
router.on_midi_from_app(&app_name, &feedback_data, &app_name);

// 3. Puis traiter feedback
if let Some(transformed) = router.process_feedback(&app_name, &feedback_data).await {
    xtouch.set_fader(channel, value14).await;
}
```

---

### Fix P0-B: Vérifier Anti-Echo AVANT State Update

**Fichier à modifier**: `src/router/feedback.rs` (fonction `on_midi_from_app`)

```rust
pub fn on_midi_from_app(&self, app_key: &str, raw_data: &[u8], app_name: &str) {
    // 1. Parser l'entrée
    let entry = match self.build_entry_from_raw(raw_data) {
        Some(e) => e,
        None => return,
    };

    // 2. NOUVEAU: Vérifier anti-echo AVANT mise à jour
    if self.should_suppress_anti_echo(app_key, &entry) {
        trace!("Suppressing echo for app {} key {:?}", app_key, entry.key);
        return;  // Ne pas mettre à jour l'état!
    }

    // 3. Mettre à jour l'état (seulement si pas supprimé)
    let app = AppKey::from(app_name);
    self.state.update_from_feedback(app, entry.clone());

    // 4. Marquer le shadow
    self.update_app_shadow(app_key, &entry);
}
```

---

### Fix P0-C: Filtrage par Page dans State Update

**Fichier à modifier**: `src/main.rs`

```rust
// OPTION 1: Filtrer la mise à jour d'état par page active
let apps_on_page = router.get_apps_for_active_page();
if apps_on_page.contains(&app_name) {
    router.on_midi_from_app(&app_name, &feedback_data, &app_name);
}

// OPTION 2: Marquer les entrées hors-page comme "background"
router.on_midi_from_app_with_context(&app_name, &feedback_data, is_on_active_page);
```

---

### Fix P1-A: Utiliser read() Bloquant pour Anti-Echo

**Fichier à modifier**: `src/router/anti_echo.rs` (ligne 72)

```rust
// AVANT
let app_shadows = match self.app_shadows.try_read() {
    Ok(shadows) => shadows,
    Err(_) => return false,  // Dangereux!
};

// APRÈS
let app_shadows = self.app_shadows.read().unwrap_or_else(|e| {
    error!("Anti-echo lock poisoned: {}", e);
    e.into_inner()  // Récupérer malgré poison
});
```

---

### Fix P1-B: Utiliser Flag Stale dans Page Refresh

**Fichier à modifier**: `src/state/store.rs` (fonction `get_known_latest_for_app`)

```rust
pub fn get_known_latest_for_app(&self, app: AppKey) -> Vec<MidiStateEntry> {
    let states = self.app_states.read().unwrap();
    if let Some(app_state) = states.get(&app) {
        app_state
            .values()
            .filter(|entry| entry.known)
            // NOUVEAU: Préférer les entrées non-stale
            .sorted_by_key(|e| (e.stale, std::cmp::Reverse(e.timestamp)))
            .cloned()
            .collect()
    } else {
        vec![]
    }
}
```

---

### Fix P2-A: Supprimer ou Implémenter le Driver QLC

**Option A - Supprimer le stub**:
```rust
// main.rs: Retirer l'enregistrement du QlcDriver
// Le MIDI bridge suffit pour QLC
```

**Option B - Implémenter correctement**:
```rust
impl QlcDriver {
    fn connection_status(&self) -> ConnectionStatus {
        // Déléguer au MIDI bridge correspondant
        match self.midi_bridge.as_ref() {
            Some(bridge) => bridge.connection_status(),
            None => ConnectionStatus::Disconnected,
        }
    }
}
```

---

### Fix P2-B: Validation Config pour Apps MIDI

**Fichier à modifier**: `src/config/mod.rs` (fonction `validate`)

```rust
// Ajouter validation que les ports MIDI existent
pub fn validate(&self) -> Result<()> {
    // ... validations existantes ...

    // NOUVEAU: Avertir si app référencée mais ports non vérifiables
    if let Some(apps) = &self.midi.apps {
        for app in apps {
            if app.output_port.is_none() && app.input_port.is_none() {
                warn!(
                    "MIDI app '{}' has no ports configured - feedback will not work",
                    app.name
                );
            }
        }
    }

    Ok(())
}
```

---

## 7. Tests de Non-Régression Recommandés

### Test 1: Anti-Echo Effectiveness
```rust
#[tokio::test]
async fn test_anti_echo_blocks_rapid_feedback() {
    // Setup: Envoyer feedback app
    // Puis: Envoyer même valeur < window ms
    // Assert: Deuxième valeur bloquée
}
```

### Test 2: Page Change State Isolation
```rust
#[tokio::test]
async fn test_page_change_isolates_app_state() {
    // Setup: App A sur page 1, App B sur page 2
    // Action: Sur page 1, App B envoie feedback
    // Assert: État App B non utilisé pour refresh page 1
}
```

### Test 3: Squelch Timing Under Load
```rust
#[tokio::test]
async fn test_squelch_protects_during_async_send() {
    // Setup: Haute charge async
    // Action: Envoyer feedback + simuler user input concurrent
    // Assert: User input squelched correctement
}
```

---

## 8. Conclusion

Les bugs identifiés forment un pattern cohérent: **l'ordre des opérations et la synchronisation entre composants** sont les causes racines. Les fixes proposés se concentrent sur:

1. **Réordonner les opérations** (squelch avant state update)
2. **Vérifier anti-echo avant commit** (filter then store)
3. **Respecter les boundaries de page** (isoler état par page)
4. **Remplacer try_read par read** (correction robuste)

La priorité devrait être donnée aux 4 bugs CRITIQUES (BUG-001 à BUG-004) qui causent des oscillations de faders visibles par l'utilisateur.

---

*Rapport généré le 2026-01-04 par Claude Code*
