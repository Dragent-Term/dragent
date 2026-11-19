import { parse as parseYaml } from "yaml";
import type { AppTheme, DesktopPalette, ThemeMode } from "./types";

export const DARK_THEME_ID = "dark";
export const LIGHT_THEME_ID = "light";
/** Stable application default. Legacy/unknown selections normalize here. */
export const DEFAULT_THEME_ID = DARK_THEME_ID;
/**
 * Legacy-only selection value. Previously followed the OS; old installs may
 * still have it persisted. It never appears in the public theme list —
 * resolve it to Dark/Light instead of adding a fifth theme.
 */
export const SYSTEM_THEME_ID = "system";

/** The only themes the mobile UI offers, in display order. */
export const CURATED_THEME_IDS: readonly string[] = ["dark", "gruvbox_dark", "gruvbox_light", "light"];

/** Desktop terminal colors (`Theme::dark()` in `theme.rs`), reused for neutral Dark. */
const DESKTOP_DARK_ANSI_NORMAL = ["#616161", "#ff8272", "#b4fa72", "#fefdc2", "#a5d5fe", "#ff8ffd", "#d0d1fe", "#f1f1f1"];
const DESKTOP_DARK_ANSI_BRIGHT = ["#8e8e8e", "#ffc4bd", "#d6fcb9", "#fefdd5", "#c1e3fe", "#ffb1fe", "#e5e6fe", "#feffff"];
/** Desktop terminal colors (`Theme::light()` in `theme.rs`), reused for neutral Light. */
const DESKTOP_LIGHT_ANSI_NORMAL = ["#1c1c1e", "#c4362a", "#348c48", "#c48412", "#246ec4", "#a03b8f", "#2a8f88", "#cacaca"];
const DESKTOP_LIGHT_ANSI_BRIGHT = ["#636363", "#d64939", "#469e5a", "#d69624", "#3680d6", "#b24da1", "#3ca19a", "#ffffff"];

/**
 * Neutral Dark: restrained graphite UI with a monochrome off-white primary.
 * Mobile-authored (not a desktop palette); the terminal keeps desktop colors.
 */
export const NEUTRAL_DARK_THEME: AppTheme = {
  id: DARK_THEME_ID,
  name: "Dark",
  mode: "dark",
  colors: {
    background: "#111113",
    surface: "#171719",
    surfaceRaised: "#202023",
    text: "#eeeeef",
    textMuted: "#acacad",
    textSubtle: "#808081",
    border: "#2b2b2f",
    accent: "#eeeeef",
    onAccent: "#0f1216",
    success: "#8fce9e",
    warning: "#d9a441",
    danger: "#e08a8a",
    selection: "#39393b",
  },
  terminal: {
    background: "#111113",
    foreground: "#eeeeef",
    cursor: "#eeeeef",
    ansiNormal: DESKTOP_DARK_ANSI_NORMAL,
    ansiBright: DESKTOP_DARK_ANSI_BRIGHT,
  },
};

/**
 * Neutral Light: near-white UI with a monochrome near-black primary.
 * Mobile-authored (not a desktop palette); the terminal keeps desktop colors.
 */
export const NEUTRAL_LIGHT_THEME: AppTheme = {
  id: LIGHT_THEME_ID,
  name: "Light",
  mode: "light",
  colors: {
    background: "#fafafa",
    surface: "#ffffff",
    surfaceRaised: "#f4f4f5",
    text: "#18181b",
    textMuted: "#5c5c5e",
    textSubtle: "#89898b",
    border: "#e4e4e7",
    accent: "#18181b",
    onAccent: "#ffffff",
    success: "#1f7a4d",
    warning: "#8a5a00",
    danger: "#c03535",
    selection: "#d1d1d2",
  },
  terminal: {
    background: "#fafafa",
    foreground: "#18181b",
    cursor: "#18181b",
    ansiNormal: DESKTOP_LIGHT_ANSI_NORMAL,
    ansiBright: DESKTOP_LIGHT_ANSI_BRIGHT,
  },
};

