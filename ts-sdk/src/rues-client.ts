// SPDX-License-Identifier: MIT OR Apache-2.0
//
// RUES HTTP client for querying Dusk contracts.

import type { ContractQuery } from "./types.js";

/**
 * HTTP client for the Dusk RUES (Rusk Universal Event System) API.
 *
 * RUES endpoints use rkyv binary serialization. Use the wasm-bindings
 * module to serialize/deserialize arguments and return values.
 */
export class RuesClient {
  private readonly baseUrl: string;

  constructor(url: string) {
    // Normalize: remove trailing slash
    this.baseUrl = url.replace(/\/+$/, "");
  }

  /**
   * Query a contract's state via RUES.
   *
   * @param contractId - 32-byte contract identifier
   * @param method - Method name to call (e.g., "nonce", "delivered")
   * @param args - rkyv-serialized arguments
   * @returns rkyv-serialized return value
   */
  async contractQuery(
    contractId: Uint8Array,
    method: string,
    args: Uint8Array
  ): Promise<Uint8Array> {
    const contractHex = bytesToHex(contractId);
    const url = `${this.baseUrl}/on/contracts:${contractHex}/call`;

    const body = JSON.stringify({
      fn_name: method,
      fn_args: Array.from(args),
    });

    const response = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Accept: "application/octet-stream",
      },
      body,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`RUES query failed (${response.status}): ${text}`);
    }

    const buffer = await response.arrayBuffer();
    return new Uint8Array(buffer);
  }

  /**
   * Propagate a signed transaction to the network.
   *
   * @param txBytes - The serialized transaction bytes
   */
  async propagateTx(txBytes: Uint8Array): Promise<void> {
    const url = `${this.baseUrl}/on/transactions/propagate`;

    const response = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/octet-stream",
      },
      body: txBytes,
    });

    if (!response.ok) {
      const text = await response.text();
      throw new Error(`TX propagation failed (${response.status}): ${text}`);
    }
  }
}

/** Convert a Uint8Array to a lowercase hex string. */
function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}
