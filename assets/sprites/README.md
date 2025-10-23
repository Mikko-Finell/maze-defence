# Sprite directory overview

Sprite textures and their editable source files live under this directory.  
Renderers resolve sprite assets by consulting `assets/manifest.toml`, so keep the
layout and filenames in sync with that manifest.

* `towers/` — shared art for tower bases and turrets.  Future tower variants may
  add subdirectories here.
* `bugs/` — art for bug bodies.  Additional enemy types should add their own
  folders with clear naming.

Each leaf directory contains a README describing the expected files,
licensing/attribution notes, and any conversion steps required before the
textures ship with the game.  Place exported `.png` textures next to their
source files so Git LFS captures the binaries while Git keeps the editable
formats diffable.
