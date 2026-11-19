/**
 * The page as the phone has seen it so far: pictures ("tiles") of the
 * desktop's visible screen, each remembered at the place on the page where
 * it was taken. The phone scrolls across them at once, like a real page,
 * while the desktop is asked for the parts not seen yet.
 *
 * Everything is in the page's own pixels, measured from the top of the page.
 * Pure data: no React, no transport.
 */

export interface PageTile {
  /** Where on the page the picture starts. */
  readonly top: number;
  /** How much of the page it shows. */
  readonly height: number;
  readonly uri: string;
  /** Larger is newer; newer tiles are drawn over older ones. */
  readonly seq: number;
}

/** Tiles kept at once. Each is one compressed picture. */
export const MAX_TILES = 18;
/** How many screens either way are fetched ahead of the reader. */
export const PEEK_SCREENS = 6;
/** Two positions this close are the same place. */
const SAME_PLACE = 2;

/**
 * Add a tile. A newer picture of the same place replaces older ones, but the
 * one just before it stays underneath until the new one can be drawn. Beyond
 * [`MAX_TILES`] the tiles furthest from `near` go first.
 */
export function addTile(tiles: readonly PageTile[], tile: PageTile, near: number): PageTile[] {
  const samePlace = tiles.filter((other) => Math.abs(other.top - tile.top) <= SAME_PLACE);
  const newestSame = samePlace.reduce<PageTile | null>((best, other) => (best === null || other.seq > best.seq ? other : best), null);
  const kept = tiles.filter((other) => Math.abs(other.top - tile.top) > SAME_PLACE || other === newestSame);
  const all = [...kept, tile];
  if (all.length <= MAX_TILES) return all;
  // Never drop the new tile; drop the ones furthest from where the reader is.
  const rest = all.filter((other) => other !== tile).sort((a, b) => Math.abs(a.top - near) - Math.abs(b.top - near));
  return [...rest.slice(0, MAX_TILES - 1), tile];
}

/** Whether the tiles together show all of `[from, to]`. */
export function covers(tiles: readonly PageTile[], from: number, to: number): boolean {
  if (to - from <= SAME_PLACE) return true;
  const spans = tiles.map((tile) => [tile.top, tile.top + tile.height] as const).sort((a, b) => a[0] - b[0]);
  let reached = from;
  for (const [start, end] of spans) {
    if (start > reached + SAME_PLACE) break;
    reached = Math.max(reached, end);
    if (reached >= to - SAME_PLACE) return true;
  }
  return false;
}

/** The furthest the page can be scrolled. */
export function maxTop(full: number, viewHeight: number): number {
  return Math.max(0, full - viewHeight);
}

export function clampTop(top: number, full: number, viewHeight: number): number {
  return Math.min(maxTop(full, viewHeight), Math.max(0, top));
}

/**
 * Where to look next so scrolling finds the page already there: the nearest
 * screen not yet seen, up to [`PEEK_SCREENS`] away. `direction` is the way
 * the reader is moving (1 down, -1 up, 0 still): that way is looked at first.
 * Null when everything in reach is known.
 */
export function nextPeek(
  tiles: readonly PageTile[],
  viewTop: number,
  viewHeight: number,
  full: number,
  direction: -1 | 0 | 1 = 0,
): number | null {
  if (viewHeight <= 0) return null;
  const limit = maxTop(full, viewHeight);
  const wanted = (top: number): number | null => {
    const at = Math.round(Math.min(limit, Math.max(0, top)));
    return covers(tiles, at, Math.min(at + viewHeight, full)) ? null : at;
  };
  const first = direction < 0 ? -1 : 1;
  for (let step = 1; step <= PEEK_SCREENS; step += 1) {
    for (const way of [first, -first]) {
      const top = viewTop + way * step * viewHeight;
      // Past either end of the page there is nothing more to see.
      if ((way > 0 && top - viewHeight >= limit) || (way < 0 && top + viewHeight <= 0)) continue;
      const at = wanted(top);
      if (at !== null) return at;
    }
  }
  return null;
}

/** Whether the window itself scrolls (some pages scroll a panel inside instead). */
export function windowScrolls(full: number, viewHeight: number): boolean {
  return full > viewHeight + SAME_PLACE;
}