/**
 * Last-resort fallback if the bundled catalog is ever empty. Mirrors the
 * neutral Dark UI so the app never flashes a foreign accent.
 */
export const BUILTIN_DARK_PALETTE: DesktopPalette = {
  id: DARK_THEME_ID,
  name: "Dark",
  mode: "dark",
  background: "#111113",
  foreground: "#eeeeef",
  accent: "#eeeeef",
  ansiNormal: DESKTOP_DARK_ANSI_NORMAL,
  ansiBright: DESKTOP_DARK_ANSI_BRIGHT,
};

/** Last-resort fallback mirroring the neutral Light UI. */
export const BUILTIN_LIGHT_PALETTE: DesktopPalette = {
  id: LIGHT_THEME_ID,
  name: "Light",
  mode: "light",
  background: "#fafafa",
  foreground: "#18181b",
  accent: "#18181b",
  ansiNormal: DESKTOP_LIGHT_ANSI_NORMAL,
  ansiBright: DESKTOP_LIGHT_ANSI_BRIGHT,
};

interface Rgb {
  r: number;
  g: number;
  b: number;
}

const HEX_BODY = /^[0-9a-f]{6}$/;

/**
 * Normalize a desktop hex color to lowercase `#rrggbb`.
 * Accepts `#rgb`, `#rrggbb`, `#rrggbbaa` (alpha ignored, like the desktop
 * parser which reads the first three bytes), with or without `#`.
 */
