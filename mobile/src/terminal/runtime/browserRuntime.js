/**
 * Runs only inside the isolated terminal document. It never creates a socket or fetches a URL.
 * `options.renderer` is 'auto' (WebGL2 when it works, else the 2D canvas), '2d' or 'gpu'.
 */
export function browserRuntime(Terminal, id, limits, kit, options = {}, gk = null, makeGpu = null) {
  const canvas = document.getElementById('screen');
  const input = document.getElementById('keys');
  const readable = document.getElementById('readable');
  const status = document.getElementById('status');
  const latest = document.getElementById('latest');
  const context = canvas.getContext('2d');
  let terminal = null, hostGrid = null, epoch = -1, received = 0, rawSeq = 0, state = null, paletteKey = '';
  // `info` is the emulator's shape as of the last paint; `contentDirty` says output arrived since.
  let info = null, contentDirty = true;
  // History scroll position: device pixels above the live bottom, so drawing never resamples.
  let scrollPx = 0, fling = null, touch = null, dragged = false, samples = [];
  // `view` is how the last paint laid the grid out, so a gesture acts on what is on screen.
  let view = null, layout = null, ring = null, scrollPixels = 0, scrollPoint = { x: 0, y: 0 }, scrollRoute = null;
  let composing = false, ticking = false, needsPaint = false, disposed = false;
  // What the last paint showed.
  let shown = null;
  let lastSize = '', lastMetrics = '', lastStatus = null, lastReadable = '', latestShown = null;
  // ── Renderer choice ────────────────────────────────────────────────────────
  // The GPU draws on a canvas of its own laid over the screen canvas, which
  // keeps every touch, focus and accessibility duty either way: a canvas that
  // has handed out a WebGL context can never become a 2D one, so this is what
  // lets the 2D renderer take over at any moment.
  const requested = options && ['auto', '2d', 'gpu'].includes(options.renderer) ? options.renderer : 'auto';
  let gpu = null, gpuCanvas = null, gpuFailed = false, gpuLosses = 0, drawnWith = '';
  const rendererNow = () => gk ? gk.chooseRenderer({ requested, available: gpu !== null, failed: gpuFailed, lost: gpu !== null && gpu.lost, losses: gpuLosses }) : '2d';
  const post = (message) => {
    const text = JSON.stringify({ v: 1, id, epoch, ...message });
    if (window.ReactNativeWebView) window.ReactNativeWebView.postMessage(text);
    else window.parent.postMessage(text, '*');
  };
  // A program on the alternate screen gets at most this many scroll events per
  // frame; the rest of a long swipe follows on the next frames instead of
  // arriving as one burst the program has to chew through.
  const SCROLL_EVENTS_PER_FRAME = 8, TAP_SLOP_PX = 12;
  // How far a glyph may drift from its cell before a run of text is split.
  const DRIFT_PX = 0.25;
  // Canvas size limits that hold on every phone WebView.
  const STRIP_MAX_PX = 8192, STRIP_MAX_AREA = 8 * 1024 * 1024;
  const bytes = (value) => Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
  const usable = () => state && state.frame.presentation !== 'resync' && state.environment.connected && state.environment.canWrite;
  const send = (text) => {
    if (usable() && typeof text === 'string' && text.length && new TextEncoder().encode(text).byteLength <= limits.inputBytes) post({ type: 'input', text });
  };
  const requestTick = () => { if (!ticking) { ticking = true; requestAnimationFrame(tick); } };
  const schedulePaint = () => { needsPaint = true; requestTick(); };

  // ── Host grid ──────────────────────────────────────────────────────────────
  // The host sends styled text spans, not character widths. Let the SAME
  // desktop emulator assign wide/combining graphemes to columns: each span is
  // written with its index as a placeholder colour, and the real colours and
  // attributes are put back afterwards, so transparent backgrounds and all
  // host attributes survive. Rows are laid out only when they are drawn.
  const prepareHostGrid = (grid) => {
    const byIndex = new Map(grid.lines.map((entry) => [entry.index, entry]));
    const lines = [];
    for (let row = 0; row < grid.rows; row++) {
      const spans = byIndex.get(row)?.spans || [];
      lines.push({ key: JSON.stringify(spans), spans });
    }
    return { rows: grid.rows, cols: grid.columns, lines, cursor_row: grid.cursorRow, cursor_col: grid.cursorColumn, cursor_visible: grid.cursorVisible };
  };
  const layoutHostRow = (spans, cols) => {
    const length = spans.reduce((sum, span) => sum + span.text.length, 0);
    const parser = new Terminal(1, Math.max(cols + 1, length * 2 + 1));
    const encoder = new TextEncoder();
    try {
      for (let index = 0; index < spans.length; index++) {
        // Display data must never become terminal control commands. Layout
        // escape sequences are generated here; host C0/C1 bytes are inert.
        // oxlint-disable-next-line no-control-regex -- Control bytes in presentation text must remain inert.
        const text = spans[index].text.replace(/[\x00-\x1f\x7f-\x9f]/gu, ' ');
        parser.write(encoder.encode(`\x1b[38;5;${index};48;5;${index}m${text}`));
      }
      return JSON.parse(parser.rows_compact(0, 1));
    } finally { parser.free(); }
  };
  const hostStyle = (spans) => (fg) => {
    const span = spans[fg];
    return { fg: span?.fg || null, bg: span?.bg || null, flags: span?.flags || 0 };
  };

  // ── Row images ─────────────────────────────────────────────────────────────
  // Every drawn row is kept as a picture in a ring of slots on offscreen
  // canvases, slot = line id modulo the slot count. A scroll frame is then a
  // couple of drawImage blits plus whatever rows newly came into view. A line
  // id is stable across output (evictions included), and a slot remembers the
  // exact row text it was drawn from, so a recheck redraws only what changed.
  // The GPU keeps its rows as instance data instead, so its ring has no pictures.
  const makeRing = (key, rows, widthPx, rowPx, pictures) => {
    const margin = Math.ceil(rows / 4) + 2, count = rows + 1 + 2 * margin;
    const perStrip = Math.max(1, Math.min(count, Math.floor(STRIP_MAX_PX / rowPx), Math.floor(STRIP_MAX_AREA / (widthPx * rowPx))));
    const strips = [];
    const old = ring && ring.widthPx === widthPx && ring.rowPx === rowPx && ring.perStrip === perStrip ? ring.strips : [];
    for (let first = 0; pictures && first < count; first += perStrip) {
      const reuse = old[strips.length];
      const strip = reuse || document.createElement('canvas');
      const height = Math.min(perStrip, count - first) * rowPx;
      if (strip.width !== widthPx) strip.width = widthPx;
      if (strip.height !== height) strip.height = height;
      strips.push(strip);
    }
    if (ring) for (const strip of ring.strips) if (!strips.includes(strip)) { strip.width = 0; strip.height = 0; }
    const contexts = strips.map((strip) => strip.getContext('2d'));
    return { key, widthPx, rowPx, count, perStrip, strips, contexts, slots: Array.from({ length: count }, () => ({ id: -1, key: '', text: '', runs: [], picture: false, gpu: false, stale: false })), serial: 0 };
  };
  const slotOf = (lineId) => ring.slots[lineId % ring.count];
  // Font advances, measured once per font and glyph on a canvas of their own:
  // measuring on a canvas being drawn on would change its font behind the
  // drawing code's back.
  let advances = new Map(), measurer = null;
  const measure = (fontName, text) => {
    measurer ||= document.createElement('canvas').getContext('2d');
    if (measurer.font !== fontName) measurer.font = fontName;
    return measurer.measureText(text).width;
  };
  const advance = (fontName, glyph) => {
    let widths = advances.get(fontName);
    if (!widths) { widths = new Map(); advances.set(fontName, widths); }
    let width = widths.get(glyph);
    if (width === undefined) { width = measure(fontName, glyph); widths.set(glyph, width); }
    return width;
  };
  /**
   * Paint one row's runs onto `g` with its top at device pixel `y`. A row
   * picture is clipped to its slot, whose neighbour holds an unrelated line;
   * on the screen the next row's own background covers any overhang.
   */
  const drawRow = (g, y, runs, clip = true) => {
    const palette = state.environment.palette, { cell, line, size, scale, dpr } = layout;
    g.setTransform(1, 0, 0, 1, 0, 0);
    g.globalAlpha = 1;
    g.fillStyle = palette.background; g.fillRect(0, y, ring.widthPx, ring.rowPx);
    if (runs.length === 0) return;
    g.save();
    if (clip) { g.beginPath(); g.rect(0, y, ring.widthPx, ring.rowPx); g.clip(); }
    g.setTransform(dpr, 0, 0, dpr, 0, y);
    // Backgrounds precede glyphs, so a wide character's neighbour cannot
    // erase the second half of its glyph.
    for (const run of runs) if (run.bg) { g.fillStyle = run.bg; g.fillRect(run.col * cell, 0, run.cells * cell, line); }
    g.textBaseline = 'top';
    let fontNow = '', fillNow = '', alphaNow = 1;
    const rule = Math.max(1, scale);
    for (const run of runs) {
      if (run.flags & 128) continue;
      const fontName = kit.font(run.flags, size);
      const pieces = run.single ? [{ col: run.col, text: run.text }]
        : kit.segments(run.text, run.col, (glyph) => advance(fontName, glyph), cell, DRIFT_PX);
      const decorated = run.flags & (8 | 16 | 32 | 256 | 1024);
      if (pieces.length === 0 && !decorated) continue;
      if (fontName !== fontNow) { g.font = fontName; fontNow = fontName; }
      if (run.fg !== fillNow) { g.fillStyle = run.fg; fillNow = run.fg; }
      const fade = kit.alpha(run.flags);
      if (fade !== alphaNow) { g.globalAlpha = fade; alphaNow = fade; }
      for (const piece of pieces) g.fillText(piece.text, piece.col * cell, 2 * scale);
      if (decorated) {
        const x = run.col * cell, width = run.cells * cell;
        if (run.flags & 56) g.fillRect(x, line - 2 * scale, width, rule);
        if (run.flags & 16) g.fillRect(x, line - 4 * scale, width, rule);
        if (run.flags & 256) g.fillRect(x, line / 2, width, rule);
        if (run.flags & 1024) g.fillRect(x, 0, width, rule);
      }
    }
    g.restore();
  };
  /** Draw a slot's row into its place on the ring. */
  const drawPicture = (slotIndex) => {
    const slot = ring.slots[slotIndex];
    drawRow(ring.contexts[Math.floor(slotIndex / ring.perStrip)], (slotIndex % ring.perStrip) * ring.rowPx, slot.runs);
    slot.picture = true;
  };
  /**
   * Bring the slots of lines `first..first + count` (in the screen's own
   * numbering) up to date, reading only rows that are missing or may have
   * changed. Returns how many of them still need a picture drawn.
   */
  const prepareRows = (first, count, base) => {
    const palette = state.environment.palette, cols = layout.cols;
    let index = 0, unpainted = 0;
    while (index < count) {
      const slot = slotOf(base + first + index);
      if (slot.id === base + first + index && !slot.stale) { if (!slot.picture) unpainted++; index++; continue; }
      let length = 1;
      while (index + length < count) {
        const next = slotOf(base + first + index + length);
        if (next.id === base + first + index + length && !next.stale) break;
        length++;
      }
      const start = first + index;
      const rows = hostGrid ? hostGrid.lines.slice(start, start + length) : terminal.rows_compact(start, length).split('\n');
      for (let offset = 0; offset < length; offset++) {
        const lineId = base + start + offset, slot = slotOf(lineId);
        const row = rows[offset];
        const key = hostGrid ? (row ? row.key : '[]') : row || '[]';
        slot.stale = false;
        if (slot.id !== lineId || slot.key !== key) {
          const decoded = hostGrid
            ? kit.decodeRow(row ? layoutHostRow(row.spans, cols) : [], palette, cols, row ? hostStyle(row.spans) : undefined)
            : kit.decodeRow(JSON.parse(key), palette, cols);
          slot.id = lineId; slot.key = key; slot.text = decoded.text; slot.runs = decoded.runs; slot.picture = false; slot.gpu = false; ring.serial++;
        }
        if (!slot.picture) unpainted++;
      }
      index += length;
    }
    return unpainted;
  };
  /**
   * Put the visible lines on screen, cropping the part rows above or below
   * the grid. Rows are copied from their pictures; when most of the screen
   * changed at once and nothing is scrolled, changed rows are drawn straight
   * onto the screen instead, which saves drawing them twice. Their pictures
   * are made later, if a scroll needs them.
   */
  const present = (first, count, base, topPx, sub, rows, direct) => {
    const bottom = topPx + rows * ring.rowPx;
    let index = 0;
    while (index < count) {
      const slot = (base + first + index) % ring.count;
      if (!ring.slots[slot].picture) {
        if (direct) { drawRow(context, topPx + index * ring.rowPx, ring.slots[slot].runs, false); index++; continue; }
        drawPicture(slot);
      }
      const strip = Math.floor(slot / ring.perStrip), local = slot % ring.perStrip;
      let length = 1;
      while (index + length < count && (base + first + index + length) % ring.count === slot + length && Math.floor((slot + length) / ring.perStrip) === strip) {
        if (!ring.slots[slot + length].picture) { if (direct) break; drawPicture(slot + length); }
        length++;
      }
      let y = topPx + index * ring.rowPx - sub, from = local * ring.rowPx, height = length * ring.rowPx;
      if (y < topPx) { from += topPx - y; height -= topPx - y; y = topPx; }
      if (y + height > bottom) height = bottom - y;
      if (height > 0) context.drawImage(ring.strips[strip], 0, from, ring.widthPx, height, 0, y, ring.widthPx, height);
      index += length;
    }
    context.setTransform(1, 0, 0, 1, 0, 0);
  };

  // ── Screen state ───────────────────────────────────────────────────────────
  /**
   * The emulator's current shape. When output has arrived, rows that were on
   * the screen may have changed (they get rechecked when next drawn), and a
   * view scrolled into history moves with its text so that text holds still.
   */
  const readInfo = () => {
    if (!contentDirty && info) return info;
    contentDirty = false;
    const next = JSON.parse(terminal.view_info());
    if (info && ring) {
      const before = info.evicted + info.scrollback_len, after = next.evicted + next.scrollback_len;
      for (const slot of ring.slots) if (slot.id >= before) slot.stale = true;
      if (after !== before && !next.alt_screen && !info.alt_screen && layout) {
        if (scrollPx > 0.5) scrollPx += (after - before) * layout.rowPx;
        // New output at the bottom ends a fling that was settling there.
        else if (fling) fling = null;
      }
    }
    info = next;
    return info;
  };
  // Distance saved for one mode means nothing in another: half a page left over
  // in a pager must not scroll the shell it returned to. Decided here, not when
  // the distance is spent, so it never eats the first move of the next swipe.
  const setView = (next) => {
    view = next;
    const route = scrollRouteNow();
    if (route !== scrollRoute) { scrollRoute = route; scrollPixels = 0; if (route !== 'history') { fling = null; scrollPx = 0; } }
  };
  const showLatest = (shown) => { if (shown !== latestShown) { latestShown = shown; latest.style.display = shown ? 'block' : 'none'; } };
  const setReadable = (text) => { if (text !== lastReadable) { lastReadable = text; readable.textContent = text; } };
  const animating = () => fling !== null || (touch !== null && dragged);

  const paint = () => {
    needsPaint = false;
    if (!state || disposed) return;
    const frame = state.frame, palette = state.environment.palette;
    const width = Math.max(1, canvas.clientWidth), height = Math.max(1, canvas.clientHeight), dpr = Math.min(3, window.devicePixelRatio || 1);
    const blocked = frame.presentation === 'resync';
    const statusKey = blocked ? `${frame.reason}\n${palette.foreground}\n${palette.background}` : '';
    if (statusKey !== lastStatus) {
      lastStatus = statusKey;
      status.textContent = blocked ? frame.reason || 'Reconnect to resynchronize this terminal.' : '';
      status.style.display = blocked ? 'flex' : 'none';
      status.style.color = palette.foreground;
      status.style.backgroundColor = palette.background;
    }
    // Resizing a canvas reallocates it, so only a real change of size does.
    // While the GPU draws, the screen canvas under it keeps a single pixel.
    const widthPx = Math.round(width * dpr), heightPx = Math.round(height * dpr);
    const kind = rendererNow(), onGpu = kind === 'gpu';
    if (kind !== drawnWith) {
      drawnWith = kind;
      // Rows start again in the new renderer's form (pictures or instance data).
      if (ring) ring.key = '';
      if (gpuCanvas) gpuCanvas.style.display = onGpu ? 'block' : 'none';
      if (canvas.setAttribute) canvas.setAttribute('data-renderer', kind);
    }
    const canvasW = onGpu ? 1 : widthPx, canvasH = onGpu ? 1 : heightPx;
    if (canvas.width !== canvasW) canvas.width = canvasW;
    if (canvas.height !== canvasH) canvas.height = canvasH;
    context.setTransform(1, 0, 0, 1, 0, 0);
    context.globalAlpha = 1;
    context.fillStyle = palette.background;
    const screen = blocked ? null : hostGrid || (terminal ? readInfo() : null);
    if (!screen) {
      if (onGpu) { gpu.layout(widthPx, heightPx, null); gpu.clear(palette.background); }
      else context.fillRect(0, 0, widthPx, heightPx);
      if (blocked) setReadable('');
      setView(null); showLatest(false); return;
    }
    const rows = screen.rows, cols = screen.cols;
    const layoutKey = `${rows}x${cols}:${width}x${height}@${dpr}`;
    if (!layout || layout.key !== layoutKey) {
      const naturalCell = measure(kit.font(0, 14), 'M');
      const scale = Math.min(1, width / (cols * naturalCell), height / (rows * 20));
      // Rows sit on whole device pixels, so row pictures blit without resampling.
      const rowPx = Math.max(1, Math.floor(20 * scale * dpr)), line = rowPx / dpr;
      // Top-anchored: the first row sits immediately on the tab strip's
      // bottom border line, with no padding or margin above the grid. Any
      // leftover sub-line space collects at the bottom, never split above.
      const top = 0;
      if (layout && scrollPx > 0) scrollPx = scrollPx / layout.rowPx * rowPx;
      layout = { key: layoutKey, rows, cols, naturalCell, scale, cell: naturalCell * scale, line, rowPx, size: 14 * scale, top, dpr };
      advances = new Map();
    }
    const { cell, line, rowPx, top, naturalCell, scale } = layout;
    const topPx = Math.round(top * dpr);
    // A host grid also carries the primary screen after a stream gap, so only
    // the reported modes can say it is full-screen.
    const modes = hostGrid ? state.frame.screenModes : null;
    const alt = hostGrid ? modes?.alternateScreen === true : screen.alt_screen === true;
    const history = hostGrid ? 0 : screen.scrollback_len || 0;
    setView({
      host: hostGrid !== null, alt, rows, cols, cell, line, top, history, dpr,
      mouse: hostGrid ? modes?.mouseReporting === true : screen.mouse_reporting === true,
      sgr: hostGrid ? modes?.sgrMouse === true : screen.sgr_mouse === true,
    });
    const space = hostGrid ? 'host' : alt ? 'alt' : `primary:${screen.rewrite}`;
    const ringKey = `${layoutKey}:${widthPx}:${space}:${paletteKey}:${kind}`;
    if (!ring || ring.key !== ringKey) ring = makeRing(ringKey, rows, widthPx, rowPx, !onGpu);
    // Which lines are on screen: the live screen, or a window into history.
    const scrolling = !hostGrid && !alt;
    const maxPx = history * rowPx;
    if (!scrolling) scrollPx = 0;
    scrollPx = Math.max(0, Math.min(maxPx, scrollPx));
    const offsetPx = Math.round(scrollPx), fromTop = maxPx - offsetPx;
    const first = Math.floor(fromTop / rowPx), sub = fromTop - first * rowPx;
    const count = Math.min(rows + (sub > 0 ? 1 : 0), history + rows - first);
    const base = scrolling ? screen.evicted : 0;
    const unpainted = prepareRows(first, count, base);
    const cursor = offsetPx === 0 && screen.cursor_visible && screen.cursor_row >= 0 && screen.cursor_row < rows && screen.cursor_col >= 0 && screen.cursor_col < cols;
    if (onGpu) {
      try {
        gpu.layout(widthPx, heightPx, { dpr, size: layout.size, scale, line, rowPx, cellPx: cell * dpr, cols });
        gpu.draw({
          ring, cols, rows, topPx, sub, firstSlot: (base + first) % ring.count, visible: count, background: palette.background,
          cursor: cursor ? { col: screen.cursor_col, row: screen.cursor_row, cell, line, top, scale, color: palette.cursor } : null,
        });
      } catch {
        // Anything the GPU cannot do, the 2D canvas does from now on.
        gpuFailed = true; paint(); return;
      }
    } else {
      // Rows cover the grid; only the bands above and below it need clearing.
      if (topPx > 0) context.fillRect(0, 0, widthPx, topPx);
      if (topPx + rows * rowPx < heightPx) context.fillRect(0, topPx + rows * rowPx, widthPx, heightPx - topPx - rows * rowPx);
      const direct = sub === 0 && unpainted * 2 > rows;
      present(first, count, base, topPx, sub, rows, direct);
      if (cursor) {
        context.setTransform(dpr, 0, 0, dpr, 0, 0);
        context.strokeStyle = palette.cursor; context.lineWidth = Math.max(1, scale);
        context.strokeRect(screen.cursor_col * cell + .5, top + screen.cursor_row * line + .5, Math.max(1, cell - 1), Math.max(1, line - 1));
      }
    }
    shown = { first, count, base, ring, sub, topPx };
    drawSelection();
    showLatest(offsetPx > 0);
    // The readable text is the rows mostly on screen. Rebuilding it costs a
    // text layout, so it waits until a scroll settles.
    if (!animating()) {
      const firstShown = Math.min(first + (sub * 2 > rowPx ? 1 : 0), history);
      const key = `${base + firstShown}:${ring.serial}`;
      if (key !== ring.readableKey) {
        ring.readableKey = key;
        const plain = [];
        for (let row = 0; row < rows; row++) {
          const lineId = base + firstShown + row, slot = slotOf(lineId);
          plain.push(slot.id === lineId ? slot.text : '');
        }
        setReadable(plain.join('\n'));
      }
    }
    if (state.environment.connected && state.environment.canResize) {
      const fitRows = Math.max(1, Math.min(limits.rows, Math.floor(height / 20)));
      const fitCols = Math.max(1, Math.min(limits.cols, Math.floor(width / naturalCell)));
      const fit = `${fitRows}x${fitCols}`;
      if (fit !== lastSize && fitRows * fitCols <= limits.cells) {
        // While the keyboard holds the fit, a shorter frame waits for the
        // release recheck; a narrower one still applies at once, so a
        // rotation reflows the PTY without waiting for the keyboard.
        const held = state.environment.holdFit === true && lastSize !== '';
        const lastCols = Number(lastSize.slice(lastSize.indexOf('x') + 1));
        if (!held || fitCols !== lastCols) { lastSize = fit; post({ type: 'resize', rows: fitRows, cols: fitCols }); }
      }
    }
    const cursorRow = Number.isSafeInteger(screen.cursor_row) ? Math.min(Math.max(screen.cursor_row, 0), rows - 1) : rows - 1;
    const drawn = `${rows}:${cursorRow}:${alt ? 1 : 0}:${line}:${top}`;
    if (drawn !== lastMetrics) { lastMetrics = drawn; post({ type: 'metrics', rows, cursorRow, altScreen: alt, rowPitch: line, top }); }
  };

  // ── Frames ─────────────────────────────────────────────────────────────────
  // One animation-frame callback does a frame's work: advance a fling, paint,
  // then send what a full-screen program is owed.
  const tick = (now) => {
    ticking = false;
    if (disposed) return;
    let again = false;
    try {
      if (fling) again = stepFling(now) || again;
      if (needsPaint) paint();
      again = flushRemote() || again;
    } catch { post({ type: 'error' }); return; }
    if (again || needsPaint) requestTick();
  };
  /**
   * Rows drawn straight to the screen have no picture yet. A scroll needs
   * them, so they are made when a touch or wheel begins: the finger has not
   * moved far enough to scroll on that first frame, and a program that keeps
   * redrawing never pays for pictures it would throw away.
   */
  const fillPictures = () => {
    if (disposed || !shown || shown.ring !== ring || !state || !layout || drawnWith === 'gpu') return;
    try {
      for (let index = 0; index < shown.count; index++) {
        const slot = (shown.base + shown.first + index) % ring.count;
        if (ring.slots[slot].id === shown.base + shown.first + index && !ring.slots[slot].picture) drawPicture(slot);
      }
    } catch { post({ type: 'error' }); }
  };
  const stepFling = (now) => {
    if (!view || scrollRouteNow() !== 'history') { fling = null; return false; }
    // A frame that comes late (a stalled page) moves by at most two frames' worth.
    const dt = Math.max(0, Math.min(34, now - fling.at));
    fling.at = now;
    const step = kit.momentum(fling.velocity, dt);
    fling.velocity = step.velocity;
    const max = view.history * layout.rowPx;
    scrollPx += step.distance;
    if (scrollPx <= 0 || scrollPx >= max || step.velocity === 0) { scrollPx = Math.max(0, Math.min(max, scrollPx)); fling = null; }
    needsPaint = true;
    return fling !== null;
  };

  const receive = (event) => {
    if (disposed || (event.source && event.source !== window.parent)) return;
    let packet;
    try { packet = typeof event.data === 'string' ? JSON.parse(event.data) : event.data; } catch { return; }
    if (!packet || packet.v !== 1 || packet.id !== id) return;
    if (packet.type === 'blur') { input.blur(); return; }
    if (packet.type !== 'state' || !Number.isSafeInteger(packet.seq) || packet.seq <= received) return;
    try {
      if (!packet.frame || !packet.environment || !Number.isSafeInteger(packet.epoch) || packet.epoch < epoch) return;
      if (packet.epoch !== epoch || packet.frame.rawReset) {
        if (terminal) terminal.free();
        terminal = new Terminal(packet.frame.rows, packet.frame.cols);
        epoch = packet.epoch; scrollPx = 0; fling = null; info = null; rawSeq = 0;
        // The next paint starts every row picture again (reusing the canvases).
        if (ring) ring.key = '';
        lastSize = ''; lastMetrics = '';
        contentDirty = true;
      }
      received = packet.seq;
      if (terminal && packet.frame.presentation === 'raw') {
        terminal.resize(packet.frame.rows, packet.frame.cols);
        for (const chunk of packet.frame.rawChunks) {
          if (chunk.seq <= rawSeq) continue;
          if (chunk.seq !== rawSeq + 1) throw new Error('raw sequence gap');
          terminal.write(bytes(chunk.data)); rawSeq = chunk.seq;
        }
        // A resize alone can reflow rows, so every raw frame is a recheck.
        contentDirty = true;
      }
      if (!state?.environment.canResize && packet.environment.canResize) lastSize = '';
      hostGrid = packet.frame.grid ? prepareHostGrid(packet.frame.grid) : null;
      if (hostGrid) for (const slot of ring?.slots || []) slot.stale = true;
      paletteKey = JSON.stringify(packet.environment.palette);
      state = packet; input.disabled = !usable();
      schedulePaint(); post({ type: 'ack', seq: packet.seq });
    } catch { post({ type: 'error' }); }
  };
  document.addEventListener('message', receive); window.addEventListener('message', receive);
  const observer = new ResizeObserver(schedulePaint); observer.observe(canvas);
  const focusInput = () => { if (usable()) input.focus(); };

  // ── Gestures ───────────────────────────────────────────────────────────────
  // Only a program that took the alternate screen may be sent scroll bytes: at a
  // shell prompt they would be typed into the person's real shell.
  const scrollRouteNow = () => {
    if (!state || !view || state.frame.presentation === 'resync') return 'none';
    if (!view.alt) return view.host ? 'none' : 'history';
    if (!usable()) return 'none';
    return view.mouse ? 'wheel' : 'page';
  };
  const clampIndex = (value, count) => Math.max(0, Math.min(count - 1, Math.floor(value || 0)));
  const wheelReport = (up) => {
    const box = canvas.getBoundingClientRect();
    const col = clampIndex((scrollPoint.x - box.left) / view.cell, view.cols) + 1;
    const row = clampIndex((scrollPoint.y - box.top - view.top) / view.line, view.rows) + 1;
    const button = up ? 64 : 65;
    if (view.sgr) return `\x1b[<${button};${col};${row}M`;
    // Input travels as text, so a coordinate byte above 0x7e would arrive as two UTF-8 bytes.
    if (col > 94 || row > 94) return '';
    return `\x1b[M${String.fromCharCode(32 + button, 32 + col, 32 + row)}`;
  };
  /** Travel in CSS px, positive toward older output. */
  const scrollBy = (pixels) => {
    if (!view) return;
    const route = scrollRouteNow();
    if (route === 'history') {
      // The text follows the finger exactly, drawn on the next frame.
      scrollPx = Math.max(0, Math.min(view.history * layout.rowPx, scrollPx + pixels * view.dpr));
      schedulePaint();
    } else if (route !== 'none') { scrollPixels += pixels; requestTick(); }
  };
  /** At most one send per frame: whatever whole units the finger has made, up to the cap. */
  const flushRemote = () => {
    if (scrollPixels === 0) return false;
    const route = scrollRouteNow();
    if (route === 'none' || route === 'history') { scrollPixels = 0; return false; }
    const unit = route === 'wheel' ? view.line : view.rows * view.line;
    const { count, rest } = kit.takeUnits(scrollPixels, unit, SCROLL_EVENTS_PER_FRAME);
    if (count === 0) return false;
    scrollPixels = rest;
    const up = count > 0, event = route === 'wheel' ? wheelReport(up) : up ? '\x1b[5~' : '\x1b[6~';
    if (event) send(event.repeat(Math.abs(count)));
    return kit.takeUnits(scrollPixels, unit, 1).count !== 0;
  };
  canvas.addEventListener('wheel', (event) => {
    event.preventDefault();
    // A trackpad pinch arrives as a ctrl wheel; it is not scrolling.
    if (event.ctrlKey || !view) return;
    const unit = event.deltaMode === 1 ? view.line : event.deltaMode === 2 ? view.rows * view.line : 1;
    fling = null;
    scrollPoint = { x: event.clientX, y: event.clientY };
    fillPictures();
    scrollBy(-event.deltaY * unit);
  }, { passive: false });
  const sample = (event, y) => {
    const t = event.timeStamp;
    if (Number.isFinite(t)) kit.trimSamples(samples, t).push({ t, y });
  };
  // ── Selecting, copying, links and clicks ──────────────────────────────────
  // Hold a finger still to select the word under it, then drag to select
  // more; letting go offers Copy (and Open for a link). A tap opens the link
  // under it, or, for a program that asked for the mouse, clicks there. A tap
  // never brings up the keyboard: the composer under the terminal types.
  const selLayer = document.getElementById('sel'), menu = document.getElementById('menu');
  const menuCopy = document.getElementById('menu-copy'), menuOpen = document.getElementById('menu-open');
  const LONG_PRESS_MS = 450;
  const later = (callback, ms) => (typeof setTimeout === 'function' ? setTimeout(callback, ms) : null);
  const cancel = (timer) => { if (timer !== null && typeof clearTimeout === 'function') clearTimeout(timer); };
  let selection = null, pressTimer = null, swallowClick = false;
  /** The cell under a point, by line id, so it stays put while history scrolls. */
  const cellAt = (x, y) => {
    if (!view || !shown || !layout || shown.count < 1) return null;
    const box = canvas.getBoundingClientRect();
    let row = Math.floor(((y - box.top) * view.dpr - shown.topPx + shown.sub) / layout.rowPx);
    if (!Number.isFinite(row)) row = 0;
    row = Math.max(0, Math.min(shown.count - 1, row));
    return { line: shown.base + shown.first + row, row, col: clampIndex((x - box.left) / view.cell, view.cols) };
  };
  const lineText = (line) => { if (!ring) return ''; const slot = slotOf(line); return slot.id === line ? slot.text : ''; };
  const visibleRows = () => Array.from({ length: shown.count }, (_, row) => lineText(shown.base + shown.first + row));
  const before = (p, q) => p.line < q.line || (p.line === q.line && p.col < q.col);
  /** [start, end] of the selection: the first word, stretched to wherever the finger went. */
  const range = () => {
    const { from, to, focus } = selection;
    return [before(focus, from) ? focus : from, before(to, focus) ? focus : to];
  };
  const selectedText = () => {
    const [a, b] = range(), rows = [];
    for (let line = a.line; line <= b.line; line++) rows.push(lineText(line));
    return kit.selectText(rows, { row: 0, col: a.col }, { row: b.line - a.line, col: b.col }, view.cols);
  };
  const tint = () => { const color = state.environment.palette.cursor; return /^[#\w(),.%\s-]{1,40}$/u.test(color) ? color : '#4c8dff'; };
  const drawSelection = () => {
    if (!selLayer) return;
    if (!selection || !view || !shown || !layout) { if (selLayer.innerHTML) selLayer.innerHTML = ''; return; }
    const [a, b] = range(), first = shown.base + shown.first, color = tint(), pitch = layout.rowPx / view.dpr;
    let html = '';
    for (let line = Math.max(a.line, first); line <= Math.min(b.line, first + shown.count - 1); line++) {
      const y = (shown.topPx - shown.sub + (line - first) * layout.rowPx) / view.dpr;
      const c0 = line === a.line ? a.col : 0, c1 = line === b.line ? b.col + 1 : view.cols;
      html += `<div style="position:absolute;left:${c0 * view.cell}px;top:${y}px;width:${Math.max(0, c1 - c0) * view.cell}px;height:${pitch}px;background:${color};opacity:.38;border-radius:2px"></div>`;
    }
    selLayer.innerHTML = html;
  };
  const linkIn = (text) => { const trimmed = (text || '').trim(); return /^https?:\/\/\S+$/u.test(trimmed) ? trimmed : null; };
  const showMenu = (visible) => {
    if (!menu) return;
    if (!visible || !selection || !view || !shown) { menu.style.display = 'none'; return; }
    const [a, b] = range(), first = shown.base + shown.first, pitch = layout.rowPx / view.dpr;
    const above = (shown.topPx - shown.sub + (a.line - first) * layout.rowPx) / view.dpr - 52;
    const below = (shown.topPx - shown.sub + (b.line - first + 1) * layout.rowPx) / view.dpr + 8;
    menu.style.top = `${Math.round(above >= 4 ? above : Math.min(below, canvas.clientHeight - 52))}px`;
    if (menuOpen) menuOpen.style.display = linkIn(selection.text) ? 'block' : 'none';
    menu.style.display = pitch > 0 ? 'flex' : 'none';
  };
  const clearSelection = () => { selection = null; drawSelection(); showMenu(false); };
  const startSelection = (x, y) => {
    const cell = cellAt(x, y);
    if (!cell) return;
    const word = kit.wordAt(lineText(cell.line), cell.col) || [cell.col, cell.col];
    fling = null; scrollPixels = 0; swallowClick = true;
    selection = { from: { line: cell.line, col: word[0] }, to: { line: cell.line, col: word[1] }, focus: { line: cell.line, col: word[0] }, dragging: true, text: '' };
    showMenu(false); drawSelection();
  };
  const tapAt = (x, y) => {
    const cell = cellAt(x, y);
    if (!cell) return;
    const url = kit.linkAt(visibleRows(), cell.row, cell.col, view.cols);
    if (url) { post({ type: 'link', url }); return; }
    // Only the live screen of a program that asked for clicks gets one.
    if (view.mouse && usable() && scrollPx === 0) {
      const report = kit.clickReport(cell.col + 1, cell.row + 1, view.sgr);
      if (report) send(report);
    }
  };
  const press = (button, action) => {
    if (!button) return;
    // Keeps focus (and the keyboard, if it is up) where it was.
    button.addEventListener('mousedown', (event) => event.preventDefault());
    button.addEventListener('click', (event) => { event.preventDefault(); action(); });
  };
  press(menuCopy, () => { if (selection && selection.text) post({ type: 'copy', text: selection.text }); clearSelection(); });
  press(menuOpen, () => { const url = selection && linkIn(selection.text); if (url) post({ type: 'link', url }); clearSelection(); });

  canvas.addEventListener('touchstart', (event) => {
    // A new touch stops a fling where it is.
    if (fling) { fling = null; schedulePaint(); }
    samples = [];
    cancel(pressTimer); pressTimer = null; swallowClick = false;
    if (event.touches.length !== 1) { touch = null; dragged = true; scrollPixels = 0; return; }
    const { clientX: x, clientY: y } = event.touches[0];
    // A touch away from a finished selection puts it down.
    if (selection && !selection.dragging) clearSelection();
    touch = { x, y }; dragged = false;
    pressTimer = later(() => { pressTimer = null; if (touch !== null && !dragged) startSelection(x, y); }, LONG_PRESS_MS);
    sample(event, y);
    fillPictures();
    // Wheel reports stay where the swipe began, so a long one cannot wander into
    // the next pane of a program and start scrolling that instead.
    scrollPoint = { x, y };
  }, { passive: true });
  canvas.addEventListener('touchmove', (event) => {
    if (touch === null || event.touches.length !== 1) return;
    const { clientX: x, clientY: y } = event.touches[0];
    if (selection && selection.dragging) {
      event.preventDefault();
      const cell = cellAt(x, y);
      if (cell) { selection.focus = { line: cell.line, col: cell.col }; drawSelection(); }
      return;
    }
    if (!dragged) {
      if (Math.hypot(x - touch.x, y - touch.y) < TAP_SLOP_PX) return;
      dragged = true;
      cancel(pressTimer); pressTimer = null;
      // Going the other way abandons what the last swipe left unsent.
      if (scrollPixels * (y - touch.y) < 0) scrollPixels = 0;
    }
    event.preventDefault();
    sample(event, y);
    const travel = y - touch.y;
    touch.y = y;
    scrollBy(travel);
  }, { passive: false });
  canvas.addEventListener('touchend', (event) => {
    cancel(pressTimer); pressTimer = null;
    if (selection && selection.dragging) {
      selection.dragging = false;
      selection.text = selectedText();
      showMenu(true);
      touch = null; samples = [];
      return;
    }
    if (touch !== null && dragged && view) {
      const route = scrollRouteNow();
      if (route === 'history') {
        // A flick carries on and slows down; a finger that stopped first does not.
        const velocity = kit.releaseVelocity(samples, event?.timeStamp);
        if (velocity !== 0) { fling = { velocity: velocity * view.dpr, at: event.timeStamp }; requestTick(); }
      } else if (route === 'page') {
        // A page is the whole screen and a thumb rarely travels that far in one go,
        // so a swipe of at least half of one counts; anything less waits for the next.
        const page = view.rows * view.line, rest = Math.abs(scrollPixels) % page;
        if (rest >= page / 2) { scrollPixels += Math.sign(scrollPixels) * (page - rest); requestTick(); }
      }
    }
    touch = null; samples = [];
    // The accessible text catches up now that the finger is off.
    if (dragged) schedulePaint();
  });
  canvas.addEventListener('touchcancel', () => { cancel(pressTimer); pressTimer = null; if (selection && selection.dragging) clearSelection(); touch = null; dragged = false; scrollPixels = 0; samples = []; fling = null; schedulePaint(); });
  // Stops the button taking focus from the hidden input, which would drop the keyboard.
  latest.addEventListener('mousedown', (event) => event.preventDefault());
  latest.addEventListener('click', () => { scrollPx = 0; fling = null; scrollPixels = 0; schedulePaint(); });
  canvas.addEventListener('click', (event) => {
    if (swallowClick) { swallowClick = false; dragged = false; return; }
    if (dragged) { dragged = false; return; }
    if (selection) { clearSelection(); return; }
    tapAt(event.clientX, event.clientY);
  });
  canvas.addEventListener('keydown', (event) => {
    if (usable() && (event.key === 'Enter' || event.key === ' ')) { event.preventDefault(); focusInput(); }
  });
  input.addEventListener('compositionstart', () => { composing = true; });
  input.addEventListener('compositionend', () => { composing = false; send(input.value); input.value = ''; });
  input.addEventListener('input', () => { if (!composing) { send(input.value); input.value = ''; } });
  input.addEventListener('paste', (event) => { if (!composing) { event.preventDefault(); send(event.clipboardData.getData('text/plain')); } });
  input.addEventListener('keydown', (event) => {
    if (composing || event.isComposing) return;
    if (event.metaKey) return;
    const keys = { Enter: '\r', Backspace: '\x7f', Tab: '\t', Escape: '\x1b', ArrowUp: '\x1b[A', ArrowDown: '\x1b[B', ArrowRight: '\x1b[C', ArrowLeft: '\x1b[D', Home: '\x1b[H', End: '\x1b[F', Delete: '\x1b[3~', PageUp: '\x1b[5~', PageDown: '\x1b[6~' };
    let value = keys[event.key];
    if (event.ctrlKey && /^[a-z@[\]\\^_]$/iu.test(event.key)) value = String.fromCharCode(event.key.toUpperCase().charCodeAt(0) & 31);
    if (value) { event.preventDefault(); send(value); }
  });
  window.addEventListener('pagehide', () => {
    disposed = true; observer.disconnect(); if (terminal) terminal.free();
    if (gpu) gpu.dispose();
    // Row pictures are the largest allocation here; give them back at once.
    if (ring) for (const strip of ring.strips) { strip.width = 0; strip.height = 0; }
  });
  if (!context || !Terminal || !kit) throw new Error('terminal unavailable');
  if (requested !== '2d' && gk && makeGpu) {
    try {
      gpuCanvas = document.createElement('canvas');
      gpu = makeGpu(kit, gk)(gpuCanvas, {
        lost: () => { gpuLosses++; schedulePaint(); },
        restored: () => { if (ring) ring.key = ''; drawnWith = ''; schedulePaint(); },
        failed: () => { gpuFailed = true; schedulePaint(); },
      });
    } catch { gpu = null; }
    if (!gpu) gpuCanvas = null;
    else {
      gpuCanvas.setAttribute('aria-hidden', 'true');
      gpuCanvas.style.cssText = 'position:absolute;left:0;top:0;width:100%;height:100%;pointer-events:none;display:none';
      canvas.after(gpuCanvas);
    }
  }
  post({ type: 'ready' });
}
