import { useEffect, useRef, useState } from "react";
import { Animated, Easing, StyleSheet } from "react-native";
import { useTheme } from "../theme";
import { radii } from "../theme/tokens";
import { Control } from "./Control";
import { Icon } from "./Icon";

/** One turn of the icon: a tap always gets at least this, so it reads as done. */
const TURN_MS = 700;

export interface RefreshButtonProps {
  label: string;
  onPress: () => void;
  /** Still reloading: keep turning until it's done. */
  refreshing?: boolean;
  disabled?: boolean;
}

/** The refresh icon as a button; it turns when tapped and while reloading. */
export function RefreshButton({ label, onPress, refreshing = false, disabled = false }: RefreshButtonProps) {
  const { theme } = useTheme();
  const rotation = useRef(new Animated.Value(0)).current;
  const [tapped, setTapped] = useState(false);
  const spinning = tapped || refreshing;

  useEffect(() => {
    if (!tapped) return;
    const done = setTimeout(() => setTapped(false), TURN_MS);
    return () => clearTimeout(done);
  }, [tapped]);

  useEffect(() => {
    if (!spinning) return;
    rotation.setValue(0);
    const spin = Animated.loop(
      Animated.timing(rotation, { toValue: 1, duration: TURN_MS, easing: Easing.linear, useNativeDriver: true }),
    );
    spin.start();
    return () => {
      spin.stop();
      rotation.setValue(0);
    };
  }, [rotation, spinning]);

  const rotate = rotation.interpolate({ inputRange: [0, 1], outputRange: ["0deg", "360deg"] });
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={label}
      accessibilityState={{ disabled, busy: spinning }}
      disabled={disabled}
      onPress={() => {
        setTapped(true);
        onPress();
      }}
      style={({ hovered, pressed }) => [
        styles.shell,
        { backgroundColor: hovered || pressed ? theme.colors.surfaceRaised : undefined },
        disabled ? styles.disabled : null,
      ]}
    >
      <Animated.View style={{ transform: [{ rotate }] }}>
        <Icon name="refresh" size={16} tone="textMuted" />
      </Animated.View>
    </Control>
  );
}

const styles = StyleSheet.create({
  shell: {
    minHeight: 36,
    minWidth: 36,
    borderRadius: radii.md,
    borderCurve: "continuous",
    alignItems: "center",
    justifyContent: "center",
  },
  disabled: { opacity: 0.4 },
});
