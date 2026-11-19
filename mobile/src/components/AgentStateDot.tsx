import { useEffect, useMemo, useRef } from "react";
import { Animated, Easing, StyleSheet, View } from "react-native";
import { statusMarkColors, useTheme } from "../theme";

export type AgentDotState =
  | "working"
  | "blocked"
  | "waiting"
  | "done"
  | "idle"
  | "interrupted"
  | "failed"
  /** A desktop that's connected (not an agent): steady, never spinning. */
  | "online";

/** The working spinner's colour on the default dark theme; screens use the theme's own. */
export const AGENT_WORKING_COLOR = "#eab308";

export function agentStateLabel(state: AgentDotState): string {
  switch (state) {
    case "working": return "Working";
    case "blocked": return "Blocked";
    case "waiting": return "Waiting for input";
    case "done": return "Done";
    case "interrupted": return "Interrupted";
    case "failed": return "Failed";
    case "idle": return "Idle";
    case "online": return "Online";
  }
}

/**
 * Small agent liveness mark, always visible: gray while idle, a glowing
 * amber spinner while working, green when finished but unopened (needs
 * review), red when blocked/waiting/failed, muted when the user stopped it;
 * each in the theme's own colour, sharpened to stand out on its background;
 * a connected desktop is a steady green dot.
 * Decorative — callers put the state words in the accessible label.
 */
export function AgentStateDot({ state }: { state: AgentDotState }) {
  const { theme } = useTheme();
  const marks = useMemo(() => statusMarkColors(theme.colors), [theme.colors]);
  const rotation = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    if (state !== "working") return;
    const spin = Animated.loop(
      Animated.timing(rotation, { toValue: 1, duration: 900, easing: Easing.linear, useNativeDriver: true }),
    );
    spin.start();
    return () => { spin.stop(); };
  }, [rotation, state]);
  const rotate = rotation.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] });

  if (state === "working") {
    return (
      <View accessible={false} aria-hidden accessibilityElementsHidden
        importantForAccessibility="no-hide-descendants"
        style={styles.wrapper}>
        {/* A soft halo, not a shadow: a shadow on a view with no background
            can't be drawn efficiently (and iOS warns about it). */}
        <View style={[styles.halo, { backgroundColor: marks.warning }]} />
        <Animated.View style={[styles.spinner, { borderColor: marks.warning, transform: [{ rotate }] }]} />
      </View>
    );
  }
  const color =
    state === "online" || state === "done" ? marks.success
    : state === "blocked" || state === "waiting" || state === "failed" ? marks.danger
    : state === "idle" ? "rgba(115,115,115,0.4)"
    : marks.neutral;
  return (
    <View accessible={false} aria-hidden accessibilityElementsHidden
      importantForAccessibility="no-hide-descendants" style={styles.wrapper}>
      <View style={[styles.dot, { backgroundColor: color }]} />
    </View>
  );
}

const styles = StyleSheet.create({
  wrapper: { width: 12, height: 12, alignItems: "center", justifyContent: "center" },
  halo: { position: "absolute", width: 12, height: 12, borderRadius: 6, opacity: 0.25 },
  dot: { width: 8, height: 8, borderRadius: 4 },
  spinner: {
    width: 10, height: 10, borderRadius: 5,
    borderWidth: 2, borderTopColor: "transparent",
  },
});
