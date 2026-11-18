//! The pinned canonical DRC20 as dusk-tx deploys and calls it:
//! `drc20-roles-pausable` from dusk-network/contracts at the revision
//! `scripts/build-canonical-drc20-fixture.sh` pins.
//!
//! The token's call types live in that example's crate, which only builds
//! for wasm, so they are mirrored here. Each must archive exactly as the
//! token's own type does, or the token reads another value or rejects the
//! call; the tests below compare them, byte for byte, with the same types
//! built from the canonical crate.

use dusk_core::abi::ContractId;
use dusk_core::signatures::bls::{PublicKey as BlsPublicKey, Signature as BlsSignature};
use dusk_core::signatures::schnorr::{
    PublicKey as SchnorrPublicKey, Signature as SchnorrSignature,
};
use hyperlane_dusk_types::drc20::Principal;

/// A role in the token's access control.
pub type Role = [u8; 32];

/// The role that grants and revokes every role. `init` gives it to the
/// admin, and only to the admin.
pub const DEFAULT_ADMIN_ROLE: Role = [0; 32];
/// The role that may `mint`. `init` gives it to the admin.
pub const MINTER_ROLE: Role = [1; 32];
/// The role that may `pause` and `unpause`. `init` gives it to the admin.
pub const PAUSER_ROLE: Role = [2; 32];

/// The token's `init` argument.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct Init {
    /// Holds every role.
    pub admin: Principal,
    pub token: TokenInit,
    /// The most the token's supply may reach; 0 for no cap.
    pub cap: u64,
}

/// The token's settings, and what `init` mints.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct TokenInit {
    pub name: String,
    pub symbol: String,
    pub decimals: u8,
    pub initial_balances: Vec<InitBalance>,
}

/// An amount `init` mints to an account.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct InitBalance {
    pub account: Principal,
    pub amount: u64,
}

/// `has_role`'s argument.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct RoleQuery {
    pub role: Role,
    pub account: Principal,
}

/// `mint`'s argument. With no `authorization`, the caller must hold the
/// minter role.
#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct MintCall {
    pub to: Principal,
    pub amount: u64,
    pub authorization: Option<SignedAuthorization>,
}

/// A signed authorization a call can carry instead of the caller's role.
/// dusk-tx never sends one, but `MintCall` archives as the token decodes it
/// only with this type's layout, so it is mirrored whole.
#[allow(dead_code)]
#[derive(rkyv::Archive, rkyv::Serialize)]
pub enum SignedAuthorization {
    Moonlight(MoonlightAuthorization),
    Phoenix(PhoenixSignatureAuthorization),
}

#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct MoonlightAuthorization {
    pub action: AuthorizedAction,
    pub public_key: BlsPublicKey,
    pub signature: BlsSignature,
}

#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct PhoenixSignatureAuthorization {
    pub action: AuthorizedAction,
    pub public_key: SchnorrPublicKey,
    pub signature: SchnorrSignature,
    pub replay_key: Option<[u8; 32]>,
}

#[derive(rkyv::Archive, rkyv::Serialize)]
pub struct AuthorizedAction {
    pub chain_id: u8,
    pub contract: ContractId,
    pub domain: [u8; 32],
    pub action_id: [u8; 32],
    pub nonce: u64,
    pub expires_at: u64,
    pub principal: Principal,
    pub payload_hash: [u8; 32],
}

/// The `init` argument of a token whose `admin` holds every role and the
/// whole `supply`, with no cap.
pub fn init(admin: Principal, name: &str, symbol: &str, decimals: u8, supply: u64) -> Init {
    Init {
        admin,
        token: TokenInit {
            name: name.to_string(),
            symbol: symbol.to_string(),
            decimals,
            initial_balances: vec![InitBalance {
                account: admin,
                amount: supply,
            }],
        },
        cap: 0,
    }
}

#[cfg(test)]
mod tests {
    use canonical_dusk_contract_standards as canonical;
    use canonical_dusk_core::abi::ContractId as CanonicalContractId;
    use hyperlane_dusk_types::drc20::BLS_PUBLIC_KEY_BYTES;

    use super::*;

    fn archived<T: rkyv::Serialize<rkyv::ser::serializers::AllocSerializer<256>>>(
        value: &T,
    ) -> Vec<u8> {
        rkyv::to_bytes::<_, 256>(value).unwrap().to_vec()
    }

    // The example's own types, as `examples/drc20_roles_pausable/src/lib.rs`
    // at the pinned revision defines them, built from the canonical crate.
    #[derive(rkyv::Archive, rkyv::Serialize)]
    struct CanonicalInit {
        admin: canonical::core::Principal,
        token: canonical::token::drc20::Init,
        cap: u64,
    }

    #[derive(rkyv::Archive, rkyv::Serialize)]
    struct CanonicalMintCall {
        to: canonical::core::Principal,
        amount: u64,
        authorization: Option<canonical::auth::SignedAuthorization>,
    }

    #[derive(rkyv::Archive, rkyv::Serialize)]
    struct CanonicalRoleQuery {
        role: canonical::access::Role,
        account: canonical::core::Principal,
    }

    const KEY: [u8; BLS_PUBLIC_KEY_BYTES] = [0x5a; BLS_PUBLIC_KEY_BYTES];

    #[test]
    fn calls_archive_as_the_canonical_token_decodes_them() {
        let account = Principal::Moonlight(KEY);
        let canonical_account = canonical::core::Principal::Moonlight(KEY);
        assert_eq!(
            archived(&init(
                account,
                "Dusk Euro (testnet)",
                "EURd",
                6,
                1_000_000_000_000
            )),
            archived(&CanonicalInit {
                admin: canonical_account,
                token: canonical::token::drc20::Init {
                    name: "Dusk Euro (testnet)".into(),
                    symbol: "EURd".into(),
                    decimals: 6,
                    initial_balances: vec![canonical::token::drc20::InitBalance {
                        account: canonical_account,
                        amount: 1_000_000_000_000,
                    }],
                },
                cap: 0,
            })
        );
        let contract = Principal::Contract(ContractId::from_bytes([0x33; 32]));
        let canonical_contract =
            canonical::core::Principal::Contract(CanonicalContractId::from_bytes([0x33; 32]));
        for (to, canonical_to) in [(account, canonical_account), (contract, canonical_contract)] {
            assert_eq!(
                archived(&MintCall {
                    to,
                    amount: 0x0102_0304_0506_0708,
                    authorization: None,
                }),
                archived(&CanonicalMintCall {
                    to: canonical_to,
                    amount: 0x0102_0304_0506_0708,
                    authorization: None,
                })
            );
        }
        for role in [DEFAULT_ADMIN_ROLE, MINTER_ROLE, PAUSER_ROLE] {
            assert_eq!(
                archived(&RoleQuery { role, account }),
                archived(&CanonicalRoleQuery {
                    role,
                    account: canonical_account,
                })
            );
        }
    }

    /// The admin role is AccessControl's. The token keeps its minter and
    /// pauser roles private: the read-backs ask `has_role` for both on every
    /// token deployed, which an honest token answers true only for these.
    #[test]
    fn the_admin_role_is_access_controls() {
        assert_eq!(DEFAULT_ADMIN_ROLE, canonical::access::DEFAULT_ADMIN_ROLE);
        assert_ne!(MINTER_ROLE, PAUSER_ROLE);
    }
}
