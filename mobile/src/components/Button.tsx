import type { ReactNode } from "react";
import {
  ActivityIndicator, StyleSheet,
  type PressableProps, type StyleProp, type ViewStyle,
} from "react-native";
import { useTheme } from "../theme";
import { componentPaddingScale, radii, spacing } from "../theme/tokens";
import { AppText, type TextTone } from "./AppText";
import { Control } from "./Control";
import { IconToneContext } from "./Icon";

export type ButtonVariant = "primary" | "ghost" | "danger";
export type ButtonSize = "md" | "sm";

export interface ButtonProps extends Omit<PressableProps, "children" | "style"> {
  label: string;
  variant?: ButtonVariant;
  size?: ButtonSize;
  loading?: boolean;
  leading?: ReactNode;
  trailing?: ReactNode;
  style?: StyleProp<ViewStyle>;
}

export function Button({
  label, variant = "primary", size = "md", disabled = false, loading = false,
  leading, trailing, accessibilityLabel = label, accessibilityState, style, ...props
}: ButtonProps) {
  const { theme } = useTheme();
  const unavailable = disabled || loading;
  const tone: TextTone = variant === "primary" ? "onAccent" : variant === "danger" ? "danger" : "text";

  return (
    <Control
      {...props}
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      accessibilityState={{ ...accessibilityState, disabled: unavailable, busy: loading }}
      disabled={unavailable}
      style={({ pressed, hovered }) => [
        styles.base,
        size === "sm" ? styles.small : styles.medium,
        {
          backgroundColor: variant === "primary" ? theme.colors.accent
            : hovered || pressed ? theme.colors.surfaceRaised : undefined,
        },
        variant === "primary" && hovered ? styles.hovered : null,
        pressed ? styles.pressed : null,
        unavailable ? styles.disabled : null,
        style,
      ]}
    >
      <IconToneContext.Provider value={tone}>
        {loading ? (
          <ActivityIndicator
            accessible={false}
            importantForAccessibility="no-hide-descendants"
            color={theme.colors[tone]}
            size="small"
          />
        ) : leading}
        <AppText variant={size === "sm" ? "caption" : "body"} tone={tone} style={styles.label}>
          {label}
        </AppText>
        {trailing}
      </IconToneContext.Provider>
    </Control>
  );
}

const styles = StyleSheet.create({
  base: {
    minWidth: 44,
    borderRadius: radii.md,
    borderCurve: "continuous",
    flexDirection: "row",
    alignItems: "center",
    justifyContent: "center",
    gap: spacing.sm,
  },
  medium: { minHeight: 44, paddingVertical: 10 * componentPaddingScale, paddingHorizontal: spacing.lg * componentPaddingScale },
  small: { minHeight: 32, paddingVertical: 4, paddingHorizontal: 10 },
  hovered: { opacity: 0.92 },
  pressed: { opacity: 0.8 },
  disabled: { opacity: 0.4 },
  label: { textAlign: "center", fontWeight: "500" },
});
