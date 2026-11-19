import { ScrollView, StyleSheet, View } from "react-native";
import { AppText, Button, Icon, IconButton, StatusBadge, TabStrip } from "../../components";
import type { BrowserPage, PreviewBrowserState } from "../../preview/types";
import { useTheme } from "../../theme";
import { spacing } from "../../theme/tokens";

interface PreviewBrowserViewProps {
  state: PreviewBrowserState;
  projectName: string;
  workspaceName: string;
  onNavigate: (page: BrowserPage) => void;
  onBack: () => void;
  onForward: () => void;
  onReload: () => void;
}

const PAGES: readonly { id: BrowserPage; label: string }[] = [
  { id: "overview", label: "Overview" },
  { id: "activity", label: "Activity" },
  { id: "settings", label: "Settings" },
];

function PageRow({ title, detail, status }: { title: string; detail: string; status?: string }) {
  const { theme } = useTheme();
  return (
    <View style={[styles.pageRow, { borderColor: theme.colors.border }]}>
      <View style={styles.rowCopy}>
        <AppText style={styles.rowTitle}>{title}</AppText>
        <AppText variant="caption" tone="textMuted">{detail}</AppText>
      </View>
      {status ? <StatusBadge label={status} tone="neutral" /> : null}
    </View>
  );
}

/**
 * An original local webpage preview. The tab owns its history; this renderer
 * has no networking or desktop-control behavior. A future browser transport
 * can supply a real desktop surface without changing workspace navigation.
 */
export function PreviewBrowserView({
  state, projectName, workspaceName, onNavigate, onBack, onForward, onReload,
}: PreviewBrowserViewProps) {
  const { theme } = useTheme();
  const address = `https://preview.example/${state.page}`;
  return (
    <View testID="preview-browser-view" style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <View style={[styles.chrome, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
        <IconButton name="arrow-left" label="Browser back" disabled={state.index === 0} onPress={onBack} />
        <IconButton name="arrow-right" label="Browser forward" disabled={state.index >= state.history.length - 1} onPress={onForward} />
        <View accessible accessibilityRole="text" accessibilityLabel={`Browser address: ${address}`}
          testID="preview-browser-address" style={[styles.address, { backgroundColor: theme.colors.surfaceRaised, borderColor: theme.colors.border }]}>
          <Icon name="browser" size={14} />
          <AppText variant="caption" tone="textMuted" numberOfLines={1} selectable style={styles.addressText}>{address}</AppText>
        </View>
        <IconButton name="refresh" label="Reload page" accessibilityHint="Refreshes this sample page." onPress={onReload} />
      </View>
      <ScrollView key={`${state.page}:${state.revision}`} style={styles.frame}
        contentContainerStyle={styles.viewport} keyboardShouldPersistTaps="handled">
        <View style={styles.page}>
          <View style={styles.pageHeading}>
            <AppText variant="title" accessibilityRole="header">{projectName}</AppText>
            <AppText variant="caption" tone="textMuted">{workspaceName} · Sample webpage</AppText>
          </View>
          <TabStrip label="Preview page sections" options={PAGES} value={state.page} onChange={onNavigate} />
          {state.page === "overview" ? (
            <View style={styles.section}>
              <AppText variant="title" accessibilityRole="header">A clear view of the work</AppText>
              <AppText tone="textMuted">An example project page running inside this workspace’s browser tab.</AppText>
              <PageRow title="Navigation" detail="Keep the route from a project to its workspace easy to follow." status="Ready" />
              <PageRow title="Workspace context" detail="Give every workspace its own branch and open tabs." status="In review" />
              <PageRow title="Independent sessions" detail="Keep terminal, agent and browser state with the tab that owns it." status="Ready" />
              <View style={styles.actions}>
                <Button label="View activity" onPress={() => onNavigate("activity")} />
                <Button label="Project settings" variant="ghost" onPress={() => onNavigate("settings")} />
              </View>
            </View>
          ) : state.page === "activity" ? (
            <View style={styles.section}>
              <AppText variant="title" accessibilityRole="header">Project activity</AppText>
              <AppText tone="textMuted">Sample activity for this workspace.</AppText>
              <PageRow title="Review the workspace layout" detail="Check that the branch and open tabs are visible in context." />
              <PageRow title="Try a second tab" detail="Browser history, agent messages and shell output stay separate." />
              <PageRow title="Check the small screen" detail="Keep primary controls reachable while the content scrolls." />
              <Button label="Back to overview" variant="ghost" onPress={() => onNavigate("overview")} />
            </View>
          ) : (
            <View style={styles.section}>
              <AppText variant="title" accessibilityRole="header">Project settings</AppText>
              <PageRow title="Project" detail={projectName} />
              <PageRow title="Workspace" detail={workspaceName} />
              <PageRow title="Appearance" detail={`${theme.name} · follows the app theme`} />
              <PageRow title="Environment" detail="Local preview" />
              <Button label="View overview" variant="ghost" onPress={() => onNavigate("overview")} />
            </View>
          )}
        </View>
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  chrome: { flexDirection: "row", alignItems: "center", gap: 2, padding: 4, borderBottomWidth: 1 },
  address: { flex: 1, minWidth: 0, minHeight: 36, flexDirection: "row", alignItems: "center", gap: 6, borderWidth: 1, borderRadius: 6, paddingHorizontal: 8 },
  addressText: { flex: 1, minWidth: 0 },
  viewport: { flexGrow: 1, alignItems: "center", padding: spacing.lg },
  page: { width: "100%", maxWidth: 920, gap: spacing.lg },
  pageHeading: { gap: spacing.xs },
  section: { gap: spacing.md },
  pageRow: { flexDirection: "row", flexWrap: "wrap", alignItems: "center", gap: spacing.sm, borderBottomWidth: 1, paddingVertical: spacing.md },
  rowCopy: { flexGrow: 1, flexShrink: 1, flexBasis: 220, minWidth: 0, gap: spacing.xs },
  rowTitle: { fontWeight: "600" },
  actions: { flexDirection: "row", flexWrap: "wrap", gap: spacing.sm },
});
