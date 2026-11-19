// The renderer's pure parts: row decoding and run building, momentum and
// scroll-distance coalescing. They run inside the terminal document, so they
// are tested here on their own and through the packaged renderer elsewhere.
import assert from 'node:assert/strict';
import { test } from 'node:test';
import { renderKit } from '../src/terminal/runtime/renderKit.js';
import type { TerminalPalette } from '../src/theme/types';

const kit = renderKit();
const palette: TerminalPalette = {
  background: '#101010', foreground: '#eeeeee', cursor: '#abcdef',
  ansiNormal: ['#000000', '#cc0000', '#00cc00', '#cccc00', '#0000cc', '#cc00cc', '#00cccc', '#cccccc'],
  ansiBright: ['#555555', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff'],
};

// ── Colours ─────────────────────────────────────────────────────────────────

test('compact colour codes resolve like the snapshot colours they replace', () => {
  assert.equal(kit.colorCss(-1, 'fallback', palette), 'fallback');
  assert.equal(kit.colorCss(1, 'fallback', palette), '#cc0000');
  assert.equal(kit.colorCss(9, 'fallback', palette), '#ff0000');
  assert.equal(kit.colorCss(0x1000000 + 0x0a141e, 'fallback', palette), 'rgb(10,20,30)');
  assert.equal(kit.colorCss(16 + 36 * 5, 'fallback', palette), 'rgb(255,0,0)');
  assert.equal(kit.colorCss(232, 'fallback', palette), 'rgb(8,8,8)');
  // Host RGBA arrays and snapshot objects still work.
  assert.equal(kit.colorCss([30, 40, 50, 255], 'fallback', palette), 'rgba(30,40,50,1)');
  assert.equal(kit.colorCss({ kind: 'rgb', value: [1, 2, 3] }, 'fallback', palette), 'rgb(1,2,3)');
  assert.equal(kit.colorCss({ kind: 'indexed', value: 2 }, 'fallback', palette), '#00cc00');
  assert.equal(kit.colorCss(null, 'fallback', palette), 'fallback');
});

// ── Rows and runs ───────────────────────────────────────────────────────────

test('a compact row decodes into runs with resolved colours and readable text', () => {
  const { runs, text } = kit.decodeRow([
    [0, -1, -1, 0, 'ab '],
    [3, 1, -1, 1, 'red'],
    [7, -1, -1, 0, '界', 2],
    [9, -1, -1, 0, 'é', 1],
    [10, -1, 3, 64, 'inv'],
    [14, -1, -1, 128, 'secret'],
  ], palette, 80);
  assert.deepEqual(runs.map((run: { col: number; cells: number; text: string; single: boolean }) => [run.col, run.cells, run.text, run.single]),
    [[0, 3, 'ab ', false], [3, 3, 'red', false], [7, 2, '界', true], [9, 1, 'é', true], [10, 3, 'inv', false], [14, 6, 'secret', false]]);
  assert.equal(runs[0]!.fg, palette.foreground);
  assert.equal(runs[0]!.bg, null, 'the default background is not painted');
  assert.equal(runs[1]!.fg, '#cc0000');
  // Inverse swaps: the background colour becomes the text, the text colour the paper.
  assert.equal(runs[4]!.fg, '#cccc00');
  assert.equal(runs[4]!.bg, palette.foreground);
  // Wide characters keep their spacer cell, hidden text reads as blanks.
  assert.equal(text, 'ab red 界 e\u0301inv');
});

test('decoding never places text past the grid and skips malformed runs', () => {
  const { runs, text } = kit.decodeRow([[2, -1, -1, 0, 'abcdef'], [9, -1, -1, 0, 'zz'], ['x'], [1, 2]], palette, 5);
  assert.equal(runs.length, 1);
  assert.equal(runs[0]!.text, 'abc');
  assert.equal(runs[0]!.cells, 3);
  assert.equal(text, '  abc');
});

test('a restyle function swaps placeholder codes for real styles', () => {
  const spans = [{ fg: [30, 40, 50, 128], bg: null, flags: 1 }, { fg: null, bg: [1, 2, 3, 255], flags: 128 }];
  const { runs, text } = kit.decodeRow([[0, 0, 0, 0, 'A'], [1, 1, 1, 0, 'HIDE']], palette, 10, (fg: number) => spans[fg]!);
  assert.equal(runs[0]!.fg, 'rgba(30,40,50,0.5019607843137255)');
  assert.equal(runs[0]!.flags, 1);
  assert.equal(runs[1]!.bg, 'rgba(1,2,3,1)');
  assert.equal(text, 'A');
});

const mono = (width: number) => (glyph: string) => (glyph === 'W' ? width * 1.5 : width);

test('text whose advance matches the grid is one piece per run, blanks trimmed', () => {
  assert.deepEqual(kit.segments('  hello world  ', 4, mono(10), 10, 0.25), [{ col: 6, text: 'hello world' }]);
  assert.deepEqual(kit.segments('    ', 0, mono(10), 10, 0.25), []);
});

test('a glyph that does not fit its cell starts a new piece, so the next glyph is back on the grid', () => {
  // W is 1.5 cells wide: whatever follows it must be placed at its own column.
  assert.deepEqual(kit.segments('aWbc', 0, mono(10), 10, 0.25), [{ col: 0, text: 'aW' }, { col: 2, text: 'bc' }]);
});

test('a slightly wider font is split before drift becomes visible', () => {
  // 0.1 px too wide per glyph: after three glyphs the fourth would be 0.3 px off.
  const pieces = kit.segments('abcdefgh', 0, () => 10.1, 10, 0.25);
  assert.deepEqual(pieces, [{ col: 0, text: 'abc' }, { col: 3, text: 'def' }, { col: 6, text: 'gh' }]);
  // A font that does not match at all draws glyph by glyph.
  assert.equal(kit.segments('abc', 0, () => 15, 10, 0.25).length, 3);
});

test('runs keep astral characters as one cell each', () => {
  assert.deepEqual(kit.segments('a😀b', 0, (glyph: string) => (glyph === '😀' ? 10 : 10), 10, 0.25), [{ col: 0, text: 'a😀b' }]);
  const { runs } = kit.decodeRow([[0, -1, -1, 0, 'a😀b']], palette, 80);
  assert.equal(runs[0]!.cells, 3);
});

// ── Momentum ────────────────────────────────────────────────────────────────

test('momentum decelerates smoothly and the path does not depend on the frame rate', () => {
  const run = (dt: number, ms: number) => {
    let velocity = 2, distance = 0;
    for (let t = 0; t < ms && velocity !== 0; t += dt) {
      const step = kit.momentum(velocity, dt);
      assert.ok(Math.abs(step.velocity) <= Math.abs(velocity), 'never speeds up');
      distance += step.distance; velocity = step.velocity;
    }
    return distance;
  };
  // 60 Hz and 120 Hz travel the same distance in the same 300 ms.
  assert.ok(Math.abs(run(1000 / 60, 300) - run(1000 / 120, 300)) < 0.5);
  // The whole fling covers about the predicted distance, and then it stops.
  const total = run(16, 10_000);
  assert.ok(Math.abs(total - kit.flingDistance(2)) < 30, `${total} vs ${kit.flingDistance(2)}`);
  assert.ok(total > 500 && total < 1500, 'a fast flick moves a few screens, not the whole history');
});

test('momentum stops once too slow to see, and works in both directions', () => {
  assert.equal(kit.momentum(0.019, 16).velocity, 0);
  const up = kit.momentum(-1, 16);
  assert.ok(up.distance < 0 && up.velocity < 0);
  assert.deepEqual(kit.momentum(1, 0), { distance: 0, velocity: 1 });
});

test('release velocity comes from the last 100 ms of travel', () => {
  const samples = [{ t: 0, y: 0 }, { t: 100, y: 10 }, { t: 116, y: 50 }, { t: 132, y: 90 }, { t: 148, y: 130 }];
  // Samples older than 100 ms before the last are ignored: 120 px in 48 ms.
  assert.equal(kit.releaseVelocity(samples, 150), 120 / 48);
  // A finger that rested before lifting does not fling.
  assert.equal(kit.releaseVelocity(samples, 260), 0);
  // A slow drag does not fling either.
  assert.equal(kit.releaseVelocity([{ t: 0, y: 0 }, { t: 50, y: 5 }], 50), 0);
  // Events without timestamps (or a single sample) give no speed.
  assert.equal(kit.releaseVelocity([{ t: Number.NaN, y: 0 }], 0), 0);
  assert.equal(kit.releaseVelocity([], 0), 0);
  // Absurd speeds are capped.
  assert.equal(kit.releaseVelocity([{ t: 0, y: 0 }, { t: 10, y: 1000 }], 10), 8);
  assert.equal(kit.releaseVelocity([{ t: 0, y: 1000 }, { t: 10, y: 0 }], 10), -8);
});

test('samples are trimmed to the window a release can use', () => {
  const samples = [{ t: 0, y: 0 }, { t: 50, y: 1 }, { t: 160, y: 2 }, { t: 170, y: 3 }];
  kit.trimSamples(samples, 170);
  assert.deepEqual(samples.map((sample: { t: number }) => sample.t), [160, 170]);
});

// ── Remote scroll coalescing ────────────────────────────────────────────────

test('travel becomes whole units up to the cap, and the rest waits', () => {
  assert.deepEqual(kit.takeUnits(45, 20, 8), { count: 2, rest: 5 });
  assert.deepEqual(kit.takeUnits(-45, 20, 8), { count: -2, rest: -5 });
  assert.deepEqual(kit.takeUnits(480, 20, 8), { count: 8, rest: 320 });
  assert.deepEqual(kit.takeUnits(19, 20, 8), { count: 0, rest: 19 });
  // Float error just under a whole unit still counts as one.
  assert.equal(kit.takeUnits(0.1 + 0.2 - 0.3 + 19.999999999, 20, 8).count, 1);
  assert.deepEqual(kit.takeUnits(10, 0, 8), { count: 0, rest: 0 });
});

// ── Cells ───────────────────────────────────────────────────────────────────

test('code points are counted without building arrays, astral characters as one', () => {
  assert.equal(kit.codePoints('abc'), 3);
  assert.equal(kit.codePoints('a😀b'), 3);
  assert.equal(kit.codePoints(''), 0);
  // A lone surrogate still fills a cell.
  assert.equal(kit.codePoints('\ud83d'), 1);
});


// ── Selection, links and clicks ─────────────────────────────────────────────

test('a long-press picks the whole word, path or link under the finger', () => {
  assert.deepEqual(kit.wordAt('open ~/dev/app.ts now', 8), [5, 16]);
  assert.deepEqual(kit.wordAt('one', 0), [0, 2]);
  assert.equal(kit.wordAt('a  b', 1), null);
  assert.equal(kit.wordAt('ab', 7), null);
});

test('selected text keeps line breaks, joins wrapped rows and drops trailing blanks', () => {
  // Rows arrive with trailing blanks already trimmed (decodeRow).
  const rows = ['hello wor', 'second', 'abcdefghij', 'klm'];
  assert.equal(kit.selectText(rows, { row: 0, col: 6 }, { row: 1, col: 5 }, 10), 'wor\nsecond');
  // Row 2 fills all 10 columns, so it wrapped onto row 3.
  assert.equal(kit.selectText(rows, { row: 2, col: 0 }, { row: 3, col: 2 }, 10), 'abcdefghijklm');
  // Either drag direction.
  assert.equal(kit.selectText(rows, { row: 1, col: 5 }, { row: 0, col: 6 }, 10), 'wor\nsecond');
});

test('a tap on a link finds all of it, even printed across several rows', () => {
  const cols = 20;
  const rows = ['Open this to sign in', 'https://claude.ai/oa', 'uth/authorize?code=1', '23&state=x.', 'next'];
  // Row 0 is exactly 20 wide too, so the join starts there; the link still resolves.
  assert.equal(kit.linkAt(rows, 2, 4, cols), 'https://claude.ai/oauth/authorize?code=123&state=x');
  assert.equal(kit.linkAt(rows, 1, 0, cols), 'https://claude.ai/oauth/authorize?code=123&state=x');
  assert.equal(kit.linkAt(rows, 4, 1, cols), null);
  assert.equal(kit.linkAt(['see (https://a.dev/x) ok'], 0, 8, 80), 'https://a.dev/x');
  assert.equal(kit.linkAt(['https://en.wikipedia.org/wiki/Foo_(bar), then'], 0, 3, 80), 'https://en.wikipedia.org/wiki/Foo_(bar)');
  assert.equal(kit.linkAt(['plain text only'], 0, 2, 80), null);
});

test('a tap becomes a left click for a program that asked for the mouse', () => {
  assert.equal(kit.clickReport(3, 7, true), '\x1b[<0;3;7M\x1b[<0;3;7m');
  assert.equal(kit.clickReport(1, 1, false), '\x1b[M !!\x1b[M#!!');
  assert.equal(kit.clickReport(95, 1, false), '');
});

test('only a plain web link may leave the terminal', async () => {
  const { openableLink } = await import('../src/terminal/runtime/types');
  assert.equal(openableLink('https://claude.ai/oauth?x=1'), 'https://claude.ai/oauth?x=1');
  assert.equal(openableLink('http://localhost:3000/'), 'http://localhost:3000/');
  for (const bad of ['javascript:alert(1)', 'file:///etc/passwd', 'dragent://x', 'https://u:p@a.dev', '', 42, 'x'.repeat(5000)]) {
    assert.equal(openableLink(bad), null, String(bad).slice(0, 30));
  }
});
