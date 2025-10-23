use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use anyhow::{anyhow, bail, Context, Result};
use macroquad::prelude::ImageFormat;
use macroquad::{
    color::{Color as MacroquadColor, WHITE},
    math::Vec2 as MacroquadVec2,
    texture::{draw_texture_ex, DrawTextureParams, FilterMode, Texture2D},
};
use maze_defence_rendering::SpriteKey;
use serde::Deserialize;

/// Default location of the sprite manifest relative to the game binary.
const MANIFEST_RELATIVE_PATH: &str = "assets/manifest.toml";

/// Collection of textures keyed by their logical sprite identifiers.
#[derive(Debug)]
pub(crate) struct SpriteAtlas {
    textures: HashMap<SpriteKey, Texture2D>,
}

impl SpriteAtlas {
    /// Loads all textures declared in `assets/manifest.toml` into GPU memory.
    pub(crate) fn new() -> Result<Self> {
        assert_sprite_api_references();

        let mut errors = Vec::new();
        let mut manifest_and_path = None;

        for candidate in manifest_search_paths() {
            match AssetManifest::load(&candidate) {
                Ok(manifest) => {
                    manifest_and_path = Some((manifest, candidate));
                    break;
                }
                Err(error) => {
                    errors.push((candidate, error));
                }
            }
        }

        let (manifest, manifest_path) = manifest_and_path.ok_or_else(|| {
            let mut message = String::from("failed to load sprite manifest");
            for (path, error) in errors {
                let _ = write!(message, "\n  {}: {error}", path.display());
            }
            anyhow!(message)
        })?;

        let sprite_sources = manifest.sprite_sources(&manifest_path)?;
        let textures = load_textures_with(&sprite_sources, |source| {
            let bytes = fs::read(&source.path).with_context(|| {
                format!(
                    "failed to read sprite file for {:?} at {}",
                    source.key,
                    source.path.display()
                )
            })?;
            ensure_valid_image_data(source.format, &bytes, &source.path)?;
            let texture = Texture2D::from_file_with_format(&bytes, Some(source.format));
            texture.set_filter(FilterMode::Nearest);
            Ok(texture)
        })?;

        Ok(Self { textures })
    }

    /// Returns `true` when the atlas contains the provided sprite key.
    pub(crate) fn contains(&self, key: SpriteKey) -> bool {
        self.textures.contains_key(&key)
    }

    /// Returns the number of sprite textures managed by the atlas.
    pub(crate) fn len(&self) -> usize {
        self.textures.len()
    }

    /// Draws the requested sprite using the supplied draw parameters.
    pub(crate) fn draw(&self, key: SpriteKey, params: DrawParams) {
        let texture = self
            .textures
            .get(&key)
            .unwrap_or_else(|| panic!("missing sprite {key:?} in atlas"));

        let dest_size = MacroquadVec2::new(
            texture.width() * params.scale.x,
            texture.height() * params.scale.y,
        );

        let draw_params = DrawTextureParams {
            dest_size: Some(dest_size),
            rotation: params.rotation,
            pivot: Some(params.pivot),
            ..Default::default()
        };

        draw_texture_ex(
            *texture,
            params.position.x,
            params.position.y,
            params.tint,
            draw_params,
        );
    }
}

/// Parameters describing how a sprite should be drawn.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DrawParams {
    /// Destination position in pixels.
    pub position: MacroquadVec2,
    /// Scale factor applied to the sprite width and height.
    pub scale: MacroquadVec2,
    /// Rotation in radians around the pivot.
    pub rotation: f32,
    /// Normalised pivot in the range 0.0..=1.0.
    pub pivot: MacroquadVec2,
    /// Tint applied to the sprite.
    pub tint: MacroquadColor,
}

impl DrawParams {
    /// Creates a new set of draw parameters positioned at the provided location.
    pub(crate) fn new(position: MacroquadVec2) -> Self {
        Self {
            position,
            scale: MacroquadVec2::new(1.0, 1.0),
            rotation: 0.0,
            pivot: MacroquadVec2::new(0.5, 0.5),
            tint: WHITE,
        }
    }

    /// Updates the scale applied when drawing the sprite.
    pub(crate) fn with_scale(mut self, scale: MacroquadVec2) -> Self {
        self.scale = scale;
        self
    }

    /// Overrides the rotation applied when drawing the sprite.
    pub(crate) fn with_rotation(mut self, rotation: f32) -> Self {
        self.rotation = rotation;
        self
    }

    /// Overrides the pivot applied when drawing the sprite.
    pub(crate) fn with_pivot(mut self, pivot: MacroquadVec2) -> Self {
        self.pivot = pivot;
        self
    }

