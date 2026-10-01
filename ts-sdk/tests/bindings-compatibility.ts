import type { WasmBindings } from "../src/mailbox.js";
import * as generated from "../../wasm-bindings/pkg/hyperlane_dusk_wasm_bindings.js";

// Check the actual generated API, including bigint and optional-return types.
const bindings: WasmBindings = generated;
void bindings;
