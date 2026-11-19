import { spacing } from "../theme/tokens";

export type LayoutSize = "compact" | "medium" | "expanded";

export interface ResponsiveLayout {
  size: LayoutSize;
  columns: 1 | 2;
  maxContentWidth: number;
  gutter: number;
}

const contentWidths: Record<LayoutSize, number> = {
  compact: 640,
  medium: 960,
  expanded: 1200,
};

/**
 * Breakpoints use readable width: increasing font size needs the same space as
 * narrowing a window. Smaller-than-default fonts never force extra columns.
 * Width is the available viewport in logical pixels, independent of device type.
 */
export function resolveLayout(width: number, fontScale: number): ResponsiveLayout {
  const viewportWidth = Number.isFinite(width) ? Math.max(0, width) : 0;
  const readableScale = Number.isFinite(fontScale) ? Math.max(1, fontScale) : 1;
  const readableWidth = viewportWidth / readableScale;
  const size: LayoutSize = readableWidth >= 1024 ? "expanded" : readableWidth >= 600 ? "medium" : "compact";
  const preferredGutter = size === "expanded" ? spacing.xxl : size === "medium" ? spacing.xl : spacing.lg;
  const gutter = Math.min(preferredGutter, viewportWidth / 2);

  return {
    size,
    columns: size === "expanded" ? 2 : 1,
    maxContentWidth: Math.min(contentWidths[size] * readableScale, viewportWidth - gutter * 2),
    gutter,
  };
}
