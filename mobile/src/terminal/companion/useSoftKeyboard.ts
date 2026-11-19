import { useEffect, useState } from "react";
import { Keyboard, Platform, type KeyboardEvent } from "react-native";
import { coveredKeyboardHeight } from "./keyboardLift";

export interface SoftKeyboard {
  /** Px between the frame's bottom edge and the keyboard's top edge. */
  readonly height: number;
  readonly visible: boolean;
  /** Last show/hide animation length in ms (0 on Android, reported after). */
  readonly duration: number;
}

const animationLength = (event: KeyboardEvent): number =>
  Platform.OS === "ios" && Number.isFinite(event.duration) && event.duration > 0
    ? Math.min(event.duration, 1000)
    : 0;

/**
 * Soft-keyboard height from platform events. iOS animates with `will` events;
 * Android only reports after the fact with `did` events. Reads
 * `Keyboard.metrics()` at mount because an already-open keyboard sends no event
 * to a late subscriber. The raw height is what is stored, so a safe-area change
 * during a rotation can't leave the covered height computed from the old inset.
 */
export function useSoftKeyboard(bottomInset = 0): SoftKeyboard {
  const [raw, setRaw] = useState(() => Keyboard.metrics()?.height ?? 0);
  const [visible, setVisible] = useState(() => (Keyboard.metrics()?.height ?? 0) > 0);
  const [duration, setDuration] = useState(250);

  useEffect(() => {
    const onShow = (event: KeyboardEvent) => {
      setRaw(event.endCoordinates.height);
      setVisible(true);
      setDuration(animationLength(event));
    };
    const onHide = (event: KeyboardEvent) => {
      setRaw(0);
      setVisible(false);
      setDuration(animationLength(event));
    };
    // iOS `will` tracks the keyboard animation; Android `did` is all there is.
    const show = Keyboard.addListener(
      Platform.OS === "ios" ? "keyboardWillShow" : "keyboardDidShow",
      onShow,
    );
    const hide = Keyboard.addListener(
      Platform.OS === "ios" ? "keyboardWillHide" : "keyboardDidHide",
      onHide,
    );
    return () => {
      show.remove();
      hide.remove();
    };
  }, []);

  return { height: coveredKeyboardHeight(raw, bottomInset, Platform.OS), visible, duration };
}
