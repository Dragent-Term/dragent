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
    readonly wasm_bindgen__convert__closures_____invoke__had8659ed34d20bfd: (a: number, b: number, c: any) => [number, number];
    readonly wasm_bindgen__convert__closures_____invoke__h0c06058a576d4d81: (a: number, b: number, c: any, d: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hc5bdc290124a0dcc: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h0543cad9071995b3: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h6ec736a1a1580524: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h7f9f0e74aba3ce86: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hcf183db98f344669: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h6f3f0237bc202465: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h8b4fb50f3bc3d2f6: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hb8323b561e45c93a: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__hbc686af04b593d6f: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h864ba4bfea3bba3d: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures________invoke__ha28d075d6a558839: (a: number, b: number, c: any) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h0672698c9ccf2358: (a: number, b: number) => number;
    readonly wasm_bindgen__convert__closures_____invoke__hd9326c540988debc: (a: number, b: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h30a68d20b9309395: (a: number, b: number) => void;
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
