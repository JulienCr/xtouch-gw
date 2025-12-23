---
name: Adapt OBS Split Spec
overview: Adapter la spécification `spec-split-obs.md` pour l'aligner avec l'architecture existante (multi-gamepad, driver OBS, config YAML), documenter les impacts sur le code et la configuration, et définir la structure OBS requise.
todos:
  - id: update-spec
    content: Mettre à jour docs/spec-split-obs.md avec structure alignée au code existant
    status: completed
  - id: add-config-types
    content: Ajouter CameraControlConfig et types associés dans src/config/mod.rs
    status: completed
    dependencies:
      - update-spec
  - id: impl-obs-state
    content: Implémenter CameraControlState et ViewMode dans src/drivers/obs.rs
    status: completed
    dependencies:
      - add-config-types
  - id: impl-obs-actions
    content: Implémenter selectCamera, enterSplit, exitSplit, setSceneItemEnabled
    status: completed
    dependencies:
      - impl-obs-state
  - id: update-config-example
    content: Ajouter camera_control et mappings D-Pad dans config.example.yaml
    status: completed
    dependencies:
      - add-config-types
---

# Adaptation de la spec OBS Split au code existant

## Contexte : Architecture actuelle

Le driver OBS ([src/drivers/obs.rs](src/drivers/obs.rs)) supporte déjà :

- `changeScene` / `setScene` : changement de scène
- `toggleStudioMode` : mode studio
- `nudgeX/Y`, `scaleUniform` : transformations

Le système multi-gamepad ([src/input/gamepad/mod.rs](src/input/gamepad/mod.rs)) utilise :

- `gamepad1.*`, `gamepad2.*` comme préfixes de contrôle
- Config par slot dans `gamepad.gamepads[]`

---

## Modifications de la spec

### 1. Modèle multi-gamepad avec état partagé

L'état sera **global** (non par manette) :

- `currentViewMode: FULL | SPLIT_LEFT | SPLIT_RIGHT`
- `lastCamera: String` (nom de la caméra active)

Toutes les manettes voient et modifient le même état. Cela permet à gamepad1 et gamepad2 de collaborer.

### 2. Nouvelles actions OBS à ajouter

| Action | Description ||--------|-------------|| `selectCamera` | Logique hybride : FULL → changeScene, SPLIT → show/hide sources || `enterSplit` | Active scène split + source lastCamera || `exitSplit` | Retour à `--- CAM [lastCamera] `|| `setSceneItemEnabled` | Masque/affiche une source dans une scène |

### 3. Structure config.yaml

```yaml
obs:
  host: "127.0.0.1"
  port: 4455
  password: "xxx"
  
  # NOUVEAU : Configuration du contrôle caméra
  camera_control:
    cameras:
    - id: "Main"
        scene: "--- CAM Main"
        split_source: "SPLIT CAM Main"
    - id: "Main2"  
        scene: "--- CAM Main 2"
        split_source: "SPLIT CAM Main 2"
    - id: "Jardin"
        scene: "--- CAM Jardin"
        split_source: "SPLIT CAM Jardin"
    - id: "Cour"
        scene: "--- CAM Cour"
        split_source: "SPLIT CAM Cour"
    splits:
      left: "--- SPLIT left"
      right: "--- SPLIT right"
```



### 4. Mapping des boutons (pages_global)

```yaml
pages_global:
  controls:
    # Boutons caméra (toutes manettes)
    gamepad1.btn.a:
      app: "obs"
      action: "selectCamera"
      params: ["Main"]
    gamepad1.btn.b:
      app: "obs"
      action: "selectCamera"
      params: ["Main2"]
    gamepad1.btn.x:
      app: "obs"
      action: "selectCamera"
      params: ["Jardin"]
    gamepad1.btn.y:
      app: "obs"
      action: "selectCamera"
      params: ["Cour"]
    
    # D-Pad split (toutes manettes)
    gamepad1.btn.dpad_left:
      app: "obs"
      action: "enterSplit"
      params: ["left"]
    gamepad1.btn.dpad_right:
      app: "obs"
      action: "enterSplit"
      params: ["right"]
    
    # Retour vue pleine
    gamepad1.btn.start:
      app: "obs"
      action: "exitSplit"
      params: []
    
    # Dupliquer pour gamepad2, etc.
```

---

## Impacts sur le code

### 1. `src/drivers/obs.rs` - Ajouts

```rust
// Nouvel état partagé
struct CameraControlState {
    current_view_mode: ViewMode,  // FULL | SPLIT_LEFT | SPLIT_RIGHT
    last_camera: String,          // "Main", "Main2", etc.
}

enum ViewMode { Full, SplitLeft, SplitRight }
```

Nouvelles méthodes :

- `select_camera(camera_id)` : logique hybride
- `enter_split(side)` : active split + source
- `exit_split()` : retour FULL
- `set_scene_item_enabled(scene, source, enabled)` : API OBS

### 2. `src/config/mod.rs` - Nouveaux types

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraControlConfig {
    pub cameras: Vec<CameraConfig>,
    pub splits: SplitConfig,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CameraConfig {
    pub id: String,
    pub scene: String,
    pub split_source: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SplitConfig {
    pub left: String,
    pub right: String,
}
```

Ajouter à `ObsConfig` :

```rust
pub camera_control: Option<CameraControlConfig>,
```



### 3. Pas d'impact sur le système gamepad

Le routage existant (`handle_control`) gère déjà les contrôles `gamepad*.btn.*`. Il suffit d'ajouter les mappings dans la config.---

## Structure OBS requise (documentation)

La spec doit documenter la structure OBS à créer manuellement :

```javascript
OBS Studio
├── Scènes caméra (plein écran)
│   ├── --- CAM Main
│   ├── --- CAM Main 2
│   ├── --- CAM Jardin
│   └── --- CAM Cour
│
├── Scènes split
│   ├── --- SPLIT left
│   │   └── Sources (même ordre, une seule visible)
│   │       ├── SPLIT CAM Main
│   │       ├── SPLIT CAM Main 2
│   │       ├── SPLIT CAM Jardin
│   │       └── SPLIT CAM Cour
│   │
│   └── --- SPLIT right
│       └── Sources (même ordre, une seule visible)
│           ├── SPLIT CAM Main
│           ├── SPLIT CAM Main 2
│           ├── SPLIT CAM Jardin
│           └── SPLIT CAM Cour
```

---