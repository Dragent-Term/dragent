import { useCallback, useEffect, useRef, useState } from "react";
import { ActivityIndicator, Linking, RefreshControl, ScrollView, StyleSheet, View } from "react-native";
import { AppText, Button, Control, EmptyState, Icon, StatusBadge, type IconName } from "../../components";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import { countChecks, readiness, sortChecks, type CheckState, type PullRequestCheck, type PullRequestReading } from "../../pullRequests/model";
import { readPullRequestStatus } from "../../pullRequests/rpc";

const LABEL: Record<CheckState, string> = {
  failed: "Failed", running: "Running", pending: "Waiting", passed: "Passed", cancelled: "Cancelled", skipped: "Skipped", neutral: "Done",
};
const ICON: Record<CheckState, IconName> = {
  failed: "close", running: "refresh", pending: "clock", passed: "check", cancelled: "minus", skipped: "minus", neutral: "minus",
};
const TONE: Record<CheckState, "danger" | "warning" | "success" | "textSubtle"> = {
  failed: "danger", running: "warning", pending: "warning", passed: "success", cancelled: "textSubtle", skipped: "textSubtle", neutral: "textSubtle",
};

/**
 * The workspace branch's pull request on GitHub: where it stands (checks,
 * conflicts, review), and every check with its result, each opening its
 * page. Read through the desktop's GitHub CLI; pull down to check again.
 */
export function PullRequestView({ rpc, root, onCreate }: { rpc: RelayRpcClient; root: string; onCreate: () => void }) {
  const { theme } = useTheme();
  const [reading, setReading] = useState<PullRequestReading | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const request = useRef(0);

  const load = useCallback(
    (pulled: boolean) => {
      const id = ++request.current;
      if (pulled) setRefreshing(true);
      readPullRequestStatus(rpc, root)
        .then((next) => {
          if (request.current !== id) return;
          setReading(next);
          setError(null);
        })
        .catch(() => request.current === id && setError("The desktop couldn't read this branch's pull request. Pull down to try again."))
        .finally(() => request.current === id && setRefreshing(false));
    },
    [rpc, root],
  );
  useEffect(() => load(false), [load]);

  const open = (url: string | null) => {
    if (url) void Linking.openURL(url).catch(() => {});
  };

  const refresh = <RefreshControl refreshing={refreshing} onRefresh={() => load(true)} tintColor={theme.colors.textMuted} />;
  if (reading === null) {
    return error ? (
      <ScrollView refreshControl={refresh} contentContainerStyle={styles.pad}>
        <EmptyState title="Couldn't read the pull request" message={error} />
      </ScrollView>
    ) : (
      <ActivityIndicator style={styles.pad} color={theme.colors.accent} />
    );
  }
  if (reading.pr === null) {
    return (
      <ScrollView refreshControl={refresh} contentContainerStyle={styles.pad}>
        <EmptyState title="No pull request" message={reading.reason} actionLabel="Create pull request" onAction={onCreate} />
      </ScrollView>
    );
  }
  const pr = reading.pr;
  const standing = readiness(pr);
  const counts = countChecks(pr.checks);
  return (
    <ScrollView refreshControl={refresh} contentContainerStyle={styles.content}>
      <View style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
        <View style={styles.row}>
          <Icon name="pull-request" size={18} tone="accent" />
          <AppText variant="caption" tone="textMuted">{`#${pr.number} · ${pr.head} → ${pr.base}`}</AppText>
        </View>
        <AppText style={styles.title} selectable>{pr.title}</AppText>
        <View style={styles.badges}>
          <StatusBadge label={standing.label} tone={standing.tone} />
          {pr.draft ? <StatusBadge label="Draft" /> : null}
          {pr.review === "approved" ? <StatusBadge label="Approved" tone="success" /> : null}
        </View>
        {pr.url ? <Button label="Open on GitHub" size="sm" variant="ghost" leading={<Icon name="arrow-up-right" size={14} />} onPress={() => open(pr.url)} /> : null}
      </View>
      <View style={styles.headingRow}>
        <AppText variant="caption" tone="textMuted" style={styles.heading}>{`CHECKS · ${pr.checks.length}`}</AppText>
        <View style={styles.badges}>
          {(["failed", "running", "pending", "passed"] as const).map((state) =>
            counts[state] ? <StatusBadge key={state} label={`${counts[state]} ${LABEL[state].toLowerCase()}`} tone={state === "passed" ? "success" : state === "failed" ? "danger" : "warning"} /> : null,
          )}
        </View>
      </View>
      {pr.checks.length === 0 ? (
        <EmptyState title="No checks" message="No checks have reported on this pull request's latest commit." />
      ) : (
        sortChecks(pr.checks).map((check, index) => <CheckRow key={`${check.name}:${index}`} check={check} onOpen={() => open(check.url)} />)
      )}
    </ScrollView>
  );
}

function CheckRow({ check, onOpen }: { check: PullRequestCheck; onOpen: () => void }) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole={check.url ? "link" : "text"}
      accessibilityLabel={`${check.name}: ${LABEL[check.state]}`}
      disabled={!check.url}
      onPress={onOpen}
      style={({ pressed }) => [styles.check, { borderColor: theme.colors.border, backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.surface }]}
    >
      <Icon name={ICON[check.state]} size={16} tone={TONE[check.state]} />
      <View style={styles.grow}>
        <AppText numberOfLines={1}>{check.name}</AppText>
        {check.workflow ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{check.workflow}</AppText> : null}
      </View>
      <AppText variant="caption" tone={TONE[check.state]}>{LABEL[check.state]}</AppText>
      {check.url ? <Icon name="arrow-up-right" size={14} tone="textSubtle" /> : null}
    </Control>
  );
}

const styles = StyleSheet.create({
  pad: { padding: spacing.md },
  content: { padding: spacing.md, gap: spacing.sm, paddingBottom: spacing.xl },
  card: { borderWidth: 1, borderRadius: radii.lg, borderCurve: "continuous", padding: spacing.md, gap: spacing.sm },
  row: { flexDirection: "row", alignItems: "center", gap: spacing.xs },
  title: { fontSize: 17, lineHeight: 23, fontWeight: "600" },
  badges: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  headingRow: { gap: spacing.xs, paddingTop: spacing.sm },
  heading: { letterSpacing: 0.6 },
  check: { flexDirection: "row", alignItems: "center", gap: spacing.sm, borderWidth: 1, borderRadius: radii.md, padding: spacing.sm, minHeight: 48 },
  grow: { flex: 1, minWidth: 0 },
});
