import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ActivityIndicator, FlatList, Platform, Pressable, RefreshControl, ScrollView, SectionList, StyleSheet, Text, View } from "react-native";
import { AppText, Control, EmptyState, Icon } from "../../components";
import { statusMarkColors, useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import {
  readGitDiff,
  readGitStatus,
  type GitChange,
  type GitChangeStatus,
  type GitFileDiff,
  type GitStatus,
} from "../../transport/relayRpcMethods";
import { baseName, parentPath } from "./paths";
import { parseUnifiedDiff, type DiffRow } from "./unifiedDiff";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });
const CHAR_WIDTH = 12 * 0.602;

export type ChangeSide = "staged" | "unstaged";

export interface OpenChange {
  readonly change: GitChange;
  readonly side: ChangeSide;
}

const LETTER: Record<GitChangeStatus, string> = {
  modified: "M", added: "A", deleted: "D", renamed: "R", copied: "C", type_changed: "T", untracked: "U", conflicted: "!",
};
const WORD: Record<GitChangeStatus, string> = {
  modified: "Modified", added: "Added", deleted: "Deleted", renamed: "Renamed", copied: "Copied",
  type_changed: "Type changed", untracked: "New file", conflicted: "Conflict",
};

function messageOf(failure: unknown, fallback: string): string {
  const text = (failure as { message?: unknown } | null)?.message;
  return typeof text === "string" && text !== "" ? text : fallback;
}

function useStatusColor(): (status: GitChangeStatus) => string {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  return (status) =>
    status === "added" || status === "untracked" ? marks.success
    : status === "deleted" || status === "conflicted" ? marks.danger
    : marks.warning;
}

const ChangeRow = memo(function ChangeRow({ change, onOpen }: { change: GitChange; onOpen: () => void }) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const color = useStatusColor()(change.status);
  const folder = parentPath(`/${change.path}`).slice(1);
  return (
    <Control
      accessibilityRole="button"
      accessibilityLabel={`${WORD[change.status]}: ${change.path}`}
      onPress={onOpen}
      style={({ pressed, hovered }) => [
        styles.row,
        { borderColor: theme.colors.border },
        pressed || hovered ? { backgroundColor: theme.colors.surfaceRaised } : null,
      ]}
    >
      <Text style={[styles.letter, { color }]}>{LETTER[change.status]}</Text>
      <View style={styles.flex}>
        <AppText numberOfLines={1} style={change.status === "deleted" ? styles.struck : null}>{baseName(change.path)}</AppText>
        {folder ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{folder}</AppText> : null}
      </View>
      {change.added !== null && change.added > 0 ? <AppText variant="caption" style={{ color: marks.success }}>+{change.added}</AppText> : null}
      {change.removed !== null && change.removed > 0 ? <AppText variant="caption" style={{ color: marks.danger }}>−{change.removed}</AppText> : null}
      <Icon name="chevron-right" size={14} tone="textSubtle" />
    </Control>
  );
});

/**
 * The workspace's uncommitted changes, as the desktop's right sidebar lists
 * them: staged, then not staged, each file with its lines added and removed.
 * Pull down to refresh. Read-only: nothing is staged or committed from here.
 */
