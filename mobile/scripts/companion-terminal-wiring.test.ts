import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import Module from "node:module";
import { test } from "node:test";
import { createElement, type ReactNode } from "react";
// @ts-expect-error -- react-dom ships no types here, and this does not justify a new dependency.
import { renderToStaticMarkup } from "react-dom/server";

// This suite has no native renderer, so the real terminal screen and input dock
// run under the server renderer with their native leaves replaced. Effects and
// later state updates do not run, which leaves the draft empty; these tests are
// about where each button's press goes, and the engine's bytes are tested elsewhere.
interface Pressable { label?: string; onPress?: () => void }
const stage: {
  buttons: Pressable[];
  dismissed: number;
  hook: Record<string, unknown>;
  terminalProps: Record<string, unknown> | null;
} = { buttons: [], dismissed: 0, hook: {}, terminalProps: null };

const Passthrough = (props: { children?: ReactNode }) => props.children ?? null;
const Nothing = () => null;
const STUBS: Array<[RegExp, Record<string, unknown>]> = [
  [/^react-native$/, {
    View: Passthrough, ScrollView: Passthrough,
    Keyboard: { dismiss: () => { stage.dismissed++; } },
    Platform: { OS: "ios" },
    StyleSheet: { create: <T,>(styles: T) => styles },
    Animated: { Value: class { setValue() {} }, View: Passthrough, timing: () => ({ start() {} }) },
  }],
  [/^react-native-safe-area-context$/, { useSafeAreaInsets: () => ({ top: 0, bottom: 0, left: 0, right: 0 }) }],
  [/\/components$/, { AppText: Passthrough, Button: (props: Pressable) => { stage.buttons.push(props); return null; } }],
  [/\/components\/FieldInput$/, { FieldInput: Nothing }],
  [/\/features\/shared\/Feedback$/, { Feedback: Nothing }],
  [/\/theme$/, { useTheme: () => ({ theme: { colors: {}, terminal: {} } }) }],
  [/\/useCompanionTerminal$/, { useCompanionTerminal: () => stage.hook }],
  [/\/useSoftKeyboard$/, { useSoftKeyboard: () => ({ visible: false, height: 0, duration: 0 }) }],
  [/\/runtime\/WasmTerminalView$/, { WasmTerminalView: (props: Record<string, unknown>) => { stage.terminalProps = props; return null; } }],
  [/\/engine$/, { isUsableCompanionInput: () => true, isUsableCompanionResize: () => true }],
];
// tsx loads this package as CommonJS, so every import of the screen's passes through here.
const loader = Module as unknown as { _load(request: string, parent: { filename?: string } | null, isMain: boolean): unknown };
const load = loader._load;
loader._load = function stubbed(request, parent, isMain) {
  const stub = parent?.filename?.includes("/src/terminal/companion/") ? STUBS.find(([pattern]) => pattern.test(request)) : undefined;
  return stub ? stub[1] : load.call(this, request, parent, isMain);
};

async function renderScreen() {
  const sent: string[] = [];
  const submitted: string[] = [];
  stage.buttons = [];
  stage.dismissed = 0;
  stage.terminalProps = null;
  stage.hook = {
    store: {},
    status: { phase: "live", message: null, canWrite: true, canResize: true },
    eligible: true,
    sendInput: async (text: string) => { sent.push(text); return { ok: true }; },
    submitDraft: async (draft: string) => { submitted.push(draft); return { ok: true }; },
    sendResize: async () => ({ ok: true }),
    retry: () => {},
  };
  const { CompanionTerminalView } = await import("../src/terminal/companion/CompanionTerminalView");
  renderToStaticMarkup(createElement(CompanionTerminalView, { deviceId: "device", tabId: 1 }));
  const press = (label: string) => {
    const button = stage.buttons.find((candidate) => candidate.label === label);
    assert.ok(button?.onPress, `${label} is on screen`);
    button.onPress();
  };
  return { sent, submitted, press };
}
const settled = () => new Promise((resolve) => setImmediate(resolve));

test("Send goes to submitDraft and not to sendInput", async () => {
  const screen = await renderScreen();
  screen.press("Send ↵");
  await settled();
  assert.equal(screen.submitted.length, 1);
  assert.deepEqual(screen.sent, []);
});

test("a terminal key goes to sendInput and not to submitDraft", async () => {
  const screen = await renderScreen();
  screen.press("Esc");
  await settled();
  assert.equal(screen.sent.length, 1);
  assert.deepEqual(screen.submitted, []);
});

test("the hook hands Send to the live engine's submitDraft", () => {
  const source = readFileSync(new URL("../src/terminal/companion/useCompanionTerminal.ts", import.meta.url), "utf8");
  assert.match(source, /return rec\.engine\.submitDraft\(draft\);/);
  assert.match(source, /return \{ store, status, eligible, sendInput, submitDraft,/);
});

test("Hide keyboard dismisses natively and blurs the terminal's own input", async () => {
  const screen = await renderScreen();
  let blurred = 0;
  const control = stage.terminalProps?.inputControl as { current: unknown } | undefined;
  assert.ok(control, "the screen gives the renderer a control ref");
  control.current = { blurInput: () => { blurred++; } };
  screen.press("Hide keyboard");
  assert.equal(stage.dismissed, 1);
  assert.equal(blurred, 1);
});

test("Hide keyboard still dismisses when no renderer is mounted", async () => {
  const screen = await renderScreen();
  assert.doesNotThrow(() => screen.press("Hide keyboard"));
  assert.equal(stage.dismissed, 1);
});
