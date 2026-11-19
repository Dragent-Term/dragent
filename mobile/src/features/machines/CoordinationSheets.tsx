/**
 * The new coordination's two choosers, as bottom sheets so the screen itself
 * stays a message box and a few rows: which agents may work besides the
 * lead, and which issues from the desktop's trackers the goal links.
 */
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ActivityIndicator, Modal, Pressable, ScrollView, StyleSheet, View, useWindowDimensions } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AgentMark, AppText, Button, Control, Icon } from "../../components";
import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { FieldInput } from "../../components/FieldInput";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import { listTrackerItems, listTrackers } from "../../tasks/rpc";
import type { StartAgent } from "../../tasks/types";
import { issueRef, type LinkedIssue } from "../../coordination/goal";

function Sheet({ visible, title, subtitle, onClose, children, footer }: {
  visible: boolean;
  title: string;
  subtitle?: string | undefined;
  onClose: () => void;
  children: ReactNode;
  footer?: ReactNode;
}) {
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const { height } = useWindowDimensions();
  return (
    <Modal visible={visible} animationType="slide" transparent onRequestClose={onClose}>
      <ThemedStatusBar />
      <View style={styles.scrim}>
        <Pressable accessibilityRole="button" accessibilityLabel="Close" style={StyleSheet.absoluteFill} onPress={onClose} />
        <View
          style={[
            styles.sheet,
            {
              maxHeight: Math.round(height * 0.8),
              backgroundColor: theme.colors.surface,
              borderColor: theme.colors.border,
              paddingBottom: Math.max(insets.bottom, spacing.sm),
            },
          ]}
        >
          <View style={[styles.grabber, { backgroundColor: theme.colors.border }]} />
          <View style={styles.head}>
            <View style={styles.flex}>
              <AppText style={styles.title}>{title}</AppText>
              {subtitle ? <AppText variant="caption" tone="textMuted">{subtitle}</AppText> : null}
            </View>
            <Button label="Done" size="sm" variant="ghost" onPress={onClose} />
          </View>
          {children}
          {footer}
        </View>
      </View>
    </Modal>
  );
}

function CheckRow({ on, label, detail, leading, onPress }: {
  on: boolean;
  label: string;
  detail?: string | null | undefined;
  leading: ReactNode;
  onPress: () => void;
}) {
  const { theme } = useTheme();
  return (
    <Control
      accessibilityRole="checkbox"
      accessibilityState={{ checked: on }}
      accessibilityLabel={detail ? `${label}, ${detail}` : label}
      onPress={onPress}
      style={({ pressed }) => [styles.row, pressed ? { backgroundColor: theme.colors.surfaceRaised } : null]}
    >
      <View style={styles.mark}>{leading}</View>
      <View style={styles.flex}>
        <AppText numberOfLines={1}>{label}</AppText>
        {detail ? <AppText variant="caption" tone="textMuted" numberOfLines={1}>{detail}</AppText> : null}
      </View>
      <View style={[styles.box, { borderColor: on ? theme.colors.accent : theme.colors.border, backgroundColor: on ? theme.colors.accent : undefined }]}>
        {on ? <Icon name="check" size={13} tone="onAccent" /> : null}
      </View>
    </Control>
  );
}

/** Agents that may take tasks besides the lead's own kind. */
export function WorkersSheet({ visible, agents, lead, selected, onChange, onClose }: {
  visible: boolean;
  agents: readonly StartAgent[];
  lead: string | null;
  selected: readonly string[];
  onChange: (next: string[]) => void;
  onClose: () => void;
}) {
  const leadLabel = agents.find((agent) => agent.key === lead)?.label;
  return (
    <Sheet
      visible={visible}
      title="Workers"
      subtitle={leadLabel ? `${leadLabel} works too, as the lead's own kind. Add others to share the tasks.` : "Agents that take the lead's tasks."}
      onClose={onClose}
    >
      <ScrollView keyboardShouldPersistTaps="handled">
        {agents.map((agent) => {
          const on = selected.includes(agent.key);
          return (
            <CheckRow
              key={agent.key}
              on={on}
              label={agent.label}
              leading={<AgentMark agent={agent.key} size={20} />}
              onPress={() => onChange(on ? selected.filter((key) => key !== agent.key) : [...selected, agent.key])}
            />
          );
        })}
      </ScrollView>
    </Sheet>
  );
}

