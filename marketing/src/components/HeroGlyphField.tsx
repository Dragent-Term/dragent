"use client";

import { useEffect, useRef } from "react";

/**
 * The hero's backdrop: a slow-drifting cloud of terminal glyphs.
 *
 * A grid of monospaced cells is shaded by layered noise, then shaped into two
 * soft masses that flank the headline, so the centre stays clear to read.
 * Denser cells pick heavier glyphs and a touch of the brand cyan. It drifts a
 * little each frame (about 15 fps), stays still for reduced motion, and pauses
 * while the hero is off screen or the tab is hidden.
 */

/** Light to heavy: the glyph a cell shows grows with its density. */
const RAMP = " .·:-=+*<>/\\|{}[]01$#%@";
const CELL_W = 8;
const CELL_H = 13;
const FRAME_MS = 66;
/** Cells lighter than this stay empty, which carves the cloud into shapes. */
const THRESHOLD = 0.4;
const ALPHA_LEVELS = 10;

/** Deterministic hash noise: 2D value noise with smooth interpolation. */
function makeNoise(seed: number) {
  const perm = new Uint8Array(512);
  const base = Array.from({ length: 256 }, (_, i) => i);
  let s = seed >>> 0;
  for (let i = 255; i > 0; i--) {
    s = (s * 1664525 + 1013904223) >>> 0;
    const j = s % (i + 1);
    [base[i], base[j]] = [base[j], base[i]];
  }
  for (let i = 0; i < 512; i++) perm[i] = base[i & 255];
  const value = (x: number, y: number) => perm[(perm[x & 255] + y) & 255] / 255;
  const fade = (t: number) => t * t * (3 - 2 * t);
  const noise = (x: number, y: number) => {
    const xi = Math.floor(x);
    const yi = Math.floor(y);
    const tx = fade(x - xi);
    const ty = fade(y - yi);
    const a = value(xi, yi);
    const b = value(xi + 1, yi);
    const c = value(xi, yi + 1);
    const d = value(xi + 1, yi + 1);
    return a + (b - a) * tx + (c - a) * ty + (a - b - c + d) * tx * ty;
  };
  /** Four octaves, normalised to 0..1. */
  return (x: number, y: number) => {
    let sum = 0;
    let amp = 0.5;
    let freq = 1;
    for (let o = 0; o < 4; o++) {
      sum += amp * noise(x * freq, y * freq);
      freq *= 2.03;
      amp *= 0.5;
    }
    return sum / 0.9375;
  };
}

/** Edge to edge: the field fills the whole area, thinning only behind the
    centred content so it stays readable. */
function fullShape(u: number, v: number) {
  const dx = (u - 0.5) / 0.3;
  const dy = (v - 0.52) / 0.3;
  return 0.75 * (1 - 0.85 * Math.exp(-(dx * dx + dy * dy)));
}

/** How much of the cloud a point keeps: two flanking masses, a clear centre. */
function shape(u: number, v: number) {
  const blob = (cx: number, cy: number, rx: number, ry: number) => {
    const dx = (u - cx) / rx;
    const dy = (v - cy) / ry;
    return Math.exp(-(dx * dx + dy * dy));
  };
  const masses = Math.max(
    blob(0.9, 0.2, 0.22, 0.46),
    blob(0.08, 0.56, 0.18, 0.36),
    0.7 * blob(0.74, 0.7, 0.16, 0.2),
    // Bands across the top and bottom so the field is not only at the sides.
    0.85 * blob(0.42, 0.0, 0.3, 0.17),
    0.8 * blob(0.4, 1.0, 0.32, 0.17),
  );
  // Keep the headline, copy and buttons clear.
  const centre = 1 - 0.97 * blob(0.5, 0.36, 0.3, 0.42);
  // Ease out towards the product tour below, but keep a sparse floor.
  const bottom = 0.18 + 0.82 * Math.min(1, Math.max(0, (0.98 - v) / 0.3));
  return masses * centre * bottom;
}

