import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useReducer, useRef, useState, type ReactNode } from "react";
import type { DeliveryResult } from "./submitDraft";
import { Animated, Platform, StyleSheet, View, type LayoutChangeEvent } from "react-native";
import { AppText, Button } from "../../components";
import { WasmTerminalView } from "../runtime/WasmTerminalView";
import type { KeyboardAvoidanceMetrics, TerminalInputControl } from "../runtime/types";
import { CompanionInput } from "./CompanionInput";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { useTheme } from "../../theme";
import { Feedback } from "../../features/shared/Feedback";
import { useCompanionTerminal } from "./useCompanionTerminal";
import {
  computeKeyboardLift,
  INITIAL_KEYBOARD_FRAME,
  keyboardFrameView,
  reduceKeyboardFrame,
  sameKeyboardLiftMetrics,
  SHRINK_SETTLE_MS,
} from "./keyboardLift";
import { useSoftKeyboard } from "./useSoftKeyboard";
import { isUsableCompanionInput, isUsableCompanionResize } from "./engine";

function useKeyboardShift(distance: number, duration: number): Animated.Value {
  const [shift] = useState(() => new Animated.Value(0));
  // Follows the keyboard animation on iOS; Android reports the keyboard after
  // the fact, so it jumps with the `did` event.
  useEffect(() => {
    if (Platform.OS === "ios") {
      Animated.timing(shift, { toValue: -distance, duration, useNativeDriver: true }).start();
    } else {
      shift.setValue(-distance);
    }
  }, [shift, distance, duration]);
  return shift;
}

/**
 * Real per-tab host terminal surface. Reuses the existing WASM renderer +
 * runtime store, palette themes (inside WasmTerminalView) and the existing
 * compact composer dock — never a duplicated parser. Raw bytes feed
 * output_chunk deltas; snapshot grids feed alt_screen; the two are never
 * mixed after a grid.
 *
 * Keyboard avoidance lifts instead of resizing: the view holding the WebView
 * stays pinned to its pre-keyboard size and the renderer holds its fit (no PTY
 * resize while the keyboard is up), the dock rides directly above the keyboard,
 * and the terminal slides up just far enough to keep the active row visible,
 * clipped so it never paints over the header. Android may or may not resize the
 * window for the keyboard, so what it gave back is measured, never assumed.
 */
