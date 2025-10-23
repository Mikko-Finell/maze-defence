# Maze Defence Asset Pipeline

This directory defines the repository structure for binary art that ships with Maze Defence.  
All assets are referenced deterministically by the rendering adapters via `assets/manifest.toml`,
so filenames and layout changes must be reflected here before any code attempts to load them.

## Licensing expectations

* Contributors **must** only add artwork they have rights to redistribute under the project licence.  
* When importing third-party art, record attribution and licence notes in the README colocated with the asset.  
* Prefer lossless source formats (such as `.aseprite`) in addition to the exported textures so revisions remain editable.

## Manifest workflow

1. Declare new sprite files inside [`assets/manifest.toml`](manifest.toml).  
   Each `SpriteKey` from `adapters/rendering` maps to exactly one relative path.  
2. Keep paths stable once published; renames require updating the manifest and any downstream cache tooling.  
3. Use comments in the manifest to document experimental variants or format notes so CI reviewers can reason about changes.  
4. Do **not** commit actual binary data without updating `.gitattributes` so Git LFS tracks the file type.

## Directory layout

```
assets/
├── manifest.toml
└── sprites/
    ├── README.md
    ├── bugs/
    │   └── README.md
    └── towers/
        └── README.md
```

The `sprites/` subtree stores source art and exported textures that correspond to `SpriteKey` values.  
Backend loaders combine the manifest with this layout to locate the files synchronously at startup.

## Git LFS configuration

`*.png` files are tracked via Git LFS (see the repository `.gitattributes`).  
Run `git lfs track` after adding new binary patterns to verify they are captured.  
Always commit the updated `.gitattributes` file alongside any new asset types so CI and other contributors pull the same filters.
