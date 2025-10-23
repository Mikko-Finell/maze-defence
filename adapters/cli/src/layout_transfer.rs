#![allow(clippy::missing_errors_doc)]

use std::{error::Error, fmt};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use maze_defence_core::{CellCoord, TowerKind};
use serde::{Deserialize, Serialize};

const SNAPSHOT_DOMAIN: &str = "maze";
const SNAPSHOT_VERSION_V2: &str = "v2";

/// Identifier prefix emitted for the compact binary snapshot payload.
pub(crate) const SNAPSHOT_HEADER_V2: &str = "maze:v2";
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
        let mut payload = Vec::with_capacity(8 + self.towers.len() * 5);
        encode_varint(self.cells_per_tile, &mut payload);
        payload.extend(self.tile_length.to_bits().to_le_bytes());
        encode_varint(self.towers.len() as u32, &mut payload);
        for tower in &self.towers {
            payload.push(encode_tower_kind(tower.kind));
            encode_varint(tower.origin.column(), &mut payload);
            encode_varint(tower.origin.row(), &mut payload);
        }
        let encoded = URL_SAFE_NO_PAD.encode(payload);
        format!(
            "{SNAPSHOT_HEADER_V2}:{}x{}:{encoded}",
            self.columns, self.rows
        )
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

        let (columns, rows) = parse_dimensions(dimensions)?;
        if version != SNAPSHOT_VERSION_V2 {
            return Err(LayoutTransferError::UnsupportedVersion(version.to_owned()));
        }

        decode_v2(columns, rows, payload)
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
    /// The binary payload terminated before all fields were read.
    TruncatedBinaryPayload,
    /// The binary payload encoded a varint that exceeds the supported width.
    VarintOverflow,
    /// The binary payload referenced a tower kind that is not recognised.
    UnknownTowerKind(u8),
    /// Additional bytes remained after decoding the binary payload.
    TrailingBinaryData,
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
            Self::TruncatedBinaryPayload => {
                write!(f, "binary layout payload terminated unexpectedly")
            }
            Self::VarintOverflow => {
                write!(f, "binary layout payload used an oversized varint")
            }
            Self::UnknownTowerKind(kind) => {
                write!(
                    f,
                    "binary layout payload referenced unknown tower kind {kind}"
                )
            }
            Self::TrailingBinaryData => {
                write!(f, "binary layout payload contained trailing bytes")
            }
        }
    }
}

impl Error for LayoutTransferError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidEncoding(error) => Some(error),
            _ => None,
        }
    }
}

fn decode_v2(
    columns: u32,
    rows: u32,
    payload: &str,
) -> Result<TowerLayoutSnapshot, LayoutTransferError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(payload.as_bytes())
        .map_err(LayoutTransferError::InvalidEncoding)?;
    let mut cursor = 0usize;

    let cells_per_tile = decode_varint(&bytes, &mut cursor)?;
    let tile_length_bits = read_u32(&bytes, &mut cursor)?;
    let tile_length = f32::from_bits(tile_length_bits);
    let tower_count = decode_varint(&bytes, &mut cursor)? as usize;
    let mut towers = Vec::with_capacity(tower_count);
    for _ in 0..tower_count {
        let kind_byte = read_u8(&bytes, &mut cursor)?;
        let kind = decode_tower_kind(kind_byte)?;
        let column = decode_varint(&bytes, &mut cursor)?;
        let row = decode_varint(&bytes, &mut cursor)?;
        towers.push(TowerLayoutTower {
            kind,
            origin: CellCoord::new(column, row),
        });
    }

    if cursor != bytes.len() {
        return Err(LayoutTransferError::TrailingBinaryData);
    }

    Ok(TowerLayoutSnapshot {
        columns,
        rows,
        tile_length,
        cells_per_tile,
        towers,
    })
}

fn encode_varint(mut value: u32, buffer: &mut Vec<u8>) {
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        if value == 0 {
            buffer.push(byte);
            break;
        }
        buffer.push(byte | 0x80);
    }
}

fn decode_varint(bytes: &[u8], cursor: &mut usize) -> Result<u32, LayoutTransferError> {
    let mut value = 0u32;
    let mut shift = 0u32;
    for _ in 0..5 {
        if *cursor >= bytes.len() {
            return Err(LayoutTransferError::TruncatedBinaryPayload);
        }
        let byte = bytes[*cursor];
        *cursor += 1;
        value |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
        shift += 7;
    }
    Err(LayoutTransferError::VarintOverflow)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, LayoutTransferError> {
    if bytes.len().saturating_sub(*cursor) < 4 {
        return Err(LayoutTransferError::TruncatedBinaryPayload);
    }
    let mut buffer = [0u8; 4];
    buffer.copy_from_slice(&bytes[*cursor..*cursor + 4]);
    *cursor += 4;
    Ok(u32::from_le_bytes(buffer))
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, LayoutTransferError> {
    if *cursor >= bytes.len() {
        return Err(LayoutTransferError::TruncatedBinaryPayload);
    }
    let byte = bytes[*cursor];
    *cursor += 1;
    Ok(byte)
}

fn encode_tower_kind(kind: TowerKind) -> u8 {
    match kind {
        TowerKind::Basic => 0,
    }
}

fn decode_tower_kind(value: u8) -> Result<TowerKind, LayoutTransferError> {
    match value {
        0 => Ok(TowerKind::Basic),
        other => Err(LayoutTransferError::UnknownTowerKind(other)),
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
        assert!(encoded.starts_with(&format!("{SNAPSHOT_HEADER_V2}:12x8:")));

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
        assert!(encoded.starts_with(&format!("{SNAPSHOT_HEADER_V2}:20x15:")));

        let decoded = TowerLayoutSnapshot::decode(&encoded).expect("snapshot decodes");
        assert_eq!(snapshot, decoded);
    }

}
