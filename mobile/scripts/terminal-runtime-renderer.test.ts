import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import vm from 'node:vm';
import { TERMINAL_BROWSER_SOURCE, TERMINAL_WASM_BASE64, TERMINAL_WASM_GLUE } from '../src/terminal/runtime/wasmAsset.generated';
import { terminalDocument } from '../src/terminal/runtime/document';
import { RUNTIME_LIMITS, RAW_RESYNC_MESSAGE, type RuntimePacket } from '../src/terminal/runtime/types';
import type { TerminalPalette } from '../src/theme/types';

interface Cell { text: string; flags: number; fg: { kind: string; value?: unknown }; bg: { kind: string; value?: unknown } }
interface WasmTerminal {
  write(data: Uint8Array): void;
  resize(rows: number, cols: number): void;
  snapshot(): string;
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
const palette: TerminalPalette = {
  background: '#101010', foreground: '#eeeeee', cursor: '#abcdef',
  ansiNormal: ['#000000','#cc0000','#00cc00','#cccc00','#0000cc','#cc00cc','#00cccc','#cccccc'],
  ansiBright: ['#555555','#ff0000','#00ff00','#ffff00','#0000ff','#ff00ff','#00ffff','#ffffff'],
};
type Listener = (event: Record<string, unknown>) => void;
class Element {
  listeners = new Map<string, Listener[]>();
  value = '';
  textContent = '';
  disabled = true;
  style: Record<string, unknown> = {};
  focusCount = 0;
  focused = false;
  addEventListener(type: string, listener: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }
  dispatch(type: string, event: Record<string, unknown> = {}) {
    for (const listener of this.listeners.get(type) ?? []) listener(event);
  }
  focus() { this.focusCount++; this.focused = true; }
  blur() { this.focused = false; }
}
/**
 * A WebGL2 context that records what the renderer asks of it: uploads to
 * instance buffers, uniforms by name, draws. `fail` makes the named call throw.
 */
function fakeWebGl() {
  const calls: Array<{ name: string; args: unknown[] }> = [];
  const uniforms = new Map<string, number[]>();
  const fail = new Set<string>();
  let lost = false;
  const names = ['u_view', 'u_cols', 'u_ring', 'u_first', 'u_visible', 'u_top', 'u_sub', 'u_rowPx', 'u_absolute', 'u_bands', 'u_rule', 'u_atlas'];
  const own: Record<string, unknown> = {
    getShaderParameter: () => true,
    getProgramParameter: (_program: unknown, which: string) => which === 'ACTIVE_UNIFORMS' ? names.length : true,
    getActiveUniform: (_program: unknown, index: number) => ({ name: names[index] }),
    getUniformLocation: (_program: unknown, name: string) => name,
    getParameter: () => 4096,
    getExtension: () => null,
    isContextLost: () => lost,
  };
  const record = (name: string) => (...args: unknown[]) => {
    if (fail.has(name)) throw new Error(`${name} failed`);
    calls.push({ name, args });
    if (/^uniform[1-4][fi]$/u.test(name)) uniforms.set(args[0] as string, args.slice(1) as number[]);
    return {};
  };
  const gl = new Proxy(own, {
    // Constants are their own names, so recorded calls read plainly.
    get: (target, property: string) => property in target ? target[property] : /^[A-Z0-9_]+$/u.test(property) ? property : record(property),
  });
  /** Instance-buffer uploads since `from`, in ring slots (each `cols` instances of 6 words). */
  const uploadedSlots = (from: number, cols: number) => calls.slice(from).filter((call) => call.name === 'bufferSubData' && call.args.length === 5)
    .reduce((sum, call) => sum + (call.args[4] as number) / (cols * 6), 0) / 3;
  return { gl, calls, uniforms, fail, uploadedSlots, setLost: (value: boolean) => { lost = value; } };
}
async function renderer(options: { gpu?: ReturnType<typeof fakeWebGl>; renderer?: string } = {}) {
  const { Terminal } = await wasmModule;
  const texts: Array<{ text: string; x: number; y: number; color: string; font: string; alpha: number }> = [];
  const strokes: number[][] = [];
  const fills: Array<{ color: string; x: number; y: number; width: number; height: number }> = [];
  const blits: Array<{ sy: number; sh: number; dy: number }> = [];
  const context = {
    fillStyle: '', font: '', globalAlpha: 1,
    setTransform() {},
    measureText(text: string) { return { width: [...text].length * (this.font.includes('bold') ? 15 : 10) }; },
    fillText(text: string, x: number, y: number) { texts.push({ text, x, y, color: this.fillStyle, font: this.font, alpha: this.globalAlpha }); },
    fillRect(x: number, y: number, width: number, height: number) { fills.push({ color: this.fillStyle, x, y, width, height }); },
    strokeRect(...args: number[]) { strokes.push(args); },
    clearRect() {},
    // Atlas glyphs: one white, fully inked pixel each.
    getImageData(_x: number, _y: number, width: number, height: number) { const data = new Uint8ClampedArray(width * height * 4); data.fill(255, 0, 4); return { data }; },
    save() {}, restore() {}, beginPath() {}, rect() {}, clip() {},
    drawImage(_source: unknown, sx: number, sy: number, sw: number, sh: number, dx: number, dy: number) { blits.push({ sy, sh, dy }); },
  };
  // Row pictures live on offscreen canvases: each gets its own drawing state
  // but records into the same lists (and fails with the main context).
  // A canvas asked for WebGL2 gets the fake one when the test provides it.
  const created: Array<Element & { width: number; height: number }> = [];
  const createElement = () => {
    const own = Object.create(context) as typeof context;
    const element = Object.assign(new Element(), { width: 0, height: 0, attributes: {} as Record<string, string>,
      getContext: (kind: string) => kind === 'webgl2' ? options.gpu?.gl ?? null : own,
      setAttribute(name: string, value: string) { this.attributes[name] = value; } });
    created.push(element);
    return element;
  };
  const attributes: Record<string, string> = {};
  const canvas = Object.assign(new Element(), { clientWidth: 800, clientHeight: 480, width: 0, height: 0, getContext: () => context, getBoundingClientRect: () => ({ left: 0, top: 0 }),
    after() {}, setAttribute(name: string, value: string) { attributes[name] = value; } });
  const keys = new Element(), readable = new Element(), status = new Element(), latest = new Element(), document = new Element(), window = new Element();
  const sel = Object.assign(new Element(), { innerHTML: '' }), menu = new Element(), menuCopy = new Element(), menuOpen = new Element();
  const elements = { screen: canvas, keys, readable, status, latest, sel, menu, 'menu-copy': menuCopy, 'menu-open': menuOpen };
  const messages: Array<Record<string, unknown>> = [];
  const parent = { postMessage(raw: string) { messages.push(JSON.parse(raw) as Record<string, unknown>); } };
  const frames: Array<(now: number) => void> = [];
  let clock = 1_000;
  const step = () => { clock += 16; frames.shift()?.(clock); };
  const drain = () => { while (frames.length > 0) step(); };
  let observed = false, released = 0, allocated = 0;
  class TrackedTerminal extends Terminal {
    constructor(rows: number, cols: number) { super(rows, cols); allocated++; }
    free() { released++; super.free(); }
  }
  vm.runInNewContext(`${TERMINAL_BROWSER_SOURCE};browserRuntime(Terminal,'renderer-test',limits,options);`, {
    Terminal: TrackedTerminal, limits: RUNTIME_LIMITS, options: { renderer: options.renderer },
    document: Object.assign(document, { getElementById: (id: keyof typeof elements) => elements[id], createElement }),
    window: Object.assign(window, { parent, devicePixelRatio: 1 }),
    TextEncoder, Uint8Array, atob, setTimeout, clearTimeout,
    requestAnimationFrame: (callback: (now: number) => void) => { frames.push(callback); },
    ResizeObserver: class { observe() { observed = true; } disconnect() { observed = false; } },
  });
  let seq = 0;
  const packet = (overrides: Partial<RuntimePacket['frame']> = {}, environment = {}) => {
    const frame: RuntimePacket = {
      v: 1, id: 'renderer-test', type: 'state', epoch: overrides.epoch ?? 1, seq: ++seq,
      frame: { epoch: 1, revision: seq, presentation: 'raw', reason: null, rawReset: seq === 1, rawChunks: [], lastRawSeq: 0, rows: 24, cols: 80, grid: null, ...overrides },
      environment: { connected: true, canWrite: true, canResize: true, holdFit: false, palette, ...environment },
    };
    window.dispatch('message', { source: parent, data: JSON.stringify(frame) });
    drain();
    return frame;
  };
  const inputs = () => messages.filter((item) => item.type === 'input').map((item) => item.text as string);
  const blurPacket = (overrides = {}) => window.dispatch('message', { source: parent, data: JSON.stringify({ v: 1, id: 'renderer-test', type: 'blur', ...overrides }) });
  return { packet, texts, strokes, fills, blits, now: () => clock, messages, canvas, keys, readable, status, latest, context, sel, menu, menuCopy, menuOpen, drain, step, frames, inputs, blurPacket, created,
    drawnWith: () => attributes['data-renderer'],
    pagehide: () => window.dispatch('pagehide'), resources: () => ({ allocated, released, observed }) };
}
const encoded = (text: string) => Buffer.from(text).toString('base64');

test('packaged desktop WASM parses ANSI/truecolour, Unicode and alternate-screen state', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(4, 20);
  try {
    terminal.write(new TextEncoder().encode('\x1b[31mR\x1b[0m界e\u0301\x1b[38;2;10;20;30mT'));
    const grid = JSON.parse(terminal.snapshot()) as { cells: Cell[][]; cursor_col: number };
    assert.deepEqual(grid.cells[0]?.[0]?.fg, { kind: 'indexed', value: 1 });
    assert.equal(grid.cells[0]?.[1]?.text, '界');
    assert.equal(grid.cells[0]?.[3]?.text, 'e\u0301');
    assert.deepEqual(grid.cells[0]?.[4]?.fg, { kind: 'rgb', value: [10,20,30] });
    assert.equal(grid.cursor_col, 5);
    terminal.write(new TextEncoder().encode('\x1b[?1049hALT'));
    assert.equal(JSON.parse(terminal.snapshot()).alt_screen, true);
    terminal.write(new TextEncoder().encode('\x1b[?1049l'));
    assert.equal(JSON.parse(terminal.snapshot()).alt_screen, false);
  } finally { terminal.free(); }
});

