import { useCallback, useEffect, useMemo, useState } from "react";
import { RefreshControl, ScrollView, StyleSheet, View } from "react-native";
import { router } from "expo-router";
import { AgentMark, AppText, Button, ConfirmationPanel, Control, EmptyState, Icon, LoadingState, SearchField } from "../../components";
import { getAgentLabel, normalizeAgentKind } from "../../components/agentArtwork";
import { useResponsiveLayout } from "../../layout";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import { useMachine } from "../../machines/MachineContext";
import { machinePath } from "../../machines/routes";
import { ageLabel, bytesLabel, groupByPeriod, matchesSearch, type PastSession } from "../../history/model";
import { continuePastSession, deletePastSession, listPastSessions } from "../../history/rpc";
import { MachineHeader } from "./MachineHeader";
import { Feedback } from "../shared/Feedback";

const EXTRA_NAMES: Record<string, string> = { mimo: "MiMo", copilot: "GitHub Copilot", "cursor-agent": "Cursor" };

export function agentName(provider: string): string {
  const kind = normalizeAgentKind(provider);
  return kind ? getAgentLabel(kind) : (EXTRA_NAMES[provider] ?? provider);
}

/**
 * A folder's earlier agent conversations on the paired desktop, newest
 * first under day captions: search them, continue one in a new desktop tab,
 * or delete one (after asking) to free its space.
 */
