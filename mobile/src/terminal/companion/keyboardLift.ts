/**
 * Keyboard avoidance arithmetic for the terminal: how far the dock and the
 * terminal move, and which sizes the terminal is held at while they do. Pure,
 * with no React Native imports, so scripts can unit-test it under plain node.
 */
export interface KeyboardLiftMetrics {
  /** Grid rows the renderer is drawing. */
  readonly rows: number;
  /** Cursor row in grid coordinates. */
  readonly cursorRow: number;
  /** Full-screen program: the anchor is the bottom row, not the cursor. */
  readonly altScreen: boolean;
  /** Drawn px per row, fit scale included; absent before first measure. */
  readonly rowPitch?: number;
  /** Drawn px from the frame top to the first grid row. */
  readonly top?: number;
}

export interface KeyboardLiftInput {
  /** Px of the frame hidden below the dock's top edge (dock height excluded). */
  readonly covered: number;
  readonly metrics: KeyboardLiftMetrics | null | undefined;
  /** Laid-out frame height from before the keyboard opened. */
  readonly frameHeight: number;
}

const finite = (value: unknown): value is number =>
  typeof value === "number" && Number.isFinite(value);

/**
 * Lift needed to keep the anchor row (cursor row, or the bottom row on an
 * alt screen) plus one row of margin visible above the dock. Never more
 * than the keyboard actually covers, never more than it hides of the drawn
 * grid, and zero when the row is already visible. A short top-aligned grid
 * only moves by what the keyboard hides of it.
 */
export function computeKeyboardLift({ covered, metrics, frameHeight }: KeyboardLiftInput): number {
  if (!finite(covered) || covered <= 0) return 0;
  if (!metrics || !Number.isSafeInteger(metrics.rows) || metrics.rows <= 0) return covered;
  if (!finite(frameHeight) || frameHeight <= 0) return covered;

  const rows = metrics.rows;
  const pitch = finite(metrics.rowPitch) && metrics.rowPitch > 0 ? metrics.rowPitch : frameHeight / rows;
  const top = finite(metrics.top) && metrics.top >= 0 ? metrics.top : 0;
  const drawnBottom = top + rows * pitch;
  const visibleBottom = frameHeight - covered;
  // What the keyboard hides of the grid as drawn.
  const hidden = drawnBottom - visibleBottom;
  if (hidden <= 0) return 0;
  const cap = Math.min(covered, hidden);
  if (metrics.altScreen) return cap;

  const anchor = Number.isSafeInteger(metrics.cursorRow)
    ? Math.min(Math.max(metrics.cursorRow, 0), rows - 1)
    : rows - 1;
  const need = top + (anchor + 2) * pitch - visibleBottom;
  return Math.min(cap, Math.max(0, need));
}

/** Same-shape check so the view only re-renders on a changed snapshot. */
export function sameKeyboardLiftMetrics(
  a: KeyboardLiftMetrics | null,
  b: KeyboardLiftMetrics,
): boolean {
  return (
    a !== null &&
    a.rows === b.rows &&
    a.cursorRow === b.cursorRow &&
    a.altScreen === b.altScreen &&
    a.rowPitch === b.rowPitch &&
    a.top === b.top
  );
}

/**
 * Distance from the frame's bottom edge up to the keyboard's top edge, for a
 * frame that stops at the safe area (the app shell pads the bottom inset).
 * iOS reports the keyboard from the screen edge, so the inset is inside it.
 * React Native's Android height is already `ime.bottom - systemBars.bottom`.
 */
export function coveredKeyboardHeight(raw: number, bottomInset: number, platform: string): number {
  if (!finite(raw) || raw <= 0) return 0;
  return platform === "ios" ? Math.max(0, raw - Math.max(0, bottomInset)) : raw;
}

/**
 * What is left for the dock to climb after the window itself gave room back.
 * Android may or may not resize the window for the keyboard, so this measures
 * it instead of assuming: `shrink` is what the frame lost since before the
 * keyboard opened, and a window that shrank further than the keyboard is tall
 * never pushes the dock back down.
 */
export function effectiveKeyboardCover({ keyboardHeight, frameBefore, frameNow }: {
  readonly keyboardHeight: number;
  readonly frameBefore: number;
  readonly frameNow: number;
}): number {
  if (!finite(keyboardHeight) || keyboardHeight <= 0) return 0;
  const shrink = finite(frameBefore) && finite(frameNow) ? Math.max(0, frameBefore - frameNow) : 0;
  return Math.max(0, keyboardHeight - shrink);
}

/**
 * How much of the held terminal is out of sight. A shrinking window gives the
 * dock room but not the terminal: the held view keeps its height and the frame
 * clips it, so the part below the clip counts as covered just like the part
 * behind the dock. Using the effective cover alone would lift nothing in a
 * fully resized window and leave the active row clipped.
 */
export function terminalCoveredHeight({ heldHeight, clipHeight, effectiveCover }: {
  readonly heldHeight: number;
  readonly clipHeight: number;
  readonly effectiveCover: number;
}): number {
  return Math.max(0, heldHeight - Math.max(0, clipHeight - effectiveCover));
}

export interface LaidOut {
  readonly height: number;
  readonly width: number;
}

export interface HeldSizes {
  /** Outer frame height with the keyboard down: only the window changes it. */
  readonly frame: number;
  /** Size of the view holding the WebView. */
  readonly terminal: number;
  readonly width: number;
}

