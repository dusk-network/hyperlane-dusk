// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Warp route contract interface for TypeScript.

import type { RuesClient } from "./rues-client.js";
import type { WasmBindings } from "./mailbox.js";
import type { HexBytes32 } from "./types.js";

/**
 * Provides a TypeScript interface to query Hyperlane warp route contracts
 * (WarpDrc20, WarpDrc20Collateral, WarpNative) deployed on Dusk via RUES.
 */
export class DuskWarpRoute {
  constructor(
    protected readonly rues: RuesClient,
    protected readonly contractId: Uint8Array,
    protected readonly wasm: WasmBindings
  ) {}

  /** Get the Mailbox contract ID. */
  async mailbox(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "mailbox",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the hook override contract ID. */
  async hook(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "hook",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the ISM override for this warp route. */
  async interchainSecurityModule(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "interchain_security_module",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the enrolled router for a domain. Returns zero bytes if not enrolled. */
  async enrolledRouter(domain: number): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_u32(domain);
    const result = await this.rues.contractQuery(
      this.contractId,
      "enrolled_router",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }
}

/**
 * Extended interface for WarpDrc20 (synthetic) tokens.
 * Adds DRC20 token query methods.
 */
export class DuskWarpDrc20 extends DuskWarpRoute {
  /** Get the token name. */
  async name(): Promise<string> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "name",
      args
    );
    return this.wasm.rkyv_deserialize_string(result);
  }

  /** Get the token symbol. */
  async symbol(): Promise<string> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "symbol",
      args
    );
    return this.wasm.rkyv_deserialize_string(result);
  }

  /** Get the number of decimals. */
  async decimals(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "decimals",
      args
    );
    return this.wasm.rkyv_deserialize_u8(result);
  }

  /** Get the total token supply. */
  async totalSupply(): Promise<bigint> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "total_supply",
      args
    );
    return this.wasm.rkyv_deserialize_u64(result);
  }
}

/**
 * Extended interface for WarpNative.
 * Adds account registration query.
 */
export class DuskWarpNative extends DuskWarpRoute {
  /** Check if an H256 has a registered account. */
  async isRegistered(h256: Uint8Array): Promise<boolean> {
    const args = this.wasm.rkyv_serialize_bytes32(h256);
    const result = await this.rues.contractQuery(
      this.contractId,
      "is_registered",
      args
    );
    return this.wasm.rkyv_deserialize_bool(result);
  }
}

/**
 * Extended interface for WarpDrc20Collateral.
 * Adds wrapped token query.
 */
export class DuskWarpCollateral extends DuskWarpRoute {
  /** Get the wrapped DRC20 token contract ID. */
  async wrappedToken(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "wrapped_token",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }
}
