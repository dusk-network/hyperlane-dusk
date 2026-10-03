// SPDX-License-Identifier: MIT OR Apache-2.0
//
// RUES HTTP client for querying Dusk contracts.

/** Bounded transport defaults may be overridden for a particular deployment. */
export interface RuesClientOptions {
  /** Overall deadline, including response-body reads. Defaults to 30 seconds. */
  timeoutMs?: number;
  /** Maximum successful query body. Defaults to 4 MiB. */
  maxResponseBytes?: number;
  /** Maximum error body. Defaults to 64 KiB. */
  maxErrorResponseBytes?: number;
}

export interface RuesRequestOptions {
  /** Cancels this request, including a pending response-body read. */
  signal?: AbortSignal;
}

/**
 * HTTP client for the binary Dusk RUES API.
 *
 * Every request has a deadline and bounded response buffering. Aborting or
 * timing out transaction propagation does not prove it was not accepted;
 * callers must reconcile their signed transaction before retrying.
 */
export class RuesClient {
  private readonly baseUrl: string;
  private readonly options: Required<RuesClientOptions>;

  constructor(url: string, options: RuesClientOptions = {}) {
    this.baseUrl = url.replace(/\/+$/, "");
    this.options = {
      timeoutMs: options.timeoutMs ?? 30_000,
      maxResponseBytes: options.maxResponseBytes ?? 4 * 1024 * 1024,
      maxErrorResponseBytes: options.maxErrorResponseBytes ?? 64 * 1024,
    };
    for (const [name, value] of Object.entries(this.options)) {
      if (!Number.isSafeInteger(value) || value <= 0) {
        throw new Error(`${name} must be a positive safe integer`);
      }
    }
    // JavaScript timers clamp larger delays to a platform-dependent value.
    if (this.options.timeoutMs > 2_147_483_647) {
      throw new Error("timeoutMs must not exceed 2147483647");
    }
  }

  /** Query contract state using rkyv-serialized arguments and return values. */
  async contractQuery(
    contractId: Uint8Array,
    method: string,
    args: Uint8Array,
    options: RuesRequestOptions = {}
  ): Promise<Uint8Array> {
    if (contractId.length !== 32) {
      throw new Error("Contract ID must contain exactly 32 bytes");
    }
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(method)) {
      throw new Error("Invalid contract method name");
    }
    return this.request(
      `${this.baseUrl}/on/contracts:${bytesToHex(contractId)}/${method}`,
      args,
      "RUES query",
      true,
      options
    );
  }

  /** Propagate signed bytes; success acknowledges acceptance, not execution. */
  async propagateTx(
    txBytes: Uint8Array,
    options: RuesRequestOptions = {}
  ): Promise<void> {
    await this.request(
      `${this.baseUrl}/on/transactions/propagate`,
      txBytes,
      "TX propagation",
      false,
      options
    );
  }

  private async request(
    url: string,
    bytes: Uint8Array,
    description: string,
    readSuccessBody: boolean,
    options: RuesRequestOptions
  ): Promise<Uint8Array> {
    const controller = new AbortController();
    const abort = () => controller.abort(options.signal?.reason);
    if (options.signal?.aborted) abort();
    else options.signal?.addEventListener("abort", abort, { once: true });
    const timer = setTimeout(() => {
      controller.abort(new DOMException("RUES request timed out", "TimeoutError"));
    }, this.options.timeoutMs);
    try {
      const response = await fetch(url, {
        method: "POST",
        headers: {
          "Content-Type": "application/octet-stream",
          Accept: "application/octet-stream",
        },
        body: Uint8Array.from(bytes).buffer,
        signal: controller.signal,
      });
      if (response.ok && !readSuccessBody) {
        // Propagation is acknowledged by status alone. Do not retain or wait
        // for an unused body after the node has accepted the transaction.
        await response.body?.cancel();
        return new Uint8Array();
      }
      const limit = response.ok
        ? this.options.maxResponseBytes
        : this.options.maxErrorResponseBytes;
      const body = await readBoundedBody(response, limit);
      if (!response.ok) {
        throw new Error(`${description} failed (${response.status}): ${new TextDecoder().decode(body)}`);
      }
      return body;
    } catch (error) {
      if (controller.signal.aborted) throw controller.signal.reason;
      throw error;
    } finally {
      clearTimeout(timer);
      options.signal?.removeEventListener("abort", abort);
    }
  }
}

async function readBoundedBody(response: Response, limit: number): Promise<Uint8Array> {
  const tooLarge = () => new Error(`RUES response exceeds ${limit} bytes`);
  const declared = response.headers.get("Content-Length");
  if (declared !== null && /^\d+$/.test(declared) && Number(declared) > limit) {
    await response.body?.cancel();
    throw tooLarge();
  }
  if (!response.body) return new Uint8Array();
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const { value, done } = await reader.read();
      if (done) break;
      if (value.byteLength > limit - length) {
        await reader.cancel();
        throw tooLarge();
      }
      chunks.push(value);
      length += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  const body = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return body;
}

function bytesToHex(bytes: Uint8Array): string {
  return Array.from(bytes)
    .map(b => b.toString(16).padStart(2, "0"))
    .join("");
}