export interface KeyboardFrameState {
  readonly frame: LaidOut | null;
  readonly terminal: LaidOut | null;
  readonly held: HeldSizes | null;
  readonly keyboardUp: boolean;
  readonly keyboardHeight: number;
  /** A shrink with the keyboard down, not yet believed: see SHRINK_SETTLE_MS. */
  readonly settling: boolean;
  /** This window has been seen giving room back to the keyboard. */
  readonly windowResizes: boolean;
  /** Rotated with the keyboard up in a resizing window: held is an estimate. */
  readonly heldFromKeyboard: boolean;
}

export type KeyboardFrameEvent =
  | { readonly type: "frame" | "terminal"; readonly height: number; readonly width: number }
  | { readonly type: "keyboard"; readonly visible: boolean; readonly height: number }
  | { readonly type: "settle" };

export const INITIAL_KEYBOARD_FRAME: KeyboardFrameState = {
  frame: null,
  terminal: null,
  held: null,
  keyboardUp: false,
  keyboardHeight: 0,
  settling: false,
  windowResizes: false,
  heldFromKeyboard: false,
};

/**
 * How long a shrink with the keyboard down waits before the terminal follows.
 * Android delivers the resized layout and `keyboardDidShow` in either order,
 * and following a shrink that was really the keyboard would send the desktop a
 * resize. Growth and rotation never wait.
 */
export const SHRINK_SETTLE_MS = 400;

function follow(state: KeyboardFrameState): KeyboardFrameState {
  const { frame, terminal, held, keyboardUp, keyboardHeight } = state;
  // The two views report separately; a rotation is only whole once both have.
  if (!frame || !terminal || frame.width !== terminal.width) return state;
  const sizes: HeldSizes = { frame: frame.height, terminal: terminal.height, width: frame.width };
  if (!held) return { ...state, held: sizes, settling: false };

  if (sizes.width !== held.width) {
    // The new orientation's keyboard-down height can't be measured yet, so a
    // window known to resize gets it estimated from the keyboard's height.
    const estimate = keyboardUp && state.windowResizes;
    const carried = estimate ? keyboardHeight : 0;
    return {
      ...state,
      held: { frame: sizes.frame + carried, terminal: sizes.terminal + carried, width: sizes.width },
      settling: false,
      heldFromKeyboard: estimate,
    };
  }

  if (keyboardUp) {
    if (state.heldFromKeyboard) {
      // The rotated keyboard reports its height late: keep the estimate true.
      const delta = sizes.frame + keyboardHeight - held.frame;
      return delta === 0
        ? state
        : { ...state, held: { ...held, frame: held.frame + delta, terminal: held.terminal + delta } };
    }
    return sizes.frame < held.frame && !state.windowResizes ? { ...state, windowResizes: true } : state;
  }

  const shrank = sizes.frame < held.frame || sizes.terminal < held.terminal;
  if (shrank) return state.settling ? state : { ...state, settling: true };
  return { ...state, held: sizes, settling: false };
}

/** Tracks laid-out sizes against the sizes held from before the keyboard. */
export function reduceKeyboardFrame(
  state: KeyboardFrameState,
  event: KeyboardFrameEvent,
): KeyboardFrameState {
  switch (event.type) {
    case "frame":
    case "terminal": {
      if (!finite(event.height) || !finite(event.width) || event.height <= 0 || event.width <= 0) return state;
      return follow({ ...state, [event.type]: { height: event.height, width: event.width } });
    }
    case "keyboard":
      if (event.visible === state.keyboardUp && event.height === state.keyboardHeight) return state;
      return follow(
        event.visible
          // Any shrink still settling was this keyboard, so it is dropped here.
          ? { ...state, keyboardUp: true, keyboardHeight: Math.max(0, event.height), settling: false }
          : { ...state, keyboardUp: false, keyboardHeight: 0, windowResizes: false, heldFromKeyboard: false },
      );
    case "settle": {
      const { frame, terminal, keyboardUp, settling } = state;
      if (keyboardUp || !settling || !frame || !terminal || frame.width !== terminal.width) return state;
      return {
        ...state,
        settling: false,
        held: { frame: frame.height, terminal: terminal.height, width: frame.width },
      };
    }
  }
}

export interface KeyboardFrameView {
  /** How far the dock climbs. */
  readonly effectiveCover: number;
  /** Input to the terminal lift, measured against the held height. */
  readonly terminalCovered: number;
  /** Size to pin the view holding the WebView to; null before first layout. */
  readonly pinned: { readonly width: number; readonly height: number } | null;
}

export function keyboardFrameView(state: KeyboardFrameState): KeyboardFrameView {
  const { held, frame, terminal, keyboardUp, keyboardHeight } = state;
  const pinned = held ? { width: held.width, height: held.terminal } : null;
  if (!keyboardUp) return { effectiveCover: 0, terminalCovered: 0, pinned };
  const effectiveCover = effectiveKeyboardCover({
    keyboardHeight,
    frameBefore: held?.frame ?? 0,
    frameNow: frame?.height ?? 0,
  });
  const terminalCovered = held && terminal
    ? terminalCoveredHeight({ heldHeight: held.terminal, clipHeight: terminal.height, effectiveCover })
    : keyboardHeight;
  return { effectiveCover, terminalCovered, pinned };
}
