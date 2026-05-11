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
    private readonly rues: RuesClient,
    private readonly contractId: Uint8Array,
    private readonly wasm: WasmBindings
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
    // String is rkyv-serialized as a Vec<u8> of UTF-8 bytes
    const bytes = this.wasm.rkyv_deserialize_bytes(result);
    return new TextDecoder().decode(bytes);
  }

  /** Get the token symbol. */
  async symbol(): Promise<string> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "symbol",
      args
    );
    const bytes = this.wasm.rkyv_deserialize_bytes(result);
    return new TextDecoder().decode(bytes);
  }

  /** Get the number of decimals. */
  async decimals(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "decimals",
      args
    );
    // u8 is deserialized via the u32 helper (rkyv pads to aligned size)
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /** Get the total token supply. */
  async totalSupply(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "total_supply",
      args
    );
    return Number(this.wasm.rkyv_deserialize_u64(result));
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
