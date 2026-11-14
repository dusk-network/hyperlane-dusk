// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Mailbox contract interface for TypeScript.

import type { RuesClient } from "./rues-client.js";
import type { HexBytes32, MailboxState } from "./types.js";

/**
 * Provides a TypeScript interface to query the Hyperlane Mailbox contract
 * deployed on Dusk via RUES.
 *
 * All query methods require the wasm-bindings module for rkyv
 * serialization/deserialization. Pass the bindings at construction time.
 */
export class DuskMailbox {
  constructor(
    private readonly rues: RuesClient,
    private readonly contractId: Uint8Array,
    private readonly wasm: WasmBindings
  ) {}

  /** Query the local domain ID. */
  async localDomain(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "local_domain",
      args
    );
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /** Query the current nonce (number of dispatched messages). */
  async nonce(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "nonce",
      args
    );
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /** Query the latest dispatched message ID. */
  async latestDispatchedId(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "latest_dispatched_id",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Check whether a message has been delivered. */
  async delivered(messageId: Uint8Array): Promise<boolean> {
    const args = this.wasm.rkyv_serialize_bytes32(messageId);
    const result = await this.rues.contractQuery(
      this.contractId,
      "delivered",
      args
    );
    return this.wasm.rkyv_deserialize_bool(result);
  }

  /** Get the block height at which a message was delivered. Returns 0n if not delivered. */
  async deliveredAt(messageId: Uint8Array): Promise<bigint> {
    const args = this.wasm.rkyv_serialize_bytes32(messageId);
    const result = await this.rues.contractQuery(
      this.contractId,
      "delivered_at",
      args
    );
    return this.wasm.rkyv_deserialize_u64(result);
  }

  /** Get the default ISM contract ID. */
  async defaultIsm(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "default_ism",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the default hook contract ID. */
  async defaultHook(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "default_hook",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the required hook contract ID. */
  async requiredHook(): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "required_hook",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /**
   * Get the ID of the message dispatched at a given nonce. The Mailbox
   * stores only the ID; the message is in that block's `Dispatch` event.
   */
  async dispatchedId(nonce: number): Promise<HexBytes32> {
    const args = this.wasm.rkyv_serialize_u32(nonce);
    const result = await this.rues.contractQuery(
      this.contractId,
      "dispatched_id",
      args
    );
    return this.wasm.rkyv_deserialize_bytes32(result);
  }

  /** Get the number of processed (delivered) messages. */
  async processedCount(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "processed_count",
      args
    );
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /**
   * Query mailbox fields, keeping the nonce and latest message ID consistent.
   * Configuration fields are independent current-state reads, not a snapshot.
   */
  async getState(): Promise<MailboxState> {
    const [localDomain, nonce, ism, hook, requiredHook] =
      await Promise.all([
        this.localDomain(),
        this.nonce(),
        this.defaultIsm(),
        this.defaultHook(),
        this.requiredHook(),
      ]);
    // Dispatched IDs are immutable. Read the ID at the observed nonce so a
    // concurrent dispatch cannot pair an old nonce with a newer ID.
    const latestId =
      nonce > 0 ? await this.dispatchedId(nonce - 1) : "00".repeat(32);
    return {
      localDomain,
      nonce,
      latestDispatchedId: latestId,
      defaultIsm: ism,
      defaultHook: hook,
      requiredHook,
    };
  }
}

/**
 * Interface for the WASM bindings module.
 * These functions are provided by the compiled `hyperlane-dusk-wasm-bindings` crate.
 */
export interface WasmBindings {
  rkyv_serialize_unit(): Uint8Array;
  rkyv_serialize_u32(value: number): Uint8Array;
  rkyv_serialize_u64(value: bigint): Uint8Array;
  rkyv_serialize_bool(value: boolean): Uint8Array;
  rkyv_serialize_bytes32(data: Uint8Array): Uint8Array;
  rkyv_deserialize_string(data: Uint8Array): string;
  rkyv_deserialize_u8(data: Uint8Array): number;
  rkyv_deserialize_u32(data: Uint8Array): number;
  rkyv_deserialize_u64(data: Uint8Array): bigint;
  rkyv_deserialize_bool(data: Uint8Array): boolean;
  rkyv_deserialize_bytes32(data: Uint8Array): string;
  rkyv_deserialize_bytes(data: Uint8Array): Uint8Array;
  encode_message(
    version: number,
    nonce: number,
    origin: number,
    sender: Uint8Array,
    destination: number,
    recipient: Uint8Array,
    body: Uint8Array
  ): Uint8Array;
  decode_message(encoded: Uint8Array): string | undefined;
  message_id(encoded: Uint8Array): Uint8Array;
  encode_token_message(recipient: Uint8Array, amount: bigint): Uint8Array;
  decode_token_message(body: Uint8Array): string | undefined;
  keccak256(data: Uint8Array): Uint8Array;
}