test('raw renderer paints actual WASM colours, inverse and concealed text without exposing hidden output', async () => {
  const h = await renderer();
  h.packet({ rawChunks: [{ seq: 1, data: encoded('\x1b[31mR\x1b[0m\x1b[38;2;10;20;30mT\x1b[0m\x1b[7mI\x1b[0m\x1b[8mSECRET') }], lastRawSeq: 1 });
  assert.equal(h.texts.find((item) => item.text === 'R')?.color, palette.ansiNormal[1]);
  assert.equal(h.texts.find((item) => item.text === 'T')?.color, 'rgb(10,20,30)');
  assert.equal(h.texts.find((item) => item.text === 'I')?.color, palette.background);
  assert.ok(!h.readable.textContent.includes('SECRET'));
  assert.ok(!h.texts.some((item) => /[SECRET]/u.test(item.text) && item.x >= 30));
  h.pagehide();
  assert.equal(h.resources().allocated, h.resources().released);
});

test('host alternate grid uses emulator columns for wide/combining text and keeps original RGBA/styles', async () => {
  const h = await renderer();
  h.packet({ presentation: 'alternate', grid: {
    rows: 1, columns: 10, cursorRow: 0, cursorColumn: 5, cursorVisible: true, truncated: false,
    lines: [{ index: 0, truncated: false, spans: [
      { text: 'A界e\u0301Z', flags: 1, fg: [30,40,50,128], bg: null },
      { text: 'HIDDEN', flags: 128, fg: null, bg: null },
    ] }],
  } });
  assert.equal(h.texts.find((item) => item.text === '界')?.x, 10);
  assert.equal(h.texts.find((item) => item.text === 'e\u0301')?.x, 30);
  assert.equal(h.texts.find((item) => item.text === 'Z')?.x, 40);
  assert.equal(h.texts.find((item) => item.text === 'Z')?.color, 'rgba(30,40,50,0.5019607843137255)');
  assert.ok(h.texts.find((item) => item.text === 'Z')?.font.includes('bold'));
  assert.equal(h.strokes.at(-1)?.[0], 50.5);
  assert.ok(!h.readable.textContent.includes('HIDDEN'));
  h.pagehide();
  assert.equal(h.resources().allocated, h.resources().released);
});

test('resync clears stale pixels/text and disables keyboard input, with explicit recovery copy', async () => {
  const h = await renderer();
  h.packet({ rawChunks: [{ seq: 1, data: encoded('old output') }], lastRawSeq: 1 });
  assert.match(h.readable.textContent, /old output/u);
  h.packet({ presentation: 'resync', reason: RAW_RESYNC_MESSAGE, rawReset: false });
  assert.equal(h.status.textContent, RAW_RESYNC_MESSAGE);
  assert.equal(h.status.style.display, 'flex');
  assert.equal(h.readable.textContent, '');
  assert.equal(h.keys.disabled, true);
  const count = h.messages.filter((item) => item.type === 'input').length;
  h.keys.dispatch('keydown', { key: 'Enter', preventDefault() {} });
  assert.equal(h.messages.filter((item) => item.type === 'input').length, count);
  h.pagehide();
});

test('renderer reissues a viewport after a resize grant and preserves IME input once per commit', async () => {
  const h = await renderer();
  h.packet();
  const resizeCount = () => h.messages.filter((item) => item.type === 'resize').length;
  assert.equal(resizeCount(), 1);
  h.packet({}, { canResize: false });
  h.packet({}, { canResize: true });
  assert.equal(resizeCount(), 2, 'same geometry is re-sent after permission is granted again');
  h.keys.dispatch('compositionstart');
  h.keys.value = '你好'; h.keys.dispatch('input');
  assert.equal(h.messages.filter((item) => item.type === 'input').length, 0);
  h.keys.dispatch('compositionend'); h.keys.dispatch('input');
  assert.deepEqual(h.messages.filter((item) => item.type === 'input').map((item) => item.text), ['你好']);
  h.canvas.dispatch('keydown', { key: 'Enter', preventDefault() {} });
  assert.equal(h.keys.focusCount, 1);
  h.pagehide();
  assert.equal(h.resources().observed, false);
});


