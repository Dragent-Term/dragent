import type { ReactNode } from "react";
import { StyleSheet, View, type ViewProps } from "react-native";
import { useTheme } from "../theme";
import { componentPaddingScale, radii, spacing } from "../theme/tokens";

export interface SurfaceProps extends ViewProps {
  tone?: "surface" | "surfaceRaised";
  appearance?: "card" | "plain";
  header?: ReactNode;
  footer?: ReactNode;
}

/** Slots accept composed content without coupling the surface to screen data. */
export function Surface({ tone = "surface", appearance = "card", header, footer, children, style, ...props }: SurfaceProps) {
  const { theme } = useTheme();
  return (
    <View
      {...props}
      style={[
        styles.shell,
        { backgroundColor: appearance === "plain" ? undefined : theme.colors[tone], borderColor: theme.colors.border },
        appearance === "plain" ? styles.plain : null,
        style,
      ]}
    >
      {header}
      {children}
      {footer}
    </View>
  );
}

const styles = StyleSheet.create({
  plain: { borderWidth: 0, borderRadius: 0, padding: 0, gap: spacing.sm },
  shell: {
    borderWidth: 1,
    borderRadius: radii.lg,
    borderCurve: "continuous",
    padding: spacing.lg * componentPaddingScale,
    gap: spacing.md,
    minWidth: 0,
  },
});
