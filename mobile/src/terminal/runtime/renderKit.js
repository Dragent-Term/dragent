/**
 * Pure helpers for the terminal renderer: no DOM, canvas or timers, so they
 * can be tested on their own. Packaged into the isolated terminal document
 * together with the runtime by scripts/sync-terminal-wasm.mjs, which is why
 * everything lives inside this one function.
 */
export function renderKit() {
  const FLAG_DIM = 2, FLAG_INVERSE = 64, FLAG_HIDDEN = 128;
  /** Compact colour codes: -1 default, 0-255 palette, DIRECT + 0xRRGGBB. */
  const DIRECT = 0x1000000;
  const paletteColor = (index, fallback, palette) => {
    if (index < 8) return palette.ansiNormal[index] || fallback;
    if (index < 16) return palette.ansiBright[index - 8] || fallback;
    if (index < 232) {
      const n = index - 16, channel = (n) => n === 0 ? 0 : 55 + n * 40;
      return `rgb(${channel(Math.floor(n / 36))},${channel(Math.floor(n / 6) % 6)},${channel(n % 6)})`;
    }
    const grey = 8 + (index - 232) * 10;
    return `rgb(${grey},${grey},${grey})`;
  };
  /**
   * A CSS colour for a compact code, a host RGBA array or a snapshot colour
   * object; `fallback` stands for the terminal's default.
   */
  const colorCss = (value, fallback, palette) => {
    if (value === null || value === undefined || value === -1) return fallback;
    if (typeof value === 'number') {
      if (value >= DIRECT) { const rgb = value - DIRECT; return `rgb(${rgb >> 16 & 255},${rgb >> 8 & 255},${rgb & 255})`; }
      return value >= 0 ? paletteColor(value, fallback, palette) : fallback;
    }
    if (Array.isArray(value)) return `rgba(${value[0]},${value[1]},${value[2]},${value[3] / 255})`;
    if (value.kind === 'default') return fallback;
    if (value.kind === 'rgb') return `rgb(${value.value.join(',')})`;
    return paletteColor(value.value, fallback, palette);
  };
  const font = (flags, size) => `${flags & 4 ? 'italic ' : ''}${flags & 1 ? 'bold ' : ''}${size}px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`;
  /**
   * Turn one compact row (`[col, fg, bg, flags, text]` runs, or
   * `[col, fg, bg, flags, grapheme, width]` for a grapheme placed on its own)
   * into drawable runs with resolved colours, plus the row's readable text.
   * `restyle` lets a caller swap the codes for real styles (a host grid lays
   * text out with placeholder colours). Nothing past `cols` is kept.
   */
  const decodeRow = (compact, palette, cols, restyle) => {
    const runs = [];
    let text = '', filled = 0;
    for (const item of compact) {
      if (!Array.isArray(item) || item.length < 5) continue;
      const col = item[0];
      if (!Number.isSafeInteger(col) || col < 0 || col >= cols) continue;
      const style = restyle ? restyle(item[1], item[2], item[3]) : null;
      const rawFg = style ? style.fg : item[1], rawBg = style ? style.bg : item[2];
      const flags = (style ? style.flags : item[3]) | 0, inverse = (flags & FLAG_INVERSE) !== 0;
      const single = item.length > 5;
      let glyphs = String(item[4]), cells;
      if (single) cells = Math.max(1, item[5] | 0);
      else {
        cells = codePoints(glyphs);
        if (col + cells > cols) { glyphs = Array.from(glyphs).slice(0, cols - col).join(''); cells = cols - col; }
      }
      if (cells === 0) continue;
      const fg = colorCss(inverse ? rawBg : rawFg, inverse ? palette.background : palette.foreground, palette);
      let bg = inverse ? colorCss(rawFg, palette.foreground, palette) : colorCss(rawBg, null, palette);
      if (bg === palette.background) bg = null;
      runs.push({ col, cells, text: glyphs, single, fg, bg, flags });
      if (filled < col) { text += ' '.repeat(col - filled); filled = col; }
      const shown = flags & FLAG_HIDDEN ? ' '.repeat(single ? 1 : cells) : glyphs;
      text += single && cells > 1 ? shown + ' '.repeat(cells - 1) : shown;
      filled = col + cells;
    }
    return { runs, text: text.trimEnd() };
  };
  /**
   * Split a run of one-cell characters into pieces that can each be drawn
   * with one fillText. A piece grows while every glyph still lands within
   * `tolerance` px of its own cell given the font's real advances, so a font
   * whose advance matches the grid draws a whole word or line at once and a
   * glyph from a fallback font gets placed on its own. Blanks never start or
   * end a piece.
   */
  const segments = (text, col, advance, cell, tolerance) => {
    const out = [];
    let start = -1, piece = '', drift = 0, at = col;
    const flush = () => { if (start >= 0) out.push({ col: start, text: piece.trimEnd() }); start = -1; piece = ''; drift = 0; };
    for (const glyph of text) {
      const error = advance(glyph) - cell;
      if (glyph === ' ') {
        if (start >= 0) { piece += glyph; drift += error; }
      } else if (start >= 0 && Math.abs(drift) <= tolerance) {
        piece += glyph; drift += error;
      } else {
        flush(); start = at; piece = glyph; drift = error;
      }
      at++;
    }
    flush();
    return out;
  };
  /** How many code points (cells, in a run of one-cell characters) `text` holds, without building an array. */
  const codePoints = (text) => {
    let count = text.length;
    for (let index = 0; index < text.length; index++) {
      const unit = text.charCodeAt(index);
      if (unit >= 0xd800 && unit <= 0xdbff && index + 1 < text.length) { const next = text.charCodeAt(index + 1); if (next >= 0xdc00 && next <= 0xdfff) { count--; index++; } }
    }
    return count;
  };
  /** Text-colour alpha for a run: dim text is drawn faded. */
  const alpha = (flags) => flags & FLAG_DIM ? 0.65 : 1;

  // ── Scrolling ──────────────────────────────────────────────────────────────
  /** Momentum keeps this fraction of its speed per millisecond. */
  const DECAY = 0.998, STOP_SPEED = 0.02, MAX_SPEED = 8, MIN_FLING = 0.15, SAMPLE_MS = 100, REST_MS = 50;
  /**
   * Advance a fling by `dt` ms: speed decays exponentially, and the distance
   * is its exact integral over the step, so the path is the same at any frame
   * rate. Speeds are px per ms; a fling stops once it is too slow to see.
   */
  const momentum = (velocity, dt) => {
    if (!(dt > 0) || velocity === 0) return { distance: 0, velocity };
    const keep = Math.pow(DECAY, dt);
    const distance = velocity * (keep - 1) / Math.log(DECAY);
    const next = velocity * keep;
    return { distance, velocity: Math.abs(next) < STOP_SPEED ? 0 : next };
  };
  /** How far a fling at this speed travels before it stops. */
  const flingDistance = (velocity) => velocity / -Math.log(DECAY);
  /**
   * The finger's speed as it lifted, from recent `{ t, y }` samples: zero if
   * it rested before lifting or moved too slowly to mean a flick.
   */
  const releaseVelocity = (samples, liftedAt) => {
    const last = samples[samples.length - 1];
    if (!last || !Number.isFinite(last.t)) return 0;
    if (Number.isFinite(liftedAt) && liftedAt - last.t > REST_MS) return 0;
    const first = samples.find((sample) => Number.isFinite(sample.t) && last.t - sample.t <= SAMPLE_MS);
    if (!first || last.t - first.t < 8) return 0;
    const speed = (last.y - first.y) / (last.t - first.t);
    if (Math.abs(speed) < MIN_FLING) return 0;
    return Math.max(-MAX_SPEED, Math.min(MAX_SPEED, speed));
  };
  /** Keep only the samples a release could still use. */
  const trimSamples = (samples, now) => {
    while (samples.length > 2 && now - samples[0].t > SAMPLE_MS) samples.shift();
    return samples;
  };
  /**
   * Whole scroll units in `pixels` of travel, at most `cap` of them, and the
   * distance left for later. A hair under a unit (float error) still counts.
   */
  const takeUnits = (pixels, unit, cap) => {
    if (!(unit > 0)) return { count: 0, rest: 0 };
    const whole = Math.trunc(pixels / unit + Math.sign(pixels) * 1e-6);
    const count = Math.max(-cap, Math.min(cap, whole));
    return { count, rest: pixels - count * unit };
  };
  // ── Text on screen: selection and links ────────────────────────────────────
  // Rows are plain text, one character per cell (see decodeRow). A row that
  // fills every column carried on onto the next one, so it joins without a
  // line break: that is how a long link printed across rows comes back whole.
  const cellsOf = (text) => Array.from(text || '');
  const wrapped = (text, cols) => cellsOf(text).length >= cols;
  /** The whitespace-delimited stretch around `col`: [start, end] inclusive, or null on a blank. */
  const wordAt = (text, col) => {
    const cells = cellsOf(text);
    if (col < 0 || col >= cells.length || /\s/u.test(cells[col])) return null;
    let start = col, end = col;
    while (start > 0 && !/\s/u.test(cells[start - 1])) start--;
    while (end + 1 < cells.length && !/\s/u.test(cells[end + 1])) end++;
    return [start, end];
  };
  /** Text from `from` to `to` ({row, col}, inclusive, any order) over `rows`. */
  const selectText = (rows, from, to, cols) => {
    const [a, b] = from.row < to.row || (from.row === to.row && from.col <= to.col) ? [from, to] : [to, from];
    let out = '';
    for (let row = a.row; row <= b.row; row++) {
      const cells = cellsOf(rows[row]);
      const start = row === a.row ? a.col : 0, end = row === b.row ? b.col + 1 : cells.length;
      out += cells.slice(start, end).join('');
      if (row < b.row) {
        if (wrapped(rows[row], cols)) continue;
        out = out.replace(/[ \t]+$/u, '') + '\n';
      }
    }
    return out.replace(/[ \t]+$/u, '');
  };
  const LINK = /https?:\/\/[^\s<>"'`]+/gu;
  /** The web link under (row, col), following it across wrapped rows; or null. */
  const linkAt = (rows, row, col, cols) => {
    if (row < 0 || row >= rows.length) return null;
    let first = row, last = row;
    while (first > 0 && wrapped(rows[first - 1], cols)) first--;
    while (last + 1 < rows.length && wrapped(rows[last], cols)) last++;
    let joined = '', at = -1;
    for (let index = first; index <= last; index++) {
      if (index === row) at = cellsOf(joined).length + col;
      joined += index < last ? cellsOf(rows[index]).slice(0, cols).join('') : rows[index] || '';
    }
    const chars = cellsOf(joined);
    for (const match of joined.matchAll(LINK)) {
      const start = cellsOf(joined.slice(0, match.index)).length;
      let url = match[0];
      // Sentence punctuation after a link is not part of it; a bracket is, when it opened inside.
      while (/[.,;:!?'")\]}>]$/u.test(url)) {
        const tail = url.slice(-1), open = { ')': '(', ']': '[', '}': '{' }[tail];
        // A closing bracket matched inside the link belongs to it.
        if (open && url.split(open).length >= url.split(tail).length) break;
        url = url.slice(0, -1);
      }
      const end = start + cellsOf(url).length - 1;
      if (at >= start && at <= end && chars.length > 0) return url;
    }
    return null;
  };
  /** The bytes a click at 1-based (col, row) sends to a program that asked for mouse input. */
  const clickReport = (col, row, sgr) => {
    if (sgr) return `\x1b[<0;${col};${row}M\x1b[<0;${col};${row}m`;
    // Input travels as text, so a coordinate byte above 0x7e would arrive as two UTF-8 bytes.
    if (col > 94 || row > 94) return '';
    return `\x1b[M${String.fromCharCode(32, 32 + col, 32 + row)}\x1b[M${String.fromCharCode(35, 32 + col, 32 + row)}`;
  };
  return { colorCss, font, decodeRow, segments, codePoints, alpha, momentum, flingDistance, releaseVelocity, trimSamples, takeUnits, wordAt, selectText, linkAt, clickReport };
}
