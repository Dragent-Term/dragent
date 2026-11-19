/* tslint:disable */
/* eslint-disable */

/**
 * Called once when the wasm module loads. Boots the warpui App on the canvas
 * and adds a window backed by the app's real `RootView`. warpui's wasm
 * windowing attaches the canvas to the element `#wasm-container` (falling back
 * to `<body>`), so the host page only needs that mount point.
 */
export function start(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly start: () => void;
    readonly wasm_bindgen__convert__closures_____invoke__hd2a849880f08b3a9: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h72d8d1071b7daac3: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_3: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h147978772fb923af: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_5: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_6: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_7: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h147978772fb923af_8: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_9: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_10: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h37852898bcb0d4df_11: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures________invoke__hc27292c51c065eb1: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h46c68d5f6ac1f587: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h48ef63aa6bd0af76: (a: number, b: number) => void;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
