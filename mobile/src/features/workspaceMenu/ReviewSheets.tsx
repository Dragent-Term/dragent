import { ThemedStatusBar } from "../../components/ThemedStatusBar";
import { useEffect, useMemo, useState } from "react";
import { FlatList, KeyboardAvoidingView, Modal, Platform, StyleSheet, Text, View } from "react-native";
import { AgentMark, AppText, Button, Control, Icon, IconButton } from "../../components";
import { FieldInput } from "../../components/FieldInput";
import { useTheme } from "../../theme";
import { radii, spacing } from "../../theme/tokens";
import type { RelayRpcClient } from "../../transport/relayRpcClient";
import { isDragentTab, type RelayTab } from "../../transport/relayCatalog";
import { sendAgentPrompt, sendTabText } from "../../transport/relayRpcMethods";
import {
  addReviewComment, clearReview, removeReviewComment, reviewMessage, useReview, type ReviewComment,
} from "../../machines/review";
import type { DiffRow } from "./unifiedDiff";

const MONO = Platform.select({ ios: "Menlo", android: "monospace", default: "monospace" });

export interface CommentTarget {
  readonly file: string;
  readonly row: DiffRow;
}

/** Where a comment on a diff row points: its line and side. */
export function commentKey(row: DiffRow): string {
  return row.newLine !== null ? `new:${row.newLine}` : `old:${row.oldLine}`;
}

/** The lines of `file` already commented on, as `commentKey`s. */
export function commentedLines(comments: readonly ReviewComment[], file: string): ReadonlySet<string> {
  return new Set(comments.filter((c) => c.file === file && c.line !== null).map((c) => `${c.side}:${c.line}`));
}