test('host presentation controls remain inert and paint faults request a renderer restart', async () => {
  const h = await renderer();
  h.packet({ presentation: 'alternate', grid: {
    rows: 1, columns: 10, cursorRow: 0, cursorColumn: 0, cursorVisible: false, truncated: false,
    lines: [{ index: 0, truncated: false, spans: [{ text: 'A\x1b[2JB', flags: 0, fg: null, bg: null }] }],
  } });
  // Text whose font advance matches the grid is drawn as one run from its first column.
  assert.equal(h.texts.find((item) => item.text.startsWith('A'))?.x, 0);
  assert.match(h.readable.textContent, /A \[2JB/u);
  h.context.fillText = () => { throw new Error('canvas stopped'); };
  h.packet({ rawChunks: [{ seq: 1, data: encoded('paint') }], lastRawSeq: 1 });
  assert.equal(h.messages.at(-1)?.type, 'error', 'async paint faults must not leave an acknowledged frozen surface');
  h.pagehide();
});


test('an older renderer epoch cannot replace a newer terminal even with a larger packet sequence', async () => {
  const h = await renderer();
  h.packet({ epoch: 2, rawChunks: [{ seq: 1, data: encoded('new epoch') }], lastRawSeq: 1 });
  h.packet({ epoch: 1, rawChunks: [{ seq: 1, data: encoded('obsolete') }], lastRawSeq: 1 });
  assert.match(h.readable.textContent, /new epoch/u);
  assert.ok(!h.readable.textContent.includes('obsolete'));
  h.pagehide();
});

test('wheel and touch navigate real primary scrollback without typing or focusing', async () => {
  const r = await renderer();
  const output = Array.from({ length: 60 }, (_, i) => `row-${i}\r\n`).join('');
  r.packet({ rawChunks: [{ seq: 1, data: encoded(output) }], lastRawSeq: 1 });
  assert.match(r.readable.textContent, /row-59/);
  r.canvas.dispatch('wheel', { deltaY: -120, preventDefault() {} });
  r.packet({ lastRawSeq: 1 });
  assert.doesNotMatch(r.readable.textContent, /row-59/);
  r.canvas.dispatch('touchstart', { touches: [{ clientY: 20 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientY: 100 }], preventDefault() {} });
  r.canvas.dispatch('touchend');
  r.canvas.dispatch('click');
  assert.equal(r.keys.focusCount, 0);
  assert.equal(r.messages.filter((m) => m.type === 'input').length, 0);
});

// ── Scroll gestures ──────────────────────────────────────────────────────────

type Renderer = Awaited<ReturnType<typeof renderer>>;
const noop = () => {};
/** One finger down, each listed point in turn, then lifted. Frames do not run until `drain`. */
function swipe(r: Renderer, x: number, ys: number[]) {
  r.canvas.dispatch('touchstart', { touches: [{ clientX: x, clientY: ys[0] }] });
  for (const y of ys.slice(1)) r.canvas.dispatch('touchmove', { touches: [{ clientX: x, clientY: y }], preventDefault: noop });
  r.canvas.dispatch('touchend', { touches: [] });
  r.drain();
}
const hostGrid = (rows = 24, columns = 80) => ({
  rows, columns, cursorRow: 0, cursorColumn: 0, cursorVisible: true, truncated: false,
  lines: [{ index: 0, truncated: false, spans: [{ text: 'SCREEN', flags: 0, fg: null, bg: null }] }],
});
const modes = (overrides = {}) => ({ alternateScreen: true, mouseReporting: false, sgrMouse: false, bracketedPaste: false, ...overrides });
const host = (r: Renderer, screenModes: ReturnType<typeof modes>, environment = {}, grid = hostGrid()) =>
  r.packet({ presentation: 'alternate', screenModes, grid }, environment);
const history = (r: Renderer) => r.packet({ rawChunks: [{ seq: 1, data: encoded(Array.from({ length: 60 }, (_, i) => `row-${i}\r\n`).join('')) }], lastRawSeq: 1 });
const topRow = (r: Renderer) => r.readable.textContent.split('\n')[0]?.trim();
const count = (r: Renderer, sequence: string) => r.inputs().join('').split(sequence).length - 1;

test('primary raw screen scrolls local history by whole lines and sends no bytes', async () => {
  const r = await renderer();
  history(r);
  assert.equal(topRow(r), 'row-37');
  swipe(r, 100, [100, 200]); // 100 px at a 20 px row
  assert.equal(topRow(r), 'row-32');
  swipe(r, 100, [200, 100]);
  assert.equal(topRow(r), 'row-37');
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('primary raw screen sends no bytes even if the program asked for mouse reports', async () => {
  const r = await renderer();
  r.packet({ rawChunks: [{ seq: 1, data: encoded(`${Array.from({ length: 60 }, (_, i) => `row-${i}\r\n`).join('')}\x1b[?1000h\x1b[?1006h`) }], lastRawSeq: 1 });
  swipe(r, 100, [100, 200]);
  assert.equal(topRow(r), 'row-32');
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('primary host grid: a swipe sends no bytes, fakes no scrolling and offers no Latest', async () => {
  const r = await renderer();
  // The bug: this grid is the shell after a stream gap, and PageUp would be typed into it.
  host(r, modes({ alternateScreen: false, mouseReporting: true, sgrMouse: true }));
  const before = r.readable.textContent;
  swipe(r, 100, [100, 580]);
  r.canvas.dispatch('wheel', { deltaY: -480, clientX: 10, clientY: 10, preventDefault: noop });
  r.drain();
  assert.deepEqual(r.inputs(), []);
  assert.equal(r.readable.textContent, before);
  assert.equal(r.latest.style.display, 'none');
  r.pagehide();
});

test('a host grid without reported modes is treated as the primary screen', async () => {
  const r = await renderer();
  r.packet({ presentation: 'alternate', grid: hostGrid() });
  swipe(r, 100, [100, 580]);
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('alt screen with SGR mouse sends wheel reports at the touched cell, up and down', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  // x 125 is column 13 at 10 px per cell; y 45 is row 3 at 20 px per row. Finger down = older.
  swipe(r, 125, [45, 65]);
  assert.deepEqual(r.inputs(), ['\x1b[<64;13;3M']);
  swipe(r, 125, [45, 25]);
  assert.deepEqual(r.inputs(), ['\x1b[<64;13;3M', '\x1b[<65;13;3M']);
  assert.equal(count(r, '\x1b[5~'), 0, 'PageUp is not the signal a mouse program scrolls from');
  r.pagehide();
});

test('alt screen with legacy mouse sends the exact legacy wheel form', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true }));
  swipe(r, 125, [45, 65]);
  // 32+64, 32+13, 32+3
  assert.deepEqual(r.inputs(), [`\x1b[M${String.fromCharCode(96, 45, 35)}`]);
  swipe(r, 125, [45, 25]);
  assert.equal(r.inputs()[1], `\x1b[M${String.fromCharCode(97, 45, 35)}`);
  r.pagehide();
});

test('legacy wheel reports stop at the last encodable position, and SGR has no such limit', async () => {
  const wide = hostGrid(24, 200);
  const r = await renderer();
  // 200 columns on 800 px scale the grid to 0.4: 4 px cells, 8 px rows,
  // top-anchored (no space above the grid). So x 374 is column 94, x 378
  // column 95, y 20 row 3, and 16 px of travel is 2 lines.
  host(r, modes({ mouseReporting: true }), {}, wide);
  swipe(r, 374, [20, 36]);
  assert.deepEqual(r.inputs(), [`\x1b[M${String.fromCharCode(96, 126, 35)}`.repeat(2)]);
  swipe(r, 378, [20, 36]);
  assert.equal(r.inputs().length, 1, 'position 95 would need a byte above 0x7e');
  host(r, modes({ mouseReporting: true, sgrMouse: true }), {}, wide);
  swipe(r, 378, [20, 36]);
  assert.equal(r.inputs().at(-1), '\x1b[<64;95;3M'.repeat(2));
  r.pagehide();
});

test('the wheel report stays on the cell where the swipe began', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  swipe(r, 125, [45, 65, 85, 105]);
  assert.equal(r.inputs().join(''), '\x1b[<64;13;3M'.repeat(3));
  r.pagehide();
});

test('alt screen from the raw stream reads its modes from the emulator', async () => {
  const sgr = await renderer();
  sgr.packet({ rawChunks: [{ seq: 1, data: encoded('\x1b[?1049h\x1b[?1000h\x1b[?1006h') }], lastRawSeq: 1 });
  swipe(sgr, 125, [45, 65]);
  assert.deepEqual(sgr.inputs(), ['\x1b[<64;13;3M']);
  sgr.pagehide();
  const legacy = await renderer();
  legacy.packet({ rawChunks: [{ seq: 1, data: encoded('\x1b[?1049h\x1b[?1000h') }], lastRawSeq: 1 });
  swipe(legacy, 125, [45, 65]);
  assert.deepEqual(legacy.inputs(), [`\x1b[M${String.fromCharCode(96, 45, 35)}`]);
  legacy.pagehide();
  const pager = await renderer();
  pager.packet({ rawChunks: [{ seq: 1, data: encoded('\x1b[?1049h') }], lastRawSeq: 1 });
  swipe(pager, 125, [0, 480]);
  assert.deepEqual(pager.inputs(), ['\x1b[5~']);
  pager.pagehide();
});

test('alt screen without mouse reporting sends one page key per viewport of travel', async () => {
  const r = await renderer();
  host(r, modes()); // 24 rows at 20 px: a 480 px viewport
  swipe(r, 100, [0, 480]);
  assert.deepEqual(r.inputs(), ['\x1b[5~']);
  swipe(r, 100, [960, 0]);
  assert.equal(r.inputs().join(''), `\x1b[5~${'\x1b[6~'.repeat(2)}`);
  r.pagehide();
});

test('page travel under half a viewport carries to the next swipe; half or more counts on lift', async () => {
  const r = await renderer();
  host(r, modes());
  swipe(r, 100, [0, 200]);
  assert.deepEqual(r.inputs(), [], '200 of 480 px is not enough on its own');
  swipe(r, 100, [0, 40]);
  assert.deepEqual(r.inputs(), ['\x1b[5~'], 'the carried 200 px made 240, half a viewport');
  swipe(r, 100, [0, 200]);
  assert.equal(r.inputs().length, 1, 'that page used up the distance, so the count starts again');
  swipe(r, 100, [200, 0]);
  assert.equal(r.inputs().length, 1, 'turning round drops what was carried rather than cancelling against it');
  swipe(r, 100, [40, 0]);
  assert.equal(r.inputs().at(-1), '\x1b[6~', '200 + 40 the new way is half a viewport');
  r.pagehide();
});

test('a swipe of known distance sends exactly its count, bounded per frame, and nothing is lost', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 125, clientY: 0 }] });
  // One enormous move: 480 px at 20 px a row is exactly 24 wheel events.
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: 480 }], preventDefault: noop });
  r.canvas.dispatch('touchend', { touches: [] });
  const report = '\x1b[<64;13;1M';
  let seen = 0, most = 0, frames = 0;
  while (r.frames.length > 0) {
    r.step(); frames++;
    const now = count(r, report);
    most = Math.max(most, now - seen);
    seen = now;
  }
  assert.equal(seen, 24);
  assert.equal(most, 8);
  assert.ok(frames >= 3, 'the rest waited for later frames instead of bursting');
  r.pagehide();
});