export function HeroGlyphField({ full = false }: { full?: boolean } = {}) {
  const ref = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = ref.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;
    const fbm = makeNoise(0x5eed1e);
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");
    const dark = () => {
      const theme = document.documentElement.dataset.theme;
      if (theme === "light") return false;
      if (theme === "dark") return true;
      return !window.matchMedia("(prefers-color-scheme: light)").matches;
    };

    let cols = 0;
    let rows = 0;
    let dpr = 1;
    let visible = true;
    let frame = 0;
    let last = 0;
    let time = 0;
    // One path batch per alpha level keeps fillStyle changes to a handful.
    const buckets: { x: number; y: number; ch: string }[][] = Array.from({ length: ALPHA_LEVELS }, () => []);

    const resize = () => {
      const rect = canvas.getBoundingClientRect();
      dpr = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = Math.max(1, Math.round(rect.height * dpr));
      cols = Math.ceil(rect.width / CELL_W);
      rows = Math.ceil(rect.height / CELL_H);
    };

    const draw = () => {
      const width = canvas.width / dpr;
      const height = canvas.height / dpr;
      context.setTransform(dpr, 0, 0, dpr, 0, 0);
      context.clearRect(0, 0, width, height);
      context.font = `10px ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`;
      context.textBaseline = "top";
      for (const bucket of buckets) bucket.length = 0;
      const drift = time * 0.000012;
      const isDark = dark();
      for (let r = 0; r < rows; r++) {
        const v = r / rows;
        for (let c = 0; c < cols; c++) {
          const u = c / cols;
          const keep = full ? fullShape(u, v) : shape(u, v);
          if (keep < 0.04) continue;
          // Domain-warped noise: ragged, organic edges with holes, not blobs.
          const x = c * 0.05 + drift * 40;
          const y = r * 0.085 - drift * 18;
          const warp = fbm(x * 0.6 + 5.2, y * 0.6 + 1.3);
          const n = fbm(x + 2.2 * warp, y + 2.2 * warp);
          const density = Math.pow(n, 1.6) * keep * 2.3;
          if (density < THRESHOLD) continue;
          const t = Math.min(1, (density - THRESHOLD) / (1 - THRESHOLD));
          // A faint shimmer: a few cells swap glyphs as the field drifts.
          const jitter = fbm(c * 0.9 + time * 0.00016, r * 0.9) * 0.18;
          const ch = RAMP[Math.min(RAMP.length - 1, 1 + Math.floor((t + jitter) * (RAMP.length - 2)))];
          const level = Math.min(ALPHA_LEVELS - 1, Math.floor(t * ALPHA_LEVELS));
          buckets[level].push({ x: c * CELL_W, y: r * CELL_H, ch });
        }
      }
      for (let level = 0; level < ALPHA_LEVELS; level++) {
        const bucket = buckets[level];
        if (bucket.length === 0) continue;
        const t = (level + 0.5) / ALPHA_LEVELS;
        const alpha = (isDark ? 0.05 : 0.05) + t * (isDark ? 0.24 : 0.18);
        // Light cells are neutral; the densest lean toward the brand cyan.
        context.fillStyle = isDark
          ? `rgba(${Math.round(205 - 48 * t)}, ${Math.round(220 + 25 * t)}, ${Math.round(230 + 25 * t)}, ${alpha})`
          : `rgba(${Math.round(60 - 52 * t)}, ${Math.round(80 + 45 * t)}, ${Math.round(100 + 56 * t)}, ${alpha})`;
        for (const cell of bucket) context.fillText(cell.ch, cell.x, cell.y);
      }
    };

    const tick = (now: number) => {
      frame = 0;
      if (!visible || document.hidden) return;
      if (now - last >= FRAME_MS) {
        time += now - last > 500 ? FRAME_MS : now - last;
        last = now;
        draw();
      }
      frame = requestAnimationFrame(tick);
    };
    const start = () => {
      if (reduced.matches) {
        draw();
        return;
      }
      if (!frame && visible && !document.hidden) {
        last = performance.now() - FRAME_MS;
        frame = requestAnimationFrame(tick);
      }
    };
    const stop = () => {
      if (frame) cancelAnimationFrame(frame);
      frame = 0;
    };

    resize();
    draw();
    start();

    const resizer = new ResizeObserver(() => {
      resize();
      draw();
    });
    resizer.observe(canvas);
    const watcher = new IntersectionObserver(([entry]) => {
      visible = entry.isIntersecting;
      if (visible) start();
      else stop();
    });
    watcher.observe(canvas);
    const onVisibility = () => (document.hidden ? stop() : start());
    document.addEventListener("visibilitychange", onVisibility);
    const onMotion = () => {
      stop();
      start();
    };
    reduced.addEventListener("change", onMotion);
    // Redraw in the right colours when the theme changes.
    const themeWatcher = new MutationObserver(() => draw());
    themeWatcher.observe(document.documentElement, { attributes: true, attributeFilter: ["data-theme"] });

    return () => {
      stop();
      resizer.disconnect();
      watcher.disconnect();
      themeWatcher.disconnect();
      document.removeEventListener("visibilitychange", onVisibility);
      reduced.removeEventListener("change", onMotion);
    };
  }, [full]);

  return <canvas ref={ref} className="hero-glyph-field" aria-hidden="true" />;
}
