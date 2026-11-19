// Loads the packaged terminal WASM exactly as the phone does and checks that
// snapshots show whichever screen the program is drawing on. Full-screen
// agents live on the alternate screen; if the snapshot read the primary screen
// instead, the phone would show stale shell output while the agent ran.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { TERMINAL_WASM_BASE64, TERMINAL_WASM_GLUE } from '../src/terminal/runtime/wasmAsset.generated';

interface Cell { text: string; flags: number; fg: { kind: string; value?: unknown }; bg: { kind: string; value?: unknown } }
interface Snapshot {
  rows: number;
  cols: number;
  cursor_row: number;
  cursor_col: number;
  cursor_visible: boolean;
  alt_screen: boolean;
  scrollback_len: number;
  scroll_offset: number;
  cells: Cell[][];
}
interface WasmTerminal {
  write(data: Uint8Array): void;
  snapshot(): string;
  viewport_snapshot(offset: number): string;
  free(): void;
}
interface WasmModule {
  Terminal: new (rows: number, cols: number) => WasmTerminal;
  initSync(options: { module: Uint8Array }): void;
}

const wasmModule: Promise<WasmModule> = import(`data:text/javascript;base64,${Buffer.from(TERMINAL_WASM_GLUE).toString('base64')}`).then((module: WasmModule) => {
  module.initSync({ module: Uint8Array.from(Buffer.from(TERMINAL_WASM_BASE64, 'base64')) });
  return module;
});

const encoder = new TextEncoder();
const write = (terminal: WasmTerminal, text: string) => terminal.write(encoder.encode(text));
const read = (json: string): Snapshot => JSON.parse(json) as Snapshot;
const rowText = (snapshot: Snapshot, row: number) => (snapshot.cells[row] ?? []).map((cell) => cell.text).join('').trimEnd();
const screenText = (snapshot: Snapshot) => snapshot.cells.map((_, row) => rowText(snapshot, row)).join('\n');

test('snapshot shows the alternate screen while it is active', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(4, 20);
  try {
    write(terminal, 'HELLO');
    write(terminal, '\x1b[?1049hALT');
    const alt = read(terminal.snapshot());
    assert.equal(alt.alt_screen, true);
    assert.equal(rowText(alt, 0), 'ALT');
    assert.doesNotMatch(screenText(alt), /HELLO/);
    assert.equal(alt.cells.length, 4);
    assert.deepEqual([alt.cursor_row, alt.cursor_col], [0, 3]);
    assert.equal(alt.scrollback_len, 0);

    write(terminal, '\x1b[H\x1b[31mRED');
    const red = read(terminal.snapshot());
    assert.equal(rowText(red, 0), 'RED');
    for (const cell of (red.cells[0] ?? []).slice(0, 3)) {
      // SGR 31 is palette entry 1 (red).
      assert.deepEqual(cell.fg, { kind: 'indexed', value: 1 });
    }
  } finally {
    terminal.free();
  }
});

test('alternate screen ignores a scrollback offset', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(3, 20);
  try {
    for (let line = 0; line < 8; line += 1) write(terminal, `row ${line}\r\n`);
    write(terminal, '\x1b[?1049hAGENT');
    const scrolled = read(terminal.viewport_snapshot(4));
    assert.equal(scrolled.alt_screen, true);
    assert.equal(scrolled.scroll_offset, 0);
    assert.equal(scrolled.scrollback_len, 0);
    assert.equal(rowText(scrolled, 0), 'AGENT');
  } finally {
    terminal.free();
  }
});

test('leaving the alternate screen restores the primary screen and its scrollback', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(3, 20);
  try {
    for (let line = 0; line < 6; line += 1) write(terminal, `row ${line}\r\n`);
    write(terminal, 'HELLO');
    const before = read(terminal.snapshot());

    write(terminal, '\x1b[?1049hALT\x1b[H\x1b[31mRED\x1b[?1049l');
    const after = read(terminal.snapshot());
    assert.equal(after.alt_screen, false);
    assert.deepEqual(after.cells, before.cells);
    assert.equal(after.scrollback_len, before.scrollback_len);
    assert.ok(after.scrollback_len > 0);
    assert.match(screenText(after), /HELLO/);
    assert.doesNotMatch(screenText(after), /ALT|RED/);
  } finally {
    terminal.free();
  }
});

test('primary viewport offset still reads scrollback', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(3, 20);
  try {
    for (let line = 0; line < 8; line += 1) write(terminal, `row ${line}\r\n`);
    write(terminal, 'HELLO');
    const live = read(terminal.snapshot());
    assert.equal(rowText(live, 2), 'HELLO');
    assert.equal(live.scrollback_len, 6);

    const scrolled = read(terminal.viewport_snapshot(2));
    assert.equal(scrolled.scroll_offset, 2);
    assert.equal(scrolled.cursor_visible, false);
    assert.equal(rowText(scrolled, 0), 'row 4');
    assert.equal(rowText(scrolled, 2), 'row 6');

    const clamped = read(terminal.viewport_snapshot(1000));
    assert.equal(clamped.scroll_offset, 6);
    assert.equal(rowText(clamped, 0), 'row 0');
  } finally {
    terminal.free();
  }
});