    /// Overrides the tint applied when drawing the sprite.
    pub(crate) fn with_tint(mut self, tint: MacroquadColor) -> Self {
        self.tint = tint;
        self
    }
}

#[derive(Debug, Deserialize)]
struct AssetManifest {
    version: u32,
    sprites: BTreeMap<String, String>,
}

impl AssetManifest {
    fn load(path: &Path) -> Result<Self> {
        let contents = fs::read_to_string(path)
            .with_context(|| format!("failed to read sprite manifest at {}", path.display()))?;
        let manifest: Self = toml::from_str(&contents)
            .with_context(|| format!("failed to parse sprite manifest at {}", path.display()))?;

        if manifest.version != 1 {
            bail!(
                "unsupported sprite manifest version {} at {}",
                manifest.version,
                path.display()
            );
        }

        Ok(manifest)
    }

    fn sprite_sources(&self, manifest_path: &Path) -> Result<Vec<SpriteAssetSource>> {
        let manifest_dir = manifest_path
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);

        let mut sources = Vec::with_capacity(self.sprites.len());
        for (key_name, relative_path) in &self.sprites {
            let key = parse_sprite_key(key_name).with_context(|| {
                format!(
                    "unknown sprite key `{key_name}` in manifest at {}",
                    manifest_path.display()
                )
            })?;
            let resolved_path = resolve_sprite_path(relative_path, &manifest_dir);
            let format = image_format_for(&resolved_path)?;
            sources.push(SpriteAssetSource {
                key,
                path: resolved_path,
                format,
            });
        }

        Ok(sources)
    }
}

#[derive(Clone, Debug)]
struct SpriteAssetSource {
    key: SpriteKey,
    path: PathBuf,
    format: ImageFormat,
}

fn manifest_search_paths() -> Vec<PathBuf> {
    vec![
        PathBuf::from(MANIFEST_RELATIVE_PATH),
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("assets")
            .join("manifest.toml"),
    ]
}

fn load_textures_with<F>(
    sources: &[SpriteAssetSource],
    mut loader: F,
) -> Result<HashMap<SpriteKey, Texture2D>>
where
    F: FnMut(&SpriteAssetSource) -> Result<Texture2D>,
{
    let mut textures = HashMap::with_capacity(sources.len());

    for source in sources {
        let texture = loader(source).with_context(|| {
            format!(
                "failed to load texture for {:?} from {}",
                source.key,
                source.path.display()
            )
        })?;

        if textures.insert(source.key, texture).is_some() {
            bail!("duplicate sprite key {:?} in manifest", source.key);
        }
    }

    ensure_required_sprites(&textures)?;
    Ok(textures)
}

fn ensure_required_sprites(textures: &HashMap<SpriteKey, Texture2D>) -> Result<()> {
    const REQUIRED: &[SpriteKey] = &[
        SpriteKey::TowerBase,
        SpriteKey::TowerTurret,
        SpriteKey::BugBody,
    ];

    for key in REQUIRED {
        if !textures.contains_key(key) {
            bail!("sprite atlas missing required entry for {key:?}");
        }
    }

    Ok(())
}

fn ensure_valid_image_data(format: ImageFormat, bytes: &[u8], path: &Path) -> Result<()> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    const GIT_LFS_POINTER_PREFIX: &[u8] = b"version https://git-lfs.github.com/spec/v1\n";

    match format {
        ImageFormat::Png => {
            if bytes.starts_with(GIT_LFS_POINTER_PREFIX) {
                bail!(
                    "sprite file {} contains a Git LFS pointer. Fetch sprite assets with `git lfs pull` or run the CLI with `--visual-style primitives`.",
                    path.display()
                );
            }

            if bytes.len() < PNG_SIGNATURE.len() || &bytes[..PNG_SIGNATURE.len()] != PNG_SIGNATURE {
                bail!(
                    "sprite file {} is not a valid PNG (missing PNG signature)",
                    path.display()
                );
            }
        }
        _ => {}
    }

    Ok(())
}

fn parse_sprite_key(name: &str) -> Result<SpriteKey> {
    match name {
        "TowerBase" => Ok(SpriteKey::TowerBase),
        "TowerTurret" => Ok(SpriteKey::TowerTurret),
        "BugBody" => Ok(SpriteKey::BugBody),
        _ => bail!("unknown sprite key `{name}`"),
    }
}

fn resolve_sprite_path(path: &str, manifest_dir: &Path) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }

    if path.starts_with("assets/") || path.starts_with("assets\\") {
        PathBuf::from(path)
    } else {
        manifest_dir.join(candidate)
    }
}

