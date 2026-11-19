import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ActivityIndicator, FlatList, KeyboardAvoidingView, Linking, Modal, Platform, RefreshControl,
  ScrollView, SectionList, StyleSheet, Switch, Text, View,
} from "react-native";
import { AppText, Button, Control, EmptyState, Icon, IconButton, SegmentedControl } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { confirmAction } from "../../components/confirmAction";
import { statusMarkColors, useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import {
  gitCommit, gitCreatePr, gitDiscard, gitPush, gitStage, gitSwitch, gitUnstage,
  readGitBranches, readGitLog, readGitStatus,
  type GitActionResult, type GitChange, type GitChangeStatus, type GitCommitSummary, type GitStatus,
} from "../../transport/relayRpcMethods";
import { ageLabel } from "../../machines/resume";
import { baseName, parentPath } from "./paths";
import type { OpenChange } from "./ChangesPanel";
import { PullRequestView } from "./PullRequestView";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });

const LETTER: Record<GitChangeStatus, string> = {
  modified: "M", added: "A", deleted: "D", renamed: "R", copied: "C", type_changed: "T", untracked: "U", conflicted: "!",
};
const WORD: Record<GitChangeStatus, string> = {
  modified: "Modified", added: "Added", deleted: "Deleted", renamed: "Renamed", copied: "Copied",
  type_changed: "Type changed", untracked: "New file", conflicted: "Conflict",
};

type View_ = "changes" | "history" | "pr";
const VIEWS = [
  { id: "changes", label: "Changes" },
  { id: "history", label: "History" },
  { id: "pr", label: "Pull request" },
] as const;

function messageOf(failure: unknown, fallback: string): string {
  const text = (failure as { message?: unknown } | null)?.message;
  return typeof text === "string" && text !== "" ? text : fallback;
}

const ChangeRow = memo(function ChangeRow({
  change, staged, busy, onOpen, onToggle, onDiscard,
}: {
  change: GitChange;
  staged: boolean;
  busy: boolean;
  onOpen: () => void;
  onToggle: () => void;
  onDiscard: (() => void) | null;
}) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const color =
    change.status === "added" || change.status === "untracked" ? marks.success
    : change.status === "deleted" || change.status === "conflicted" ? marks.danger
    : marks.warning;
  const folder = parentPath(`/${change.path}`).slice(1);
  return (
    <View style={[styles.row, { borderColor: theme.colors.border }]}>
      <Control
        accessibilityRole="button"
        accessibilityLabel={`${WORD[change.status]}: ${change.path}. Shows its diff.`}
        accessibilityHint={onDiscard ? "Press and hold to discard this change." : undefined}
        onPress={onOpen}
        onLongPress={onDiscard ?? undefined}
        style={({ pressed }) => [styles.rowMain, pressed ? { backgroundColor: theme.colors.surfaceRaised } : null]}
      >
        <Text style={[styles.letter, { color }]}>{LETTER[change.status]}</Text>
        <View style={styles.flex}>
          <AppText numberOfLines={1} style={change.status === "deleted" ? styles.struck : null}>{baseName(change.path)}</AppText>
          {folder ? <AppText variant="caption" tone="textSubtle" numberOfLines={1}>{folder}</AppText> : null}
        </View>
        {change.added !== null && change.added > 0 ? <AppText variant="caption" style={{ color: marks.success }}>+{change.added}</AppText> : null}
        {change.removed !== null && change.removed > 0 ? <AppText variant="caption" style={{ color: marks.danger }}>−{change.removed}</AppText> : null}
      </Control>
      {busy ? (
        <View style={styles.toggle}><ActivityIndicator size="small" color={theme.colors.textMuted} /></View>
      ) : (
        <IconButton
          name={staged ? "minus" : "plus"}
          label={staged ? `Unstage ${change.path}` : `Stage ${change.path}`}
          size="sm"
          onPress={onToggle}
          style={styles.toggle}
        />
      )}
    </View>
  );
});

/** A short note under the header: what just happened, or git's words why not. */
function Notice({ text, tone, action }: { text: string; tone: "ok" | "error"; action?: { label: string; onPress: () => void } | undefined }) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  return (
    <View style={[styles.notice, { borderColor: tone === "ok" ? marks.success : marks.danger, backgroundColor: theme.colors.surface }]}>
      <AppText variant="caption" style={styles.flex} tone={tone === "ok" ? "text" : "danger"} selectable>{text}</AppText>
      {action ? <Button label={action.label} size="sm" variant="ghost" onPress={action.onPress} /> : null}
    </View>
  );
}