export function normalizeHex(input: unknown): string {
  if (typeof input !== "string") {
    throw new Error(`Invalid color: expected a hex string, got ${typeof input}`);
  }
  const body = input.trim().replace(/^#/, "").toLowerCase();
  if (/^[0-9a-f]{3}$/.test(body)) {
    const [r, g, b] = body.split("");
    return `#${r}${r}${g}${g}${b}${b}`;
  }
  const six = body.slice(0, 6);
  if (six.length === 6 && HEX_BODY.test(six)) {
    return `#${six}`;
  }
  throw new Error(`Invalid color "${input}": expected #rrggbb`);
}

export function hexToRgb(hex: string): Rgb {
  const normalized = normalizeHex(hex);
  return {
    r: Number.parseInt(normalized.slice(1, 3), 16),
    g: Number.parseInt(normalized.slice(3, 5), 16),
    b: Number.parseInt(normalized.slice(5, 7), 16),
  };
}

export function rgbToHex({ r, g, b }: Rgb): string {
  const byte = (v: number): string => Math.round(v).toString(16).padStart(2, "0");
  return `#${byte(r)}${byte(g)}${byte(b)}`.toLowerCase();
}

/** Opaque mix of `fg` over `bg`, matching the desktop `blend()` helper. */
export function blendHex(bgHex: string, fgHex: string, ratio: number): string {
  const t = Math.min(1, Math.max(0, ratio));
  const bg = hexToRgb(bgHex);
  const fg = hexToRgb(fgHex);
  return rgbToHex({
    r: bg.r + (fg.r - bg.r) * t,
    g: bg.g + (fg.g - bg.g) * t,
    b: bg.b + (fg.b - bg.b) * t,
  });
}

export function rgbaFor(hex: string, alpha: number): string {
  const { r, g, b } = hexToRgb(hex);
  const a = Math.min(1, Math.max(0, alpha));
  return `rgba(${r}, ${g}, ${b}, ${a})`;
}

function linearize(channel: number): number {
  const v = channel / 255;
  return v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4;
}

/** WCAG relative luminance for an opaque hex color. */
export function relativeLuminance(hex: string): number {
  const { r, g, b } = hexToRgb(hex);
  return 0.2126 * linearize(r) + 0.7152 * linearize(g) + 0.0722 * linearize(b);
}

/** WCAG contrast ratio between two opaque hex colors. */
export function contrastRatio(aHex: string, bHex: string): number {
  const a = relativeLuminance(aHex);
  const b = relativeLuminance(bHex);
  const [hi, lo] = a >= b ? [a, b] : [b, a];
  return (hi + 0.05) / (lo + 0.05);
}

/** Effective contrast of `fg` at `alpha` composited over opaque `bg`. */
export function translucentContrast(fgHex: string, alpha: number, bgHex: string): number {
  if (alpha >= 1) return contrastRatio(fgHex, bgHex);
  const blended = blendHex(bgHex, fgHex, alpha);
  return contrastRatio(blended, bgHex);
}

/**
 * Pick the first alpha meeting `target` contrast over `bg`.
 * Falls back to fully opaque `fg` so muted text never stays unreadable.
 */
export function alphaMeetingContrast(
  fgHex: string,
  bgHex: string,
  candidates: readonly number[],
  target: number,
): number {
  for (const alpha of candidates) {
    if (translucentContrast(fgHex, alpha, bgHex) >= target) return alpha;
  }
  return 1;
}

/**
 * Opaque foreground with the stronger WCAG contrast against `accent`.
 * Prefer desktop ink, using pure black when needed for small button labels.
 */
export function accentTextFor(accentHex: string): string {
  const luminance = relativeLuminance(accentHex);
  const blackContrast = (luminance + 0.05) / 0.05;
  const whiteContrast = 1.05 / (luminance + 0.05);
  if (whiteContrast > blackContrast) return "#ffffff";
  return contrastRatio("#0f1216", accentHex) >= 4.5 ? "#0f1216" : "#000000";
}

/** The least contrast a small status mark keeps against its background. */
export const STATUS_MARK_CONTRAST = 4.5;

/**
 * `fg` in its own colour family, darkened on a light background (or
 * lightened on a dark one) just enough to reach `target` contrast. Blending
 * toward black or white keeps the hue, so a theme's green stays its green.
 */
export function withContrast(fgHex: string, bgHex: string, target: number = STATUS_MARK_CONTRAST): string {
  if (contrastRatio(fgHex, bgHex) >= target) return normalizeHex(fgHex);
  const toward = isDarkBackground(bgHex) ? "#ffffff" : "#000000";
  for (let step = 1; step <= 20; step += 1) {
    const candidate = blendHex(fgHex, toward, step / 20);
    if (contrastRatio(candidate, bgHex) >= target) return candidate;
  }
  return toward;
}

/** The status colours as small marks (dots, spinners) draw them. */
export interface StatusMarkColors {
  success: string;
  warning: string;
  danger: string;
  neutral: string;
}

/** A theme's own success, warning and danger, sharpened for its background. */
export function statusMarkColors(colors: {
  background: string;
  success: string;
  warning: string;
  danger: string;
  textMuted: string;
}): StatusMarkColors {
  return {
    success: withContrast(colors.success, colors.background),
    warning: withContrast(colors.warning, colors.background),
    danger: withContrast(colors.danger, colors.background),
    // Muted ink may be translucent; only an opaque one is sharpened.
    neutral: colors.textMuted.startsWith("#")
      ? withContrast(colors.textMuted, colors.background, 3)
      : colors.textMuted,
  };
}

/** Desktop `is_dark` luminance heuristic for `details`-less themes. */
export function isDarkBackground(bgHex: string): boolean {
  const { r, g, b } = hexToRgb(bgHex);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b < 128;
}

/** Desktop `display_name()`: filename stems to Title Case. */
export function displayNameForId(stem: string): string {
  const words = stem
    .split(/[_-]+/)
    .filter((part) => part.length > 0)
    .map((word) => word.charAt(0).toUpperCase() + word.slice(1).toLowerCase());
  return words.join(" ") || stem;
}

const ANSI_KEYS = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"] as const;

function readAnsiArray(value: unknown, label: string): string[] {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`Invalid theme: "${label}" must be a color map`);
  }
  const record = value as Record<string, unknown>;
  return ANSI_KEYS.map((key) => {
    if (!(key in record)) throw new Error(`Invalid theme: "${label}.${key}" is missing`);
    try {
      return normalizeHex(record[key]);
    } catch {
      throw new Error(`Invalid theme: "${label}.${key}" must be #rrggbb`);
    }
  });
}

