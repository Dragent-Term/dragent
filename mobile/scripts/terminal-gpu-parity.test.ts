// Pixel parity between the WebGL2 and 2D terminal renderers, in headless
// Chrome: the same styled screens are drawn by each and screenshotted.
//
// Tolerance. Glyph edges may differ a little: the GPU places every glyph at
// its exact cell, where the 2D canvas lets a run of text advance by the
// font's own widths, and antialiasing can round differently. So the images
// are compared in cell-sized tiles: in every tile the mean difference (the
// largest channel difference per pixel, 0-255), with the GPU image moved by
// whichever of -1, 0 or +1 device pixels matches best, must stay under
// TILE_MEAN_LIMIT; unshifted over the whole image it must stay under
// IMAGE_MEAN_LIMIT. A wrong
// colour, style, position or missing glyph shows up as a tile far over the
// limit, which the control below proves on a one-character change.
//
// Skipped where Chrome is not installed. TERMINAL_PARITY_OUT=<dir> also writes
// each screenshot and a diff image there.
import assert from 'node:assert/strict';
import { mkdir, mkdtemp, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { test } from 'node:test';
import { terminalDocument } from '../src/terminal/runtime/document';
import { chromeAvailable, decodePng, encodePng, launchChrome, openPage, type Image } from './headless-chrome';

export const TILE_MEAN_LIMIT = 24;
export const IMAGE_MEAN_LIMIT = 1.5;
const OUT = process.env.TERMINAL_PARITY_OUT ?? '';
const palette = { background: '#101010', foreground: '#eeeeee', cursor: '#abcdef',
  ansiNormal: ['#000000', '#cc0000', '#00cc00', '#cccc00', '#0000cc', '#cc00cc', '#00cccc', '#cccccc'],
  ansiBright: ['#555555', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff'] };

/** Every style the renderer draws, one or two per line. */
const STYLES = [
  'plain text 0123456789 ~!@#$%^&*()_+-={}[]|\\:;"\'<>,.?/',
  '\x1b[1mbold\x1b[0m \x1b[3mitalic\x1b[0m \x1b[1;3mbold italic\x1b[0m \x1b[2mdim text\x1b[0m',
  '\x1b[7minverse\x1b[0m \x1b[8mhidden\x1b[0m|\x1b[4munderline\x1b[0m \x1b[21mdouble\x1b[0m \x1b[4:3mcurly\x1b[0m',
  '\x1b[9mstrike\x1b[0m \x1b[53moverline\x1b[0m \x1b[2;4;31mdim red underline\x1b[0m',
  '\x1b[30;41m bg1 \x1b[30;42m bg2 \x1b[30;43m bg3 \x1b[37;44m bg4 \x1b[30;45m bg5 \x1b[30;46m bg6 \x1b[30;47m bg7 \x1b[0m',
  '\x1b[91mbr\x1b[92mig\x1b[93mht\x1b[94m c\x1b[95mol\x1b[96mou\x1b[97mrs\x1b[0m \x1b[38;5;208m208\x1b[38;5;45m 45\x1b[38;5;244m 244\x1b[0m',
  '\x1b[38;2;255;128;0mtrue\x1b[38;2;0;200;255mcolour\x1b[0m \x1b[48;2;60;20;90m truecolour bg \x1b[0m',
  'wide: 界面テスト 한국어 | emoji: ✅🚀👍 | é ñ ü',
  'box: ┌─┬─┐ │ │ ├─┼─┤ └─┴─┘ ░▒▓█ arrows → ← ↑ ↓',
  '\x1b[1;7;4;32m all at once \x1b[0m \x1b[2;7m dim inverse \x1b[0m \x1b[4;48;5;24m underline on bg \x1b[0m',
];

interface Scenario { name: string; rows: number; cols: number; text: string; scroll?: number }
const scenarios = (): Scenario[] => {
  const page = STYLES.join('\r\n') + '\r\n$ cursor here';
  const history = Array.from({ length: 80 }, (_, line) => `${STYLES[line % STYLES.length]} #${line}`).join('\r\n');
  return [
    // A grid that fits: text at its natural size.
    { name: 'styles', rows: 24, cols: 44, text: page },
    // A grid wider than the screen: everything scaled down.
    { name: 'scaled', rows: 50, cols: 100, text: page },
    // Scrolled into history by a distance that is not a whole row.
    { name: 'scrolled', rows: 24, cols: 44, text: history, scroll: 137 },
  ];
};

const compare = (a: Image, b: Image, tile: { w: number; h: number }) => {
  assert.equal(a.width, b.width); assert.equal(a.height, b.height);
  const diff = new Uint8Array(a.rgba.length);
  const tilesX = Math.ceil(a.width / tile.w), tilesY = Math.ceil(a.height / tile.h);
  // Tile sums for the GPU image shifted by -1, 0 and +1 device pixels: a tile
  // passes on its best shift, since a glyph may sit up to a pixel from where a
  // run of 2D text puts it.
  const shifts = [-1, 0, 1], tileSum = shifts.map(() => new Float64Array(tilesX * tilesY)), tileCount = new Float64Array(tilesX * tilesY);
  const delta = (o: number, p: number) => Math.max(Math.abs((a.rgba[o] ?? 0) - (b.rgba[p] ?? 0)), Math.abs((a.rgba[o + 1] ?? 0) - (b.rgba[p + 1] ?? 0)), Math.abs((a.rgba[o + 2] ?? 0) - (b.rgba[p + 2] ?? 0)));
  let sum = 0, over32 = 0, max = 0;
  for (let y = 0; y < a.height; y++) for (let x = 0; x < a.width; x++) {
    const o = (y * a.width + x) * 4, d = delta(o, o);
    sum += d; if (d > 32) over32++; if (d > max) max = d;
    const t = Math.floor(y / tile.h) * tilesX + Math.floor(x / tile.w);
    tileCount[t] = (tileCount[t] ?? 0) + 1;
    shifts.forEach((shift, index) => {
      const sx = Math.max(0, Math.min(a.width - 1, x + shift)), sums = tileSum[index] as Float64Array;
      sums[t] = (sums[t] ?? 0) + delta((y * a.width + sx) * 4, o);
    });
    diff[o] = Math.min(255, d * 4); diff[o + 1] = 0; diff[o + 2] = 0; diff[o + 3] = 255;
  }
  let worst = 0;
  for (let t = 0; t < tileCount.length; t++) worst = Math.max(worst, Math.min(...tileSum.map((sums) => (sums[t] ?? 0) / (tileCount[t] || 1))));
  const pixels = a.width * a.height;
  return { mean: sum / pixels, over32: over32 / pixels, max, worstTile: worst, diff: { width: a.width, height: a.height, rgba: diff } };
};
const within = (result: { mean: number; worstTile: number }) => result.worstTile < TILE_MEAN_LIMIT && result.mean < IMAGE_MEAN_LIMIT;

test('the GPU and 2D renderers draw every style alike, within the defined tolerance', { skip: !chromeAvailable() && 'Chrome not installed', timeout: 120_000 }, async () => {
  const dir = await mkdtemp(join(tmpdir(), 'terminal-parity-'));
  if (OUT) await mkdir(OUT, { recursive: true });
  const browser = await launchChrome(join(dir, 'profile'), ['--force-color-profile=srgb', '--disable-renderer-backgrounding']);
  try {
    const pages: Record<string, string> = {};
    for (const renderer of ['2d', 'gpu'] as const) {
      pages[renderer] = join(dir, `${renderer}.html`);
      await writeFile(pages[renderer], terminalDocument('parity', { renderer }));
    }
    const shoot = async (renderer: '2d' | 'gpu', scenario: Scenario, alter?: (text: string) => string): Promise<{ image: Image; png: Buffer; drawnWith: string; cell: { w: number; h: number } }> => {
      const page = await openPage(browser.cdp);
      try {
        await page.send('Page.enable');
        await page.send('Emulation.setDeviceMetricsOverride', { width: 390, height: 760, deviceScaleFactor: 3, mobile: true });
        const loaded = new Promise<void>((resolve) => browser.cdp.on('Page.loadEventFired', () => resolve()));
        await page.send('Page.navigate', { url: pathToFileURL(pages[renderer] ?? '').href });
        await loaded;
        const text = alter ? alter(scenario.text) : scenario.text;
        const result = await page.evaluate(`(async () => {
          const frames = (n) => new Promise((resolve) => { const step = () => (n-- <= 0 ? resolve() : requestAnimationFrame(step)); requestAnimationFrame(step); });
          while (!document.getElementById('screen')) await frames(1);
          const bytes = new TextEncoder().encode(${JSON.stringify(text)}); let s = ''; for (const b of bytes) s += String.fromCharCode(b);
          window.postMessage(JSON.stringify({ v: 1, id: 'parity', type: 'state', epoch: 1, seq: 1,
            frame: { epoch: 1, revision: 1, presentation: 'raw', reason: null, rawReset: true, rawChunks: [{ seq: 1, data: btoa(s) }], lastRawSeq: 1, rows: ${scenario.rows}, cols: ${scenario.cols}, grid: null },
            environment: { connected: true, canWrite: true, canResize: false, holdFit: false, palette: ${JSON.stringify(palette)} } }), '*');
          await frames(4);
          const canvas = document.getElementById('screen');
          if (${scenario.scroll ?? 0} > 0) {
            const touch = (type, y) => { const point = new Touch({ identifier: 1, target: canvas, clientX: 200, clientY: y }); canvas.dispatchEvent(new TouchEvent(type, { touches: type === 'touchend' ? [] : [point], changedTouches: [point], bubbles: true, cancelable: true })); };
            touch('touchstart', 100); await frames(1);
            let y = 100; for (let i = 0; i < 20; i++) { y += ${(scenario.scroll ?? 0) / 20}; touch('touchmove', y); await frames(1); }
            // Let it rest before lifting, so no fling moves it on.
            await new Promise((resolve) => setTimeout(resolve, 120)); touch('touchend', y); await frames(3);
          }
          await frames(3);
          const rows = ${scenario.rows}, cols = ${scenario.cols};
          return { drawnWith: canvas.getAttribute('data-renderer'), width: canvas.clientWidth, height: canvas.clientHeight, rows, cols };
        })()`) as { drawnWith: string; width: number; height: number; rows: number; cols: number };
        const shot = await page.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false }) as { data: string };
        const png = Buffer.from(shot.data, 'base64');
        // A tile is about one cell: the grid's width and height split by its columns and rows (capped to the screen).
        const cell = { w: Math.max(8, Math.floor(result.width * 3 / Math.max(result.cols, result.width / 8.4))), h: Math.max(16, Math.floor(Math.min(20, result.height / result.rows) * 3)) };
        return { image: decodePng(png), png, drawnWith: result.drawnWith, cell };
      } finally { await page.close(); }
    };
    const report: string[] = [];
    for (const scenario of scenarios()) {
      const flat = await shoot('2d', scenario), gpu = await shoot('gpu', scenario);
      assert.equal(flat.drawnWith, '2d');
      assert.equal(gpu.drawnWith, 'gpu', 'the GPU renderer drew (WebGL2 available in this Chrome)');
      const result = compare(gpu.image, flat.image, gpu.cell);
      report.push(`${scenario.name}: mean ${result.mean.toFixed(3)}, worst tile ${result.worstTile.toFixed(2)}, ${(result.over32 * 100).toFixed(3)}% px > 32, max ${result.max}`);
      if (OUT) {
        await writeFile(join(OUT, `${scenario.name}-2d.png`), flat.png);
        await writeFile(join(OUT, `${scenario.name}-gpu.png`), gpu.png);
        await writeFile(join(OUT, `${scenario.name}-diff.png`), encodePng(result.diff));
      }
      assert.ok(within(result), `${scenario.name} differs beyond tolerance: ${report.at(-1)}`);
    }
    // Control: one character in another colour must fail the same check.
    const control = scenarios()[0] as Scenario;
    const flat = await shoot('2d', control, (text) => text.replace('plain text', 'plain \x1b[31mt\x1b[0mext'));
    const gpu = await shoot('gpu', control);
    const changed = compare(gpu.image, flat.image, gpu.cell);
    report.push(`control (one red letter): mean ${changed.mean.toFixed(3)}, worst tile ${changed.worstTile.toFixed(2)}`);
    if (OUT) await writeFile(join(OUT, 'parity-report.txt'), `${report.join('\n')}\nlimits: tile mean < ${TILE_MEAN_LIMIT}, image mean < ${IMAGE_MEAN_LIMIT}\n`);
    console.log(report.join('\n'));
    assert.ok(!within(changed), `a one-letter colour change must exceed the tolerance: ${report.at(-1)}`);
  } finally {
    await browser.close();
    await rm(dir, { recursive: true, force: true, maxRetries: 5 });
  }
});