test('sends at most once per frame, with everything the finger made since the last one', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 125, clientY: 0 }] });
  for (let frame = 1; frame <= 30; frame++) {
    // Two moves between frames: one row each, coalesced into one send.
    r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: frame * 40 - 20 }], preventDefault: noop });
    r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: frame * 40 }], preventDefault: noop });
    const before = r.inputs().length;
    r.step();
    assert.ok(r.inputs().length - before <= 1, 'one send per frame');
    assert.equal(r.inputs().at(-1), '\x1b[<64;13;1M'.repeat(2), 'no waiting for a timer: each frame sends what was travelled');
  }
  r.canvas.dispatch('touchend', { touches: [] });
  r.drain();
  assert.equal(count(r, '\x1b[<64;13;1M'), 60);
  r.pagehide();
});

test('distance under a line carries across moves and frames, and is not sent early', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 125, clientY: 0 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: 15 }], preventDefault: noop });
  r.drain();
  assert.deepEqual(r.inputs(), [], '15 px is under one 20 px row');
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: 30 }], preventDefault: noop });
  r.drain();
  assert.equal(count(r, '\x1b[<64;13;1M'), 1);
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: 40 }], preventDefault: noop });
  r.drain();
  assert.equal(count(r, '\x1b[<64;13;1M'), 2, 'the 10 px left over counted towards this one');
  r.canvas.dispatch('touchend', { touches: [] });
  r.pagehide();
});

test('the row height used for pacing is the drawn one, so a scaled grid paces by its real rows', async () => {
  const r = await renderer();
  // 48 rows in 480 px are drawn at half height: 10 px each.
  host(r, modes({ mouseReporting: true, sgrMouse: true }), {}, hostGrid(48, 80));
  swipe(r, 100, [0, 100]);
  assert.equal(r.inputs().join('').split('\x1b[<64').length - 1, 10);
  r.pagehide();
});

test('no write access or a resync means no bytes in either alt-screen mode', async () => {
  for (const screenModes of [modes(), modes({ mouseReporting: true, sgrMouse: true })]) {
    const readOnly = await renderer();
    host(readOnly, screenModes, { canWrite: false });
    swipe(readOnly, 100, [0, 960]);
    assert.deepEqual(readOnly.inputs(), []);
    readOnly.pagehide();
    const offline = await renderer();
    host(offline, screenModes, { connected: false });
    swipe(offline, 100, [0, 960]);
    assert.deepEqual(offline.inputs(), []);
    offline.pagehide();
    const blocked = await renderer();
    host(blocked, screenModes);
    blocked.packet({ presentation: 'resync', reason: RAW_RESYNC_MESSAGE, screenModes });
    swipe(blocked, 100, [0, 960]);
    assert.deepEqual(blocked.inputs(), []);
    blocked.pagehide();
  }
});

test('losing write access mid-swipe drops what was queued, and regaining it sends nothing stale', async () => {
  const r = await renderer();
  const wheel = modes({ mouseReporting: true, sgrMouse: true });
  host(r, wheel);
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 125, clientY: 0 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 125, clientY: 480 }], preventDefault: noop });
  r.step();
  const sent = r.inputs().length;
  assert.equal(sent, 1);
  host(r, wheel, { canWrite: false });
  host(r, wheel, { canWrite: true });
  r.drain();
  assert.equal(r.inputs().length, sent);
  r.pagehide();
});

test('travel made without write access does not count once it returns', async () => {
  const r = await renderer();
  host(r, modes(), { canWrite: false });
  swipe(r, 100, [0, 200]);
  host(r, modes(), { canWrite: true });
  swipe(r, 100, [0, 40]);
  assert.deepEqual(r.inputs(), [], 'only the last 40 px of travel counted');
  r.pagehide();
});

test('read-only still scrolls local history, since that sends nothing', async () => {
  const r = await renderer();
  r.packet({ rawChunks: [{ seq: 1, data: encoded(Array.from({ length: 60 }, (_, i) => `row-${i}\r\n`).join('')) }], lastRawSeq: 1 }, { canWrite: false });
  swipe(r, 100, [100, 200]);
  assert.equal(topRow(r), 'row-32');
  r.pagehide();
});

test('distance saved in a pager does not scroll the shell it returns to', async () => {
  const r = await renderer();
  r.packet({ rawChunks: [{ seq: 1, data: encoded(`${Array.from({ length: 60 }, (_, i) => `row-${i}\r\n`).join('')}\x1b[?1049h`) }], lastRawSeq: 1 });
  swipe(r, 100, [0, 200]);
  assert.deepEqual(r.inputs(), []);
  r.packet({ rawChunks: [{ seq: 2, data: encoded('\x1b[?1049l') }], lastRawSeq: 2 });
  const before = topRow(r);
  r.canvas.dispatch('wheel', { deltaY: -1, clientX: 0, clientY: 0, preventDefault: noop });
  r.drain();
  assert.equal(topRow(r), before);
  r.pagehide();
});

