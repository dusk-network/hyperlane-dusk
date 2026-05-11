// SPDX-License-Identifier: MIT OR Apache-2.0
//
// TypeScript type definitions for Hyperlane on Dusk.

/** A 32-byte hex string (without 0x prefix). */
export type HexBytes32 = string;

/** A variable-length hex string (without 0x prefix). */
export type HexString = string;

/** Decoded Hyperlane message. */
export interface DecodedMessage {
  version: number;
  nonce: number;
  origin: number;
  sender: HexBytes32;
  destination: number;
  recipient: HexBytes32;
  body: HexString;
}

/** Decoded token message. */
export interface DecodedTokenMessage {
  recipient: HexBytes32;
  amount: number;
  metadata: HexString;
}

/** Gas configuration for a remote domain. */
export interface DomainGasConfig {
  gasOverhead: number;
  tokenExchangeRate: number;
  gasPrice: number;
}

/** RUES contract query request. */
export interface ContractQuery {
  contractId: Uint8Array;
  method: string;
  args: Uint8Array;
}

/** Mailbox state. */
export interface MailboxState {
  localDomain: number;
  nonce: number;
  latestDispatchedId: HexBytes32;
  defaultIsm: HexBytes32;
  defaultHook: HexBytes32;
  requiredHook: HexBytes32;
}
