import { StyleSheet, View } from "react-native";
import { AppText } from "./AppText";
import { Button } from "./Button";
import { useTheme } from "../theme";

/** Inline confirmation for actions in a page, including embedded browser previews. */
export function ConfirmationPanel({ title, message, actionLabel, cancelLabel = "Cancel", onConfirm, onCancel }: {
  title: string;
  message: string;
  actionLabel: string;
  cancelLabel?: string;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { theme } = useTheme();
  return <View style={[styles.panel, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}
    accessibilityRole="alert" accessibilityLiveRegion="assertive">
    <AppText style={styles.title}>{title}</AppText>
    <AppText variant="caption" tone="textMuted">{message}</AppText>
    <View style={styles.actions}>
      <Button label={cancelLabel} size="sm" variant="ghost" onPress={onCancel} />
      <Button label={actionLabel} size="sm" variant="danger" onPress={onConfirm} />
    </View>
  </View>;
}
const styles = StyleSheet.create({
  panel: { padding: 12, gap: 8, borderBottomWidth: 1 }, title: { fontWeight: "600" },
  actions: { flexDirection: "row", flexWrap: "wrap", justifyContent: "flex-end", gap: 8 },
});