export function ChangesPanel({ rpc, root, onOpen }: {
  rpc: RelayRpcClient;
  root: string;
  onOpen: (open: OpenChange) => void;
}) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const request = useRef(0);

  const load = useCallback((pulled: boolean) => {
    const id = ++request.current;
    if (pulled) setRefreshing(true);
    setError(null);
    readGitStatus(rpc, root)
      .then((next) => {
        if (request.current === id) setStatus(next);
      })
      .catch((failure: unknown) => {
        if (request.current === id) setError(messageOf(failure, "The desktop couldn't read this workspace's changes."));
      })
      .finally(() => {
        if (request.current === id) setRefreshing(false);
      });
  }, [rpc, root]);

  useEffect(() => {
    setStatus(null);
    load(false);
  }, [load]);

  const sections = useMemo(() => {
    if (!status) return [];
    return [
      { key: "staged" as const, title: "Staged", data: status.staged },
      { key: "unstaged" as const, title: "Changes", data: status.unstaged },
    ].filter((section) => section.data.length > 0);
  }, [status]);

  if (error && !status) {
    return <EmptyState title="Can't show changes" message={error} actionLabel="Try again" onAction={() => load(false)} />;
  }
  if (!status) {
    return <View style={styles.center}><ActivityIndicator color={theme.colors.accent} /></View>;
  }
  if (!status.repository) {
    return <EmptyState title="Not a git repository" message="This workspace's folder isn't under git, so there are no changes to show." />;
  }

  const total = status.staged.length + status.unstaged.length;
  const added = [...status.staged, ...status.unstaged].reduce((sum, change) => sum + (change.added ?? 0), 0);
  const removed = [...status.staged, ...status.unstaged].reduce((sum, change) => sum + (change.removed ?? 0), 0);

  return (
    <SectionList
      sections={sections}
      keyExtractor={(change, index) => `${index}:${change.path}`}
      stickySectionHeadersEnabled={false}
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => load(true)} tintColor={theme.colors.textMuted} />}
      contentContainerStyle={total === 0 ? styles.grow : styles.listContent}
      ListHeaderComponent={
        <View style={[styles.summary, { borderColor: theme.colors.border }]}>
          <Icon name="branch" size={15} tone="textMuted" />
          <AppText numberOfLines={1} style={[styles.flex, styles.strong]}>{status.branch ?? "Detached HEAD"}</AppText>
          {status.ahead > 0 ? <AppText variant="caption" tone="textMuted">↑{status.ahead}</AppText> : null}
          {status.behind > 0 ? <AppText variant="caption" tone="textMuted">↓{status.behind}</AppText> : null}
          {total > 0 ? (
            <>
              <AppText variant="caption" tone="textMuted">{total} file{total === 1 ? "" : "s"}</AppText>
              <AppText variant="caption" style={{ color: marks.success }}>+{added}</AppText>
              <AppText variant="caption" style={{ color: marks.danger }}>−{removed}</AppText>
            </>
          ) : null}
        </View>
      }
      renderSectionHeader={({ section }) => (
        <View style={[styles.sectionHead, { backgroundColor: theme.colors.background }]}>
          <AppText variant="caption" tone="textMuted" style={styles.strong}>
            {section.title} · {section.data.length}
          </AppText>
        </View>
      )}
      renderItem={({ item, section }) => (
        <ChangeRow change={item} onOpen={() => onOpen({ change: item, side: section.key })} />
      )}
      ListEmptyComponent={
        <EmptyState title="No changes" message="This workspace matches its last commit. Pull down to check again." />
      }
      ListFooterComponent={
        <View style={styles.footer}>
          {error ? <AppText variant="caption" tone="danger">{error}</AppText> : null}
          {status.truncated ? <AppText variant="caption" tone="textSubtle">Only the first changes are listed. The rest are on your desktop.</AppText> : null}
        </View>
      }
    />
  );
}

const DiffLine = memo(function DiffLine({ row, gutter, commented, onComment }: {
  row: DiffRow;
  gutter: number;
  commented: boolean;
  onComment: ((row: DiffRow) => void) | null;
}) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  if (row.kind === "hunk") {
    return (
      <View style={[styles.hunk, { backgroundColor: theme.colors.surfaceRaised }]}>
        <Text style={[styles.code, { color: theme.colors.textMuted }]}>{row.text}</Text>
      </View>
    );
  }
  const color = row.kind === "add" ? marks.success : row.kind === "del" ? marks.danger : row.kind === "note" ? theme.colors.textSubtle : theme.colors.text;
  const tint = row.kind === "add" ? `${marks.success}22` : row.kind === "del" ? `${marks.danger}22` : undefined;
  const line = (
    <View style={[styles.diffLine, tint ? { backgroundColor: tint } : null, commented ? { borderLeftWidth: 3, borderLeftColor: theme.colors.accent } : null]}>
      <Text style={[styles.code, styles.number, { width: gutter, color: theme.colors.textSubtle }]}>{row.oldLine ?? ""}</Text>
      <Text style={[styles.code, styles.number, { width: gutter, color: theme.colors.textSubtle }]}>{row.newLine ?? ""}</Text>
      <Text style={[styles.code, styles.sign, { color }]}>{row.kind === "add" ? "+" : row.kind === "del" ? "−" : " "}</Text>
      <Text style={[styles.code, { color }]}>{row.text.replace(/\t/gu, "  ")}</Text>
    </View>
  );
  if (!onComment || row.kind === "note") return line;
  // Tap a line to comment on it for the review.
  return (
    <Pressable
      onPress={() => onComment(row)}
      accessibilityRole="button"
      accessibilityLabel={`Line ${row.newLine ?? row.oldLine ?? ""}${commented ? ", commented" : ""}. Comment on this line.`}
      style={({ pressed }) => (pressed ? { opacity: 0.6 } : null)}
    >
      {line}
    </Pressable>
  );
});