type Loaded =
  | { state: "loading" }
  | { state: "failed"; message: string }
  | { state: "ready"; trackers: number; issues: readonly LinkedIssue[] };

/**
 * Open issues from every tracker connected on the desktop, asked from the
 * chosen project, to link to the goal. Searching filters by key and title.
 */
export function IssuesSheet({ visible, rpc, project, linked, onChange, onClose }: {
  visible: boolean;
  rpc: RelayRpcClient | null;
  project: string | null;
  linked: readonly LinkedIssue[];
  onChange: (next: LinkedIssue[]) => void;
  onClose: () => void;
}) {
  const { theme } = useTheme();
  const [loaded, setLoaded] = useState<Loaded>({ state: "loading" });
  const [query, setQuery] = useState("");
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    if (!visible) return;
    if (!rpc) {
      setLoaded({ state: "failed", message: "Your desktop isn't connected." });
      return;
    }
    let active = true;
    setLoaded({ state: "loading" });
    (async () => {
      const trackers = await listTrackers(rpc);
      const lists = await Promise.allSettled(trackers.map((tracker) => listTrackerItems(rpc, tracker.key, project)));
      const issues: LinkedIssue[] = [];
      lists.forEach((result, index) => {
        if (result.status !== "fulfilled") return;
        const tracker = trackers[index]!;
        for (const item of result.value) {
          issues.push({ ref: issueRef(tracker.key, item.id), provider: tracker.key, trackerLabel: tracker.label, key: item.key, title: item.title, url: item.url });
        }
      });
      if (trackers.length > 0 && lists.every((result) => result.status === "rejected")) {
        throw new Error("Couldn't load issues from the desktop's trackers.");
      }
      return { trackers: trackers.length, issues };
    })().then(
      (result) => { if (active) setLoaded({ state: "ready", ...result }); },
      (error: unknown) => {
        if (active) setLoaded({ state: "failed", message: error instanceof Error && error.message ? error.message : "Couldn't load issues." });
      },
    );
    return () => { active = false; };
  }, [visible, rpc, project, attempt]);

  const shown = useMemo(() => {
    if (loaded.state !== "ready") return [];
    const search = query.trim().toLowerCase();
    // Linked ones first, so they're easy to unlink.
    const chosen = new Set(linked.map((issue) => issue.ref));
    const matches = search
      ? loaded.issues.filter((issue) => `${issue.key} ${issue.title}`.toLowerCase().includes(search))
      : loaded.issues;
    return [...matches.filter((issue) => chosen.has(issue.ref)), ...matches.filter((issue) => !chosen.has(issue.ref))];
  }, [loaded, query, linked]);

  const toggle = (issue: LinkedIssue) => {
    const on = linked.some((entry) => entry.ref === issue.ref);
    onChange(on ? linked.filter((entry) => entry.ref !== issue.ref) : [...linked, issue]);
  };

  return (
    <Sheet visible={visible} title="Link issues" subtitle="The lead gets each one's key, title and link." onClose={onClose}>
      {loaded.state === "ready" && loaded.issues.length > 0 ? (
        <View style={styles.search}>
          <FieldInput
            leading={<Icon name="search" size={15} tone="textMuted" />}
            value={query}
            onChangeText={setQuery}
            placeholder="Find an issue"
            accessibilityLabel="Find an issue"
            autoCorrect={false}
            autoCapitalize="none"
          />
        </View>
      ) : null}
      {loaded.state === "loading" ? (
        <View style={styles.status}><ActivityIndicator color={theme.colors.textMuted} /></View>
      ) : loaded.state === "failed" ? (
        <View style={styles.status}>
          <AppText tone="textMuted">{loaded.message}</AppText>
          <Button label="Try again" size="sm" variant="ghost" onPress={() => setAttempt((n) => n + 1)} />
        </View>
      ) : loaded.trackers === 0 ? (
        <View style={styles.status}>
          <AppText tone="textMuted">No issue tracker is connected on the desktop. Connect Linear, GitHub or another tracker there first.</AppText>
        </View>
      ) : shown.length === 0 ? (
        <View style={styles.status}>
          <AppText tone="textMuted">{query.trim() ? `No issue matches “${query.trim()}”.` : "No open issues."}</AppText>
        </View>
      ) : (
        <ScrollView keyboardShouldPersistTaps="handled">
          {shown.map((issue) => (
            <CheckRow
              key={issue.ref}
              on={linked.some((entry) => entry.ref === issue.ref)}
              label={issue.title}
              detail={[issue.key, issue.trackerLabel].filter(Boolean).join(" · ")}
              leading={<Icon name="link" size={16} tone="textMuted" />}
              onPress={() => toggle(issue)}
            />
          ))}
        </ScrollView>
      )}
    </Sheet>
  );
}

