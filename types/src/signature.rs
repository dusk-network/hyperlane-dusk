// SPDX-License-Identifier: MIT OR Apache-2.0

//! Ethereum ECDSA signature rules shared by the contracts that recover
//! validator addresses.

/// Half the secp256k1 group order, big-endian. `OpenZeppelin`'s
/// `ECDSA.recover` rejects any `s` above it.
const SECP256K1_HALF_ORDER: [u8; 32] = [
    0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    0x5d, 0x57, 0x6e, 0x73, 0x57, 0xa4, 0x50, 0x1d, 0xdf, 0xe9, 0x2f, 0x46, 0x68, 0x1b, 0x20, 0xa0,
];

/// Whether a 65-byte `r || s || v` signature is in the canonical form that
/// `OpenZeppelin`'s `ECDSA.recover` accepts: `v` is 27 or 28 and `s` is in the
/// lower half of the curve order.
///
/// The Dusk `secp256k1_recover` host query also accepts `v` of 0 or 1 and
/// high-`s` twins, so contracts check this first to accept exactly the
/// signatures the Solidity contracts accept.
#[must_use]
pub fn is_canonical(signature: &[u8; 65]) -> bool {
    let s: &[u8] = &signature[32..64];
    matches!(signature[64], 27 | 28) && s <= SECP256K1_HALF_ORDER.as_slice()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signature(s: [u8; 32], v: u8) -> [u8; 65] {
        let mut sig = [0u8; 65];
        sig[..32].fill(0x11);
        sig[32..64].copy_from_slice(&s);
        sig[64] = v;
        sig
    }

    #[test]
    fn accepts_low_s_with_ethereum_v() {
        assert!(is_canonical(&signature([0x01; 32], 27)));
        assert!(is_canonical(&signature([0x01; 32], 28)));
        assert!(is_canonical(&signature(SECP256K1_HALF_ORDER, 27)));
    }

    #[test]
    fn rejects_raw_recovery_ids_and_other_v() {
        for v in [0, 1, 26, 29, 255] {
            assert!(!is_canonical(&signature([0x01; 32], v)), "v = {v}");
        }
    }

    #[test]
    fn rejects_high_s() {
        let mut above = SECP256K1_HALF_ORDER;
        above[31] += 1;
        assert!(!is_canonical(&signature(above, 27)));
        assert!(!is_canonical(&signature([0xff; 32], 28)));
    }
}