/** One changed file's diff: old and new line numbers, additions green, removals red. */
export function DiffViewer({ rpc, root, open, onComment, commentedLines }: {
  rpc: RelayRpcClient;
  root: string;
  open: OpenChange;
  /** Tap a line to comment on it (review); omitted: lines aren't tappable. */
  onComment?: ((row: DiffRow) => void) | undefined;
  /** Lines already commented on ("new:12", "old:4"). */
  commentedLines?: ReadonlySet<string> | undefined;
}) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const [diff, setDiff] = useState<GitFileDiff | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const { change, side } = open;

  useEffect(() => {
    let active = true;
    setDiff(null);
    setError(null);
    readGitDiff(rpc, root, change.path, change.status === "untracked" ? "untracked" : side)
      .then((next) => { if (active) setDiff(next); })
      .catch((failure: unknown) => { if (active) setError(messageOf(failure, "The desktop couldn't produce this diff.")); });
    return () => { active = false; };
  }, [rpc, root, change.path, change.status, side, attempt]);

  const parsed = useMemo(() => (diff ? parseUnifiedDiff(diff.diff) : null), [diff]);
  const widest = useMemo(() => (parsed ? parsed.rows.reduce((max, row) => Math.max(max, row.text.length), 0) : 0), [parsed]);

  if (error) {
    return <EmptyState title="Can't show this diff" message={error} actionLabel="Try again" onAction={() => setAttempt((n) => n + 1)} />;
  }
  if (!diff || !parsed) {
    return <View style={styles.center}><ActivityIndicator color={theme.colors.accent} /></View>;
  }
  if (diff.binary) {
    return <EmptyState title="Binary file" message="This file isn't text, so there are no lines to compare." />;
  }
  if (parsed.rows.length === 0) {
    return <EmptyState title="No line changes" message={change.status === "renamed" ? "The file was renamed without changing its contents." : "Nothing in this file's text changed."} />;
  }
  const lastLine = parsed.rows.reduce((max, row) => Math.max(max, row.oldLine ?? 0, row.newLine ?? 0), 0);
  const gutter = Math.max(3, String(lastLine).length) * CHAR_WIDTH + 8;
  const width = gutter * 2 + 16 + widest * CHAR_WIDTH + spacing.lg * 2;
  return (
    <View style={styles.flex}>
      <View style={[styles.summary, { borderColor: theme.colors.border }]}>
        <AppText variant="caption" tone="textMuted" style={styles.flex} numberOfLines={1}>
          {WORD[change.status]}{side === "staged" ? " · staged" : ""}{change.oldPath ? ` · was ${change.oldPath}` : ""}
        </AppText>
        <AppText variant="caption" style={{ color: marks.success }}>+{parsed.added}</AppText>
        <AppText variant="caption" style={{ color: marks.danger }}>−{parsed.removed}</AppText>
      </View>
      <ScrollView horizontal style={styles.flex} contentContainerStyle={styles.grow}>
        <FlatList
          data={parsed.rows}
          keyExtractor={(_, index) => String(index)}
          renderItem={({ item }) => (
            <DiffLine
              row={item}
              gutter={gutter}
              commented={commentedLines?.has(item.newLine !== null ? `new:${item.newLine}` : `old:${item.oldLine}`) ?? false}
              onComment={onComment && item.kind !== "hunk" ? onComment : null}
            />
          )}
          extraData={commentedLines}
          initialNumToRender={60}
          maxToRenderPerBatch={80}
          windowSize={11}
          style={{ width: Math.max(width, 320) }}
          contentContainerStyle={styles.listContent}
          ListFooterComponent={
            diff.truncated ? (
              <AppText variant="caption" tone="textSubtle" style={styles.cut}>This diff is long. The rest is on your desktop.</AppText>
            ) : undefined
          }
        />
      </ScrollView>
    </View>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1, minWidth: 0, minHeight: 0 },
  grow: { flexGrow: 1 },
  strong: { fontWeight: "600" },
  struck: { textDecorationLine: "line-through" },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
  listContent: { paddingBottom: spacing.xl },
  summary: {
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    paddingHorizontal: spacing.lg,
    minHeight: 44,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  sectionHead: { paddingHorizontal: spacing.lg, paddingTop: spacing.md, paddingBottom: spacing.xs },
  row: {
    minHeight: 50,
    flexDirection: "row",
    alignItems: "center",
    gap: spacing.sm,
    paddingHorizontal: spacing.lg,
    borderBottomWidth: StyleSheet.hairlineWidth,
  },
  letter: { width: 18, fontFamily: MONO, fontSize: 13, fontWeight: "700", textAlign: "center" },
  footer: { padding: spacing.lg, gap: spacing.xs },
  code: { fontFamily: MONO, fontSize: 12, lineHeight: 18 },
  number: { textAlign: "right", paddingRight: 6 },
  sign: { width: 16, textAlign: "center" },
  diffLine: { flexDirection: "row", minHeight: 18 },
  hunk: { paddingHorizontal: spacing.md, paddingVertical: 3, marginTop: spacing.sm, borderRadius: radii.sm },
  cut: { padding: spacing.lg },
});
