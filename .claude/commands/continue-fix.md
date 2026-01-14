# Command: /continue-fix

Reprend les corrections de bugs là où les agents précédents se sont arrêtés.

## Instructions

Quand cette commande est invoquée:

1. **Lire le fichier de progression**:
   - Ouvrir `AUDIT_STATE_PROGRESS.md`
   - Identifier les bugs marqués `PENDING` ou `IN_PROGRESS`
   - Lire la section "Pour Reprendre" pour le contexte

2. **Déterminer le prochain bug à corriger**:
   - Priorité: P0 > P1 > P2 > P3
   - Dans chaque priorité: ordre numérique (BUG-001 avant BUG-002)
   - Si un bug est `IN_PROGRESS`, le terminer d'abord

Important : si possible, traiter pluseurs bugs indépendants en lançant plusieurs agents simultanément.

3. **Lancer les agents appropriés**:
   - Pour bugs `feedback.rs` ou `anti_echo.rs` → Agent `rust-engineer`
   - Pour bugs `router/` → Agent `router-expert`
   - Pour bugs `config/` → Agent `config-expert`
   - Pour bugs `drivers/qlc.rs` → Agent `rust-engineer`

4. **Mettre à jour le fichier de progression**:
   - Marquer le bug comme `IN_PROGRESS` au début
   - Marquer comme `DONE` une fois corrigé
   - Ajouter une entrée dans le journal des modifications
   - Mettre à jour la section "Pour Reprendre"

5. **Rapport au user**:
   - Résumer ce qui a été fait
   - Indiquer le prochain bug à traiter
   - Signaler tout blocage

## Exemple d'utilisation

```
User: /continue-fix
Assistant:
Je reprends les corrections. Dernier état:
- BUG-001: DONE
- BUG-002: IN_PROGRESS (50%)

Je continue BUG-002 (Squelch timing)...
[Lance agent rust-engineer pour terminer BUG-002]
```

## Fichiers de référence

- `AUDIT_STATE_BUGS.md` - Description détaillée des bugs
- `AUDIT_STATE_PROGRESS.md` - Suivi de progression
- `src/router/feedback.rs` - Cible principale des fixes P0
- `src/main.rs` - Cible principale des fixes P0

## Notes importantes

- Les bugs P0 sont interdépendants - les traiter ensemble si possible
- Toujours lancer `cargo check` après chaque modification
- Ne pas committer avant que tous les bugs P0 soient corrigés
- Utiliser des branches feature si nécessaire
