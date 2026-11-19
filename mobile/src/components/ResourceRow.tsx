import type { ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { AgentStateDot, type AgentDotState } from "./AgentStateDot";
import { AppText } from "./AppText";
import { Control } from "./Control";
import { Icon, type IconName } from "./Icon";
import { useResponsiveLayout } from "../layout/useResponsiveLayout";
import { useTheme } from "../theme";
import { spacing } from "../theme/tokens";

export interface ResourceRowProps {
  icon: IconName;
  /** Custom glyph replacing the boxed icon (e.g. a per-agent mark). */
  iconNode?: ReactNode | undefined;
  /** Liveness overlay on the icon box; omitted when idle. */
  dot?: AgentDotState | null | undefined;
  title: string;
  detail: string;
  accessibilityLabel: string;
  onPress: () => void;
  /** Long-press: the row's actions (pin, delete…). */
  onLongPress?: (() => void) | undefined;
  /** Its accessibility hint, naming what long-press offers. */
  accessibilityHint?: string | undefined;
  trailing?: ReactNode;
  /** The divider runs past the page gutter, from one screen edge to the other. */
  fullBleed?: boolean | undefined;
}

/**
 * Exact product artwork renders bare (dot, then the brand mark);
 * catalog and navigation rows keep the boxed generic glyph with a liveness
 * overlay pinned to its corner.
 */
function RowIcon({ icon, iconNode, dot }: { icon: IconName; iconNode?: ReactNode | undefined; dot?: AgentDotState | null | undefined }) {
  const { theme } = useTheme();
  if (iconNode) {
    return (
      <View style={styles.bareRow}>
        {dot ? <AgentStateDot state={dot} /> : null}
        {iconNode}
      </View>
    );
  }
  return (
    <View style={styles.iconWrap}>
      <View style={[styles.icon, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
        <Icon name={icon} size={18} tone="text" />
      </View>
      {dot ? (
        <View style={[styles.dotBadge, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
          <AgentStateDot state={dot} />
        </View>
      ) : null}
    </View>
  );
}

/** One quiet catalog row reused for desktops, projects, workspaces, and tabs. */
export function ResourceRow({ icon, iconNode, dot, title, detail, accessibilityLabel, onPress, onLongPress, accessibilityHint, trailing, fullBleed }: ResourceRowProps) {
  const { theme } = useTheme();
  const { gutter } = useResponsiveLayout();
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={accessibilityLabel}
      accessibilityHint={accessibilityHint}
      accessibilityActions={onLongPress ? [{ name: "longpress", label: "More actions" }] : undefined}
      onAccessibilityAction={onLongPress ? (event) => { if (event.nativeEvent.actionName === "longpress") onLongPress(); } : undefined}
      onPress={onPress}
      onLongPress={onLongPress}
      style={({ pressed, hovered }) => [
        styles.row,
        {
          borderColor: theme.colors.border,
          backgroundColor: pressed || hovered ? theme.colors.surfaceRaised : theme.colors.background,
        },
        fullBleed ? { marginHorizontal: -gutter, paddingHorizontal: gutter + spacing.xs } : null,
      ]}
    >
      <RowIcon icon={icon} iconNode={iconNode} dot={dot} />
      <View style={styles.copy}>
        <AppText style={styles.name} numberOfLines={2}>{title}</AppText>
        <AppText variant="caption" tone="textMuted" numberOfLines={1}>{detail}</AppText>
      </View>
      {/* Whatever trails the row sits just before the chevron, centred with it. */}
      <View style={styles.trailing}>
        {trailing ? <View style={styles.trailingItem}>{trailing}</View> : null}
        <Icon name="chevron-right" size={16} />
      </View>
    </Control>
  );
}

export function countLabel(count: number, one: string, many: string): string {
  return `${count} ${count === 1 ? one : many}`;
}

const styles = StyleSheet.create({
  row: {
    minHeight: 64,
    borderBottomWidth: 1,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.md,
    paddingVertical: spacing.sm,
    paddingHorizontal: spacing.xs,
  },
  bareRow: { flexDirection: "row", alignItems: "center", gap: 6, flexShrink: 0, minWidth: 0 },
  iconWrap: { position: "relative", flexShrink: 0 },
  icon: {
    width: 36,
    height: 36,
    borderRadius: 10,
    borderWidth: 1,
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
  },
  dotBadge: {
    position: "absolute",
    right: -3,
    bottom: -3,
    borderRadius: 999,
    borderWidth: 1,
    padding: 2,
  },
  copy: { flex: 1, minWidth: 0, gap: 2 },
  trailing: { flexDirection: "row", alignItems: "center", alignSelf: "center", gap: spacing.xs, flexShrink: 0 },
  trailingItem: { justifyContent: "center" },
  name: { fontWeight: "600" },
});
