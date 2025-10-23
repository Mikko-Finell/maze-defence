# Tower sprite placeholders

This folder holds the base and turret sprites referenced by
`SpriteKey::TowerBase` and `SpriteKey::TowerTurret`.

Expected files:

* `base.png` — Axis-aligned square texture that fills a single tower footprint.
* `turret.png` — Rotating turret sprite centred on the same pivot as `base.png`.

When adding production art:

1. Include source files (`.aseprite`, `.psd`, etc.) alongside the exported
   textures so future edits remain non-destructive.
2. Document attribution/licence information below so downstream builds can vet
   usage rights.
3. Export textures at power-of-two resolutions when possible to keep backend
   upload paths efficient and deterministic.

## Attribution

_(Record creator names, licence terms, and upstream links here.)_
