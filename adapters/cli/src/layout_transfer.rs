#![allow(clippy::missing_errors_doc)]

use std::{error::Error, fmt};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use maze_defence_core::{CellCoord, TowerKind};
use serde::{Deserialize, Serialize};

const SNAPSHOT_DOMAIN: &str = "maze";
const SNAPSHOT_VERSION: &str = "v1";

/// Identifier prefix emitted before the encoded snapshot payload.
pub(crate) const SNAPSHOT_HEADER: &str = "maze:v1";
/// Delimiter used to separate the prefix, grid dimensions and payload.
const FIELD_DELIMITER: char = ':';

/// Snapshot of the towers placed within the maze and the grid configuration.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct TowerLayoutSnapshot {
    /// Number of tile columns contained in the grid.
    pub columns: u32,
    /// Number of tile rows contained in the grid.
    pub rows: u32,
    /// Length of a single tile edge expressed in world units.
    pub tile_length: f32,
    /// Number of cells rendered along each tile edge.
    pub cells_per_tile: u32,
    /// Towers composing the layout captured by the snapshot.
    pub towers: Vec<TowerLayoutTower>,
}

impl TowerLayoutSnapshot {
    /// Encodes the snapshot into a single-line string suitable for clipboard transfer.
    #[must_use]
    pub(crate) fn encode(&self) -> String {
        let payload = SerializableSnapshot {
            tile_length: self.tile_length,
            cells_per_tile: self.cells_per_tile,
            towers: self.towers.clone(),
        };
        let json = serde_json::to_vec(&payload).expect("layout snapshot serialization never fails");
        let encoded = STANDARD_NO_PAD.encode(json);
        format!("{SNAPSHOT_HEADER}:{}x{}:{encoded}", self.columns, self.rows)
    }

    /// Decodes a snapshot from the provided string representation.
    pub(crate) fn decode(value: &str) -> Result<Self, LayoutTransferError> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            return Err(LayoutTransferError::EmptyPayload);
        }

        let mut parts = trimmed.split(FIELD_DELIMITER);
        let domain = parts.next().ok_or(LayoutTransferError::MissingPrefix)?;
        let version = parts.next().ok_or(LayoutTransferError::MissingVersion)?;
        let dimensions = parts.next().ok_or(LayoutTransferError::MissingDimensions)?;
        let payload = parts.next().ok_or(LayoutTransferError::MissingPayload)?;

        if domain != SNAPSHOT_DOMAIN {
            return Err(LayoutTransferError::InvalidPrefix(domain.to_owned()));
        }
        if version != SNAPSHOT_VERSION {
            return Err(LayoutTransferError::UnsupportedVersion(version.to_owned()));
        }

        let (columns, rows) = parse_dimensions(dimensions)?;
        let bytes = STANDARD_NO_PAD
            .decode(payload.as_bytes())
            .map_err(LayoutTransferError::InvalidEncoding)?;
        let decoded: SerializableSnapshot =
            serde_json::from_slice(&bytes).map_err(LayoutTransferError::InvalidPayload)?;

        Ok(Self {
            columns,
            rows,
            tile_length: decoded.tile_length,
            cells_per_tile: decoded.cells_per_tile,
            towers: decoded.towers,
        })
    }
}

/// Tower description captured within a layout snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct TowerLayoutTower {
    /// Type of tower represented by the snapshot.
    pub kind: TowerKind,
    /// Upper-left cell anchoring the tower's footprint.
    pub origin: CellCoord,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct SerializableSnapshot {
    tile_length: f32,
    cells_per_tile: u32,
    towers: Vec<TowerLayoutTower>,
}

