import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { ActivityIndicator, FlatList, StyleSheet, View, type ListRenderItemInfo } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { AppText, Button, DragentMark } from "../../components";
import { useTheme } from "../../theme";
import { pendingAgentRequest, type AgentRow } from "../../transport/agentConversation";
import { useSoftKeyboard } from "../../terminal/companion/useSoftKeyboard";
import { AgentRowView } from "./AgentRows";
import { useStickToBottom } from "../chat/useStickToBottom";
import { firstLine, layoutTranscript, stepFromToolRow, type Entry, type Line, type Step } from "../chat/transcript";
import { TranscriptLine, type LineHandlers } from "../chat/TranscriptParts";
import { ChatComposer, ComposerButton, FloatingPill } from "../chat/ChatParts";
import { ChatAttachmentStrip, ChatExtrasBar, ChatExtrasTray, useChatExtras } from "../chat/ChatExtras";
import { useVoiceConversation } from "../chat/useVoiceConversation";
import { VoicePanel } from "../chat/VoicePanel";
import { useAgentConversation } from "./useAgentConversation";

type DragentLine = Line<AgentRow>;

function keyOf(row: AgentRow): string {
  return `${row.kind}:${row.id}`;
}

/** A tool call of the built-in agent as a step on the rail. */
function toolStep(row: Extract<AgentRow, { kind: "tool_step" }>): Step {
  const first = row.diff[0];
  const step = stepFromToolRow({
    id: keyOf(row),
    tool: row.name,
    text: row.args,
    error: row.status === "error",
    running: row.status === "running",
    path: first?.path ?? null,
    added: row.diff.length > 0 ? row.diff.reduce((sum, file) => sum + file.added, 0) : null,
    removed: row.diff.length > 0 ? row.diff.reduce((sum, file) => sum + file.removed, 0) : null,
    at: row.tsMs,
  }, true);
  // A file change shows as one; the diff itself is in Source control.
  if (first && step.kind !== "edit") {
    return { ...step, kind: "edit", verb: row.status === "running" ? "Editing" : "Edited", file: first.path.split("/").pop() ?? first.path, label: "", mono: false, detail: first.path };
  }
  return row.result && !step.detail ? { ...step, detail: row.result } : step;
}

/** The conversation's rows as entries: cards (approvals, questions, plans, errors) stay as they are. */
function entriesOf(rows: readonly AgentRow[]): Entry<AgentRow>[] {
  const entries: Entry<AgentRow>[] = [];
  for (const row of rows) {
    const id = keyOf(row);
    switch (row.kind) {
      case "user_query":
        entries.push({ kind: "you", id, text: row.text, at: row.tsMs });
        break;
      case "queued":
        entries.push({ kind: "you", id, text: row.text, at: row.tsMs, note: { kind: "queued" } });
        break;
      case "agent_message":
        if (row.text.trim() || row.streaming) entries.push({ kind: "reply", id, text: row.text, at: row.tsMs, streaming: row.streaming });
        break;
      case "thinking":
        entries.push({
          kind: "step",
          at: row.tsMs,
          step: {
            id, kind: "think", verb: null, file: null,
            label: row.complete ? firstLine(row.text, 120) || "Thought" : "Thinking…",
            mono: false, detail: row.complete && row.text.length > 120 ? row.text : null,
            added: null, removed: null, running: !row.complete, failed: false, startedAt: row.tsMs,
          },
        });
        break;
      case "tool_step":
        entries.push({ kind: "step", step: toolStep(row), at: row.tsMs });
        break;
      case "action_result":
        entries.push({
          kind: "step",
          at: row.tsMs,
          step: {
            id, kind: "execute", verb: null, file: null, label: firstLine(row.command, 200), mono: true,
            detail: row.output ? `$ ${row.command}\n${row.output}` : null,
            added: null, removed: null, running: false, failed: row.exitCode !== null && row.exitCode !== 0, startedAt: row.tsMs,
          },
        });
        break;
      case "turn_end":
        entries.push({ kind: "end", id, at: row.tsMs, durationMs: row.durationMs });
        if (row.note) entries.push({ kind: "card", id, card: row, at: row.tsMs });
        break;
      case "loading":
        break;
      default:
        entries.push({ kind: "card", id, card: row, at: row.tsMs });
    }
  }
  return entries;
}

