/** Shared dimensions and typography; colors come exclusively from useTheme(). */
export const spacing = {
  xs: 4,
  sm: 8,
  md: 12,
  lg: 16,
  xl: 24,
  xxl: 32,
} as const;

/** Internal control/card insets are denser; page gutters use spacing unchanged. */
export const componentPaddingScale = 0.8;

export const radii = {
  sm: 6,
  md: 8,
  lg: 10,
  xl: 12,
  pill: 999,
} as const;

export const type = {
  hero: { fontSize: 28, fontWeight: "600" as const, lineHeight: 34, letterSpacing: -0.5 },
  title: { fontSize: 20, fontWeight: "600" as const, lineHeight: 26, letterSpacing: -0.3 },
  body: { fontSize: 14, fontWeight: "400" as const, lineHeight: 21 },
  caption: { fontSize: 12, fontWeight: "400" as const, lineHeight: 18 },
  micro: { fontSize: 10, fontWeight: "600" as const, lineHeight: 14, letterSpacing: 0.6 },
} as const;