test('the first swipe after a mode change is not cut short', async () => {
  const r = await renderer();
  const wheel = modes({ mouseReporting: true, sgrMouse: true });
  host(r, modes({ alternateScreen: false }));
  host(r, wheel);
  swipe(r, 125, [0, 480]);
  assert.equal(count(r, '\x1b[<64;13;1M'), 24);
  r.pagehide();
});

test('a tap never brings up the keyboard (the composer types); a drag is never a tap', async () => {
  const r = await renderer();
  history(r);
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 50, clientY: 50 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 54, clientY: 54 }], preventDefault: noop });
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click');
  assert.equal(r.keys.focusCount, 0, 'a few pixels of jitter is still a tap, and a tap does not focus');
  swipe(r, 50, [50, 150]);
  r.canvas.dispatch('click');
  assert.equal(r.keys.focusCount, 0);
  // A sideways drag scrolls nothing and is still not a tap.
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 50, clientY: 50 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 150, clientY: 50 }], preventDefault: noop });
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click');
  assert.equal(r.keys.focusCount, 0);
  // Enter on a hardware keyboard still focuses the terminal's own input.
  r.canvas.dispatch('keydown', { key: 'Enter', preventDefault: noop });
  assert.equal(r.keys.focusCount, 1);
  r.pagehide();
});

test('a tap does not claim the touch, so the browser can still make it a click', async () => {
  const r = await renderer();
  history(r);
  let prevented = 0;
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 50, clientY: 50 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 52, clientY: 52 }], preventDefault: () => { prevented++; } });
  assert.equal(prevented, 0);
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 52, clientY: 90 }], preventDefault: () => { prevented++; } });
  assert.equal(prevented, 1);
  r.pagehide();
});

test('multi-touch does nothing: no bytes, no scrolling, no focus, and uncommitted travel is dropped', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  const one = (y: number) => ({ touches: [{ clientX: 100, clientY: y }], preventDefault: noop });
  const two = (y: number) => ({ touches: [{ clientX: 100, clientY: y }, { clientX: 300, clientY: y }], preventDefault: noop });
  r.canvas.dispatch('touchstart', one(0));
  r.canvas.dispatch('touchmove', one(200));
  r.canvas.dispatch('touchstart', two(200));
  r.canvas.dispatch('touchmove', two(400));
  r.canvas.dispatch('touchend', one(400));
  r.canvas.dispatch('touchmove', one(600)); // the finger left behind does not start a swipe
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click');
  r.drain();
  assert.deepEqual(r.inputs(), []);
  assert.equal(r.keys.focusCount, 0);
  r.canvas.dispatch('touchstart', one(10));
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click', { clientX: 100, clientY: 10 });
  assert.deepEqual(r.inputs(), ['\x1b[<0;11;1M\x1b[<0;11;1m'], 'a normal tap works again afterwards: a click at that cell');
  r.pagehide();
});

test('a cancelled touch drops its queued distance', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 100, clientY: 0 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 100, clientY: 480 }], preventDefault: noop });
  r.canvas.dispatch('touchcancel', { touches: [] });
  r.drain();
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('Latest appears once scrolled back, returns to the bottom, and takes no focus or bytes', async () => {
  const r = await renderer();
  history(r);
  assert.equal(r.latest.style.display, 'none');
  swipe(r, 100, [100, 200]);
  assert.equal(r.latest.style.display, 'block');
  r.latest.dispatch('click');
  r.drain();
  assert.equal(topRow(r), 'row-37');
  assert.equal(r.latest.style.display, 'none');
  assert.equal(r.keys.focusCount, 0);
  assert.deepEqual(r.inputs(), []);
  let kept = 0;
  r.latest.dispatch('mousedown', { preventDefault: () => { kept++; } });
  assert.equal(kept, 1, 'the press must not take focus from the input and drop the keyboard');
  r.pagehide();
});

test('Latest is hidden in the alternate screen and on a host grid', async () => {
  const raw = await renderer();
  history(raw);
  swipe(raw, 100, [100, 200]);
  assert.equal(raw.latest.style.display, 'block');
  raw.packet({ rawChunks: [{ seq: 2, data: encoded('\x1b[?1049h') }], lastRawSeq: 2 });
  assert.equal(raw.latest.style.display, 'none');
  raw.pagehide();
  const grid = await renderer();
  history(grid);
  swipe(grid, 100, [100, 200]);
  host(grid, modes({ alternateScreen: false }));
  assert.equal(grid.latest.style.display, 'none');
  grid.pagehide();
});

test('Latest is hidden during a resync, when its grid is not on screen', async () => {
  const r = await renderer();
  history(r);
  swipe(r, 100, [100, 200]);
  r.packet({ presentation: 'resync', reason: RAW_RESYNC_MESSAGE });
  assert.equal(r.latest.style.display, 'none');
  r.pagehide();
});

test('new output while scrolled back keeps the viewed text stable', async () => {
  const r = await renderer();
  history(r);
  swipe(r, 100, [100, 200]);
  const viewed = r.readable.textContent;
  assert.match(viewed, /^row-32/u);
  r.packet({ rawChunks: [{ seq: 2, data: encoded('more-1\r\nmore-2\r\nmore-3\r\n') }], lastRawSeq: 2 });
  assert.equal(r.readable.textContent, viewed);
  r.latest.dispatch('click');
  r.drain();
  assert.match(r.readable.textContent, /more-3/u);
  r.pagehide();
});

test('a trackpad pinch is not a scroll, and line-mode wheels count in lines', async () => {
  const r = await renderer();
  history(r);
  r.canvas.dispatch('wheel', { deltaY: -500, ctrlKey: true, clientX: 0, clientY: 0, preventDefault: noop });
  r.drain();
  assert.equal(topRow(r), 'row-37');
  r.canvas.dispatch('wheel', { deltaY: -3, deltaMode: 1, clientX: 0, clientY: 0, preventDefault: noop });
  r.drain();
  assert.equal(topRow(r), 'row-34');
  r.pagehide();
});

test('the blur message unfocuses the hidden input, and Enter focuses it again', async () => {
  const r = await renderer();
  history(r);
  const enter = () => r.canvas.dispatch('keydown', { key: 'Enter', preventDefault: noop });
  enter();
  assert.equal(r.keys.focused, true);
  r.blurPacket();
  assert.equal(r.keys.focused, false);
  enter();
  assert.equal(r.keys.focused, true);
  r.pagehide();
});

test('a blur message for another renderer or version is ignored', async () => {
  const r = await renderer();
  history(r);
  r.canvas.dispatch('keydown', { key: 'Enter', preventDefault: noop });
  r.blurPacket({ id: 'someone-else' });
  r.blurPacket({ v: 2 });
  assert.equal(r.keys.focused, true);
  r.pagehide();
});

