/**
 * Frame-cost benchmark for the phone terminal renderer.
 *
 * Loads the real packaged terminal document (WASM + runtime) in headless
 * Chrome with a phone viewport and CPU throttling, fills a 50x100 grid with
 * 3000 lines of styled history, then measures:
 *   - scroll: a finger dragging up through history, one touchmove per frame;
 *   - fling: a fast flick released mid-move, the frames after release;
 *   - repaint: full-screen redraws (every row changes) arriving one per frame;
 *   - append: one new line of output per frame while at the bottom;
 *   - altstream: a full-screen program whose screen scrolls a line per output.
 *   - floor: a control that only blits a ready image each frame, the cheapest
 *     possible scroll frame, to show what the environment itself costs.
 * Per frame it reports the time spent in the renderer's animation-frame work
 * and touch handling (script and canvas calls), and from a trace the whole
 * main-thread rendering task (script, style, layout, paint, commit).
 * Before measuring, every path runs once unthrottled so the JIT is warm, as it
 * is after the first few seconds of real use.
 *
 * Run: npm run terminal:bench [-- --throttle 4 --runs 3 --json out.json]
 *      --revision <git rev>  measures that commit's renderer instead
 *      --ab <git rev>        measures that commit and the working tree in turn,
 *                            alternating run by run, so load changes hit both
 *      --gpu-canvas          keeps Chrome's GPU 2D canvas (default: software,
 *                            closer to a phone WebView's CPU-drawn 2D canvas)
 *      --renderer <auto|2d|gpu>  which renderer the working tree draws with
 *                            (default auto: WebGL2 when available); the table
 *                            header says which one each variant really used
 *      --profile <scenario> [--cpu]  prints where that scenario spent its time
 */
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { pathToFileURL } from 'node:url';
import { launchChrome, type Json } from './headless-chrome';

const argument = (name: string, fallback: string): string => {
  const index = process.argv.indexOf(`--${name}`);
  return index > 0 ? process.argv[index + 1] ?? fallback : fallback;
};
const THROTTLE = Number(argument('throttle', '4'));
const RUNS = Number(argument('runs', '3'));
const JSON_OUT = argument('json', '');
/** `--profile scroll` prints where the main thread spent that scenario's time. */
const PROFILE = argument('profile', '');
const REVISION = argument('revision', '');
/** `--ab <rev>` measures that commit and the working tree alternately in one browser, run by run. */
const AB = argument('ab', '');
/** `--only repaint,append` runs just those scenarios (the floor control always runs). */
const ONLY = argument('only', '').split(',').filter(Boolean);
/** With `--profile`, also sample the CPU (slows the run; use for where, not how much). */
const CPU = process.argv.includes('--cpu');
const [ROWS, COLS] = argument('grid', '50x100').split('x').map(Number) as [number, number];
const HISTORY = 3000;
const RENDERER = argument('renderer', 'auto') as 'auto' | '2d' | 'gpu';

interface TraceEvent { name: string; ph: string; ts: number; dur?: number; tid: number; pid: number; args?: { name?: string } }

/** Coloured, styled history like an agent transcript or a build log. */
function history(lines: number): string {
  const words = ['compile', 'dragent_terminal', 'warning:', 'unused', 'variable', '`scroll`', 'src/lib.rs:42:7', 'ok', 'FAILED', 'test', 'result', '→', 'done', 'agent', 'tool_call', 'edit_file', 'Read', '12.4ms'];
  let out = '';
  for (let line = 0; line < lines; line++) {
    let text = `\x1b[2m${String(line).padStart(5, ' ')}\x1b[0m `;
    let width = 6, n = line;
    while (width < COLS - 14) {
      n = (n * 1103515245 + 12345) & 0x7fffffff;
      const word = words[n % words.length] ?? 'x';
      const style = n % 11;
      const sgr = style === 0 ? '\x1b[1;31m' : style === 1 ? `\x1b[38;5;${n % 256}m` : style === 2 ? `\x1b[38;2;${n % 256};${(n >> 8) % 256};${(n >> 16) % 256}m`
        : style === 3 ? '\x1b[4;36m' : style === 4 ? '\x1b[3;33m' : style === 5 ? '\x1b[7m' : style === 6 ? '\x1b[42;30m' : style === 7 ? '\x1b[1;34m' : '';
      text += `${sgr}${word}\x1b[0m `;
      width += word.length + 1;
    }
    if (line % 17 === 0) text += '界面 ✅';
    out += `${text}\r\n`;
  }
  return out;
}
/** A full-screen redraw on the primary screen: every row changes. */
function fullScreen(frame: number): string {
  let out = '\x1b[H';
  for (let row = 0; row < ROWS; row++) {
    let text = '';
    for (let col = 0; col < COLS - 10; col += 10) text += `\x1b[38;5;${(frame + row + col) % 256}m${`r${row}f${frame}c${col}`.padEnd(10, '.')}`;
    out += `${text}\x1b[0m\x1b[K${row < ROWS - 1 ? '\r\n' : ''}`;
  }
  return out;
}