export function MachinePastSessionsScreen({ deviceId, folder, title }: { deviceId: string; folder: string; title: string }) {
  const { theme } = useTheme();
  const { gutter } = useResponsiveLayout();
  const machine = useMachine();
  const rpc = machine.rpc;
  const [sessions, setSessions] = useState<readonly PastSession[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [query, setQuery] = useState("");
  const [open, setOpen] = useState<PastSession | null>(null);
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState<"continue" | "delete" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!rpc || !folder) return;
    setLoading(true);
    try {
      setSessions(await listPastSessions(rpc, folder));
      setError(null);
    } catch {
      setError("The conversations couldn't be read. Check the desktop is online, then pull to refresh.");
    } finally {
      setLoading(false);
    }
  }, [rpc, folder]);
  useEffect(() => {
    void load();
  }, [load]);

  const groups = useMemo(() => {
    const shown = (sessions ?? []).filter((session) => matchesSearch(session, agentName(session.provider), query));
    return groupByPeriod(shown, new Date());
  }, [sessions, query]);

  const back = machinePath(deviceId);
  const leave = () => (router.canGoBack() ? router.back() : router.navigate({ pathname: back.pathname, params: back.params }));
  const close = () => {
    setOpen(null);
    setConfirming(false);
  };

  const resume = async (session: PastSession) => {
    if (!rpc) return;
    setBusy("continue");
    setError(null);
    try {
      const result = await continuePastSession(rpc, folder, session);
      if (result.started) {
        close();
        setNote(`“${session.title}” is continuing in a new tab on the desktop.`);
      } else setError(result.reason ?? "The desktop didn't continue it.");
    } catch {
      setError("The desktop didn't answer. Check it's online, then try again.");
    } finally {
      setBusy(null);
    }
  };

  const remove = async (session: PastSession) => {
    if (!rpc) return;
    setBusy("delete");
    setError(null);
    try {
      const result = await deletePastSession(rpc, folder, session);
      if (!result.deleted) {
        setError(result.reason);
        return;
      }
      setSessions((list) => (list ?? []).filter((entry) => !(entry.provider === session.provider && entry.id === session.id)));
      close();
      setNote(
        result.freed > 0
          ? `Deleted. ${bytesLabel(result.freed)} freed.`
          : result.reusable > 0
            ? `Deleted. ${bytesLabel(result.reusable)} is free inside ${agentName(session.provider)}'s database; Free up space on the desktop gives it back to your disk.`
            : "Deleted.",
      );
    } catch {
      setError("The desktop didn't answer. Check it's online, then try again.");
    } finally {
      setBusy(null);
    }
  };

  const now = Date.now();
  return (
    <View style={[styles.frame, { backgroundColor: theme.colors.background }]}>
      <MachineHeader title="Past sessions" subtitle={title} onBack={leave} backLabel="Back" online={machine.catalog.status === "live" ? machine.catalog.machine.online : null} />
      {sessions === null && loading ? (
        <LoadingState label="Reading earlier conversations on your desktop…" />
      ) : (
        <ScrollView
          contentContainerStyle={[styles.content, { paddingHorizontal: gutter }]}
          keyboardShouldPersistTaps="handled"
          refreshControl={<RefreshControl refreshing={loading} onRefresh={() => void load()} />}
        >
          <SearchField label="Search past sessions" value={query} onChangeText={setQuery} placeholder="Search title, agent or account…" />
          <Feedback message={error} />
          {note ? <AppText variant="caption" tone="textMuted">{note}</AppText> : null}
          {sessions !== null && groups.length === 0 ? (
            <EmptyState
              title={query ? "No matching sessions" : "No earlier conversations"}
              message={query ? "Try other words." : "Agents haven't been run in this folder yet, or their history isn't one the desktop reads."}
            />
          ) : null}
          {groups.map((group) => (
            <View key={group.caption} style={styles.group}>
              <AppText variant="micro" tone="textSubtle" style={styles.caption}>{group.caption}</AppText>
              {group.sessions.map((session) => {
                const selected = open?.provider === session.provider && open.id === session.id;
                const detail = [agentName(session.provider), ageLabel(session.updatedAt, now), session.accountLabel].filter(Boolean).join(" · ");
                return (
                  <View
                    key={`${session.provider}:${session.id}`}
                    style={[styles.card, { backgroundColor: theme.colors.surface, borderColor: selected ? theme.colors.accent : theme.colors.border }]}
                  >
                    <Control
                      accessibilityRole="button"
                      accessibilityLabel={`${session.title}, ${detail}`}
                      accessibilityState={{ expanded: selected }}
                      onPress={() => {
                        setOpen(selected ? null : session);
                        setConfirming(false);
                      }}
                      style={styles.row}
                    >
                      <AgentMark agent={session.provider} size={20} />
                      <View style={styles.grow}>
                        <AppText numberOfLines={2} style={styles.title}>{session.title}</AppText>
                        <AppText variant="caption" tone="textMuted" numberOfLines={1}>{detail}</AppText>
                      </View>
                      {session.size !== null ? <AppText variant="caption" tone="textSubtle">{bytesLabel(session.size)}</AppText> : null}
                      <View style={selected ? styles.flipped : undefined}><Icon name="chevron-down" size={14} /></View>
                    </Control>
                    {selected ? (
                      confirming ? (
                        <ConfirmationPanel
                          title={`Delete “${session.title}”?`}
                          message={`${session.size ? `Frees ${bytesLabel(session.size)}. ` : ""}It can't be undone or continued again.`}
                          actionLabel={busy === "delete" ? "Deleting…" : "Delete"}
                          onCancel={() => setConfirming(false)}
                          onConfirm={() => void remove(session)}
                        />
                      ) : (
                        <View style={styles.actions}>
                          <Button label="Delete" size="sm" variant="ghost" disabled={busy !== null} onPress={() => setConfirming(true)} />
                          <Button label="Continue on the desktop" size="sm" loading={busy === "continue"} disabled={busy !== null} onPress={() => void resume(session)} />
                        </View>
                      )
                    ) : null}
                  </View>
                );
              })}
            </View>
          ))}
        </ScrollView>
      )}
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  content: { gap: spacing.md, paddingVertical: spacing.md, paddingBottom: spacing.xl },
  group: { gap: spacing.xs },
  caption: { letterSpacing: 0.6, paddingHorizontal: 2 },
  card: { borderWidth: 1, borderRadius: radii.lg, borderCurve: "continuous", padding: spacing.sm, gap: spacing.sm },
  row: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  grow: { flex: 1, minWidth: 0, gap: 2 },
  title: { fontWeight: "600" },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
  flipped: { transform: [{ rotate: "180deg" }] },
});