test('the WebView and document leave vertical drags to the renderer', () => {
  const html = terminalDocument('gesture-test');
  assert.match(html, /#screen\{touch-action:none/u);
  assert.match(html, /#latest\{[^}]*touch-action:manipulation/u);
  assert.match(html, /#latest\{[^}]*z-index:1/u);
  assert.ok(html.indexOf('<canvas id="screen"') < html.indexOf('<button id="latest"'), 'the button sits above the canvas');
  assert.ok(html.indexOf('<canvas id="screen"') < html.indexOf('<div id="menu"'), 'the Copy menu sits above the canvas');
  const view = readFileSync(new URL('../src/terminal/runtime/WasmTerminalView.tsx', import.meta.url), 'utf8');
  assert.match(view, /scrollEnabled=\{false\}/u);
  assert.match(view, /bounces=\{false\}/u);
  assert.match(view, /overScrollMode="never"/u);
});

test('held fit posts no resize for a shorter frame, applies a narrower one, and rechecks on release', async () => {
  const h = await renderer();
  h.packet();
  const resizeCount = () => h.messages.filter((item) => item.type === 'resize').length;
  assert.equal(resizeCount(), 1);
  // Keyboard opens: the frame shortens but the PTY must not follow it.
  h.canvas.clientHeight = 200;
  h.packet({}, { holdFit: true });
  assert.equal(resizeCount(), 1);
  // A narrower canvas still reflows at once, even while held.
  h.canvas.clientWidth = 400;
  h.packet({}, { holdFit: true });
  assert.equal(resizeCount(), 2);
  assert.deepEqual(h.messages.filter((item) => item.type === 'resize').at(-1), { v: 1, id: 'renderer-test', epoch: 1, type: 'resize', rows: 10, cols: 40 });
  // Release rechecks: the held height change now posts once.
  h.canvas.clientWidth = 800;
  h.packet({}, { holdFit: true });
  h.packet({}, { holdFit: false });
  const resizes = h.messages.filter((item) => item.type === 'resize');
  assert.deepEqual(resizes.at(-1), { v: 1, id: 'renderer-test', epoch: 1, type: 'resize', rows: 10, cols: 80 });
  h.pagehide();
});

test('renderer posts avoidance metrics when rows, cursor, alt-screen or pitch change', async () => {
  const h = await renderer();
  h.packet();
  const metrics = () => h.messages.filter((item) => item.type === 'metrics');
  assert.deepEqual(metrics().at(-1), { v: 1, id: 'renderer-test', epoch: 1, type: 'metrics', rows: 24, cursorRow: 0, altScreen: false, rowPitch: 20, top: 0 });
  // Same grid again: no repeat message.
  h.packet({ lastRawSeq: 1 });
  assert.equal(metrics().length, 1);
  // Alt-screen grid: new rows, cursor row and drawn pitch/top (top-anchored).
  h.packet({ presentation: 'alternate', screenModes: { alternateScreen: true, mouseReporting: false, sgrMouse: false, bracketedPaste: false }, grid: {
    rows: 3, columns: 10, cursorRow: 2, cursorColumn: 1, cursorVisible: true, truncated: false,
    lines: [{ index: 2, truncated: false, spans: [{ text: 'ALT', flags: 0, fg: null, bg: null }] }],
  } });
  assert.deepEqual(metrics().at(-1), { v: 1, id: 'renderer-test', epoch: 1, type: 'metrics', rows: 3, cursorRow: 2, altScreen: true, rowPitch: 20, top: 0 });
  h.pagehide();
});

test('a host grid showing the primary screen reports altScreen false, so the lift anchors on the cursor', async () => {
  const h = await renderer();
  h.packet();
  // After a stream gap the host sends a grid for the primary screen too.
  h.packet({ presentation: 'alternate', screenModes: { alternateScreen: false, mouseReporting: false, sgrMouse: false, bracketedPaste: false }, grid: {
    rows: 3, columns: 10, cursorRow: 1, cursorColumn: 0, cursorVisible: true, truncated: false,
    lines: [{ index: 1, truncated: false, spans: [{ text: '$ ls', flags: 0, fg: null, bg: null }] }],
  } });
  const metrics = h.messages.filter((item) => item.type === 'metrics');
  assert.deepEqual(metrics.at(-1), { v: 1, id: 'renderer-test', epoch: 1, type: 'metrics', rows: 3, cursorRow: 1, altScreen: false, rowPitch: 20, top: 0 });
  h.pagehide();
});

test('WASM alternate-screen viewport never exposes primary history', async () => {
  const { Terminal } = await wasmModule;
  const terminal = new Terminal(3, 20) as WasmTerminal & { viewport_snapshot(offset: number): string };
  terminal.write(new TextEncoder().encode('one\r\ntwo\r\nthree\r\nfour\r\n'));
  const history = JSON.parse(terminal.viewport_snapshot(2));
  assert.ok(history.scrollback_len > 0);
  assert.equal(history.scroll_offset, 2);
  assert.equal(history.cursor_visible, false);
  terminal.write(new TextEncoder().encode('\x1b[?1049hALT'));
  const alt = JSON.parse(terminal.viewport_snapshot(100));
  assert.equal(alt.alt_screen, true);
  assert.equal(alt.scroll_offset, 0);
  assert.equal(alt.scrollback_len, 0);
  terminal.free();
});

// ── Finger-following scroll and momentum ────────────────────────────────────

const longHistory = (r: Renderer, lines = 300) =>
  r.packet({ rawChunks: [{ seq: 1, data: encoded(Array.from({ length: lines }, (_, i) => `row-${i}\r\n`).join('')) }], lastRawSeq: 1 });
const rowNumber = (r: Renderer) => Number(topRow(r)?.replace('row-', ''));
/** A timed drag: one move per 16 ms frame, `step` px each, lifted `rest` ms after the last move. */
function timedDrag(r: Renderer, y: number, step: number, moves: number, rest = 2) {
  let t = r.now();
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 100, clientY: y }], timeStamp: t });
  for (let move = 0; move < moves; move++) {
    t += 16; y += step;
    r.canvas.dispatch('touchmove', { touches: [{ clientX: 100, clientY: y }], timeStamp: t, preventDefault: noop });
    r.step();
  }
  r.canvas.dispatch('touchend', { touches: [], timeStamp: t + rest });
}
/** Rows drawn since `from`: each row drawing starts by clearing its full width. */
const rowsDrawn = (r: Renderer, from: number) => r.fills.slice(from).filter((fill) => fill.color === palette.background && fill.width === 800 && fill.height === 20).length;
/** The last paint's blits: where history landed on screen. */
const lastPaint = (r: Renderer, from: number) => r.blits.slice(from);

test('history follows the finger to the pixel, not in whole lines', async () => {
  const r = await renderer();
  longHistory(r, 60);
  const before = r.blits.length;
  swipe(r, 100, [100, 130]); // a row and a half at 20 px rows
  const blits = lastPaint(r, before);
  const paint = blits.slice(blits.findIndex((blit) => blit.dy === 0));
  // The first row is cut 10 px into its picture and the grid stays exactly covered.
  assert.equal((paint[0]?.sy ?? -1) % 20, 10);
  assert.equal(paint.reduce((sum, blit) => sum + blit.sh, 0), 480);
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('a flick keeps scrolling after the finger lifts, slows down and stops', async () => {
  const r = await renderer();
  longHistory(r);
  const start = rowNumber(r);
  timedDrag(r, 100, 40, 3); // 40 px per 16 ms: a quick flick
  const lifted = start - 6; // the finger itself travelled 120 px, six rows
  assert.ok(r.frames.length > 0, 'the fling keeps asking for frames');
  r.drain();
  const settled = rowNumber(r);
  assert.ok(settled < lifted - 20, `momentum carried on past the finger: row ${settled}`);
  assert.equal(r.frames.length, 0, 'and then it stops');
  assert.equal(r.latest.style.display, 'block');
  assert.deepEqual(r.inputs(), []);
  r.pagehide();
});

test('a finger that stops before lifting does not fling', async () => {
  const r = await renderer();
  longHistory(r);
  const start = rowNumber(r);
  timedDrag(r, 100, 40, 3, 200);
  r.drain();
  assert.equal(rowNumber(r), start - 6);
  r.pagehide();
});

test('a new touch stops a fling where it is', async () => {
  const r = await renderer();
  longHistory(r);
  timedDrag(r, 100, 40, 3);
  for (let frame = 0; frame < 5; frame++) r.step();
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 100, clientY: 300 }], timeStamp: r.now() });
  r.canvas.dispatch('touchend', { touches: [], timeStamp: r.now() });
  r.drain();
  const held = rowNumber(r);
  for (let frame = 0; frame < 30; frame++) r.step();
  assert.equal(rowNumber(r), held);
  assert.equal(r.frames.length, 0);
  r.pagehide();
});

