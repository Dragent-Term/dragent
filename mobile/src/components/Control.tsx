import { useState, type ComponentRef, type Ref } from "react";
import { Pressable, StyleSheet, type PressableProps, type StyleProp, type ViewStyle } from "react-native";
import { useTheme } from "../theme";

interface ControlState {
  pressed: boolean;
  hovered: boolean;
  focused: boolean;
}

interface ControlProps extends Omit<PressableProps, "style"> {
  ref?: Ref<ComponentRef<typeof Pressable>>;
  style?: StyleProp<ViewStyle> | ((state: ControlState) => StyleProp<ViewStyle>);
}

/** Shared native press semantics and pointer/keyboard feedback for kit controls. */
export function Control({
  disabled, style, onHoverIn, onHoverOut, onFocus, onBlur, ...props
}: ControlProps) {
  const { theme } = useTheme();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);

  return (
    <Pressable
      {...props}
      aria-busy={props["aria-busy"] ?? props.accessibilityState?.busy}
      aria-checked={props["aria-checked"] ?? props.accessibilityState?.checked}
      aria-selected={props["aria-selected"] ?? props.accessibilityState?.selected}
      aria-expanded={props["aria-expanded"] ?? props.accessibilityState?.expanded}
      aria-disabled={props["aria-disabled"] ?? disabled}
      disabled={disabled}
      onHoverIn={(event) => { setHovered(true); onHoverIn?.(event); }}
      onHoverOut={(event) => { setHovered(false); onHoverOut?.(event); }}
      onFocus={(event) => { setFocused(true); onFocus?.(event); }}
      onBlur={(event) => { setFocused(false); onBlur?.(event); }}
      style={({ pressed }) => [
        typeof style === "function"
          ? style({ pressed, hovered: hovered && !disabled, focused: focused && !disabled })
          : style,
        focused && !disabled ? [styles.focused, { outlineColor: theme.colors.accent }] : null,
      ]}
    />
  );
}

const styles = StyleSheet.create({
  focused: { outlineWidth: 2, outlineOffset: 2, outlineStyle: "solid" },
});