function readBackground(value: unknown): string {
  if (typeof value === "string") {
    try {
      return normalizeHex(value);
    } catch {
      throw new Error(`Invalid theme: "background" must be #rrggbb or {top, bottom}`);
    }
  }
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    const record = value as Record<string, unknown>;
    if (typeof record["top"] !== "string") {
      throw new Error(`Invalid theme: "background.top" must be #rrggbb`);
    }
    let top: string;
    try {
      top = normalizeHex(record["top"]);
    } catch {
      throw new Error(`Invalid theme: "background.top" must be #rrggbb`);
    }
    if (record["bottom"] === undefined) return top;
    if (typeof record["bottom"] !== "string") {
      throw new Error(`Invalid theme: "background.bottom" must be #rrggbb`);
    }
    let bottom: string;
    try {
      bottom = normalizeHex(record["bottom"]);
    } catch {
      throw new Error(`Invalid theme: "background.bottom" must be #rrggbb`);
    }
    const a = hexToRgb(top);
    const b = hexToRgb(bottom);
    return rgbToHex({ r: (a.r + b.r) / 2, g: (a.g + b.g) / 2, b: (a.b + b.b) / 2 });
  }
  throw new Error(`Invalid theme: "background" must be #rrggbb or {top, bottom}`);
}

export interface ParseDesktopOptions {
  id: string;
  name?: string | undefined;
}

/**
 * Validate a desktop Warp-style YAML theme and normalize it.
 * Build-time helper for the bundled Gruvbox pair (never shipped user data);
 * `background_image` is accepted and ignored because mobile renders solid
 * surfaces.
 */
export function parseDesktopYaml(raw: string, options: ParseDesktopOptions): DesktopPalette {
  if (typeof raw !== "string" || raw.trim().length === 0) {
    throw new Error("Invalid theme: YAML text is empty");
  }
  let parsed: unknown;
  try {
    parsed = parseYaml(raw);
  } catch (error) {
    throw new Error(`Invalid theme: not valid YAML (${error instanceof Error ? error.message : String(error)})`);
  }
  if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
    throw new Error("Invalid theme: expected a mapping with accent/background/foreground/terminal_colors");
  }
  const doc = parsed as Record<string, unknown>;

  if (typeof doc["accent"] !== "string") throw new Error(`Invalid theme: "accent" must be #rrggbb`);
  if (typeof doc["foreground"] !== "string") throw new Error(`Invalid theme: "foreground" must be #rrggbb`);
  if (!("background" in doc)) throw new Error(`Invalid theme: "background" is missing`);
  if (!("terminal_colors" in doc)) throw new Error(`Invalid theme: "terminal_colors" is missing`);

  let accent: string;
  let foreground: string;
  let background: string;
  try {
    accent = normalizeHex(doc["accent"]);
  } catch {
    throw new Error(`Invalid theme: "accent" must be #rrggbb`);
  }
  try {
    foreground = normalizeHex(doc["foreground"]);
  } catch {
    throw new Error(`Invalid theme: "foreground" must be #rrggbb`);
  }
  background = readBackground(doc["background"]);

  const term = doc["terminal_colors"];
  if (term === null || typeof term !== "object" || Array.isArray(term)) {
    throw new Error(`Invalid theme: "terminal_colors" must have normal/bright maps`);
  }
  const termRecord = term as Record<string, unknown>;
  if (!("normal" in termRecord)) throw new Error(`Invalid theme: "terminal_colors.normal" is missing`);
  if (!("bright" in termRecord)) throw new Error(`Invalid theme: "terminal_colors.bright" is missing`);
  const ansiNormal = readAnsiArray(termRecord["normal"], "terminal_colors.normal");
  const ansiBright = readAnsiArray(termRecord["bright"], "terminal_colors.bright");

  let mode: ThemeMode;
  if (typeof doc["details"] === "string") {
    if (doc["details"] === "darker") mode = "dark";
    else if (doc["details"] === "lighter") mode = "light";
    else mode = isDarkBackground(background) ? "dark" : "light";
  } else {
    mode = isDarkBackground(background) ? "dark" : "light";
  }

  const name = typeof options.name === "string" && options.name.trim().length > 0
    ? options.name.trim()
    : displayNameForId(options.id);

  return { id: options.id, name, mode, background, foreground, accent, ansiNormal, ansiBright };
}

