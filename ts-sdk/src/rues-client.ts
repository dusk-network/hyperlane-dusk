// SPDX-License-Identifier: MIT OR Apache-2.0
//
// RUES HTTP client for querying Dusk contracts.

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
    if (contractId.length !== 32) {
      throw new Error("Contract ID must contain exactly 32 bytes");
    }
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(method)) {
      throw new Error("Invalid contract method name");
    }
    const contractHex = bytesToHex(contractId);
    const url = `${this.baseUrl}/on/contracts:${contractHex}/${method}`;

    const response = await fetch(url, {
      method: "POST",
      headers: {
        "Content-Type": "application/octet-stream",
        Accept: "application/octet-stream",
      },
      body: Uint8Array.from(args).buffer,
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
      body: Uint8Array.from(txBytes).buffer,
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
