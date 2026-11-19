import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { test } from 'node:test';
import {
  computeKeyboardLift,
  coveredKeyboardHeight,
  effectiveKeyboardCover,
  INITIAL_KEYBOARD_FRAME,
  keyboardFrameView,
  reduceKeyboardFrame,
  sameKeyboardLiftMetrics,
  terminalCoveredHeight,
  type KeyboardFrameEvent,
  type KeyboardFrameState,
  type KeyboardLiftMetrics,
} from '../src/terminal/companion/keyboardLift';

const FRAME = 800;
const ROWS = 40;
const COVERED = 300;

const metrics = (overrides: Partial<KeyboardLiftMetrics> = {}): KeyboardLiftMetrics => ({
  rows: ROWS,
  cursorRow: 0,
  altScreen: false,
  ...overrides,
});

test('closed keyboard never lifts', () => {
  assert.equal(computeKeyboardLift({ covered: 0, metrics: metrics({ cursorRow: 39 }), frameHeight: FRAME }), 0);
  assert.equal(computeKeyboardLift({ covered: -10, metrics: metrics(), frameHeight: FRAME }), 0);
});

test('missing metrics, rows or frame fall back to the covered height', () => {
  assert.equal(computeKeyboardLift({ covered: COVERED, metrics: null, frameHeight: FRAME }), COVERED);
  assert.equal(computeKeyboardLift({ covered: COVERED, metrics: undefined, frameHeight: FRAME }), COVERED);
  assert.equal(computeKeyboardLift({ covered: COVERED, metrics: metrics({ rows: 0 }), frameHeight: FRAME }), COVERED);
  assert.equal(computeKeyboardLift({ covered: COVERED, metrics: metrics(), frameHeight: 0 }), COVERED);
});

test('row already visible lifts nothing', () => {
  assert.equal(
    computeKeyboardLift({ covered: COVERED, metrics: metrics({ cursorRow: 5 }), frameHeight: FRAME }),
    0,
  );
});

test('hidden row lifts exactly enough to clear it plus one row of margin', () => {
  // Dock top at 500; anchor row 30 ends at 620, plus one margin row = 640.
  assert.equal(
    computeKeyboardLift({ covered: COVERED, metrics: metrics({ cursorRow: 30 }), frameHeight: FRAME }),
    140,
  );
});

test('alt screen keeps the bottom row visible with the full hidden strip', () => {
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ cursorRow: 0, altScreen: true }),
      frameHeight: FRAME,
    }),
    COVERED,
  );
});

test('short top-aligned grid only moves by what the keyboard hides', () => {
  // 10 drawn rows end at 200, dock top at 500: nothing hidden.
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ rows: 10, cursorRow: 9, altScreen: true, rowPitch: 20 }),
      frameHeight: FRAME,
    }),
    0,
  );
  // 30 drawn rows end at 600: 100 hidden, so the lift caps there even for the last row.
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ rows: 30, cursorRow: 29, rowPitch: 20 }),
      frameHeight: FRAME,
    }),
    100,
  );
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ rows: 30, cursorRow: 29, altScreen: true, rowPitch: 20 }),
      frameHeight: FRAME,
    }),
    100,
  );
});

test('drawn pitch and top offset move the anchor with the grid as drawn', () => {
  // Grid drawn 100px lower: the same anchor row needs 240 instead of 140.
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ cursorRow: 30, rowPitch: 20, top: 100 }),
      frameHeight: FRAME,
    }),
    240,
  );
  // A scaled-down pitch draws a short grid: nothing hidden, no lift.
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ cursorRow: 39, rowPitch: 10 }),
      frameHeight: FRAME,
    }),
    0,
  );
});

test('lift never exceeds the covered height', () => {
  assert.equal(
    computeKeyboardLift({ covered: COVERED, metrics: metrics({ cursorRow: 39 }), frameHeight: FRAME }),
    COVERED,
  );
  assert.equal(
    computeKeyboardLift({
      covered: COVERED,
      metrics: metrics({ cursorRow: 99 }),
      frameHeight: FRAME,
    }),
    COVERED,
  );
});

