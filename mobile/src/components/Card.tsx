import type { ReactNode } from "react";
import { StyleSheet, View, type StyleProp, type ViewStyle } from "react-native";
import { componentPaddingScale, radii, spacing } from "../theme/tokens";
import { AppText } from "./AppText";
import { Avatar } from "./Avatar";
import { Control } from "./Control";
import { StatusBadge, type StatusTone } from "./StatusBadge";
import { Surface } from "./Surface";

export interface CardProps {
  title: string;
  subtitle?: string;
  meta?: string;
  accentDot?: StatusTone;
  /** Username to show as a round initials avatar left of the title. */
  avatarName?: string;
  avatarUri?: string | null;
  onPress?: () => void;
  accessibilityLabel?: string;
  accessibilityHint?: string;
  children?: ReactNode;
  style?: StyleProp<ViewStyle>;
}

export function Card({
  title, subtitle, meta, accentDot, avatarName, avatarUri, onPress, accessibilityLabel, accessibilityHint, children, style,
}: CardProps) {
  const body = (
    <Surface style={[styles.surface, style]} header={
      <View style={styles.header}>
        {avatarName ? <Avatar name={avatarName} uri={avatarUri} size={28} /> : null}
        <View style={styles.titles}>
          <AppText style={styles.title}>{title}</AppText>
          {subtitle ? <AppText variant="caption" tone="textMuted">{subtitle}</AppText> : null}
        </View>
        {meta ? <StatusBadge label={meta} tone={accentDot ?? "neutral"} /> : null}
      </View>
    }>
      {children}
    </Surface>
  );

  return onPress ? (
    <Control
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel ?? [title, subtitle, meta].filter(Boolean).join(". ")}
      accessibilityHint={accessibilityHint}
      onPress={onPress}
      style={({ pressed, hovered }) => [
        styles.touchTarget,
        hovered ? styles.hovered : null,
        pressed ? styles.pressed : null,
      ]}
    >{body}</Control>
  ) : body;
}

const styles = StyleSheet.create({
  surface: { padding: spacing.md * componentPaddingScale },
  header: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.md },
  titles: { flexGrow: 1, flexShrink: 1, flexBasis: 180, minWidth: 0, gap: spacing.xs },
  title: { fontWeight: "500" },
  touchTarget: { minHeight: 48, minWidth: 48, borderRadius: radii.lg, borderCurve: "continuous" },
  hovered: { opacity: 0.94 },
  pressed: { opacity: 0.8 },
});