/**
 * Times the renderer's own animation-frame callbacks (and the bench's touch
 * driver, whose time is mostly the renderer's touch handlers), by name, so the
 * bench's frame-waiting helpers do not count as frames.
 */
const INSTRUMENT = `(() => {
  const raf = window.requestAnimationFrame.bind(window);
  const work = new Set(['tick', 'paint', 'flushScroll', 'move', 'floorFrame']);
  const bench = window.__bench = { frames: [], recording: false, errors: 0 };
  // The renderer reports a fault by posting an error message; a run with any is not valid.
  window.addEventListener('message', (event) => { try { if (JSON.parse(event.data).type === 'error') bench.errors++; } catch {} });
  window.requestAnimationFrame = (callback) => raf((time) => {
    const start = performance.now();
    try { callback(time); } finally {
      if (bench.recording && work.has(callback.name)) {
        const spent = performance.now() - start;
        const last = bench.frames[bench.frames.length - 1];
        if (last && last.time === time) last.ms += spent; else bench.frames.push({ time, ms: spent });
      }
    }
  });
})();`;

/** The terminal document of the working tree, or of a commit. */
async function loadDocument(dir: string, revision: string): Promise<(id: string) => string> {
  if (!revision) {
    const { terminalDocument } = await import('../src/terminal/runtime/document');
    return (id) => terminalDocument(id, { renderer: RENDERER });
  }
  const copy = join(dir, `revision-${revision.replace(/[^a-zA-Z0-9]/gu, '_')}`);
  await mkdir(copy, { recursive: true });
  for (const file of ['document.ts', 'types.ts', 'wasmAsset.generated.ts']) {
    const text = execFileSync('git', ['show', `${revision}:mobile/src/terminal/runtime/${file}`], { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
    await writeFile(join(copy, file), text);
  }
  return ((await import(pathToFileURL(join(copy, 'document.ts')).href)) as { terminalDocument: (id: string) => string }).terminalDocument;
}

const percentile = (values: number[], p: number): number => {
  if (values.length === 0) return 0;
  const sorted = [...values].sort((a, b) => a - b);
  return sorted[Math.min(sorted.length - 1, Math.ceil(p * sorted.length) - 1)] ?? 0;
};
const stats = (values: number[]) => ({ frames: values.length, p50: +percentile(values, 0.5).toFixed(2), p95: +percentile(values, 0.95).toFixed(2), max: +percentile(values, 1).toFixed(2) });

async function main(): Promise<void> {
  const dir = await mkdtemp(join(tmpdir(), 'terminal-bench-'));
  type Results = Record<string, { callback: ReturnType<typeof stats>; task: ReturnType<typeof stats>; frames: number[] }[]>;
  const variants: Array<{ label: string; page: string; results: Results; drawnWith?: string }> = [];
  for (const [label, revision] of AB ? [['before', AB], ['after', '']] : [[REVISION || 'working tree', REVISION]]) {
    const page = join(dir, `terminal-${variants.length}.html`);
    await writeFile(page, (await loadDocument(dir, revision ?? ''))('bench'));
    variants.push({ label: label ?? '', page, results: {} });
  }
  const browser = await launchChrome(join(dir, 'profile'), [
    // A headless page counts as a background one, whose renderer gets a low
    // priority and coalesced timers: that turns the CPU throttler's short
    // pauses into stalls of hundreds of ms that have nothing to do with the page.
    '--disable-renderer-backgrounding', '--disable-background-timer-throttling', '--disable-backgrounding-occluded-windows',
    '--disable-features=IntensiveWakeUpThrottling,CalculateNativeWinOcclusion',
    ...(process.argv.includes('--gpu-canvas') ? [] : ['--disable-accelerated-2d-canvas'])]);
  const cdp = browser.cdp;
  try {
    for (let run = 0; run < RUNS; run++) for (const variant of run % 2 ? [...variants].reverse() : variants) {
      const { page, results } = variant;
      const { targetId } = await cdp.send('Target.createTarget', { url: 'about:blank' }) as { targetId: string };
      const { sessionId } = await cdp.send('Target.attachToTarget', { targetId, flatten: true }) as { sessionId: string };
      const send = (method: string, params: Json = {}) => cdp.send(method, params, sessionId);
      const evaluate = async (expression: string): Promise<unknown> => {
        const reply = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true }) as { result: { value?: unknown }; exceptionDetails?: { text: string; exception?: { description?: string } } };
        if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.exception?.description ?? reply.exceptionDetails.text);
        return reply.result.value;
      };
      await send('Page.enable'); await send('Runtime.enable');
      await send('Emulation.setDeviceMetricsOverride', { width: 390, height: 760, deviceScaleFactor: 3, mobile: true });
      await send('Emulation.setTouchEmulationEnabled', { enabled: true, maxTouchPoints: 1 });
      await send('Page.addScriptToEvaluateOnNewDocument', { source: INSTRUMENT });
      const loaded = new Promise<void>((resolve) => cdp.on('Page.loadEventFired', () => resolve()));
      await send('Page.navigate', { url: pathToFileURL(page).href });
      await loaded;
      await evaluate(`new Promise((resolve) => { const check = () => document.getElementById('screen') ? resolve() : setTimeout(check, 20); check(); })`);
      const palette = { background: '#101010', foreground: '#eeeeee', cursor: '#abcdef',
        ansiNormal: ['#000000', '#cc0000', '#00cc00', '#cccc00', '#0000cc', '#cc00cc', '#00cccc', '#cccccc'],
        ansiBright: ['#555555', '#ff0000', '#00ff00', '#ffff00', '#0000ff', '#ff00ff', '#00ffff', '#ffffff'] };
      // Page helpers: post a raw packet, wait frames, drive touches.
      await evaluate(`(() => {
        const palette = ${JSON.stringify(palette)};
        let seq = 0, raw = 0;
        const b64 = (text) => { const bytes = new TextEncoder().encode(text); let s = ''; for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000)); return btoa(s); };
        window.__post = (text, reset) => {
          const chunks = [];
          for (let i = 0; i < text.length; i += 200000) chunks.push({ seq: ++raw, data: b64(text.slice(i, i + 200000)) });
          seq++;
          window.postMessage(JSON.stringify({ v: 1, id: 'bench', type: 'state', epoch: 1, seq,
            frame: { epoch: 1, revision: seq, presentation: 'raw', reason: null, rawReset: !!reset, rawChunks: chunks, lastRawSeq: raw, rows: ${ROWS}, cols: ${COLS}, grid: null },
            environment: { connected: true, canWrite: true, canResize: false, holdFit: false, palette } }), '*');
        };
        window.__frames = (n) => new Promise((resolve) => { const step = () => (n-- <= 0 ? resolve() : requestAnimationFrame(step)); requestAnimationFrame(step); });
        const canvas = document.getElementById('screen');
        const touch = (type, y) => {
          const point = new Touch({ identifier: 1, target: canvas, clientX: 200, clientY: y });
          canvas.dispatchEvent(new TouchEvent(type, { touches: type === 'touchend' ? [] : [point], changedTouches: [point], bubbles: true, cancelable: true }));
        };
        window.__drag = (from, step, frames, release) => new Promise((resolve) => {
          let y = from, left = frames;
          touch('touchstart', y);
          // The finger lifts straight after its last move, as a flick does; a
          // frame later would read as a finger that stopped before lifting.
          const move = () => {
            y += step; touch('touchmove', y);
            if (--left > 0) { requestAnimationFrame(move); return; }
            if (release) touch('touchend', y);
            resolve();
          };
          requestAnimationFrame(move);
        });
      })()`);
      await evaluate(`window.__post(${JSON.stringify(history(HISTORY))}, true)`);
      await evaluate('window.__frames(30)');
      // Which renderer drew: a revision from before the GPU renderer has only the 2D one.
      variant.drawnWith ??= String(await evaluate(`document.getElementById('screen').getAttribute('data-renderer') ?? '2d'`));
      const screens = Array.from({ length: 60 }, (_, frame) => fullScreen(frame));
      // Warm-up: every measured path once, unthrottled.
      await evaluate(`(async () => {
        await window.__drag(150, 6, 60, true); await window.__frames(5);
        document.getElementById('latest').click(); await window.__frames(5);
        await window.__drag(150, 40, 6, true); await window.__frames(60);
        document.getElementById('latest').click(); await window.__frames(5);
        for (const screen of ${JSON.stringify(screens.slice(0, 5))}) { window.__post(screen); await window.__frames(1); }
        for (let i = 0; i < 5; i++) { window.__post('\\x1b[999;1H\\r\\nwarm ' + i); await window.__frames(1); }
        await window.__frames(10);
      })()`);
      await send('Emulation.setCPUThrottlingRate', { rate: THROTTLE });
      await evaluate('window.__frames(10)');

      const trace: TraceEvent[] = [];
      cdp.on('Tracing.dataCollected', (params) => { trace.push(...(params.value as TraceEvent[])); });
      const scenario = async (name: string, body: string): Promise<void> => {
        if (ONLY.length > 0 && name !== 'floor' && !ONLY.includes(name)) return;
        trace.length = 0;
        await send('Tracing.start', { categories: PROFILE ? 'devtools.timeline,disabled-by-default-devtools.timeline,v8,disabled-by-default-v8.gc,v8.execute,blink.user_timing' : 'devtools.timeline,disabled-by-default-devtools.timeline', transferMode: 'ReportEvents' });
        await evaluate('window.__bench.frames = []; window.__bench.recording = true');
        if (PROFILE === name && run === 0 && CPU) { await send('Profiler.enable'); await send('Profiler.setSamplingInterval', { interval: 100 }); await send('Profiler.start'); }
        await evaluate(body);
        if (PROFILE === name && run === 0 && CPU) {
          const { profile } = await send('Profiler.stop') as { profile: { nodes: Array<{ id: number; callFrame: { functionName: string; lineNumber: number }; hitCount?: number }>; samples: number[]; timeDeltas: number[] } };
          const self = new Map<string, number>();
          const byId = new Map(profile.nodes.map((node) => [node.id, node]));
          profile.samples.forEach((sampleId, index) => {
            const node = byId.get(sampleId);
            const key = node ? `${node.callFrame.functionName || '(anonymous)'}:${node.callFrame.lineNumber}` : '?';
            self.set(key, (self.get(key) ?? 0) + (profile.timeDeltas[index] ?? 0) / 1000);
          });
          console.log(`Self time in ${name} (ms):`);
          console.table([...self.entries()].sort((a, b) => b[1] - a[1]).slice(0, 25).map(([fn, ms]) => ({ fn, ms: +ms.toFixed(1) })));
        }
        const callback = await evaluate('window.__bench.recording = false; window.__bench.frames.map((f) => f.ms)') as number[];
        const errors = await evaluate('window.__bench.errors') as number;
        if (errors > 0) throw new Error(`the renderer reported ${errors} fault(s) during ${name}`);
        const complete = new Promise<void>((resolve) => cdp.on('Tracing.tracingComplete', () => resolve()));
        await send('Tracing.end');
        await complete;
        const main = trace.find((event) => event.name === 'thread_name' && event.args?.name === 'CrRendererMain' && trace.some((other) => other.pid === event.pid && other.name === 'FireAnimationFrame'));
        const tasks = trace.filter((event) => main && event.pid === main.pid && event.tid === main.tid && event.ph === 'X');
        const frames = tasks.filter((event) => event.name === 'RunTask' && tasks.some((inner) => inner.name === 'FireAnimationFrame' && inner.ts >= event.ts && inner.ts <= event.ts + (event.dur ?? 0)));
        if (PROFILE === name && run === 0) {
          const spent = new Map<string, number>();
          for (const event of tasks) if (event.name !== 'RunTask' && frames.some((frame) => event.ts >= frame.ts && event.ts <= frame.ts + (frame.dur ?? 0))) spent.set(event.name, (spent.get(event.name) ?? 0) + (event.dur ?? 0) / 1000);
          for (const frame of [...frames].sort((a, b) => (b.dur ?? 0) - (a.dur ?? 0)).slice(1, 6)) {
            const inside = tasks.filter((event) => event !== frame && event.ts >= frame.ts && event.ts <= frame.ts + (frame.dur ?? 0) && (event.dur ?? 0) > 1000);
            console.log(`slow frame ${((frame.dur ?? 0) / 1000).toFixed(1)} ms:`, inside.map((event) => `${event.name} ${((event.dur ?? 0) / 1000).toFixed(1)}`).join(', '));
          }
          console.log(`Trace events inside ${name} frames (ms, nested events overlap):`);
          console.table([...spent.entries()].sort((a, b) => b[1] - a[1]).slice(0, 20).map(([event, ms]) => ({ event, ms: +ms.toFixed(1) })));
        }
        (results[name] ??= []).push({ callback: stats(callback), task: stats(frames.map((event) => (event.dur ?? 0) / 1000)), frames: callback.map((ms) => +ms.toFixed(2)) });
      };
      // Control: the cheapest scroll frame there can be, one full-screen blit of
      // a ready image per frame. Whatever this costs is the environment's floor.
      await scenario('floor', `(async () => {
        const screen = document.getElementById('screen'), w = screen.width, h = screen.height;
        const overlay = document.createElement('canvas'); overlay.width = w; overlay.height = h;
        overlay.style.cssText = 'position:absolute;inset:0;width:100%;height:100%;pointer-events:none';
        document.body.appendChild(overlay);
        const image = document.createElement('canvas'); image.width = w; image.height = h + 64;
        const draw = image.getContext('2d'); draw.fillStyle = '#123'; draw.fillRect(0, 0, w, h + 64);
        const g = overlay.getContext('2d');
        for (let i = 0; i < 240; i++) await new Promise((done) => requestAnimationFrame(function floorFrame() { g.drawImage(image, 0, i % 64, w, h, 0, 0, w, h); done(); }));
        overlay.remove();
      })()`);
      // Drag up through history: 6 px per frame for 4 seconds.
      await scenario('scroll', 'window.__drag(150, 6, 240, true).then(() => window.__frames(5))');
      await evaluate(`document.getElementById('latest').click()`);
      await evaluate('window.__frames(10)');
      // A quick flick, then the frames that follow the release.
      await scenario('fling', 'window.__drag(150, 40, 6, true).then(() => window.__frames(90))');
      await evaluate(`document.getElementById('latest').click()`);
      await evaluate('window.__frames(10)');
      await scenario('repaint', `(async () => { const screens = ${JSON.stringify(screens)}; for (const screen of screens) { window.__post(screen); await window.__frames(1); } await window.__frames(3); })()`);
      await scenario('append', `(async () => { for (let i = 0; i < 60; i++) { window.__post('\\x1b[999;1H\\r\\n\\x1b[32mappended line ' + i + '\\x1b[0m'); await window.__frames(1); } await window.__frames(3); })()`);
      // A full-screen program streaming: each output scrolls its screen one line.
      const altScreen = '\x1b[?1049h\x1b[H' + Array.from({ length: ROWS }, (_, row) => `\x1b[3${row % 8}mscreen line ${row}\x1b[0m`).join('\r\n');
      await evaluate(`(async () => { window.__post(${JSON.stringify(altScreen)}); await window.__frames(5); })()`);
      const streamed = Array.from({ length: 60 }, (_, i) => `\x1b[${ROWS};1H\r\n\x1b[36mstreamed ${i} \x1b[33m${'token '.repeat(12)}\x1b[0m`);
      await scenario('altstream', `(async () => { for (const text of ${JSON.stringify(streamed)}) { window.__post(text); await window.__frames(1); } await window.__frames(3); })()`);
      await send('Emulation.setCPUThrottlingRate', { rate: 1 });
      await cdp.send('Target.closeTarget', { targetId });
    }
  } finally {
    await browser.close();
    await rm(dir, { recursive: true, force: true, maxRetries: 5 });
  }
  // Each run gives a p50 and a p95; the table shows the median of those across runs.
  const median = (values: number[]) => +([...values].sort((a, b) => a - b)[Math.floor(values.length / 2)] ?? 0).toFixed(2);
  const canvasKind = process.argv.includes('--gpu-canvas') ? 'GPU' : 'software';
  console.log(`Terminal renderer frame cost (ms; median over ${RUNS} runs of each run's p50/p95; ${THROTTLE}x CPU throttle, ${canvasKind} canvas, ${ROWS}x${COLS}, ${HISTORY} lines${AB ? `; before = ${AB}, after = working tree, interleaved` : ''}; drawn with ${variants.map((variant) => `${variant.label}: ${variant.drawnWith ?? '?'}`).join(', ')}):`);
  const summary: Record<string, Json> = {};
  for (const name of Object.keys(variants[0]?.results ?? {})) {
    for (const { label, results } of variants) {
      const runs = results[name] ?? [];
      summary[variants.length > 1 ? `${name} ${label}` : name] = {
        frames: runs[0]?.callback.frames,
        callbackP50: median(runs.map((run) => run.callback.p50)), callbackP95: median(runs.map((run) => run.callback.p95)),
        taskP50: median(runs.map((run) => run.task.p50)), taskP95: median(runs.map((run) => run.task.p95)),
      };
    }
  }
  console.table(summary);
  if (JSON_OUT) await writeFile(JSON_OUT, JSON.stringify({ throttle: THROTTLE, runs: RUNS, ab: AB, grid: `${ROWS}x${COLS}`, summary, variants: variants.map(({ label, results, drawnWith }) => ({ label, drawnWith, results })) }, null, 2));
}

main().catch((error: unknown) => { console.error(error); process.exitCode = 1; });