test('same-shape check gates view updates', () => {
  const base = metrics({ cursorRow: 3 });
  assert.equal(sameKeyboardLiftMetrics(base, { ...base }), true);
  assert.equal(sameKeyboardLiftMetrics(null, base), false);
  assert.equal(sameKeyboardLiftMetrics(base, { ...base, cursorRow: 4 }), false);
  assert.equal(sameKeyboardLiftMetrics(base, { ...base, rowPitch: 20 }), false);
});

test('effective cover subtracts what the window already gave back', () => {
  const cover = (frameNow: number, keyboardHeight = 300) =>
    effectiveKeyboardCover({ keyboardHeight, frameBefore: 800, frameNow });
  assert.equal(cover(800), 300, 'window not resized');
  assert.equal(cover(700), 200, 'window partly resized');
  assert.equal(cover(500), 0, 'window fully resized');
  assert.equal(cover(450), 0, 'window shrank further than the keyboard is tall');
  assert.equal(cover(900), 300, 'a taller window gives nothing back');
  assert.equal(cover(500, 0), 0, 'no keyboard');
  assert.equal(effectiveKeyboardCover({ keyboardHeight: 300, frameBefore: Number.NaN, frameNow: 500 }), 300);
});

test('covered height removes the home-indicator strip on iOS only', () => {
  assert.equal(coveredKeyboardHeight(336, 34, 'ios'), 302);
  assert.equal(coveredKeyboardHeight(20, 34, 'ios'), 0, 'a hardware-keyboard bar shorter than the inset');
  assert.equal(coveredKeyboardHeight(300, 48, 'android'), 300, 'React Native already removed the system bars');
  assert.equal(coveredKeyboardHeight(0, 34, 'ios'), 0);
});

// The app shell's safe area stops the frame above the bottom inset, and the
// dock adds no padding of its own, so the frame's bottom edge is the dock's.
test('dock content lands exactly on the keyboard in every window behaviour', () => {
  const dockBottom = (frameBottom: number, keyboardHeight: number, shrink: number) =>
    frameBottom - effectiveKeyboardCover({ keyboardHeight, frameBefore: 1000, frameNow: 1000 - shrink });
  // iOS 844pt screen, 34pt inset, keyboard 336pt from the screen edge.
  assert.equal(dockBottom(844 - 34, coveredKeyboardHeight(336, 34, 'ios'), 0), 844 - 336);
  // Android 900dp window, 48dp nav bar, `ime.bottom` 348 so React Native says 300.
  const keyboardTop = 900 - 348;
  const reported = coveredKeyboardHeight(348 - 48, 48, 'android');
  assert.equal(dockBottom(900 - 48, reported, 0), keyboardTop, 'edge-to-edge window keeps its height');
  // Resized by the IME inset, the safe area collapses too, so the frame lost `reported`.
  assert.equal(dockBottom(900 - 48 - reported, reported, reported), keyboardTop, 'window fully resized');
  assert.equal(dockBottom(900 - 48 - 100, reported, 100), keyboardTop, 'window partly resized');
});

test('dock adds no inset of its own and the error sits inside the raised dock', () => {
  const source = readFileSync(new URL('../src/terminal/companion/CompanionTerminalView.tsx', import.meta.url), 'utf8');
  assert.doesNotMatch(source, /paddingBottom/);
  const dock = source.slice(source.indexOf('translateY: dockShift'), source.indexOf('const styles'));
  const error = dock.indexOf('<Feedback message={inputError} />');
  assert.ok(error > 0, 'the input error is inside the dock container');
  assert.ok(error < dock.indexOf('</Animated.View>'), 'and before it closes');
  assert.equal(source.split('<Feedback message={inputError} />').length, 2, 'and rendered only there');
  // The shell is what makes the padding unnecessary: it must keep its bottom edge here.
  const shell = readFileSync(new URL('../src/navigation/AppShell.tsx', import.meta.url), 'utf8');
  assert.match(shell, /path\.startsWith\("\/machine"\)\) return \(\s*<SafeAreaView edges=\{\["top", "bottom", "left", "right"\]\}/);
});

