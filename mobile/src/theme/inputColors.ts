/**
 * Caret and selection colours for text fields, readable in every theme.
 *
 * iOS draws the caret and the selection handles in `selectionColor` and makes
 * its own see-through highlight from it, so it needs a strong colour. Android
 * fills the highlight with `selectionColor` as given, so it gets the same
 * colour see-through, with the caret and handles in the strong one. A theme
 * whose accent is a grey (the plain light and dark themes) gets Dragent's cyan
 * instead: a grey caret disappears into the text.
 */
export interface InputColors {
  /** The caret and selection handles. */
  readonly caret: string;
  /** The highlight behind selected text (Android). */
  readonly highlight: string;
  /** What to pass as `selectionColor` on this platform. */
  readonly selection: string;
}

const BRAND_ON_DARK = "#3cbfeb";
const BRAND_ON_LIGHT = "#0a78ad";

function rgb(hex: string): [number, number, number] | null {
  const match = /^#([0-9a-f]{6})$/iu.exec(hex.trim());
  if (!match) return null;
  const value = Number.parseInt(match[1] ?? "0", 16);
  return [(value >> 16) & 255, (value >> 8) & 255, value & 255];
}

/** True for a grey: no hue for a caret to stand out with. */
export function isGrey(hex: string): boolean {
  const channels = rgb(hex);
  if (!channels) return true;
  return Math.max(...channels) - Math.min(...channels) < 24;
}

function luminance(hex: string): number {
  const channels = rgb(hex) ?? [0, 0, 0];
  const linear = (channel: number) => {
    const c = channel / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * linear(channels[0]) + 0.7152 * linear(channels[1]) + 0.0722 * linear(channels[2]);
}

export function inputColorsFor(accent: string, background: string, os: string): InputColors {
  const dark = luminance(background) < 0.25;
  const caret = isGrey(accent) ? (dark ? BRAND_ON_DARK : BRAND_ON_LIGHT) : accent;
  const highlight = `${caret}${dark ? "66" : "4d"}`;
  return { caret, highlight, selection: os === "ios" ? caret : highlight };
}