export function CompanionTerminalView({
  deviceId,
  tabId,
  onTakenByDesktop,
  renderInput,
}: {
  deviceId: string;
  tabId: number;
  /** The desktop took this terminal back: the screen leaves the tab. */
  onTakenByDesktop?: () => void;
  /** The input under the terminal; without one, the plain terminal input. */
  renderInput?: (input: {
    connected: boolean;
    sendInput: (text: string) => Promise<DeliveryResult>;
    submitDraft: (draft: string) => Promise<DeliveryResult>;
    blurTerminalInput: () => void;
  }) => ReactNode;
}) {
  const { store, status, eligible, sendInput, submitDraft, sendResize, retry, inputLost, clearInputLost } =
    useCompanionTerminal(tabId);

  // The desktop took this terminal back: leave for the list of tabs. Opening
  // the tab again takes the terminal over.
  const leaveTab = useRef(onTakenByDesktop);
  leaveTab.current = onTakenByDesktop;
  useEffect(() => {
    if (status.takenByDesktop) leaveTab.current?.();
  }, [status.takenByDesktop]);

  // Manual recovery for stopped, non-authoritative phases only: after the
  // bounded reattach attempts fail, the user reattaches in place without
  // leaving the route. Gated on eligibility (implies lease active), so no
  // retry is offered while background/offline/lease-ended — the ended
  // logical session stays parent-owned (Start new session) — nor after an
  // authoritative host close, nor during attaching/live polling.
  const showRetry =
    eligible &&
    (status.phase === "error" || status.phase === "unavailable" || status.phase === "rejected");

  const insets = useSafeAreaInsets();
  const { theme } = useTheme();
  const keyboard = useSoftKeyboard(insets.bottom);
  const [inputError, setInputError] = useState<string | null>(null);
  const [metrics, setMetrics] = useState<KeyboardAvoidanceMetrics | null>(null);
  const connected = eligible && status.canWrite;
  const sessionId = `companion:${deviceId}:${tabId}`;

  const terminalInput = useRef<TerminalInputControl>(null);
  const blurTerminalInput = useCallback(() => terminalInput.current?.blurInput(), []);

  const handleMetrics = useCallback((next: KeyboardAvoidanceMetrics) => {
    setMetrics((previous) => (sameKeyboardLiftMetrics(previous, next) ? previous : next));
  }, []);

  const [layout, dispatchLayout] = useReducer(reduceKeyboardFrame, INITIAL_KEYBOARD_FRAME);
  // Before paint, so the dock starts moving with the keyboard's own animation.
  useLayoutEffect(() => {
    dispatchLayout({ type: "keyboard", visible: keyboard.visible, height: keyboard.height });
  }, [keyboard.visible, keyboard.height]);
  useEffect(() => {
    if (!layout.settling) return;
    const timer = setTimeout(() => dispatchLayout({ type: "settle" }), SHRINK_SETTLE_MS);
    return () => clearTimeout(timer);
  }, [layout.settling, layout.frame, layout.terminal]);
  // Measured on the outer frame because only the window changes its height; the
  // inner clip also moves when the dock or status line grows.
  const handleFrameLayout = useCallback((event: LayoutChangeEvent) => {
    const { height, width } = event.nativeEvent.layout;
    dispatchLayout({ type: "frame", height, width });
  }, []);
  const handleTerminalLayout = useCallback((event: LayoutChangeEvent) => {
    const { height, width } = event.nativeEvent.layout;
    dispatchLayout({ type: "terminal", height, width });
  }, []);

  const { effectiveCover, terminalCovered, pinned } = keyboardFrameView(layout);
  const terminalLift = useMemo(
    () => computeKeyboardLift({ covered: terminalCovered, metrics, frameHeight: pinned?.height ?? 0 }),
    [terminalCovered, metrics, pinned?.height],
  );
  const terminalShift = useKeyboardShift(terminalLift, keyboard.duration);
  const dockShift = useKeyboardShift(effectiveCover, keyboard.duration);

  return (
    <View style={styles.frame} onLayout={handleFrameLayout}>
      {status.message ? (
        <View style={styles.status}>
          {showRetry ? <Feedback message={status.message} /> : <AppText variant="caption" tone="textMuted" accessibilityLiveRegion="polite">{status.message}</AppText>}
          {showRetry ? (
            <Button label="Retry" size="sm" variant="ghost" onPress={retry} />
          ) : null}
        </View>
      ) : null}
      {inputLost ? (
        <View style={styles.notice}>
          <AppText variant="caption" tone="textMuted" accessibilityLiveRegion="polite" style={styles.noticeText}>{inputLost.message}</AppText>
          <Button label="Dismiss" size="sm" variant="ghost" onPress={clearInputLost} />
        </View>
      ) : null}
      <View
        style={[styles.terminal, { backgroundColor: theme.terminal.background }]}
        onLayout={handleTerminalLayout}
      >
        {/* Pinned, not flexed: the frame can shrink under it and only clips it. */}
        <Animated.View style={[pinned ?? styles.fill, { transform: [{ translateY: terminalShift }] }]}>
          <WasmTerminalView
            store={store}
            connected={connected}
            canWrite={status.canWrite}
            canResize={status.canResize}
            holdFit={keyboard.visible}
            onInput={(text) => {
              if (!status.canWrite || !isUsableCompanionInput(text)) return false;
              void sendInput(text).then((result) => setInputError(result.ok ? null : result.message));
              return true;
            }}
            onResize={(rows, cols) => {
              if (!status.canResize || !isUsableCompanionResize(rows, cols)) return false;
              void sendResize(rows, cols);
              return true;
            }}
            onMetrics={handleMetrics}
            inputControl={terminalInput}
          />
        </Animated.View>
      </View>
      {/* No bottom padding: the app shell's safe area already stops the frame above the inset. */}
      <Animated.View style={{ transform: [{ translateY: dockShift }] }}>
        <Feedback message={inputError} />
        {renderInput ? (
          <Fragment key={sessionId}>{renderInput({ connected, sendInput, submitDraft, blurTerminalInput })}</Fragment>
        ) : (
          <CompanionInput key={sessionId} connected={connected} sendInput={sendInput} submitDraft={submitDraft} blurTerminalInput={blurTerminalInput} />
        )}
      </Animated.View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0, overflow: "hidden" },
  fill: { flex: 1, minHeight: 0 },
  status: { paddingHorizontal: 16, paddingTop: 8 },
  notice: { flexDirection: "row", alignItems: "center", gap: 8, paddingHorizontal: 16, paddingTop: 8 },
  noticeText: { flex: 1, minWidth: 0 },
  terminal: { flex: 1, minHeight: 100, overflow: "hidden" },
});
