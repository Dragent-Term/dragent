import type { ReactNode } from "react";
import { StyleSheet, View } from "react-native";
import { AppText } from "./AppText";
import { IconButton } from "./IconButton";
import { useTheme } from "../theme";

export interface CompanionHeaderProps {
  title: string;
  subtitle?: string;
  onBack: () => void;
  backLabel: string;
  actions?: ReactNode;
  navigation?: ReactNode;
  status?: ReactNode;
}

/** Compact navigation shared by the desktop catalog, projects and open tabs. */
export function CompanionHeader({ title, subtitle, onBack, backLabel, actions, navigation, status }: CompanionHeaderProps) {
  const { theme } = useTheme();
  return (
    <View style={[styles.header, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
      <IconButton name="arrow-left" label={backLabel} onPress={onBack} />
      <View style={[styles.title, navigation ? styles.narrowTitle : null]}>
        <AppText variant="caption" style={styles.name} numberOfLines={1} accessibilityRole="header">{title}</AppText>
        {subtitle ? <AppText variant="micro" tone="textMuted" numberOfLines={1}>{subtitle}</AppText> : null}
      </View>
      {navigation ? <View style={styles.navigation}>{navigation}</View> : null}
      {status}
      {actions}
    </View>
  );
}

const styles = StyleSheet.create({
  header: { minHeight: 52, paddingHorizontal: 6, paddingVertical: 2, gap: 6, flexDirection: "row", alignItems: "center", borderBottomWidth: 1 },
  title: { flex: 1, minWidth: 0, gap: 2 },
  narrowTitle: { flexGrow: 0, maxWidth: 120 },
  navigation: { flex: 1, minWidth: 0 },
  name: { fontWeight: "600" },
});