fn image_format_for(path: &Path) -> Result<ImageFormat> {
    let extension = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .ok_or_else(|| anyhow!("sprite path {} is missing an extension", path.display()))?;

    match extension.as_str() {
        "png" => Ok(ImageFormat::Png),
        "jpg" | "jpeg" => Ok(ImageFormat::Jpeg),
        "bmp" => Ok(ImageFormat::Bmp),
        other => bail!(
            "unsupported sprite image format `{other}` for {}",
            path.display()
        ),
    }
}

fn assert_sprite_api_references() {
    let draw_fn: fn(&SpriteAtlas, SpriteKey, DrawParams) = SpriteAtlas::draw;
    let new_fn: fn(MacroquadVec2) -> DrawParams = DrawParams::new;
    let scale_fn: fn(DrawParams, MacroquadVec2) -> DrawParams = DrawParams::with_scale;
    let rotation_fn: fn(DrawParams, f32) -> DrawParams = DrawParams::with_rotation;
    let pivot_fn: fn(DrawParams, MacroquadVec2) -> DrawParams = DrawParams::with_pivot;
    let tint_fn: fn(DrawParams, MacroquadColor) -> DrawParams = DrawParams::with_tint;
    let _ = (draw_fn, new_fn, scale_fn, rotation_fn, pivot_fn, tint_fn);
}

#[cfg(test)]
mod tests {
    use super::*;
    use macroquad::texture::Texture2D;
    use std::fs;

    fn dummy_sources() -> Vec<SpriteAssetSource> {
        vec![
            SpriteAssetSource {
                key: SpriteKey::TowerBase,
                path: PathBuf::from("base.png"),
                format: ImageFormat::Png,
            },
            SpriteAssetSource {
                key: SpriteKey::TowerTurret,
                path: PathBuf::from("turret.png"),
                format: ImageFormat::Png,
            },
            SpriteAssetSource {
                key: SpriteKey::BugBody,
                path: PathBuf::from("bug.png"),
                format: ImageFormat::Png,
            },
        ]
    }

    #[test]
    fn manifest_rejects_unknown_version() {
        let temp_dir = std::env::temp_dir();
        let unique = format!(
            "sprite_manifest_test_{}_{}.toml",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let path = temp_dir.join(unique);
        fs::write(&path, "version = 2\n[sprites]\n").unwrap();

        let result = AssetManifest::load(&path);
        assert!(result.is_err());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn parse_sprite_key_rejects_unknown_entries() {
        assert!(parse_sprite_key("Unknown").is_err());
        assert_eq!(parse_sprite_key("TowerBase").unwrap(), SpriteKey::TowerBase);
    }

    #[test]
    fn resolve_paths_respect_manifest_directory() {
        let manifest_dir = Path::new("assets");
        let resolved = resolve_sprite_path("towers/base.png", manifest_dir);
        assert_eq!(resolved, Path::new("assets").join("towers/base.png"));
    }

    #[test]
    fn image_format_detects_png() {
        let format = image_format_for(Path::new("base.png")).unwrap();
        assert_eq!(format, ImageFormat::Png);
    }

    #[test]
    fn ensure_valid_image_detects_git_lfs_pointer() {
        let data = b"version https://git-lfs.github.com/spec/v1\nobject";
        let path = Path::new("assets/sprites/towers/base.png");
        let error = ensure_valid_image_data(ImageFormat::Png, data, path).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("Git LFS pointer"), "{message}");
    }

    #[test]
    fn ensure_valid_image_detects_invalid_png_signature() {
        let data = b"not a png";
        let path = Path::new("assets/sprites/towers/base.png");
        let error = ensure_valid_image_data(ImageFormat::Png, data, path).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("not a valid PNG"), "{message}");
    }

    #[test]
    fn ensure_valid_image_accepts_png_signature() {
        let mut data = Vec::from(&b"\x89PNG\r\n\x1a\n"[..]);
        data.extend_from_slice(&[0, 0, 0, 0]);
        let path = Path::new("assets/sprites/towers/base.png");
        assert!(ensure_valid_image_data(ImageFormat::Png, &data, path).is_ok());
    }

    #[test]
    fn load_textures_detects_missing_required_entries() {
        let mut sources = dummy_sources();
        let _ = sources.pop();

        let result = load_textures_with(&sources, |_| Ok(Texture2D::empty()));
        assert!(result.is_err());
    }

    #[test]
    fn load_textures_rejects_duplicate_keys() {
        let mut sources = dummy_sources();
        sources.push(SpriteAssetSource {
            key: SpriteKey::TowerBase,
            path: PathBuf::from("duplicate.png"),
            format: ImageFormat::Png,
        });

        let result = load_textures_with(&sources, |_| Ok(Texture2D::empty()));
        assert!(result.is_err());
    }
}
