use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
use glam::Vec2;
use macroquad::{
    math::Vec2 as MacroquadVec2,
    texture::{self, DrawTextureParams, Texture2D},
};
use maze_defence_rendering::{Color, SpriteKey};

use crate::to_macroquad_color;

const SUPPORTED_MANIFEST_VERSION: u32 = 1;
const ALL_SPRITE_KEYS: [SpriteKey; 3] = [
    SpriteKey::TowerBase,
    SpriteKey::TowerTurret,
    SpriteKey::BugBody,
];

/// Parameters describing how a sprite should be drawn on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawParams {
    /// Position in screen-space pixels where the sprite's top-left corner is placed.
    pub position: Vec2,
    /// Desired size in screen-space pixels.
    pub scale: Vec2,
    /// Rotation applied around the computed pivot, in radians.
    pub rotation_radians: f32,
    /// Pivot expressed in normalised sprite coordinates (0.0..=1.0).
    pub pivot: Vec2,
    /// Tint applied to the sprite.
    pub tint: Color,
}

impl DrawParams {
    /// Creates draw parameters anchored at the provided position and scale.
    #[must_use]
    pub fn new(position: Vec2, scale: Vec2) -> Self {
        Self {
            position,
            scale,
            rotation_radians: 0.0,
            pivot: Vec2::splat(0.5),
            tint: Color::new(1.0, 1.0, 1.0, 1.0),
        }
    }

    /// Overrides the rotation applied when drawing the sprite.
    #[must_use]
    pub fn with_rotation(mut self, rotation_radians: f32) -> Self {
        self.rotation_radians = rotation_radians;
        self
    }

    /// Overrides the pivot used when rotating the sprite.
    #[must_use]
    pub fn with_pivot(mut self, pivot: Vec2) -> Self {
        self.pivot = pivot;
        self
    }

    /// Overrides the tint colour used when drawing the sprite.
    #[must_use]
    pub fn with_tint(mut self, tint: Color) -> Self {
        self.tint = tint;
        self
    }
}

/// Cache of textures loaded from the sprite manifest.
#[derive(Debug)]
pub struct SpriteAtlas {
    textures: HashMap<SpriteKey, Texture2D>,
}

impl SpriteAtlas {
    /// Loads the default sprite manifest from disk.
    pub fn from_default_manifest() -> Result<Self> {
        Self::from_manifest_path(Self::default_manifest_path())
    }

    /// Loads sprites from the manifest located at the provided path.
    pub fn from_manifest_path(path: impl AsRef<Path>) -> Result<Self> {
        Self::from_manifest_with_loader(path, default_loader)
    }

    /// Returns the default manifest path relative to the repository root.
    #[must_use]
    pub fn default_manifest_path() -> PathBuf {
        PathBuf::from("assets/manifest.toml")
    }

    /// Draws the requested sprite using the supplied parameters.
    pub fn draw(&self, key: SpriteKey, params: DrawParams) -> Result<()> {
        let texture = *self
            .textures
            .get(&key)
            .with_context(|| format!("sprite {key:?} missing from atlas"))?;

        let dest_size = MacroquadVec2::new(params.scale.x, params.scale.y);
        let pivot_offset =
            MacroquadVec2::new(params.pivot.x * dest_size.x, params.pivot.y * dest_size.y);
        let pivot = MacroquadVec2::new(
            params.position.x + pivot_offset.x,
            params.position.y + pivot_offset.y,
        );

        let draw_params = DrawTextureParams {
            dest_size: Some(dest_size),
            rotation: params.rotation_radians,
            pivot: Some(pivot),
            ..DrawTextureParams::default()
        };

        texture::draw_texture_ex(
            texture,
            params.position.x,
            params.position.y,
            to_macroquad_color(params.tint),
            draw_params,
        );

        Ok(())
    }

    /// Returns whether the atlas contains the provided key.
    #[must_use]
    pub fn contains(&self, key: SpriteKey) -> bool {
        self.textures.contains_key(&key)
    }

    /// Returns the number of textures stored in the atlas.
    #[must_use]
    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    /// Retrieves the texture associated with the provided key.
    #[must_use]
    pub fn texture(&self, key: SpriteKey) -> Option<Texture2D> {
        self.textures.get(&key).copied()
    }

    fn from_manifest_with_loader(
        path: impl AsRef<Path>,
        mut loader: impl FnMut(SpriteKey, &Path) -> Result<Texture2D>,
    ) -> Result<Self> {
        let manifest_path = path.as_ref();
        let contents = fs::read_to_string(manifest_path).with_context(|| {
            format!(
                "failed to read sprite manifest at {}",
                manifest_path.display()
            )
        })?;
        let base = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let entries = parse_manifest(&contents, &base)?;
        Self::from_entries(entries, &mut loader)
    }

    fn from_entries(
        entries: Vec<(SpriteKey, PathBuf)>,
        loader: &mut impl FnMut(SpriteKey, &Path) -> Result<Texture2D>,
    ) -> Result<Self> {
        let mut textures = HashMap::with_capacity(entries.len());
        for (key, path) in entries {
            let texture = loader(key, &path).with_context(|| {
                format!("failed to load sprite {key:?} from {}", path.display())
            })?;
            if textures.insert(key, texture).is_some() {
                bail!("duplicate sprite entry for {key:?}");
            }
        }
        Ok(Self { textures })
    }
}

