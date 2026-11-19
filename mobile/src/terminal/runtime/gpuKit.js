/**
 * Pure helpers for the WebGL2 terminal renderer: colour parsing, glyph
 * placement, atlas packing, per-row instance data and the choice between the
 * GPU and 2D renderers. No DOM, canvas or GL here, so all of it is tested on
 * its own. Packaged into the isolated terminal document by
 * scripts/sync-terminal-wasm.mjs, which is why everything lives inside this
 * one function.
 */
export function gpuKit() {
  /**
   * Every instance is six 32-bit words. A rectangle (cell background or text
   * decoration) is `x0, x1, y0, y1, bands, colour`; a glyph is
   * `quadX, quadWidth, atlasX, atlasY, kind, colour`. Positions are device
   * pixels within the row; colour is packed 0xRRGGBBAA in the last word.
   */
  const WORDS = 6;
  /** Glyph kinds: a coverage mask tinted with the text colour, or a colour glyph (emoji) drawn as is. */
  const MASK = 0, COLOUR = 1;
  /** Decoration bands a rectangle may carry instead of filling its box. */
  const UNDERLINE = 1, DOUBLE = 2, STRIKE = 4, OVERLINE = 8;
  /** A glyph's left edge is rounded to a quarter pixel, and drawn once per quarter. */
  const PHASES = 4;
  const FLAG_BOLD = 1, FLAG_ITALIC = 4, FLAG_HIDDEN = 128, FLAG_DIM = 2;
  const FLAG_UNDERLINES = 8 | 16 | 32, FLAG_DOUBLE = 16, FLAG_STRIKE = 256, FLAG_OVERLINE = 1024;

  const byte = (value) => Math.max(0, Math.min(255, Math.round(value)));
  const pack = (r, g, b, a) => ((byte(r) << 24) | (byte(g) << 16) | (byte(b) << 8) | byte(a)) >>> 0;
  const channel = (text) => {
    const value = text.trim();
    return value.endsWith('%') ? Number(value.slice(0, -1)) * 255 / 100 : Number(value);
  };
  /**
   * A CSS colour as the renderer produces them (`#rgb`, `#rgba`, `#rrggbb`,
   * `#rrggbbaa`, `rgb()`, `rgba()`), packed 0xRRGGBBAA, or null for anything
   * else (the caller asks the browser to normalise those).
   */
  const parseColor = (css) => {
    if (typeof css !== 'string') return null;
    const text = css.trim().toLowerCase();
    if (text[0] === '#') {
      const hex = text.slice(1);
      if (!/^[0-9a-f]+$/u.test(hex)) return null;
      if (hex.length === 3 || hex.length === 4) {
        const digit = (index) => parseInt(hex[index] + hex[index], 16);
        return pack(digit(0), digit(1), digit(2), hex.length === 4 ? digit(3) : 255);
      }
      if (hex.length === 6 || hex.length === 8) {
        const pair = (index) => parseInt(hex.slice(index, index + 2), 16);
        return pack(pair(0), pair(2), pair(4), hex.length === 8 ? pair(6) : 255);
      }
      return null;
    }
    const match = /^rgba?\(([^)]*)\)$/u.exec(text);
    if (!match) return null;
    const parts = match[1].split(/[,/\s]+/u).filter(Boolean);
    if (parts.length !== 3 && parts.length !== 4) return null;
    const [r, g, b] = parts.slice(0, 3).map(channel);
    const alpha = parts.length === 4 ? (parts[3].endsWith('%') ? Number(parts[3].slice(0, -1)) / 100 : Number(parts[3])) : 1;
    if (![r, g, b, alpha].every(Number.isFinite)) return null;
    return pack(r, g, b, Math.max(0, Math.min(1, alpha)) * 255);
  };
  /** The packed colour with its alpha multiplied by `factor` (dim text). */
  const fade = (packed, factor) => factor === 1 ? packed : ((packed & 0xffffff00) | byte((packed & 255) * factor)) >>> 0;
  /** Packed colour to straight `[r, g, b, a]` in 0..1. */
  const unpack = (packed) => [(packed >>> 24) / 255, (packed >>> 16 & 255) / 255, (packed >>> 8 & 255) / 255, (packed & 255) / 255];

  /**
   * Where a glyph starting at `col` lands: the whole device pixel left of it
   * and the quarter-pixel phase it is drawn at, the same rounding the 2D
   * canvas applies to text positions.
   */
  const placement = (col, cellPx) => {
    const left = col * cellPx;
    let whole = Math.floor(left), phase = Math.round((left - whole) * PHASES);
    if (phase === PHASES) { whole++; phase = 0; }
    return { whole, phase };
  };
  /** The atlas family a glyph belongs to: bold/italic by phase. */
  const family = (flags, phase) => ((flags & FLAG_BOLD ? 1 : 0) | (flags & FLAG_ITALIC ? 2 : 0)) * PHASES + phase;
  /** Decoration bands for a run's flags, as the 2D renderer draws them. */
  const bands = (flags) => (flags & FLAG_UNDERLINES ? UNDERLINE : 0) | (flags & FLAG_DOUBLE ? DOUBLE : 0)
    | (flags & FLAG_STRIKE ? STRIKE : 0) | (flags & FLAG_OVERLINE ? OVERLINE : 0);
  /**
   * The top of each band within a row, and the rule height, in device px:
   * underline 2 px (scaled) above the row's foot, a second one 2 px above
   * that, strike through the middle, overline along the top.
   */
  const bandGeometry = (line, scale, dpr) => ({
    underline: (line - 2 * scale) * dpr, double: (line - 4 * scale) * dpr, strike: line / 2 * dpr, overline: 0, rule: Math.max(1, scale) * dpr,
  });
  /**
   * The four rectangles of the cursor's outline in device px, matching a 2D
   * stroke of `lineWidth` centred on the cell box inset by half a pixel.
   */
  const cursorRects = (col, row, cell, line, top, scale, dpr) => {
    const x = (col * cell + 0.5) * dpr, y = (top + row * line + 0.5) * dpr;
    const w = Math.max(1, cell - 1) * dpr, h = Math.max(1, line - 1) * dpr, half = Math.max(1, scale) * dpr / 2;
    return [
      [x - half, x + w + half, y - half, y + half],
      [x - half, x + w + half, y + h - half, y + h + half],
      [x - half, x + half, y + half, y + h - half],
      [x + w - half, x + w + half, y + half, y + h - half],
    ];
  };

  /**
   * A shelf packer for glyph boxes. Boxes of one height share shelves; a
   * shelf is opened below the last one while there is room. Growing keeps
   * every box where it is, so a grown atlas only has to copy its old texels.
   */
  const createPacker = (width, height) => {
    const shelves = [];
    let next = 0, used = 0;
    return {
      get width() { return width; }, get height() { return height; }, get used() { return used; },
      alloc(w, h) {
        if (!(w > 0 && h > 0) || w > width || h > height) return null;
        for (const shelf of shelves) {
          if (shelf.h === h && shelf.x + w <= width) { const at = { x: shelf.x, y: shelf.y }; shelf.x += w; used += w * h; return at; }
        }
        if (next + h > height) return null;
        shelves.push({ y: next, h, x: w });
        const at = { x: 0, y: next };
        next += h; used += w * h;
        return at;
      },
      grow(w, h) { width = Math.max(width, w); height = Math.max(height, h); },
      reset() { shelves.length = 0; next = 0; used = 0; },
    };
  };
  /** The next atlas size when full: double the height first, then the width, up to `max`; null at the limit. */
  const nextAtlasSize = (width, height, max) => {
    if (height <= width && height * 2 <= max) return { width, height: height * 2 };
    if (width * 2 <= max) return { width: width * 2, height };
    if (height * 2 <= max) return { width, height: height * 2 };
    return null;
  };

  /**
   * Write one decoded row (runs from renderKit.decodeRow) as instance data
   * into its slot's regions of the three instance arrays: `bg`, `glyphs` and
   * `deco`, each `cols` instances long starting at instance `at`. `geometry`
   * is `{ cols, cellPx, rowPx }` in device pixels. Unused
   * instances are zeroed (zero width draws nothing). `glyphAt(text, flags,
   * col, cells)` returns the atlas box for a glyph (`{ x, w, ax, ay, kind }`)
   * or null for one with no ink; `colorOf(css)` packs a CSS colour.
   * Hidden runs draw nothing but their background, like the 2D renderer.
   */
  const writeRow = (runs, geometry, at, arrays, glyphAt, colorOf) => {
    const { bg, glyphs, deco } = arrays, { cols, cellPx, rowPx } = geometry;
    let nb = 0, ng = 0, nd = 0;
    const put = (target, index, a, b, c, d, e, color) => {
      const o = (at + index) * WORDS;
      target.f32[o] = a; target.f32[o + 1] = b; target.f32[o + 2] = c; target.f32[o + 3] = d; target.f32[o + 4] = e; target.u32[o + 5] = color;
    };
    for (const run of runs) {
      const x0 = run.col * cellPx, x1 = (run.col + run.cells) * cellPx;
      if (run.bg && nb < cols) put(bg, nb++, x0, x1, 0, rowPx, 0, colorOf(run.bg));
      if (run.flags & FLAG_HIDDEN) continue;
      const color = fade(colorOf(run.fg), run.flags & FLAG_DIM ? 0.65 : 1);
      if (run.single) {
        const box = ng < cols ? glyphAt(run.text, run.flags, run.col, run.cells) : null;
        if (box) put(glyphs, ng++, box.x, box.w, box.ax, box.ay, box.kind, color);
      } else {
        let col = run.col;
        for (const glyph of run.text) {
          if (glyph !== ' ' && ng < cols) {
            const box = glyphAt(glyph, run.flags, col, 1);
            if (box) put(glyphs, ng++, box.x, box.w, box.ax, box.ay, box.kind, color);
          }
          col++;
        }
      }
      const marks = bands(run.flags);
      if (marks && nd < cols) put(deco, nd++, x0, x1, 0, rowPx, marks, color);
    }
    for (const [target, from] of [[bg, nb], [glyphs, ng], [deco, nd]]) target.f32.fill(0, (at + from) * WORDS, (at + cols) * WORDS);
    return { bg: nb, glyphs: ng, deco: nd };
  };

  /**
   * Which renderer draws: the 2D canvas when asked for, when WebGL2 is
   * missing, after a GPU fault, while the context is lost, or once it has
   * been lost too often to trust; the GPU otherwise.
   */
  const MAX_CONTEXT_LOSSES = 3;
  const chooseRenderer = ({ requested, available, failed, lost, losses }) => {
    if (requested === '2d' || !available || failed || lost) return '2d';
    if ((losses || 0) >= MAX_CONTEXT_LOSSES) return '2d';
    return 'gpu';
  };
  return { WORDS, MASK, COLOUR, UNDERLINE, DOUBLE, STRIKE, OVERLINE, PHASES, MAX_CONTEXT_LOSSES, parseColor, fade, unpack, placement, family, bands, bandGeometry, cursorRects, createPacker, nextAtlasSize, writeRow, chooseRenderer };
}