/**
 * Derive semantic app + terminal colors from a desktop palette.
 * Pure and dimension-independent, so phone/tablet/foldable/resize and
 * large-font settings all share one code path. Used at build time for the
 * bundled Gruvbox pair so the shipped catalog carries no desktop loader.
 */
export function toAppTheme(palette: DesktopPalette): AppTheme {
  const { id, name, mode, background, foreground, accent } = palette;
  const ansiNormal = [...palette.ansiNormal];
  const ansiBright = [...palette.ansiBright];

  const surface = blendHex(background, foreground, 0.05);
  const surfaceRaised = blendHex(background, foreground, 0.1);
  const border = blendHex(background, foreground, 0.26);

  const mutedAlpha = alphaMeetingContrast(foreground, background, [0.72, 0.85, 1] as const, 4.5);
  const subtleAlpha = alphaMeetingContrast(foreground, background, [0.6, 0.72, 0.85, 1] as const, 3);
  const textMuted = mutedAlpha >= 1 ? foreground : rgbaFor(foreground, mutedAlpha);
  const textSubtle = subtleAlpha >= 1 ? foreground : rgbaFor(foreground, subtleAlpha);

  const onAccent = accentTextFor(accent);
  const success = ansiNormal[2] ?? accent;
  const warning = ansiNormal[3] ?? accent;
  const danger = ansiNormal[1] ?? accent;
  const selection = blendHex(background, accent, 0.18);

  const cursor = contrastRatio(accent, background) >= 3 ? accent : foreground;

  return {
    id,
    name,
    mode,
    colors: {
      background,
      surface,
      surfaceRaised,
      text: foreground,
      textMuted,
      textSubtle,
      border,
      accent,
      onAccent,
      success,
      warning,
      danger,
      selection,
    },
    terminal: {
      background,
      foreground,
      cursor,
      ansiNormal,
      ansiBright,
    },
  };
}

/**
 * Normalize a persisted selection to the curated catalog. Legacy values
 * (`system`, `github_dark`, `custom`, typos) collapse to Dark — or to Light
 * for a legacy `system` choice on a light OS — so storage never holds a
 * hidden fifth selection.
 */
export function normalizeSelection(
  selection: string,
  osScheme: "light" | "dark" | null | undefined,
): string {
  if (CURATED_THEME_IDS.includes(selection)) return selection;
  if (selection === SYSTEM_THEME_ID) return osScheme === "light" ? LIGHT_THEME_ID : DARK_THEME_ID;
  return DEFAULT_THEME_ID;
}

/** Resolve the active theme. Unknown or legacy selections fall back to Dark. */
export function resolveAppTheme(
  selection: string,
  osScheme: "light" | "dark" | null | undefined,
  themes: readonly AppTheme[],
): AppTheme {
  const byId = new Map(themes.map((entry) => [entry.id, entry]));
  const direct = byId.get(selection);
  if (direct) return direct;
  const normalized = normalizeSelection(selection, osScheme);
  return byId.get(normalized) ?? byId.get(DEFAULT_THEME_ID) ?? byId.get(DARK_THEME_ID) ?? themes[0] ?? toAppTheme(BUILTIN_DARK_PALETTE);
}