/** Write one comment on a diff line. */
export function CommentSheet({ root, target, onClose }: { root: string; target: CommentTarget | null; onClose: () => void }) {
  const { theme } = useTheme();
  const [text, setText] = useState("");
  useEffect(() => { if (target) setText(""); }, [target]);
  if (!target) return null;
  const { row, file } = target;
  const side = row.newLine !== null ? "new" : "old";
  const line = row.newLine ?? row.oldLine;
  const add = () => {
    addReviewComment(root, { file, line, side, quote: row.text, text: text.trim() });
    onClose();
  };
  return (
    <Modal visible transparent animationType="slide" onRequestClose={onClose}>
      <ThemedStatusBar />
      <KeyboardAvoidingView style={styles.scrim} behavior={Platform.OS === "ios" ? "padding" : undefined}>
        <View style={[styles.sheet, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
          <AppText style={styles.title}>Comment on line {line ?? ""}</AppText>
          <AppText variant="caption" tone="textMuted" numberOfLines={1}>{file}{side === "old" ? " · removed line" : ""}</AppText>
          <View style={[styles.quote, { backgroundColor: theme.colors.surface, borderColor: theme.colors.border }]}>
            <Text style={[styles.code, { color: theme.colors.text }]} numberOfLines={3}>{row.text.trim() || " "}</Text>
          </View>
          <FieldInput
            value={text}
            onChangeText={setText}
            placeholder="What should change here?"
            accessibilityLabel="Review comment"
            multiline
            autoFocus
            style={styles.input}
          />
          <View style={styles.actions}>
            <Button label="Cancel" variant="ghost" onPress={onClose} />
            <Button label="Add to review" disabled={text.trim() === ""} onPress={add} />
          </View>
        </View>
      </KeyboardAvoidingView>
    </Modal>
  );
}

function agentLabel(tab: RelayTab): string {
  if (isDragentTab(tab)) return "Dragent";
  const kind = tab.agent_kind ?? "";
  return tab.title || kind || "Agent";
}

/**
 * The review in progress: "Review · N comments" along the bottom of the
 * source control screens; open it to read, trim, add a note and send it to
 * one of the workspace's agents as one message.
 */
export function ReviewBar({ rpc, root, agents }: { rpc: RelayRpcClient; root: string; agents: readonly RelayTab[] }) {
  const { theme } = useTheme();
  const comments = useReview(root);
  const [open, setOpen] = useState(false);
  const [summary, setSummary] = useState("");
  const [target, setTarget] = useState<number | null>(null);
  const [sending, setSending] = useState(false);
  const [result, setResult] = useState<{ ok: boolean; text: string } | null>(null);
  const preferred = useMemo(
    () => agents.find((tab) => tab.attention === "needs_input") ?? agents.find((tab) => tab.active) ?? agents[0] ?? null,
    [agents],
  );
  useEffect(() => {
    if (target === null || !agents.some((tab) => tab.id === target)) setTarget(preferred?.id ?? null);
  }, [agents, preferred, target]);

  if (comments.length === 0 && !open) return null;
  const chosen = agents.find((tab) => tab.id === target) ?? null;

  const send = async () => {
    if (!chosen) return;
    setSending(true);
    setResult(null);
    const message = reviewMessage(comments, summary);
    try {
      await (isDragentTab(chosen) ? sendAgentPrompt(rpc, chosen.id, message) : sendTabText(rpc, chosen.id, message));
      clearReview(root);
      setSummary("");
      setResult({ ok: true, text: `Sent to ${agentLabel(chosen)}.` });
      setOpen(false);
    } catch (failure) {
      const text = (failure as { message?: unknown } | null)?.message;
      setResult({ ok: false, text: typeof text === "string" && text ? text : "The desktop couldn't deliver the review." });
    } finally {
      setSending(false);
    }
  };

  return (
    <>
      {comments.length > 0 ? (
        <Control
          accessibilityRole="button"
          accessibilityLabel={`Review, ${comments.length} comment${comments.length === 1 ? "" : "s"}. Open to send it to an agent.`}
          onPress={() => setOpen(true)}
          style={({ pressed }) => [styles.bar, { backgroundColor: pressed ? theme.colors.surfaceRaised : theme.colors.accent }]}
        >
          <Icon name="message" size={16} tone="onAccent" />
          <AppText style={[styles.barText, { color: theme.colors.onAccent }]}>
            Review · {comments.length} comment{comments.length === 1 ? "" : "s"}
          </AppText>
          <AppText variant="caption" style={{ color: theme.colors.onAccent }}>Send to agent</AppText>
          <Icon name="chevron-right" size={16} tone="onAccent" />
        </Control>
      ) : null}
      {result && !open ? (
        <AppText variant="caption" tone={result.ok ? "textMuted" : "danger"} style={styles.result}>{result.text}</AppText>
      ) : null}
      <Modal visible={open} transparent animationType="slide" onRequestClose={() => setOpen(false)}>
        <ThemedStatusBar />
        <KeyboardAvoidingView style={styles.scrim} behavior={Platform.OS === "ios" ? "padding" : undefined}>
          <View style={[styles.sheet, styles.tall, { backgroundColor: theme.colors.background, borderColor: theme.colors.border }]}>
            <AppText style={styles.title}>Review</AppText>
            <FlatList
              data={[...comments]}
              keyExtractor={(comment) => comment.id}
              style={styles.list}
              renderItem={({ item }) => (
                <View style={[styles.comment, { borderColor: theme.colors.border }]}>
                  <View style={styles.flex}>
                    <AppText variant="caption" tone="textMuted" numberOfLines={1}>
                      {item.file}{item.line !== null ? `:${item.line}` : ""}
                    </AppText>
                    <AppText>{item.text}</AppText>
                  </View>
                  <IconButton name="close" label="Remove this comment" size="sm" onPress={() => removeReviewComment(root, item.id)} />
                </View>
              )}
              ListEmptyComponent={<AppText variant="caption" tone="textMuted">No comments. Tap a line in a diff to add one.</AppText>}
            />
            <FieldInput value={summary} onChangeText={setSummary} placeholder="Anything else for the agent? (optional)" accessibilityLabel="Review note" multiline style={styles.input} />
            <AppText variant="caption" tone="textMuted">Send to</AppText>
            {agents.length === 0 ? (
              <AppText variant="caption" tone="danger">No agent is open in this workspace. Start one from the workspace menu, then send.</AppText>
            ) : (
              <View style={styles.agents}>
                {agents.map((tab) => {
                  const selected = tab.id === target;
                  return (
                    <Control
                      key={tab.id}
                      accessibilityRole="radio"
                      accessibilityState={{ selected }}
                      accessibilityLabel={agentLabel(tab)}
                      onPress={() => setTarget(tab.id)}
                      style={[styles.agent, { borderColor: selected ? theme.colors.accent : theme.colors.border, backgroundColor: selected ? theme.colors.surfaceRaised : theme.colors.surface }]}
                    >
                      {tab.agent_kind ? <AgentMark agent={isDragentTab(tab) ? "dragent" : tab.agent_kind} size={16} /> : <Icon name="terminal" size={14} />}
                      <AppText numberOfLines={1} style={styles.agentName}>{agentLabel(tab)}</AppText>
                    </Control>
                  );
                })}
              </View>
            )}
            {result && !result.ok ? <AppText variant="caption" tone="danger">{result.text}</AppText> : null}
            <View style={styles.actions}>
              <Button label="Close" variant="ghost" onPress={() => setOpen(false)} />
              <Button label="Send review" loading={sending} disabled={!chosen || comments.length === 0} onPress={() => void send()} />
            </View>
          </View>
        </KeyboardAvoidingView>
      </Modal>
    </>
  );
}

const styles = StyleSheet.create({
  flex: { flex: 1, minWidth: 0 },
  scrim: { flex: 1, justifyContent: "flex-end", backgroundColor: "rgba(0,0,0,0.45)" },
  sheet: { borderTopWidth: StyleSheet.hairlineWidth, borderTopLeftRadius: radii.xl, borderTopRightRadius: radii.xl, padding: spacing.md, paddingBottom: spacing.xl, gap: spacing.sm },
  tall: { maxHeight: "85%" },
  title: { fontWeight: "600", fontSize: 17 },
  quote: { borderWidth: 1, borderRadius: radii.md, padding: spacing.sm },
  code: { fontFamily: MONO, fontSize: 12, lineHeight: 18 },
  input: { maxHeight: 160, textAlignVertical: "top" },
  actions: { flexDirection: "row", justifyContent: "flex-end", gap: spacing.sm },
  bar: { flexDirection: "row", alignItems: "center", gap: spacing.sm, margin: spacing.sm, paddingHorizontal: spacing.md, minHeight: 46, borderRadius: radii.lg },
  barText: { flex: 1, fontWeight: "600" },
  result: { textAlign: "center", paddingBottom: spacing.xs },
  list: { flexGrow: 0, maxHeight: 260 },
  comment: { flexDirection: "row", alignItems: "flex-start", gap: spacing.sm, paddingVertical: spacing.xs, borderBottomWidth: StyleSheet.hairlineWidth },
  agents: { flexDirection: "row", flexWrap: "wrap", gap: spacing.xs },
  agent: { flexDirection: "row", alignItems: "center", gap: 6, borderWidth: 1, borderRadius: radii.md, paddingHorizontal: 10, minHeight: 36, maxWidth: "100%" },
  agentName: { maxWidth: 180 },
});
