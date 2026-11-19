import { useEffect, type ComponentRef, type RefObject } from "react";
import { Platform, type View } from "react-native";
import { radioKeyIndex } from "./keyboard";

/** One keyboard policy for radio groups and editor-style tab strips. */
export function useChoiceKeyboard(
  ref: RefObject<ComponentRef<typeof View> | null>,
  role: "radio" | "tab",
  count: number,
  activate: (index: number) => void,
) {
  useEffect(() => {
    if (Platform.OS !== "web") return;
    const element = ref.current as unknown as HTMLElement | null;
    if (!element) return;
    const keyDown = (event: KeyboardEvent) => {
      if (event.altKey || event.ctrlKey || event.metaKey) return;
      const choices = Array.from(element.querySelectorAll<HTMLElement>(`[role="${role}"]`));
      const index = choices.findIndex((choice) => choice === event.target || choice.contains(event.target as Node));
      const next = radioKeyIndex(event.key, index, count);
      if (next === null) return;
      event.preventDefault();
      event.stopPropagation();
      activate(next);
      choices[next]?.focus();
    };
    element.addEventListener("keydown", keyDown, true);
    return () => element.removeEventListener("keydown", keyDown, true);
  }, [ref, role, count, activate]);
}
