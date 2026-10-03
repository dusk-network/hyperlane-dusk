import type { WasmBindings } from "../src/mailbox.js";
import * as generated from "../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js";

// Check the actual generated API, including bigint and optional-return types.
const bindings: WasmBindings = generated;
void bindings;

import type { DomainGasConfig } from "../src/types.js";
const gasConfig: DomainGasConfig = { gasOverhead: 1n, tokenExchangeRate: 10000000000n, gasPrice: 1n };
void gasConfig;
// @ts-expect-error u64 gas fields require bigint, including values below 2^53.
const lossyGasConfig: DomainGasConfig = { gasOverhead: 1, tokenExchangeRate: 10000000000n, gasPrice: 1n };
void lossyGasConfig;
