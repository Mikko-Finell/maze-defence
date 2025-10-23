# Ground sprite placeholders

This folder stores tileable ground textures referenced by `SpriteKey::GroundGrass`.

Expected files:

* `grass.png` — Large ground tile rendered beneath the maze when sprite assets are available.

When adding art:

1. Include editable source files (e.g. `.aseprite`, `.psd`) alongside the exported texture.
2. Document attribution/licence information below so downstream builds can verify usage rights.
3. Export textures so that their content seamlessly tiles; the renderer repeats this sprite across the
   maze interior with a footprint four times larger than a standard tower.

## Attribution

_(Record creator names, licence terms, and upstream links here.)_