/// Errors that can occur while decoding layout transfer strings.
#[derive(Debug)]
pub(crate) enum LayoutTransferError {
    /// The provided string was empty or contained only whitespace.
    EmptyPayload,
    /// The prefix segment was missing from the encoded snapshot.
    MissingPrefix,
    /// The encoded snapshot did not contain a version segment.
    MissingVersion,
    /// The encoded snapshot did not include grid dimensions.
    MissingDimensions,
    /// The encoded snapshot did not include the payload segment.
    MissingPayload,
    /// The encoded snapshot used an unexpected prefix segment.
    InvalidPrefix(String),
    /// The encoded snapshot used an unsupported version identifier.
    UnsupportedVersion(String),
    /// The grid dimensions could not be parsed from the encoded snapshot.
    InvalidDimensions(String),
    /// The base64 payload could not be decoded.
    InvalidEncoding(base64::DecodeError),
    /// The decoded payload could not be deserialised.
    InvalidPayload(serde_json::Error),
}

impl fmt::Display for LayoutTransferError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPayload => write!(f, "clipboard payload was empty"),
            Self::MissingPrefix => write!(f, "layout string is missing the prefix"),
            Self::MissingVersion => write!(f, "layout string is missing the version"),
            Self::MissingDimensions => write!(f, "layout string is missing the grid dimensions"),
            Self::MissingPayload => write!(f, "layout string is missing the payload"),
            Self::InvalidPrefix(prefix) => write!(f, "layout prefix '{prefix}' is not supported"),
            Self::UnsupportedVersion(version) => {
                write!(f, "layout version '{version}' is not supported")
            }
            Self::InvalidDimensions(dimensions) => {
                write!(f, "could not parse grid dimensions '{dimensions}'")
            }
            Self::InvalidEncoding(error) => {
                write!(f, "could not decode layout payload: {error}")
            }
            Self::InvalidPayload(error) => {
                write!(f, "could not parse layout payload: {error}")
            }
        }
    }
}

impl Error for LayoutTransferError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidEncoding(error) => Some(error),
            Self::InvalidPayload(error) => Some(error),
            _ => None,
        }
    }
}

fn parse_dimensions(dimensions: &str) -> Result<(u32, u32), LayoutTransferError> {
    let (columns, rows) = dimensions
        .split_once(['x', 'X'])
        .ok_or_else(|| LayoutTransferError::InvalidDimensions(dimensions.to_owned()))?;

    let columns = columns
        .trim()
        .parse::<u32>()
        .map_err(|_| LayoutTransferError::InvalidDimensions(dimensions.to_owned()))?;
    let rows = rows
        .trim()
        .parse::<u32>()
        .map_err(|_| LayoutTransferError::InvalidDimensions(dimensions.to_owned()))?;

    if columns == 0 || rows == 0 {
        return Err(LayoutTransferError::InvalidDimensions(
            dimensions.to_owned(),
        ));
    }

    Ok((columns, rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_empty_layout() {
        let snapshot = TowerLayoutSnapshot {
            columns: 12,
            rows: 8,
            tile_length: 64.0,
            cells_per_tile: 4,
            towers: Vec::new(),
        };

        let encoded = snapshot.encode();
        assert!(encoded.starts_with(&format!("{SNAPSHOT_HEADER}:12x8:")));

        let decoded = TowerLayoutSnapshot::decode(&encoded).expect("snapshot decodes");
        assert_eq!(snapshot, decoded);
    }

    #[test]
    fn round_trip_populated_layout() {
        let towers = vec![
            TowerLayoutTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(5, 7),
            },
            TowerLayoutTower {
                kind: TowerKind::Basic,
                origin: CellCoord::new(12, 4),
            },
        ];
        let snapshot = TowerLayoutSnapshot {
            columns: 20,
            rows: 15,
            tile_length: 96.0,
            cells_per_tile: 6,
            towers,
        };

        let encoded = snapshot.encode();
        assert!(encoded.starts_with(&format!("{SNAPSHOT_HEADER}:20x15:")));

        let decoded = TowerLayoutSnapshot::decode(&encoded).expect("snapshot decodes");
        assert_eq!(snapshot, decoded);
    }
}