fn default_loader(_key: SpriteKey, path: &Path) -> Result<Texture2D> {
    let bytes = fs::read(path)
        .with_context(|| format!("failed to read sprite asset at {}", path.display()))?;
    Ok(Texture2D::from_file_with_format(&bytes, None))
}

#[derive(Debug, serde::Deserialize)]
struct Manifest {
    version: u32,
    sprites: HashMap<String, String>,
}

fn parse_manifest(contents: &str, base_path: &Path) -> Result<Vec<(SpriteKey, PathBuf)>> {
    let manifest: Manifest =
        toml::from_str(contents).context("failed to parse sprite manifest toml contents")?;
    if manifest.version != SUPPORTED_MANIFEST_VERSION {
        bail!(
            "unsupported sprite manifest version {}; expected {}",
            manifest.version,
            SUPPORTED_MANIFEST_VERSION
        );
    }

    let mut resolved = HashMap::new();
    for (name, relative_path) in manifest.sprites {
        let key = parse_sprite_key(&name)
            .with_context(|| format!("unknown sprite key `{name}` in manifest"))?;
        let path = base_path.join(relative_path);
        if resolved.insert(key, path).is_some() {
            bail!("sprite manifest contains duplicate entry for {key:?}");
        }
    }

    let mut ordered = Vec::with_capacity(ALL_SPRITE_KEYS.len());
    for key in ALL_SPRITE_KEYS {
        let Some(path) = resolved.remove(&key) else {
            bail!("sprite manifest missing entry for {key:?}");
        };
        ordered.push((key, path));
    }

    if !resolved.is_empty() {
        let unexpected = resolved
            .into_keys()
            .map(|key| format!("{key:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!("sprite manifest contains unexpected keys: {unexpected}");
    }

    Ok(ordered)
}

fn parse_sprite_key(name: &str) -> Result<SpriteKey> {
    match name {
        "TowerBase" => Ok(SpriteKey::TowerBase),
        "TowerTurret" => Ok(SpriteKey::TowerTurret),
        "BugBody" => Ok(SpriteKey::BugBody),
        _ => bail!("unknown sprite key `{name}`"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, path::Path};

    #[test]
    fn parse_manifest_requires_all_known_keys() {
        let manifest = r#"
            version = 1

            [sprites]
            TowerBase = "towers/base.png"
            TowerTurret = "towers/turret.png"
        "#;

        let result = parse_manifest(manifest, Path::new("assets"));
        assert!(result.is_err(), "manifest missing BugBody should fail");
    }

    #[test]
    fn manifest_rejects_unknown_keys() {
        let manifest = r#"
            version = 1

            [sprites]
            TowerBase = "towers/base.png"
            TowerTurret = "towers/turret.png"
            BugBody = "bugs/bug.png"
            Extra = "extra.png"
        "#;

        let result = parse_manifest(manifest, Path::new("assets"));
        assert!(result.is_err(), "unknown keys must be rejected");
    }

    #[test]
    fn manifest_resolves_paths_relative_to_base_directory() {
        let manifest = r#"
            version = 1

            [sprites]
            TowerTurret = "towers/turret.png"
            BugBody = "bugs/bug.png"
            TowerBase = "towers/base.png"
        "#;

        let parsed = parse_manifest(manifest, Path::new("root")).expect("manifest should parse");
        let expected = vec![
            (SpriteKey::TowerBase, PathBuf::from("root/towers/base.png")),
            (
                SpriteKey::TowerTurret,
                PathBuf::from("root/towers/turret.png"),
            ),
            (SpriteKey::BugBody, PathBuf::from("root/bugs/bug.png")),
        ];
        assert_eq!(parsed, expected);
    }

    #[test]
    fn atlas_loads_textures_using_deterministic_order() {
        let manifest = r#"
            version = 1

            [sprites]
            BugBody = "bugs/bug.png"
            TowerBase = "towers/base.png"
            TowerTurret = "towers/turret.png"
        "#;
        let entries = parse_manifest(manifest, Path::new("assets"))
            .expect("manifest should parse into canonical order");
        let load_order = RefCell::new(Vec::new());
        let atlas = SpriteAtlas::from_entries(entries, &mut |key, _| {
            load_order.borrow_mut().push(key);
            Ok(Texture2D::empty())
        })
        .expect("atlas should load using provided loader");

        assert_eq!(load_order.borrow().as_slice(), &ALL_SPRITE_KEYS);
        assert_eq!(atlas.texture_count(), ALL_SPRITE_KEYS.len());
    }

    #[test]
    fn atlas_reuses_cached_textures() {
        let entries = vec![
            (SpriteKey::TowerBase, PathBuf::from("base.png")),
            (SpriteKey::TowerTurret, PathBuf::from("turret.png")),
            (SpriteKey::BugBody, PathBuf::from("bug.png")),
        ];
        let load_counts = RefCell::new(HashMap::new());
        let atlas = SpriteAtlas::from_entries(entries, &mut |key, _| {
            *load_counts.borrow_mut().entry(key).or_insert(0) += 1;
            Ok(Texture2D::empty())
        })
        .expect("atlas should load textures once");

        for key in ALL_SPRITE_KEYS {
            assert!(atlas.contains(key));
            assert!(atlas.texture(key).is_some());
        }

        let counts = load_counts.into_inner();
        for key in ALL_SPRITE_KEYS {
            assert_eq!(
                counts.get(&key),
                Some(&1),
                "loader should be invoked exactly once per key"
            );
        }
    }
}
