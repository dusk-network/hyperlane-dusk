// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Standard hook metadata parsing for Dusk.

//! Helpers for parsing the `StandardHookMetadata` format.
//!
//! On EVM this is an 86-byte struct. On Dusk we use a simplified binary
//! encoding: the first 8 bytes (little-endian `u64`) encode the gas limit.
//! If the metadata is empty or too short, defaults are returned.

/// Default gas usage when no metadata is provided.
pub const DEFAULT_GAS_USAGE: u64 = 50_000;

/// Extract the gas limit from hook metadata.
///
/// - Empty metadata → [`DEFAULT_GAS_USAGE`].
/// - Metadata with ≥ 8 bytes → first 8 bytes as little-endian `u64`.
/// - Metadata with < 8 bytes → [`DEFAULT_GAS_USAGE`].
#[must_use]
pub fn gas_limit(metadata: &[u8]) -> u64 {
    if metadata.len() >= 8 {
        u64::from_le_bytes([
            metadata[0],
            metadata[1],
            metadata[2],
            metadata[3],
            metadata[4],
            metadata[5],
            metadata[6],
            metadata[7],
        ])
    } else {
        DEFAULT_GAS_USAGE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_metadata_returns_default() {
        assert_eq!(gas_limit(&[]), DEFAULT_GAS_USAGE);
    }

    #[test]
    fn short_metadata_returns_default() {
        assert_eq!(gas_limit(&[1, 2, 3]), DEFAULT_GAS_USAGE);
    }

    #[test]
    fn valid_metadata_returns_gas_limit() {
        let gas: u64 = 100_000;
        let bytes = gas.to_le_bytes();
        assert_eq!(gas_limit(&bytes), 100_000);
    }

    #[test]
    fn metadata_with_extra_bytes() {
        let gas: u64 = 200_000;
        let mut bytes = gas.to_le_bytes().to_vec();
        bytes.extend_from_slice(&[0xAA, 0xBB]); // extra bytes
        assert_eq!(gas_limit(&bytes), 200_000);
    }
}
