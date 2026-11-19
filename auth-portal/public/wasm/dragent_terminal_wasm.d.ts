/* tslint:disable */
/* eslint-disable */

export class Terminal {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Mount the terminal surface. React supplies only the host element;
     * canvas creation, keyboard handling, and painting stay in WASM.
     */
    mount(container: HTMLElement, can_type: boolean): void;
    constructor(rows: number, cols: number);
    resize(rows: number, cols: number): void;
    /**
     * Rows `start..start + count` of the active screen, one JSON array per
     * line, joined by newlines; a row past the end is `[]`. Each array holds
     * style runs: `[col, fg, bg, flags, text]`, where every character of
     * `text` fills one cell from `col` on, or `[col, fg, bg, flags, text,
     * width]` for a wide character or combining cluster placed on its own.
     * Colours are -1 (default), 0-255 (palette) or 0x1000000 + 0xRRGGBB;
     * flags are the SGR bits of `snapshot()`. Trailing blanks are left out.
     * On the primary screen `start` is an absolute line: 0 is the oldest
     * history line and `scrollback_len` the top of the screen. On the
     * alternate screen it is a screen row.
     */
    rows_compact(start: number, count: number): string;
    /**
     * Apply the host's non-PTY presentation state.
     */
    set_surface_state(json: string): void;
    snapshot(): string;
    /**
     * Dimensions, cursor, modes and history position without any cells.
     */
    view_info(): string;
    /**
     * Read a bounded window into primary scrollback without changing the PTY.
     * While a program is on the alternate screen there is no history to
     * scroll: the snapshot is always that screen as it stands.
     */
    viewport_snapshot(offset: number): string;
    write(bytes: Uint8Array): void;
}

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_terminal_free: (a: number, b: number) => void;
    readonly terminal_mount: (a: number, b: any, c: number) => [number, number];
    readonly terminal_new: (a: number, b: number) => number;
    readonly terminal_resize: (a: number, b: number, c: number) => void;
    readonly terminal_rows_compact: (a: number, b: number, c: number) => [number, number];
    readonly terminal_set_surface_state: (a: number, b: number, c: number) => [number, number];
    readonly terminal_snapshot: (a: number) => [number, number, number, number];
    readonly terminal_view_info: (a: number) => [number, number, number, number];
    readonly terminal_viewport_snapshot: (a: number, b: number) => [number, number, number, number];
    readonly terminal_write: (a: number, b: number, c: number) => void;
    readonly wasm_bindgen__convert__closures_____invoke__h5483210f658f8daf: (a: number, b: number, c: any) => void;
    readonly __wbindgen_exn_store: (a: number) => void;
    readonly __externref_table_alloc: () => number;
    readonly __wbindgen_externrefs: WebAssembly.Table;
    readonly __wbindgen_malloc: (a: number, b: number) => number;
    readonly __wbindgen_realloc: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_destroy_closure: (a: number, b: number) => void;
    readonly __externref_table_dealloc: (a: number) => void;
    readonly __wbindgen_free: (a: number, b: number, c: number) => void;
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
