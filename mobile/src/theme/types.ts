/**
 * Semantic theme types for Dragent mobile.
 *
 * Colors are always ` #rrggbb ` hex or ` rgba(...) ` strings so they work
 * unchanged across phone, tablet, foldable, and window-resizing layouts as
 * well as large-font (Dynamic Type) settings — no dimension is baked in.
 */

/** Light or dark appearance. Mirrors the desktop `is_dark` flag. */
export type ThemeMode = "light" | "dark";

/** Semantic surface + text + status colors every screen consumes. */
export interface ThemeColors {
  background: string;
  surface: string;
  surfaceRaised: string;
  text: string;
  textMuted: string;
  textSubtle: string;
  border: string;
  accent: string;
  onAccent: string;
  success: string;
  warning: string;
  danger: string;
  selection: string;
}

/** Terminal palette derived from the same desktop source. */
export interface TerminalPalette {
  background: string;
  foreground: string;
  cursor: string;
  ansiNormal: readonly string[];
  ansiBright: readonly string[];
}

/** A fully derived, ready-to-render theme. */
export interface AppTheme {
  id: string;
  name: string;
  mode: ThemeMode;
  colors: ThemeColors;
  terminal: TerminalPalette;
}

/**
 * Raw desktop palette as stored in the generated catalog. Hex strings are
 * normalized `#rrggbb`; `mode` mirrors desktop `details`/luminance logic.
 */
export interface DesktopPalette {
  id: string;
  name: string;
  mode: ThemeMode;
  background: string;
  foreground: string;
  accent: string;
  ansiNormal: readonly string[];
  ansiBright: readonly string[];
}

/** Persisted selection: a theme id, or `"system"` to follow the OS. */
export type ThemeSelection = string;

/** Storage surface owned exclusively by the theme context. */
export interface ThemeStorage {
  getItem(key: string): Promise<string | null>;
  setItem(key: string, value: string): Promise<void>;
}

export interface UseThemeResult {
  theme: AppTheme;
  themes: readonly AppTheme[];
  /** The persisted selection: one of the four curated theme ids. */
  selection: ThemeSelection;
  setThemeId(id: string): Promise<void>;
  isReady: boolean;
  error: string | null;
}
