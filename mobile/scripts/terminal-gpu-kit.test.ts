// The WebGL2 renderer's pure parts: colour packing, glyph placement, atlas
// packing, per-row instance data and the renderer choice. The GL side is
// tested through the packaged runtime (terminal-runtime-renderer.test.ts) and
// against the 2D renderer pixel by pixel (terminal-gpu-parity.test.ts).
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { gpuKit } from '../src/terminal/runtime/gpuKit.js';
import { renderKit } from '../src/terminal/runtime/renderKit.js';
import type { TerminalPalette } from '../src/theme/types';

const gk = gpuKit();
const kit = renderKit();
const palette: TerminalPalette = {
  background: '#101010', foreground: '#eeeeee', cursor: '#abcdef',
  ansiNormal: ['#000000', '#cc0000', '#00cc00', '#cccc00', '#0000cc', '#cc00cc', '#00cccc', '#cccccc'],
  ansiBright: ['#555555', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff'],
};

// ── Colours ─────────────────────────────────────────────────────────────────

test('every colour form the renderer produces packs to 0xRRGGBBAA', () => {
  assert.equal(gk.parseColor('#cc0000'), 0xcc0000ff);
  assert.equal(gk.parseColor('#ABC'), 0xaabbccff);
  assert.equal(gk.parseColor('#abcd'), 0xaabbccdd);
  assert.equal(gk.parseColor('#11223344'), 0x11223344);
  assert.equal(gk.parseColor('rgb(10,20,30)'), 0x0a141eff);
  assert.equal(gk.parseColor('rgba(30,40,50,0.5019607843137255)'), 0x1e283280);
  assert.equal(gk.parseColor('rgba(1, 2, 3, 0)'), 0x01020300);
  assert.equal(gk.parseColor('rgb(100% 0% 50% / 50%)'), 0xff008080);
  // Anything else is for the browser to normalise.
  for (const other of ['red', 'hsl(0 50% 50%)', '#12', 'rgb(1,2)', '', null]) assert.equal(gk.parseColor(other as string), null);
});

test('every compact colour code resolves to the same packed colour the 2D canvas would paint', () => {
  for (const code of [-1, 1, 9, 16 + 36 * 5, 232, 255, 0x1000000 + 0x0a141e]) {
    const css = kit.colorCss(code, palette.foreground, palette);
    assert.notEqual(gk.parseColor(css), null, `${code} → ${css}`);
  }
  assert.equal(gk.parseColor(kit.colorCss(16 + 36 * 5, '', palette)), 0xff0000ff);
  assert.equal(gk.parseColor(kit.colorCss(232, '', palette)), 0x080808ff);
});

test('dim fades only the alpha, by the same factor as the 2D renderer', () => {
  assert.equal(gk.fade(0xeeeeeeff, 0.65), 0xeeeeeea6);
  assert.equal(gk.fade(0x11223380, 0.5), 0x11223340);
  assert.equal(gk.fade(0x11223344, 1), 0x11223344);
  assert.deepEqual(gk.unpack(0xff000080), [1, 0, 0, 128 / 255]);
});

// ── Geometry ────────────────────────────────────────────────────────────────

test('a glyph lands on its cell, rounded to a quarter pixel the way canvas text is', () => {
  assert.deepEqual(gk.placement(0, 25.29), { whole: 0, phase: 0 });
  assert.deepEqual(gk.placement(1, 25.29), { whole: 25, phase: 1 }); // .29 → .25
  assert.deepEqual(gk.placement(3, 25.29), { whole: 75, phase: 3 }); // .87 → .75
  assert.deepEqual(gk.placement(1, 9.9), { whole: 10, phase: 0 }); // .9 rounds up to the next pixel
  for (let col = 0; col < 200; col++) {
    const { whole, phase } = gk.placement(col, 8.4287);
    assert.ok(Math.abs(whole + phase / 4 - col * 8.4287) <= 0.125 + 1e-9);
  }
});

test('bold, italic and phase pick distinct atlas families', () => {
  const seen = new Set<number>();
  for (const flags of [0, 1, 4, 5]) for (let phase = 0; phase < 4; phase++) seen.add(gk.family(flags, phase));
  assert.equal(seen.size, 16);
  // Colour, dim, inverse and decorations do not change the glyph's shape.
  assert.equal(gk.family(1 | 2 | 8 | 64 | 256, 2), gk.family(1, 2));
});

test('decoration flags map to the bands the 2D renderer draws', () => {
  assert.equal(gk.bands(0), 0);
  assert.equal(gk.bands(8), gk.UNDERLINE);
  assert.equal(gk.bands(32), gk.UNDERLINE, 'curly draws as a straight underline');
  assert.equal(gk.bands(16), gk.UNDERLINE | gk.DOUBLE);
  assert.equal(gk.bands(256), gk.STRIKE);
  assert.equal(gk.bands(1024), gk.OVERLINE);
  assert.equal(gk.bands(8 | 256 | 1024 | 1 | 2), gk.UNDERLINE | gk.STRIKE | gk.OVERLINE);
  const bands = gk.bandGeometry(20, 1, 3);
  assert.deepEqual(bands, { underline: 54, double: 48, strike: 30, overline: 0, rule: 3 });
  assert.equal(gk.bandGeometry(10, 0.5, 2).rule, 2, 'a rule is never thinner than one CSS pixel');
});

test('the cursor outline is the 2D stroke: centred on the inset cell box, corners filled once', () => {
  const rects = gk.cursorRects(2, 1, 10, 20, 4, 1, 2);
  // Box at x = 20.5, y = 24.5 CSS (41, 49 device), 9 x 19 CSS (18 x 38 device), 2 device px line.
  assert.deepEqual(rects, [
    [40, 60, 48, 50], [40, 60, 86, 88],
    [40, 42, 50, 86], [58, 60, 50, 86],
  ]);
  // No two rectangles overlap, so no pixel is blended twice.
  const area = rects.reduce((sum, [x0 = 0, x1 = 0, y0 = 0, y1 = 0]) => sum + (x1 - x0) * (y1 - y0), 0);
  assert.equal(area, 20 * 40 - 16 * 36);
});

// ── Atlas packing ───────────────────────────────────────────────────────────

test('the packer fills shelves left to right, opens shelves below, and never overlaps', () => {
  const packer = gk.createPacker(100, 60);
  const boxes: Array<{ x: number; y: number; w: number; h: number }> = [];
  for (const w of [30, 30, 30, 30, 50, 60]) {
    const at = packer.alloc(w, 20);
    assert.ok(at, `room for ${w}`);
    boxes.push({ ...at, w, h: 20 });
  }
  assert.deepEqual(boxes.map(({ x, y }) => [x, y]), [[0, 0], [30, 0], [60, 0], [0, 20], [30, 20], [0, 40]]);
  for (const a of boxes) for (const b of boxes) if (a !== b) assert.ok(a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y, 'boxes overlap');
  assert.equal(packer.alloc(50, 20), null, 'full');
  assert.equal(packer.used, 230 * 20);
});

test('a grown packer keeps every box and fills the new room', () => {
  const packer = gk.createPacker(40, 20);
  assert.deepEqual(packer.alloc(40, 20), { x: 0, y: 0 });
  assert.equal(packer.alloc(10, 20), null);
  packer.grow(80, 20);
  assert.deepEqual(packer.alloc(10, 20), { x: 40, y: 0 }, 'the existing shelf extends');
  packer.grow(80, 40);
  assert.deepEqual(packer.alloc(80, 20), { x: 0, y: 20 });
  packer.reset();
  assert.deepEqual(packer.alloc(10, 20), { x: 0, y: 0 });
  assert.equal(packer.alloc(0, 20), null);
  assert.equal(packer.alloc(200, 20), null, 'wider than the atlas never fits');
});

test('the atlas grows height first, then width, and stops at the limit', () => {
  assert.deepEqual(gk.nextAtlasSize(1024, 1024, 4096), { width: 1024, height: 2048 });
  assert.deepEqual(gk.nextAtlasSize(1024, 2048, 4096), { width: 2048, height: 2048 });
  assert.deepEqual(gk.nextAtlasSize(4096, 2048, 4096), { width: 4096, height: 4096 });
  assert.equal(gk.nextAtlasSize(4096, 4096, 4096), null);
  assert.deepEqual(gk.nextAtlasSize(2048, 2048, 2048), null);
});

// ── Instance data ───────────────────────────────────────────────────────────

const arrays = (instances: number) => {
  const make = () => { const buffer = new ArrayBuffer(instances * gk.WORDS * 4); return { f32: new Float32Array(buffer), u32: new Uint32Array(buffer) }; };
  return { bg: make(), glyphs: make(), deco: make() };
};
const instance = (target: { f32: Float32Array; u32: Uint32Array }, index: number) => [...target.f32.slice(index * 6, index * 6 + 5), target.u32[index * 6 + 5]];
const geometry = { cols: 10, cellPx: 10, rowPx: 20 };
const glyphs: Array<[string, number, number, number]> = [];
const glyphAt = (text: string, flags: number, col: number, cells: number) => {
  glyphs.push([text, flags, col, cells]);
  return text === '·' ? null : { x: col * 10 - 1, w: cells * 10 + 2, ax: text.codePointAt(0) ?? 0, ay: 0, kind: text === '✅' ? gk.COLOUR : gk.MASK };
};
const colorOf = (css: string) => gk.parseColor(css) ?? 0;

test('a row becomes background, glyph and decoration instances in its slot, the rest zeroed', () => {
  glyphs.length = 0;
  const out = arrays(30);
  // Fill the slot with junk first: a rewrite must clear what it does not use.
  for (const target of Object.values(out)) target.f32.fill(7, 10 * 6, 20 * 6);
  const decoded = kit.decodeRow([[0, 1, -1, 2 | 8, 'ab c'], [4, -1, 4, 0, 'x'], [6, -1, -1, 128, 'hid'], [9, -1, -1, 0, '✅', 1]], palette, 10);
  const counts = gk.writeRow(decoded.runs, geometry, 10, out, glyphAt, colorOf);
  assert.deepEqual(counts, { bg: 1, glyphs: 5, deco: 1 });
  // The blue background behind x, full row height.
  assert.deepEqual(instance(out.bg, 10), [40, 50, 0, 20, 0, 0x0000ccff]);
  // Dim red text: alpha faded; the space is skipped; each glyph at its column.
  assert.deepEqual(glyphs.map(([text, , col]) => `${text}@${col}`), ['a@0', 'b@1', 'c@3', 'x@4', '✅@9']);
  assert.deepEqual(instance(out.glyphs, 10), [-1, 12, 97, 0, gk.MASK, 0xcc0000a6]);
  assert.deepEqual(instance(out.glyphs, 14), [89, 12, 0x2705, 0, gk.COLOUR, 0xeeeeeeff]);
  // The underline spans the whole run, spaces included, in the run's colour.
  assert.deepEqual(instance(out.deco, 10), [0, 40, 0, 20, gk.UNDERLINE, 0xcc0000a6]);
  // Unused instances in the slot are zero (draw nothing), and other slots are untouched.
  for (const [target, used] of [[out.bg, 1], [out.glyphs, 5], [out.deco, 1]] as const) {
    assert.ok(target.f32.slice((10 + used) * 6, 20 * 6).every((value) => value === 0));
    assert.ok(target.f32.slice(0, 10 * 6).every((value) => value === 0));
  }
});

test('hidden text keeps its background but draws no glyphs or decorations', () => {
  glyphs.length = 0;
  const out = arrays(10);
  const decoded = kit.decodeRow([[0, -1, 2, 128 | 8 | 256, 'SECRET']], palette, 10);
  assert.deepEqual(gk.writeRow(decoded.runs, geometry, 0, out, glyphAt, colorOf), { bg: 1, glyphs: 0, deco: 0 });
  assert.equal(glyphs.length, 0, 'not even rasterised');
});

test('inverse swaps colours before they are packed; a glyph without ink takes no instance', () => {
  const out = arrays(10);
  const decoded = kit.decodeRow([[0, 1, -1, 64, 'A·']], palette, 10);
  assert.deepEqual(gk.writeRow(decoded.runs, geometry, 0, out, glyphAt, colorOf), { bg: 1, glyphs: 1, deco: 0 });
  assert.equal(out.bg.u32[5], 0xcc0000ff, 'the red foreground becomes the background');
  assert.equal(out.glyphs.u32[5], 0x101010ff, 'the default background becomes the text colour');
});

test('a wide glyph takes one instance spanning both cells', () => {
  glyphs.length = 0;
  const out = arrays(10);
  const decoded = kit.decodeRow([[2, -1, -1, 0, '界', 2], [4, -1, -1, 0, 'é', 1]], palette, 10);
  gk.writeRow(decoded.runs, geometry, 0, out, glyphAt, colorOf);
  assert.deepEqual(glyphs.map(([text, , col, cells]) => [text, col, cells]), [['界', 2, 2], ['é', 4, 1]]);
  assert.equal(out.glyphs.f32[1], 22);
});

test('a row can never write past its slot, however many runs it has', () => {
  const out = arrays(20);
  const runs = Array.from({ length: 30 }, (_, col) => ({ col, cells: 1, text: 'x', single: false, fg: '#fff', bg: '#123', flags: 8 }));
  const counts = gk.writeRow(runs, geometry, 0, out, glyphAt, colorOf);
  assert.deepEqual(counts, { bg: 10, glyphs: 10, deco: 10 });
  assert.ok(out.bg.f32.slice(10 * 6).every((value) => value === 0), 'the next slot is untouched');
});

// ── Renderer choice ─────────────────────────────────────────────────────────

test('the GPU draws only when it is available, healthy and wanted', () => {
  const base = { requested: 'auto', available: true, failed: false, lost: false, losses: 0 };
  assert.equal(gk.chooseRenderer(base), 'gpu');
  assert.equal(gk.chooseRenderer({ ...base, requested: 'gpu' }), 'gpu');
  assert.equal(gk.chooseRenderer({ ...base, requested: '2d' }), '2d');
  assert.equal(gk.chooseRenderer({ ...base, available: false }), '2d');
  assert.equal(gk.chooseRenderer({ ...base, requested: 'gpu', available: false }), '2d', 'asking for the GPU cannot conjure one');
  assert.equal(gk.chooseRenderer({ ...base, failed: true }), '2d');
  assert.equal(gk.chooseRenderer({ ...base, lost: true }), '2d');
  assert.equal(gk.chooseRenderer({ ...base, losses: gk.MAX_CONTEXT_LOSSES - 1 }), 'gpu');
  assert.equal(gk.chooseRenderer({ ...base, losses: gk.MAX_CONTEXT_LOSSES }), '2d');
});