/** Linked issues, inside the message box above what you type. */
export function LinkedIssueChips({ issues, onRemove }: { issues: readonly LinkedIssue[]; onRemove: (ref: string) => void }) {
  const { theme } = useTheme();
  if (issues.length === 0) return null;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.chips} keyboardShouldPersistTaps="handled">
      {issues.map((issue) => (
        <View key={issue.ref} style={[styles.chip, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
          <Icon name="link" size={13} tone="textMuted" />
          <AppText variant="caption" numberOfLines={1} style={styles.chipText}>
            {issue.key ? `${issue.key} ${issue.title}` : issue.title}
          </AppText>
          <Control
            accessibilityRole="button"
            accessibilityLabel={`Unlink ${issue.key || issue.title}`}
            onPress={() => onRemove(issue.ref)}
            hitSlop={8}
            style={[styles.remove, { backgroundColor: theme.colors.surfaceRaised }]}
          >
            <Icon name="close" size={11} tone="textMuted" />
          </Control>
        </View>
      ))}
    </ScrollView>
  );
}

const styles = StyleSheet.create({
  scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: {
    borderTopWidth: StyleSheet.hairlineWidth,
    borderTopLeftRadius: radii.xl,
    borderTopRightRadius: radii.xl,
    paddingTop: spacing.sm,
  },
  grabber: { alignSelf: "center", width: 36, height: 4, borderRadius: 2, marginBottom: spacing.xs },
  head: { flexDirection: "row", alignItems: "center", gap: spacing.md, paddingLeft: spacing.lg, paddingRight: spacing.sm, paddingVertical: spacing.xs },
  title: { fontWeight: "600" },
  flex: { flex: 1, minWidth: 0 },
  row: { minHeight: 52, flexDirection: "row", alignItems: "center", gap: spacing.md, paddingHorizontal: spacing.lg, paddingVertical: 6 },
  mark: { width: 24, alignItems: "center" },
  box: { width: 22, height: 22, borderRadius: 6, borderWidth: 1.5, alignItems: "center", justifyContent: "center" },
  search: { paddingHorizontal: spacing.lg, paddingBottom: spacing.sm },
  status: { padding: spacing.lg, gap: spacing.sm, alignItems: "flex-start" },
  chips: { gap: 6, paddingHorizontal: 4, paddingTop: 4, paddingBottom: 2 },
  chip: { flexDirection: "row", alignItems: "center", gap: 6, borderWidth: StyleSheet.hairlineWidth, borderRadius: 999, paddingLeft: 10, paddingRight: 4, height: 30, maxWidth: 240 },
  chipText: { flexShrink: 1 },
  remove: { width: 20, height: 20, borderRadius: 10, alignItems: "center", justifyContent: "center" },
});