/**
 * Source control for one workspace, from the phone: its changes (stage or
 * unstage each with one tap, hold to discard), a commit box, the branch with
 * switching, push, opening a pull request, and the branch's history. Every
 * action runs on the desktop with its own git and GitHub login, and git's own
 * words come back when something is refused.
 */
export function SourceControlPanel({ rpc, root, onOpen }: {
  rpc: RelayRpcClient;
  root: string;
  onOpen: (open: OpenChange) => void;
}) {
  const { theme } = useTheme();
  const marks = statusMarkColors(theme.colors);
  const [view, setView] = useState<View_>("changes");
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [working, setWorking] = useState<null | "commit" | "push" | "switch" | "pr">(null);
  const [message, setMessage] = useState("");
  const [notice, setNotice] = useState<{ text: string; tone: "ok" | "error"; url?: string | null } | null>(null);
  const [history, setHistory] = useState<readonly GitCommitSummary[] | null>(null);
  const [branchSheet, setBranchSheet] = useState<{ branches: readonly string[]; current: string | null } | null>(null);
  const [prSheet, setPrSheet] = useState(false);
  const request = useRef(0);

  const load = useCallback((pulled: boolean) => {
    const id = ++request.current;
    if (pulled) setRefreshing(true);
    setError(null);
    readGitStatus(rpc, root)
      .then((next) => { if (request.current === id) setStatus(next); })
      .catch((failure: unknown) => { if (request.current === id) setError(messageOf(failure, "The desktop couldn't read this workspace's changes.")); })
      .finally(() => { if (request.current === id) setRefreshing(false); });
    if (view === "history") {
      readGitLog(rpc, root).then(setHistory).catch(() => setHistory([]));
    }
  }, [rpc, root, view]);

  useEffect(() => { load(false); }, [load]);

  /** Run one action, show its answer, then re-read the changes. */
  const act = useCallback(async (
    run: () => Promise<GitActionResult>,
    done: (result: GitActionResult) => string | null,
  ): Promise<GitActionResult | null> => {
    setNotice(null);
    try {
      const result = await run();
      if (result.done) {
        const text = done(result);
        if (text) setNotice({ text, tone: "ok", url: result.url });
      } else {
        setNotice({ text: result.message ?? "Git refused that.", tone: "error" });
      }
      load(false);
      return result;
    } catch (failure) {
      setNotice({ text: messageOf(failure, "The desktop couldn't do that right now."), tone: "error" });
      return null;
    }
  }, [load]);

  const toggle = useCallback((change: GitChange, staged: boolean) => {
    setBusy((current) => new Set(current).add(change.path));
    void act(
      () => (staged ? gitUnstage(rpc, root, [change.path]) : gitStage(rpc, root, [change.path])),
      () => null,
    ).finally(() => setBusy((current) => { const next = new Set(current); next.delete(change.path); return next; }));
  }, [act, rpc, root]);

  const discard = useCallback((change: GitChange) => {
    const untracked = change.status === "untracked";
    confirmAction({
      title: untracked ? "Delete this new file?" : "Discard these changes?",
      message: untracked
        ? `${change.path} will be deleted from your desktop. This can't be undone.`
        : `Your unstaged changes to ${change.path} will be thrown away. This can't be undone.`,
      actionLabel: untracked ? "Delete" : "Discard",
      actionStyle: "destructive",
      onConfirm: () => {
        void act(() => gitDiscard(rpc, root, [change.path], untracked), () => `Discarded ${baseName(change.path)}.`);
      },
    });
  }, [act, rpc, root]);

  const stageAll = (files: readonly GitChange[], stage: boolean) =>
    act(
      () => (stage ? gitStage(rpc, root, files.map((f) => f.path)) : gitUnstage(rpc, root, files.map((f) => f.path))),
      () => null,
    );

  const commit = async (allFirst: boolean) => {
    if (!status) return;
    setWorking("commit");
    try {
      if (allFirst) {
        const staged = await stageAll(status.unstaged, true);
        if (!staged?.done) return;
      }
      const result = await act(() => gitCommit(rpc, root, message), (r) => `Committed ${r.commit?.hash ?? ""} · ${r.commit?.subject ?? ""}`);
      if (result?.done) setMessage("");
    } finally {
      setWorking(null);
    }
  };

  const push = async () => {
    setWorking("push");
    try {
      await act(() => gitPush(rpc, root), () => `Pushed ${status?.branch ?? "the branch"}.`);
    } finally {
      setWorking(null);
    }
  };

  const openBranches = async () => {
    setWorking("switch");
    try {
      setBranchSheet(await readGitBranches(rpc, root));
    } catch (failure) {
      setNotice({ text: messageOf(failure, "The desktop couldn't list branches."), tone: "error" });
    } finally {
      setWorking(null);
    }
  };

  const sections = useMemo(() => {
    if (!status) return [];
    return [
      { key: "staged" as const, title: "Staged", data: [...status.staged] },
      { key: "unstaged" as const, title: "Changes", data: [...status.unstaged] },
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

  const stagedCount = status.staged.length;
  const unstagedCount = status.unstaged.length;
  const canPush = status.ahead > 0 || (status.upstream === null && status.branch !== null);

  const header = (
    <View style={styles.headerBlock}>
      {/* Scrolls sideways when the branch, Push and Create PR don't all fit. */}
      <ScrollView
        horizontal
        showsHorizontalScrollIndicator={false}
        keyboardShouldPersistTaps="handled"
        style={[styles.branchScroll, { borderColor: theme.colors.border }]}
        contentContainerStyle={styles.branchBar}
      >
        <Control
          accessibilityRole="button"
          accessibilityLabel={`Branch ${status.branch ?? "detached"}. Switch branch.`}
          onPress={() => void openBranches()}
          style={({ pressed }) => [styles.branchButton, { borderColor: theme.colors.border, backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.surface }]}
        >
          <Icon name="branch" size={15} tone="textMuted" />
          <AppText numberOfLines={1} style={[styles.strong, styles.branchName]}>{status.branch ?? "Detached HEAD"}</AppText>
          {working === "switch" ? <ActivityIndicator size="small" color={theme.colors.textMuted} /> : <Icon name="chevron-down" size={14} tone="textSubtle" />}
        </Control>
        {status.behind > 0 ? <AppText variant="caption" tone="textMuted">↓{status.behind}</AppText> : null}
        <View style={styles.spacer} />
        {canPush ? (
          <Button
            label={status.ahead > 0 ? `Push ↑${status.ahead}` : "Publish"}
            size="sm"
            variant="ghost"
            leading={<Icon name="upload" size={14} />}
            loading={working === "push"}
            onPress={() => void push()}
          />
        ) : null}
        <Button label="Create PR" size="sm" variant="ghost" leading={<Icon name="pull-request" size={14} />} onPress={() => setPrSheet(true)} />
      </ScrollView>
      <View style={styles.segment}>
        <SegmentedControl label="Source control view" options={VIEWS} value={view} onChange={(next) => setView(next as View_)} />
      </View>
      {notice ? (
        <View style={styles.noticeWrap}>
          <Notice
            text={notice.text}
            tone={notice.tone}
            action={notice.url ? { label: "Open", onPress: () => void Linking.openURL(notice.url!) } : undefined}
          />
        </View>
      ) : null}
    </View>
  );

  const createPrSheet = (
    <CreatePrSheet
      visible={prSheet}
      branch={status.branch}
      onClose={() => setPrSheet(false)}
      onCreate={async (title, body, draft) => {
        setWorking("pr");
        try {
          const result = await act(() => gitCreatePr(rpc, root, title, body, draft), (r) => (r.url ? `Pull request opened: ${r.url}` : "Pull request opened."));
          if (result?.done) setPrSheet(false);
        } finally {
          setWorking(null);
        }
      }}
      busy={working === "pr"}
    />
  );

  if (view === "pr") {
    return (
      <View style={styles.flex}>
        {header}
        <PullRequestView rpc={rpc} root={root} onCreate={() => setPrSheet(true)} />
        {createPrSheet}
      </View>
    );
  }

  if (view === "history") {
    return (
      <FlatList
        data={history ?? []}
        keyExtractor={(commit) => commit.hash}
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => load(true)} tintColor={theme.colors.textMuted} />}
        ListHeaderComponent={header}
        renderItem={({ item }) => (
          <View style={[styles.commitRow, { borderColor: theme.colors.border }]}>
            <Icon name="git-commit" size={16} tone="textSubtle" />
            <View style={styles.flex}>
              <AppText numberOfLines={2}>{item.subject}</AppText>
              <AppText variant="caption" tone="textSubtle" numberOfLines={1}>
                <Text style={{ fontFamily: MONO }}>{item.short}</Text> · {item.author} · {ageLabel(item.at * 1000)}
              </AppText>
            </View>
          </View>
        )}
        ListEmptyComponent={history === null ? <ActivityIndicator style={styles.pad} color={theme.colors.accent} /> : <EmptyState title="No commits yet" message="This branch has no history to show." />}
      />
    );
  }

  return (
    <KeyboardAvoidingView style={styles.flex} behavior={Platform.OS === "ios" ? "padding" : undefined}>
      <SectionList
        style={styles.flex}
        sections={sections}
        keyExtractor={(change, index) => `${index}:${change.path}`}
        stickySectionHeadersEnabled={false}
        keyboardShouldPersistTaps="handled"
        refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => load(true)} tintColor={theme.colors.textMuted} />}
        ListHeaderComponent={header}
        renderSectionHeader={({ section }) => (
          <View style={[styles.sectionHead, { backgroundColor: theme.colors.background }]}>
            <AppText variant="caption" tone="textMuted" style={[styles.strong, styles.flex]}>{section.title} · {section.data.length}</AppText>
            <Button
              label={section.key === "staged" ? "Unstage all" : "Stage all"}
              size="sm"
              variant="ghost"
              onPress={() => void stageAll(section.data, section.key !== "staged")}
            />
          </View>
        )}
        renderItem={({ item, section }) => (
          <ChangeRow
            change={item}
            staged={section.key === "staged"}
            busy={busy.has(item.path)}
            onOpen={() => onOpen({ change: item, side: section.key })}
            onToggle={() => toggle(item, section.key === "staged")}
            onDiscard={section.key === "unstaged" ? () => discard(item) : null}
          />
        )}
        ListEmptyComponent={<EmptyState title="No changes" message="This workspace matches its last commit. Pull down to check again." />}
        ListFooterComponent={
          status.truncated ? <AppText variant="caption" tone="textSubtle" style={styles.pad}>Only the first changes are listed. The rest are on your desktop.</AppText> : undefined
        }
      />
      {stagedCount + unstagedCount > 0 ? (
        <View style={[styles.commitBox, { borderColor: theme.colors.border, backgroundColor: theme.colors.surface }]}>
          <FieldInput
            containerStyle={styles.flex}
            value={message}
            onChangeText={setMessage}
            placeholder={stagedCount > 0 ? `Commit message (${stagedCount} staged)` : "Commit message"}
            accessibilityLabel="Commit message"
            multiline
            style={styles.commitInput}
          />
          <Button
            label={stagedCount > 0 ? "Commit" : "Stage all & commit"}
            size="sm"
            leading={<Icon name="check" size={14} tone="onAccent" />}
            loading={working === "commit"}
            disabled={message.trim() === ""}
            onPress={() => void commit(stagedCount === 0)}
          />
        </View>
      ) : null}
      <AppText variant="caption" tone="textSubtle" style={styles.legend}>
        <Text style={{ color: marks.success }}>+</Text> stages a file, <Text style={{ color: marks.danger }}>−</Text> unstages it. Hold a change to discard it.
      </AppText>

      <Modal visible={branchSheet !== null} transparent animationType="slide" onRequestClose={() => setBranchSheet(null)}>
        <ThemedStatusBar />
        <View style={styles.scrim}>
          <View style={[styles.sheet, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
            <AppText style={[styles.strong, styles.sheetTitle]}>Switch branch</AppText>
            <FlatList
              data={branchSheet?.branches ?? []}
              keyExtractor={(name) => name}
              style={styles.branchList}
              renderItem={({ item }) => (
                <Control
                  accessibilityRole="button"
                  accessibilityLabel={item === branchSheet?.current ? `${item}, current branch` : `Switch to ${item}`}
                  onPress={() => {
                    const current = branchSheet?.current;
                    setBranchSheet(null);
                    if (item !== current) void act(() => gitSwitch(rpc, root, item), () => `Switched to ${item}.`);
                  }}
                  style={({ pressed }) => [styles.sheetRow, pressed ? { backgroundColor: theme.colors.surfaceRaised } : null]}
                >
                  <Icon name={item === branchSheet?.current ? "check" : "branch"} size={16} tone={item === branchSheet?.current ? "accent" : "textMuted"} />
                  <AppText numberOfLines={1} style={styles.flex}>{item}</AppText>
                </Control>
              )}
            />
            <Button label="Cancel" variant="ghost" onPress={() => setBranchSheet(null)} />
          </View>
        </View>
      </Modal>

      {createPrSheet}
    </KeyboardAvoidingView>
  );
}

function CreatePrSheet({ visible, branch, busy, onClose, onCreate }: {
  visible: boolean;
  branch: string | null;
  busy: boolean;
  onClose: () => void;
  onCreate: (title: string, body: string, draft: boolean) => void;
}) {
  const { theme } = useTheme();
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [draft, setDraft] = useState(false);
  useEffect(() => {
    if (visible && title === "" && branch) setTitle(branch.replace(/[-_/]+/g, " ").replace(/^\w/, (c) => c.toUpperCase()));
  }, [visible, branch, title]);
  return (
    <Modal visible={visible} transparent animationType="slide" onRequestClose={onClose}>
      <ThemedStatusBar />
      <KeyboardAvoidingView style={styles.scrim} behavior={Platform.OS === "ios" ? "padding" : undefined}>
        <View style={[styles.sheet, styles.prSheet, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
          <AppText style={[styles.strong, styles.sheetTitle]}>Create pull request</AppText>
          <AppText variant="caption" tone="textMuted">
            From {branch ?? "this branch"}, with your desktop's GitHub login. Push first if the branch isn't on GitHub yet.
          </AppText>
          <FieldInput value={title} onChangeText={setTitle} placeholder="Title" accessibilityLabel="Pull request title" />
          <FieldInput value={body} onChangeText={setBody} placeholder="What changed and why (optional)" accessibilityLabel="Pull request description" multiline style={styles.prBody} />
          <View style={styles.draftRow}>
            <AppText style={styles.flex}>Open as a draft</AppText>
            <Switch value={draft} onValueChange={setDraft} accessibilityLabel="Open as a draft" />
          </View>
          <View style={styles.sheetActions}>
            <Button label="Cancel" variant="ghost" onPress={onClose} />
            <Button label="Create PR" loading={busy} disabled={title.trim() === ""} onPress={() => onCreate(title.trim(), body, draft)} />
          </View>
        </View>
      </KeyboardAvoidingView>
    </Modal>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1, minWidth: 0, minHeight: 0 },
  strong: { fontWeight: "600" },
  struck: { textDecorationLine: "line-through" },
  center: { flex: 1, alignItems: "center", justifyContent: "center" },
  pad: { padding: spacing.lg },
  headerBlock: { gap: spacing.xs, paddingBottom: spacing.xs },
  branchScroll: { flexGrow: 0, borderBottomWidth: StyleSheet.hairlineWidth },
  branchBar: {
    flexGrow: 1, flexDirection: "row", alignItems: "center", gap: spacing.xs,
    paddingHorizontal: spacing.md, paddingVertical: spacing.sm,
  },
  spacer: { flexGrow: 1, minWidth: spacing.xs },
  branchButton: {
    flexDirection: "row", alignItems: "center", gap: 6, maxWidth: 220,
    borderWidth: 1, borderRadius: radii.md, paddingHorizontal: 10, minHeight: 34,
  },
  branchName: { flexShrink: 1 },
  segment: { paddingHorizontal: spacing.md, paddingTop: spacing.xs },
  noticeWrap: { paddingHorizontal: spacing.md },
  notice: { flexDirection: "row", alignItems: "center", gap: spacing.sm, borderWidth: 1, borderRadius: radii.md, padding: spacing.sm },
  sectionHead: { flexDirection: "row", alignItems: "center", paddingHorizontal: spacing.lg, paddingTop: spacing.sm },
  row: { flexDirection: "row", alignItems: "center", borderBottomWidth: StyleSheet.hairlineWidth },
  rowMain: { flex: 1, minHeight: 52, flexDirection: "row", alignItems: "center", gap: spacing.sm, paddingLeft: spacing.lg, paddingRight: spacing.sm },
  toggle: { width: 44, height: 44, alignItems: "center", justifyContent: "center", marginRight: spacing.sm },
  letter: { width: 18, fontFamily: MONO, fontSize: 13, fontWeight: "700", textAlign: "center" },
  commitRow: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm, paddingHorizontal: spacing.lg, paddingVertical: spacing.sm, borderBottomWidth: StyleSheet.hairlineWidth },
  commitBox: { flexDirection: "row", alignItems: "flex-end", gap: spacing.sm, padding: spacing.sm, borderTopWidth: StyleSheet.hairlineWidth },
  commitInput: { minHeight: 40, maxHeight: 120 },
  legend: { textAlign: "center", paddingVertical: 4 },
  scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: { borderTopWidth: StyleSheet.hairlineWidth, borderTopLeftRadius: radii.xl, borderTopRightRadius: radii.xl, padding: spacing.md, gap: spacing.sm, maxHeight: "75%" },
  prSheet: { paddingBottom: spacing.xl },
  sheetTitle: { fontSize: 17 },
  branchList: { flexGrow: 0 },
  sheetRow: { minHeight: 48, flexDirection: "row", alignItems: "center", gap: spacing.md, paddingHorizontal: spacing.sm, borderRadius: radii.md },
  prBody: { textAlignVertical: "top" },
  draftRow: { flexDirection: "row", alignItems: "center", gap: spacing.sm },
  sheetActions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
});