test('Latest ends a fling and returns to the bottom', async () => {
  const r = await renderer();
  longHistory(r);
  const bottom = rowNumber(r);
  timedDrag(r, 100, 40, 3);
  for (let frame = 0; frame < 5; frame++) r.step();
  r.latest.dispatch('click');
  r.drain();
  assert.equal(rowNumber(r), bottom);
  assert.equal(r.latest.style.display, 'none');
  r.pagehide();
});

test('a fling down stops at the bottom, and output then keeps showing as it arrives', async () => {
  const r = await renderer();
  longHistory(r);
  swipe(r, 100, [100, 200]);
  assert.equal(r.latest.style.display, 'block');
  timedDrag(r, 400, -40, 3);
  r.drain();
  assert.equal(r.latest.style.display, 'none');
  r.packet({ rawChunks: [{ seq: 2, data: encoded('fresh-output\r\n') }], lastRawSeq: 2 });
  assert.match(r.readable.textContent, /fresh-output/u);
  r.pagehide();
});

test('a fling in a full-screen program sends nothing beyond what the finger travelled', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }));
  timedDrag(r, 0, 40, 3);
  r.drain();
  assert.equal(count(r, '\x1b[<64;11;1M'), 6, '120 px at 20 px rows, and no momentum');
  r.pagehide();
});

test('the readable text catches up once the finger lifts, not on every frame of a drag', async () => {
  const r = await renderer();
  longHistory(r, 60);
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 100, clientY: 100 }] });
  r.canvas.dispatch('touchmove', { touches: [{ clientX: 100, clientY: 200 }], preventDefault: noop });
  r.drain();
  assert.equal(topRow(r), 'row-37', 'still the text from before the drag');
  r.canvas.dispatch('touchend', { touches: [] });
  r.drain();
  assert.equal(topRow(r), 'row-32');
  r.pagehide();
});

test('scrolling draws only rows that newly came into view', async () => {
  const r = await renderer();
  longHistory(r);
  // The first touch makes pictures for the rows the opening paint drew directly.
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 50, clientY: 50 }] });
  r.canvas.dispatch('touchend', { touches: [] });
  r.drain();
  const before = r.fills.length;
  swipe(r, 100, [100, 140]); // two rows
  assert.ok(rowsDrawn(r, before) <= 3, `drew ${rowsDrawn(r, before)} rows`);
  r.pagehide();
});

test('a screen that changed all at once is drawn straight onto the canvas, and a touch makes its pictures', async () => {
  const r = await renderer();
  const before = r.blits.length;
  r.packet({ rawChunks: [{ seq: 1, data: encoded(Array.from({ length: 24 }, (_, i) => `line-${i}`).join('\r\n')) }], lastRawSeq: 1 });
  assert.equal(r.blits.length, before, 'nothing was drawn twice');
  assert.match(r.readable.textContent, /line-23/u);
  // A touch makes the missing pictures; then one more line of output is a copy plus one row.
  r.canvas.dispatch('touchstart', { touches: [{ clientX: 50, clientY: 50 }] });
  r.canvas.dispatch('touchend', { touches: [] });
  r.drain();
  const fills = r.fills.length;
  r.packet({ rawChunks: [{ seq: 2, data: encoded('\r\nnext') }], lastRawSeq: 2 });
  assert.ok(r.blits.length > before);
  assert.ok(rowsDrawn(r, fills) <= 2, `${rowsDrawn(r, fills)} rows drawn`);
  r.pagehide();
});

test('text after a styled blank run keeps its own font, whichever canvas it is drawn on', async () => {
  const r = await renderer();
  // A bold blank run with a coloured background between two plain words.
  r.packet({ rawChunks: [{ seq: 1, data: encoded('AAAA\x1b[1;44m  \x1b[0mAAAA\r\n') }], lastRawSeq: 1 });
  // Every A drawn, whole word or glyph by glyph, is in the plain font.
  const glyphs = () => r.texts.filter((item) => item.text.includes('A'));
  assert.equal(glyphs().reduce((sum, item) => sum + item.text.length, 0), 8);
  for (const item of glyphs()) assert.ok(!item.font.includes('bold'), `${item.text} at ${item.x} drawn as ${item.font}`);
  // And again once the rows come from their pictures, after a scroll.
  swipe(r, 100, [100, 140]);
  r.latest.dispatch('click');
  r.drain();
  for (const item of glyphs()) assert.ok(!item.font.includes('bold'), `${item.text} at ${item.x} drawn as ${item.font}`);
  r.pagehide();
});

test('text after a dim, coloured blank run keeps its own colour and opacity', async () => {
  const r = await renderer();
  r.packet({ rawChunks: [{ seq: 1, data: encoded('AAAA\x1b[2;31;44m  \x1b[0mAAAA\x1b[2;32m  \x1b[0mBBBB\r\n') }], lastRawSeq: 1 });
  const words = r.texts.filter((item) => /[AB]/u.test(item.text));
  // Blanks whose colour cannot show join the plain text beside them.
  assert.deepEqual(words.map((item) => item.text), ['AAAA', 'AAAA  BBBB']);
  for (const word of words) {
    assert.equal(word.color, palette.foreground, `${word.text} drawn in ${word.color}`);
    assert.equal(word.alpha, 1, `${word.text} drawn at alpha ${word.alpha}`);
    assert.ok(!word.font.includes('bold'));
  }
  // The blue blank behind the dim run is painted; nothing else is.
  assert.ok(r.fills.some((fill) => fill.color === palette.ansiNormal[4]));
  r.pagehide();
});

// ── WebGL2 renderer ──────────────────────────────────────────────────────────

const draws = (gpu: ReturnType<typeof fakeWebGl>, from = 0) => gpu.calls.slice(from).filter((call) => call.name === 'drawArraysInstanced');
const gpuCanvasOf = (r: Renderer) => r.created.find((element) => element.listeners.has('webglcontextlost'));

test('with WebGL2 the rows become instance data and each glyph is drawn once into the atlas', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello\r\nworld\r\n\x1b[8mXYZ\x1b[0m') }], lastRawSeq: 1 });
  assert.equal(r.drawnWith(), 'gpu');
  // Backgrounds, glyphs, decorations and the cursor: four instanced draws.
  assert.equal(draws(gpu).length, 4);
  assert.equal(draws(gpu).at(-1)?.args[3], 4, 'the cursor is four rectangles');
  // Text goes to the atlas glyph by glyph, never as words on a canvas.
  assert.ok(!r.texts.some((item) => item.text.length > 1), r.texts.map((item) => item.text).join(','));
  assert.equal(r.texts.filter((item) => item.text === 'l').length, 1, 'a repeated glyph is rasterised once');
  assert.ok(!r.texts.some((item) => /[XYZ]/u.test(item.text)), 'hidden text never reaches the atlas');
  // The screen canvas under the GPU one keeps a single pixel.
  assert.equal(r.canvas.width, 1);
  assert.match(r.readable.textContent, /hello\nworld/u);
  r.pagehide();
});

test('a GPU scroll moves the row offset and uploads only rows that newly came into view', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  longHistory(r);
  const from = gpu.calls.length, sub = gpu.uniforms.get('u_sub'), first = gpu.uniforms.get('u_first');
  swipe(r, 100, [100, 130]); // a row and a half
  assert.ok(gpu.uploadedSlots(from, 80) <= 3, `uploaded ${gpu.uploadedSlots(from, 80)} rows`);
  assert.notDeepEqual([gpu.uniforms.get('u_sub'), gpu.uniforms.get('u_first')], [sub, first]);
  assert.equal(gpu.uniforms.get('u_sub')?.[0], 10, 'half a row of the top line is scrolled off');
  // Scrolling back over rows already uploaded uploads nothing.
  const again = gpu.calls.length;
  swipe(r, 100, [130, 100]);
  assert.equal(gpu.uploadedSlots(again, 80), 0);
  assert.ok(draws(gpu, again).length >= 3);
  r.pagehide();
});

