// SPDX-License-Identifier: MIT OR Apache-2.0
//
// InterchainGasPaymaster (IGP) contract interface for TypeScript.

import type { RuesClient } from "./rues-client.js";
import type { WasmBindings } from "./mailbox.js";
import type { PayForGasCall } from "./types.js";

/**
 * Queries the Hyperlane InterchainGasPaymaster (IGP) deployed on Dusk via
 * RUES, and encodes its `pay_for_gas` call for a wallet to sign.
 *
 * A message pays the IGP when it is dispatched. Anyone can pay for more
 * gas for it later with `pay_for_gas`, as with `payForGas` on EVM, and the
 * relayer adds up every payment for a message ID and destination.
 */
export class DuskIgp {
  constructor(
    private readonly rues: RuesClient,
    private readonly contractId: Uint8Array,
    private readonly wasm: WasmBindings
  ) {}

  /**
   * What `pay_for_gas` charges, in LUX, for `gasAmount` gas on
   * `destination`: gas x gas price x exchange rate / 1e10, with no
   * overhead. The query fails for a destination the IGP doesn't price, for
   * no gas or more than 1e9, and for a price that rounds to zero.
   */
  async quotePayForGas(destination: number, gasAmount: bigint): Promise<bigint> {
    const args = this.wasm.rkyv_serialize_quote_pay_for_gas(
      destination,
      gasAmount
    );
    const result = await this.rues.contractQuery(
      this.contractId,
      "quote_pay_for_gas",
      args
    );
    return this.wasm.rkyv_deserialize_u64(result);
  }

  /**
   * What a dispatch that asks for `gasLimit` gas pays, in LUX: the
   * destination's overhead is added to the gas.
   */
  async quoteGasPayment(destination: number, gasLimit: bigint): Promise<bigint> {
    const args = this.wasm.rkyv_serialize_quote_pay_for_gas(
      destination,
      gasLimit
    );
    const result = await this.rues.contractQuery(
      this.contractId,
      "quote_gas_payment",
      args
    );
    return this.wasm.rkyv_deserialize_u64(result);
  }

  /** The number of payment records, from dispatches and top-ups. */
  async gasPaymentCount(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "gas_payment_count",
      args
    );
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /** The contract's state version: 4 and up take `pay_for_gas`. */
  async stateVersion(): Promise<number> {
    const args = this.wasm.rkyv_serialize_unit();
    const result = await this.rues.contractQuery(
      this.contractId,
      "state_version",
      args
    );
    return this.wasm.rkyv_deserialize_u32(result);
  }

  /**
   * The call that pays for `gasAmount` more gas for the message
   * `messageId` to `destination`. Sign it as the target of a Moonlight
   * transaction whose deposit is `deposit`, the most to pay: the IGP keeps
   * its price when the transaction runs and sends the rest back to the
   * signer. Quote first with `quotePayForGas`, and leave a margin if the
   * price may change before the transaction is included. Nothing checks
   * that the message exists: a payment for a wrong ID or destination is
   * lost.
   */
  payForGasCall(
    messageId: Uint8Array,
    destination: number,
    gasAmount: bigint,
    deposit: bigint
  ): PayForGasCall {
    return {
      contractId: this.contractId,
      fnName: "pay_for_gas",
      args: this.wasm.rkyv_serialize_pay_for_gas(
        messageId,
        destination,
        gasAmount,
        deposit
      ),
      deposit,
    };
  }
}
