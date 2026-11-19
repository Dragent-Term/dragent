import { Text, StyleSheet, type TextProps } from "react-native";
import { useTheme } from "../theme";
import { type } from "../theme/tokens";

export type TextVariant = keyof typeof type;
export type TextTone =
  | "text"
  | "textMuted"
  | "textSubtle"
  | "accent"
  | "onAccent"
  | "success"
  | "warning"
  | "danger";

export interface AppTextProps extends TextProps {
  variant?: TextVariant;
  tone?: TextTone;
}

/** Shared typography; text wraps and follows the system font scale by default. */
export function AppText({ variant = "body", tone = "text", style, ...props }: AppTextProps) {
  const { theme } = useTheme();
  return (
    <Text
      allowFontScaling
      accessibilityRole={variant === "hero" || variant === "title" ? "header" : undefined}
      {...props}
      style={[styles.base, type[variant], { color: theme.colors[tone] }, style]}
    />
  );
}

const styles = StyleSheet.create({
  base: { flexShrink: 1 },
});