test('new output uploads only the rows it changed', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  longHistory(r);
  const from = gpu.calls.length;
  r.packet({ rawChunks: [{ seq: 2, data: encoded('one more line') }], lastRawSeq: 2 });
  assert.ok(gpu.uploadedSlots(from, 80) <= 2, `uploaded ${gpu.uploadedSlots(from, 80)} rows`);
  assert.match(r.readable.textContent, /one more line/u);
  r.pagehide();
});

test('a lost WebGL context hands drawing to the 2D canvas, and a restored one takes it back', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello') }], lastRawSeq: 1 });
  assert.equal(r.drawnWith(), 'gpu');
  let prevented = false;
  gpu.setLost(true);
  gpuCanvasOf(r)?.dispatch('webglcontextlost', { preventDefault: () => { prevented = true; } });
  r.drain();
  assert.ok(prevented, 'the context is asked back');
  assert.equal(r.drawnWith(), '2d');
  assert.equal(r.canvas.width, 800);
  assert.ok(r.texts.some((item) => item.text === 'hello'), 'the 2D canvas drew the screen');
  assert.equal(gpuCanvasOf(r)?.style.display, 'none');
  gpu.setLost(false);
  gpuCanvasOf(r)?.dispatch('webglcontextrestored');
  r.drain();
  assert.equal(r.drawnWith(), 'gpu');
  assert.equal(gpuCanvasOf(r)?.style.display, 'block');
  assert.ok(!r.messages.some((message) => message.type === 'error'));
  r.pagehide();
});

test('a context lost three times stays on the 2D canvas', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello') }], lastRawSeq: 1 });
  for (let loss = 0; loss < 3; loss++) {
    gpuCanvasOf(r)?.dispatch('webglcontextlost', { preventDefault: noop });
    gpuCanvasOf(r)?.dispatch('webglcontextrestored');
    r.drain();
  }
  assert.equal(r.drawnWith(), '2d');
  r.pagehide();
});

test('a GPU fault switches to the 2D canvas for good instead of restarting the renderer', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu });
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello') }], lastRawSeq: 1 });
  gpu.fail.add('drawArraysInstanced');
  r.packet({ rawChunks: [{ seq: 2, data: encoded(' world') }], lastRawSeq: 2 });
  assert.equal(r.drawnWith(), '2d');
  assert.ok(r.texts.some((item) => item.text.includes('world')));
  assert.ok(!r.messages.some((message) => message.type === 'error'));
  gpu.fail.clear();
  r.packet({ rawChunks: [{ seq: 3, data: encoded('!') }], lastRawSeq: 3 });
  assert.equal(r.drawnWith(), '2d');
  r.pagehide();
});

test('the 2D renderer can be pinned even where WebGL2 works', async () => {
  const gpu = fakeWebGl();
  const r = await renderer({ gpu, renderer: '2d' });
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello') }], lastRawSeq: 1 });
  assert.equal(r.drawnWith(), '2d');
  assert.equal(gpu.calls.length, 0);
  r.pagehide();
});

test('without WebGL2 the 2D renderer draws', async () => {
  const r = await renderer();
  r.packet({ rawChunks: [{ seq: 1, data: encoded('hello') }], lastRawSeq: 1 });
  assert.equal(r.drawnWith(), '2d');
  assert.ok(r.texts.some((item) => item.text === 'hello'));
  r.pagehide();
});

// ── Taps, links and copying ─────────────────────────────────────────────────
const textGrid = (lines: string[], columns = 80) => ({
  rows: 24, columns, cursorRow: 0, cursorColumn: 0, cursorVisible: true, truncated: false,
  lines: lines.map((text, index) => ({ index, truncated: false, spans: [{ text, flags: 0, fg: null, bg: null }] })),
});
// Cells are 10 px wide and rows 20 px tall on the test canvas.
const at = (col: number, row: number) => ({ clientX: col * 10 + 5, clientY: row * 20 + 10 });
const tap = (r: Renderer, col: number, row: number) => {
  r.canvas.dispatch('touchstart', { touches: [at(col, row)] });
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click', at(col, row));
};
const hold = async (r: Renderer, from: [number, number], to?: [number, number]) => {
  r.canvas.dispatch('touchstart', { touches: [at(...from)] });
  await new Promise((resolve) => setTimeout(resolve, 520));
  if (to) r.canvas.dispatch('touchmove', { touches: [at(...to)], preventDefault: noop });
  r.canvas.dispatch('touchend', { touches: [] });
  r.canvas.dispatch('click', at(...(to ?? from)));
};

test('a tap on a link opens it, even printed across rows; elsewhere it sends nothing', async () => {
  const r = await renderer();
  const cols = 20;
  host(r, modes({ alternateScreen: false }), {}, textGrid(['Open this to sign in', 'https://claude.ai/oa', 'uth/x?code=1 then', 'plain'], cols));
  tap(r, 3, 2);
  assert.deepEqual(r.messages.filter((m) => m.type === 'link').map((m) => m.url), ['https://claude.ai/oauth/x?code=1']);
  tap(r, 1, 3);
  assert.equal(r.messages.filter((m) => m.type === 'link').length, 1);
  assert.deepEqual(r.inputs(), []);
  assert.equal(r.keys.focusCount, 0);
  r.pagehide();
});

test('a program that asked for the mouse gets a click where the finger tapped; others get nothing', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }), {}, textGrid(['  [ Jump to bottom ]']));
  tap(r, 6, 0);
  assert.deepEqual(r.inputs(), ['\x1b[<0;7;1M\x1b[<0;7;1m']);
  host(r, modes({ mouseReporting: false }), {}, textGrid(['  [ Jump to bottom ]']));
  tap(r, 6, 0);
  assert.equal(r.inputs().length, 1);
  // Read-only: no click goes out.
  host(r, modes({ mouseReporting: true, sgrMouse: true }), { canWrite: false }, textGrid(['x']));
  tap(r, 0, 0);
  assert.equal(r.inputs().length, 1);
  r.pagehide();
});

test('touch and hold selects a word, dragging selects more, and Copy sends exactly that text', async () => {
  const r = await renderer();
  host(r, modes({ mouseReporting: true, sgrMouse: true }), {}, textGrid(['first line here', 'second row text']));
  await hold(r, [7, 0], [5, 1]);
  assert.equal(r.menu.style.display, 'flex');
  assert.equal(r.menuOpen.style.display, 'none');
  assert.match(r.sel.innerHTML, /<div/u, 'the selection is highlighted');
  assert.deepEqual(r.inputs(), [], 'a hold is not a click');
  r.menuCopy.dispatch('click', { preventDefault: noop });
  assert.deepEqual(r.messages.filter((m) => m.type === 'copy').map((m) => m.text), ['line here\nsecond']);
  assert.equal(r.menu.style.display, 'none');
  assert.equal(r.sel.innerHTML, '');
  r.pagehide();
});

test('holding on a link offers Open link; a tap elsewhere puts the selection down', async () => {
  const r = await renderer();
  host(r, modes({ alternateScreen: false }), {}, textGrid(['go to https://dragent.dev/docs now']));
  await hold(r, [12, 0]);
  assert.equal(r.menuOpen.style.display, 'block');
  tap(r, 0, 5);
  assert.equal(r.menu.style.display, 'none');
  assert.equal(r.messages.filter((m) => m.type === 'link').length, 0, 'putting it down opens nothing');
  await hold(r, [12, 0]);
  r.menuOpen.dispatch('click', { preventDefault: noop });
  assert.deepEqual(r.messages.filter((m) => m.type === 'link').map((m) => m.url), ['https://dragent.dev/docs']);
  r.pagehide();
});