test('the view pins the WebView holder and holds the fit from the same keyboard state', () => {
  const source = readFileSync(new URL('../src/terminal/companion/CompanionTerminalView.tsx', import.meta.url), 'utf8');
  assert.match(source, /<Animated\.View style=\{\[pinned \?\? styles\.fill,/);
  assert.match(source, /holdFit=\{keyboard\.visible\}/);
  assert.match(source, /terminal: \{[^}]*overflow: "hidden"/, 'the clip keeps the lift off the header');
});

test('app config stays strict JSON with no global keyboard mode', () => {
  const config = JSON.parse(readFileSync(new URL('../app.json', import.meta.url), 'utf8'));
  assert.equal(config.expo.android.softwareKeyboardLayoutMode, undefined);
  assert.equal(config.expo.softwareKeyboardLayoutMode, undefined);
});

const WIDTH = 400;
const run = (events: KeyboardFrameEvent[], from: KeyboardFrameState = INITIAL_KEYBOARD_FRAME) =>
  events.reduce(reduceKeyboardFrame, from);
const laidOut = (frame: number, terminal: number, width = WIDTH): KeyboardFrameEvent[] => [
  { type: 'frame', height: frame, width },
  { type: 'terminal', height: terminal, width },
];
const keyboard = (height: number): KeyboardFrameEvent => ({ type: 'keyboard', visible: height > 0, height });
const rested = run(laidOut(800, 700));

test('first layout pins the terminal at its measured size', () => {
  assert.equal(keyboardFrameView(INITIAL_KEYBOARD_FRAME).pinned, null);
  assert.deepEqual(keyboardFrameView(rested), { effectiveCover: 0, terminalCovered: 0, pinned: { width: WIDTH, height: 700 } });
});

test('window that keeps its height: the dock climbs the full keyboard', () => {
  const up = run([keyboard(300)], rested);
  assert.deepEqual(keyboardFrameView(up), { effectiveCover: 300, terminalCovered: 300, pinned: { width: WIDTH, height: 700 } });
});

test('window that resizes: layout after the keyboard event keeps the pin and the dock stays put', () => {
  const up = run([keyboard(300), ...laidOut(500, 400)], rested);
  assert.deepEqual(keyboardFrameView(up), { effectiveCover: 0, terminalCovered: 300, pinned: { width: WIDTH, height: 700 } });
});

test('window that resizes: layout before the keyboard event is not mistaken for a new height', () => {
  const early = run(laidOut(500, 400), rested);
  assert.equal(early.settling, true);
  assert.deepEqual(keyboardFrameView(early).pinned, { width: WIDTH, height: 700 }, 'the canvas must not shrink, or a resize goes out');
  const up = run([keyboard(300)], early);
  assert.equal(up.settling, false);
  assert.deepEqual(keyboardFrameView(up), { effectiveCover: 0, terminalCovered: 300, pinned: { width: WIDTH, height: 700 } });
});

test('active row stays visible whichever way the window behaves', () => {
  const cursor = metrics({ rows: 35, cursorRow: 30, rowPitch: 20 });
  const lift = (state: KeyboardFrameState) => {
    const view = keyboardFrameView(state);
    return computeKeyboardLift({ covered: view.terminalCovered, metrics: cursor, frameHeight: view.pinned?.height ?? 0 });
  };
  const still = run([keyboard(300)], rested);
  const resized = run([keyboard(300), ...laidOut(500, 400)], rested);
  // Row 30 plus its margin ends at 640 and 400 stays visible.
  assert.equal(lift(still), 240);
  assert.equal(lift(resized), 240);
  // A partly resized window also leaves 400 visible: a 600 clip less the 200 the dock covers.
  assert.equal(lift(run([keyboard(300), ...laidOut(700, 600)], rested)), 240);
  // The row's bottom edge, after the lift, is above what the clip and dock leave visible.
  const view = keyboardFrameView(resized);
  assert.ok(640 - lift(resized) <= 400 - view.effectiveCover);
});

test('held terminal coverage counts both the clip and the dock', () => {
  assert.equal(terminalCoveredHeight({ heldHeight: 700, clipHeight: 700, effectiveCover: 300 }), 300);
  assert.equal(terminalCoveredHeight({ heldHeight: 700, clipHeight: 400, effectiveCover: 0 }), 300);
  assert.equal(terminalCoveredHeight({ heldHeight: 700, clipHeight: 500, effectiveCover: 100 }), 300);
  assert.equal(terminalCoveredHeight({ heldHeight: 700, clipHeight: 50, effectiveCover: 100 }), 700);
});

test('a composer growing under the keyboard keeps the pin and counts as covered', () => {
  const up = run([keyboard(300), { type: 'terminal', height: 660, width: WIDTH }], rested);
  assert.deepEqual(keyboardFrameView(up), { effectiveCover: 300, terminalCovered: 340, pinned: { width: WIDTH, height: 700 } });
});

test('a shrink with no keyboard behind it is applied once it settles', () => {
  const shrunk = run(laidOut(600, 500), rested);
  assert.deepEqual(keyboardFrameView(shrunk).pinned, { width: WIDTH, height: 700 });
  assert.deepEqual(keyboardFrameView(run([{ type: 'settle' }], shrunk)).pinned, { width: WIDTH, height: 500 });
  // And settle does nothing with no shrink pending or with the keyboard up.
  assert.equal(run([{ type: 'settle' }], rested), rested);
  const up = run([keyboard(300)], shrunk);
  assert.equal(run([{ type: 'settle' }], up), up);
});

test('growth with the keyboard down is applied at once', () => {
  assert.deepEqual(keyboardFrameView(run(laidOut(900, 800), rested)).pinned, { width: WIDTH, height: 800 });
});

test('hiding the keyboard releases cleanly in both window behaviours', () => {
  const stillUp = run([keyboard(300)], rested);
  const stillDown = run([keyboard(0)], stillUp);
  assert.deepEqual(keyboardFrameView(stillDown), { effectiveCover: 0, terminalCovered: 0, pinned: { width: WIDTH, height: 700 } });
  assert.equal(stillDown.settling, false);

  // Resized window: the keyboard event can beat the regrow, and the pin must not drop in between.
  const resizedUp = run([keyboard(300), ...laidOut(500, 400)], rested);
  const hiding = run([keyboard(0)], resizedUp);
  assert.equal(hiding.settling, true);
  assert.deepEqual(keyboardFrameView(hiding).pinned, { width: WIDTH, height: 700 });
  const regrown = run(laidOut(800, 700), hiding);
  assert.equal(regrown.settling, false);
  assert.deepEqual(keyboardFrameView(regrown).pinned, { width: WIDTH, height: 700 });
  assert.equal(regrown.windowResizes, false);
  // And the other order: regrow first, keyboard event second.
  const other = run([...laidOut(800, 700), keyboard(0)], resizedUp);
  assert.equal(other.settling, false);
  assert.deepEqual(keyboardFrameView(other).pinned, { width: WIDTH, height: 700 });
});

test('rotation with the keyboard up applies the new width in one step', () => {
  const up = run([keyboard(300)], rested);
  // The first report alone is half a rotation: nothing moves until both views agree.
  const half = run([{ type: 'frame', height: 340, width: 800 }], up);
  assert.deepEqual(keyboardFrameView(half).pinned, { width: WIDTH, height: 700 });
  const rotated = run([{ type: 'terminal', height: 240, width: 800 }, keyboard(200)], half);
  assert.deepEqual(keyboardFrameView(rotated), { effectiveCover: 200, terminalCovered: 200, pinned: { width: 800, height: 240 } });
});

test('rotation with the keyboard up in a resizing window restores the height the keyboard took', () => {
  const up = run([keyboard(300), ...laidOut(500, 400)], rested);
  const rotated = run(laidOut(140, 100, 800), up);
  assert.deepEqual(keyboardFrameView(rotated).pinned, { width: 800, height: 400 });
  assert.equal(keyboardFrameView(rotated).effectiveCover, 0);
  // The rotated keyboard then reports its real, shorter height.
  const settled = run([keyboard(200)], rotated);
  assert.deepEqual(keyboardFrameView(settled), { effectiveCover: 0, terminalCovered: 200, pinned: { width: 800, height: 300 } });
});

test('malformed layout samples are ignored', () => {
  assert.equal(run([{ type: 'frame', height: 0, width: WIDTH }], rested), rested);
  assert.equal(run([{ type: 'terminal', height: Number.NaN, width: WIDTH }], rested), rested);
});