/**
 * The built-in agent on the phone, laid out as the desktop chat lays it out:
 * your message, the turn's clock, its replies, its tool calls on a rail, and
 * the approval, question and plan cards answered in place. `fallback` is
 * shown instead when this desktop can't stream the conversation (an older build).
 */
export function DragentAgentView({ tabId, fallback, workspaceRoot = null }: { tabId: number; fallback: ReactNode; workspaceRoot?: string | null }) {
  const extras = useChatExtras({ tabId, workspaceRoot });
  const { theme } = useTheme();
  const insets = useSafeAreaInsets();
  const keyboard = useSoftKeyboard(insets.bottom);
  const { state, phase, error, send, act, retry } = useAgentConversation(tabId);
  const stick = useStickToBottom<DragentLine>();
  const [draft, setDraft] = useState("");
  const [sending, setSending] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [dockHeight, setDockHeight] = useState(60);
  const [opened, setOpened] = useState<ReadonlySet<string>>(new Set());

  const waiting = pendingAgentRequest(state) !== null;
  const lines = useMemo(
    () => layoutTranscript(entriesOf(state.rows), { working: state.busy, waiting }),
    [state.rows, state.busy, waiting],
  );

  // The keyboard and the message box take room from the list: keep the latest line in view.
  const { pin } = stick;
  useEffect(() => {
    pin();
  }, [keyboard.visible, dockHeight, pin]);

  const handlers = useMemo<LineHandlers<AgentRow>>(() => ({
    isOpen: (key) => opened.has(key),
    onToggle: (key) => setOpened((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    }),
    renderCard: (row) => <AgentRowView row={row} act={act} />,
    onCancel: (id) => {
      const row = state.rows.find((candidate) => keyOf(candidate) === id);
      if (row?.kind === "queued" && row.ref) void act("cancel_queued", { ref: row.ref });
    },
  }), [opened, act, state.rows]);

  const renderItem = useCallback(
    ({ item }: ListRenderItemInfo<DragentLine>) => <TranscriptLine line={item} handlers={handlers} />,
    [handlers],
  );

  if (phase === "unsupported") return <>{fallback}</>;

  // Voice mode: what you said goes like a typed message; Dragent's last
  // finished reply is read aloud.
  const sendSpoken = useCallback(async (text: string) => {
    stick.follow();
    const result = await send(text);
    return result.ok;
  }, [send, stick]);
  const lastReply = useMemo(() => {
    for (let index = state.rows.length - 1; index >= 0; index -= 1) {
      const row = state.rows[index]!;
      if (row.kind === "user_query") return null;
      if (row.kind === "agent_message" && !row.streaming && row.text.trim()) return { id: String(row.id), text: row.text };
    }
    return null;
  }, [state.rows]);
  const voice = useVoiceConversation({ send: sendSpoken, reply: lastReply, busy: state.busy });

  const submit = () => {
    const typed = draft.trim();
    const text = extras.message(draft).trim();
    if (text === "" || sending || extras.uploading) return;
    setSending(true);
    setSendError(null);
    setDraft("");
    stick.follow();
    void send(text).then((result) => {
      setSending(false);
      if (result.ok) {
        extras.clear();
      } else {
        // Nothing was sent: put it back unless something new was typed.
        setDraft((current) => (current === "" ? typed : current));
        setSendError(result.message);
      }
    });
  };
  const stop = () => {
    if (stopping) return;
    setStopping(true);
    setSendError(null);
    void act("stop").then((result) => {
      setStopping(false);
      if (!result.ok) setSendError(result.message);
    });
  };

  const live = phase === "live";
  const empty = live && state.ready && lines.length === 0;

  return (
    <View style={[styles.frame, { paddingBottom: keyboard.height }]}>
      {/* Everything floats inside this body, which ends at the top of the keyboard. */}
      <View style={styles.flex}>
      {!live ? (
        <View style={[styles.center, { paddingBottom: dockHeight }]}>
          {phase === "lost" ? (
            <>
              <AppText variant="body" style={styles.centerText}>
                {error ?? "The conversation stopped updating."}
              </AppText>
              <Button label="Reconnect" size="sm" onPress={retry} />
            </>
          ) : (
            <ActivityIndicator color={theme.colors.textMuted} />
          )}
        </View>
      ) : empty ? (
        <View style={[styles.center, { paddingBottom: dockHeight }]}>
          <DragentMark size={40} />
          <AppText style={styles.emptyTitle}>What should Dragent do?</AppText>
          <AppText variant="caption" tone="textMuted" style={styles.centerText}>
            It works in this workspace on your desktop.
          </AppText>
        </View>
      ) : (
        <FlatList
          {...stick.listProps}
          data={lines}
          keyExtractor={(line) => line.key}
          renderItem={renderItem}
          extraData={handlers}
          style={styles.flex}
          contentContainerStyle={styles.content}
          keyboardShouldPersistTaps="handled"
          keyboardDismissMode="interactive"
          accessibilityLabel="Dragent conversation"
          ListHeaderComponent={
            state.clipped ? (
              <AppText variant="caption" tone="textSubtle" style={styles.clipped}>
                Earlier messages are on your desktop.
              </AppText>
            ) : undefined
          }
          ListFooterComponent={
            <>
              {!state.ready ? <ActivityIndicator color={theme.colors.textMuted} /> : null}
              <View style={{ height: dockHeight + 16 }} />
            </>
          }
        />
      )}
      {stick.away ? (
        <View style={[styles.floating, { bottom: dockHeight + 10 }]} pointerEvents="box-none">
          <FloatingPill
            label={waiting ? "Needs your answer" : "Latest"}
            icon="chevron-down"
            tone={waiting ? "attention" : "neutral"}
            onPress={stick.jumpToEnd}
          />
        </View>
      ) : null}
      {voice.active ? (
        <VoicePanel
          phase={voice.phase}
          heard={voice.heard}
          level={voice.level}
          error={voice.error}
          name="Dragent"
          onNext={voice.next}
          onEnd={voice.end}
          bottom={0}
        />
      ) : (
      <ChatComposer
        value={draft}
        onChangeText={setDraft}
        placeholder={state.busy ? "Add a follow-up" : "Ask Dragent"}
        accessibilityLabel="Message to Dragent"
        accessibilityHint={state.busy ? "Dragent is working. Your message is queued and runs next." : "Send starts Dragent on your desktop."}
        onSend={submit}
        sending={sending}
        canSend={live && !extras.uploading}
        onStop={state.busy ? stop : undefined}
        stopping={stopping}
        error={sendError ?? extras.notice}
        onHeight={setDockHeight}
        top={<ChatExtrasTray extras={extras} draft={draft} onDraft={setDraft} />}
        inside={<ChatAttachmentStrip extras={extras} />}
        bottom={
          <ChatExtrasBar extras={extras}>
            <ComposerButton icon="voice" label="Talk with Dragent" onPress={voice.start} />
          </ChatExtrasBar>
        }
        hasContent={extras.hasAttachments}
      />
      )}
      </View>
    </View>
  );
}

const styles = StyleSheet.create({
  frame: { flex: 1, minHeight: 0 },
  flex: { flex: 1, minHeight: 0 },
  center: { flex: 1, alignItems: "center", justifyContent: "center", gap: 10, padding: 32 },
  centerText: { textAlign: "center" },
  emptyTitle: { fontSize: 19, lineHeight: 26, fontWeight: "600", textAlign: "center" },
  content: { paddingTop: 16 },
  clipped: { textAlign: "center", paddingBottom: 12 },
  floating: { position: "absolute", left: 0, right: 0, alignItems: "center" },
});
