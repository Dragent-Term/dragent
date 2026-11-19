import { useContext } from "react";
import { useWindowDimensions } from "react-native";
import { LayoutViewportContext } from "./LayoutViewportProvider";
import { resolveLayout, type ResponsiveLayout } from "./resolveLayout";

/** Nearest pane width takes priority; system font scale always remains live. */
export function useResponsiveLayout(): ResponsiveLayout {
  const { width, fontScale } = useWindowDimensions();
  const availableWidth = useContext(LayoutViewportContext);
  return resolveLayout(availableWidth ?? width, fontScale);
}
